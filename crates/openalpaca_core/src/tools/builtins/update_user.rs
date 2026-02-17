use async_trait::async_trait;
use base64::Engine as _;
use crate::middleware::user::{parse_user_markdown, render_user_markdown};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

use super::UserToolContext;

struct UserUpdateTool {
    ctx: UserToolContext,
}

/// Known top-level fields in the update_user tool arguments.
const KNOWN_USER_FIELDS: &[&str] = &["mode", "content_b64", "sections"];
/// Known section-patch fields inside the `sections` object.
const KNOWN_USER_SECTION_FIELDS: &[&str] = &[
    "identity",
    "communication_style",
    "expertise",
    "projects",
    "preferences",
    "notes",
];

#[async_trait]
impl BuiltInTool for UserUpdateTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Reject unknown top-level fields
        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
                if key == "owner_id" {
                    continue; // Injected by ContextualToolExecutor
                }
                if !KNOWN_USER_FIELDS.contains(&key.as_str()) {
                    return Err(format!(
                        "Unknown field '{}'. Allowed fields: {}",
                        key,
                        KNOWN_USER_FIELDS.join(", ")
                    ));
                }
            }
        }

        let mode = arguments
            .get("mode")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Missing required parameter: mode (\"replace\" or \"sections\")".to_string()
            })?;

        let (validated_markdown, modified_sections) = match mode {
            "replace" => (self.validate_replace(arguments)?, vec!["all".to_string()]),
            "sections" => self.validate_sections(arguments).await?,
            other => {
                return Err(format!(
                    "Invalid mode '{}'. Must be \"replace\" or \"sections\".",
                    other
                ));
            }
        };

        // Compute content hash for deduplication
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(validated_markdown.as_bytes()));

        // -- Backup current USER file (if it exists) --
        let backup_path = if self.ctx.user_path.exists() {
            let backup_dir = &self.ctx.backup_dir;

            tokio::fs::create_dir_all(backup_dir)
                .await
                .map_err(|e| format!("Failed to create backup directory: {}", e))?;

            let backup_path = unique_user_backup_path(backup_dir);
            tokio::fs::copy(&self.ctx.user_path, &backup_path)
                .await
                .map_err(|e| format!("Failed to create backup: {}", e))?;

            Some(backup_path.display().to_string())
        } else {
            None
        };

        // Prune old backups if retention limit is configured
        if let Some(max) = self.ctx.max_backups {
            prune_user_backups(&self.ctx.backup_dir, max).await;
        }

        // -- Atomic write: temp file → fsync → rename --
        let user_dir = self
            .ctx
            .user_path
            .parent()
            .ok_or_else(|| "USER path has no parent directory".to_string())?;

        let tmp_path = user_dir.join(".USER.md.tmp");

        // Write to temp file
        tokio::fs::write(&tmp_path, &validated_markdown)
            .await
            .map_err(|e| format!("Failed to write temp file: {}", e))?;

        // fsync the temp file for durability
        let file = tokio::fs::File::open(&tmp_path)
            .await
            .map_err(|e| format!("Failed to open temp file for sync: {}", e))?;
        file.sync_all()
            .await
            .map_err(|e| format!("Failed to fsync temp file: {}", e))?;

        // Atomic rename
        tokio::fs::rename(&tmp_path, &self.ctx.user_path)
            .await
            .map_err(|e| format!("Atomic rename failed: {}", e))?;

        // Publish UserProfileUpdated event for synchronous activation
        self.ctx
            .bus
            .publish(crate::events::SystemEvent::UserProfileUpdated {
                actor: "agent".to_string(),
                mode: mode.to_string(),
                content_sha256: hash.clone(),
                modified_sections: modified_sections.clone(),
                backup_path: backup_path.clone(),
                timestamp: chrono::Utc::now(),
            });

        let mut result = serde_json::json!({
            "status": "applied",
            "mode": mode,
            "path": self.ctx.user_path.display().to_string(),
            "content_sha256": hash,
            "content_length": validated_markdown.len(),
            "modified_sections": modified_sections,
            "message": "USER.md updated successfully."
        });

        if let Some(bp) = backup_path {
            result["backup_path"] = serde_json::Value::String(bp);
        }

        Ok(result.to_string())
    }
}

impl UserUpdateTool {
    /// Validate the "replace" mode: decode base64, parse as USER markdown.
    fn validate_replace(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let b64 = arguments
            .get("content_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Missing required parameter: content_b64 (base64-encoded USER markdown)".to_string()
            })?;

        if b64.trim().is_empty() {
            return Err("content_b64 must not be empty.".to_string());
        }

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("Invalid base64 encoding: {}", e))?;

        let content = String::from_utf8(decoded)
            .map_err(|e| format!("Decoded content is not valid UTF-8: {}", e))?;

        if content.trim().is_empty() {
            return Err("Decoded USER content is empty.".to_string());
        }

        // Validate frontmatter (lenient: sections are optional)
        parse_user_markdown(&content)
            .map_err(|e| format!("USER validation failed: {}", e))?;

        Ok(content)
    }

    /// Validate the "sections" mode: read current USER, apply patches, render, reparse.
    async fn validate_sections(
        &self,
        arguments: &serde_json::Value,
    ) -> Result<(String, Vec<String>), String> {
        let sections = arguments.get("sections").ok_or_else(|| {
            "Missing required parameter: sections (object with section patches)".to_string()
        })?;

        let obj = sections
            .as_object()
            .ok_or_else(|| "Parameter 'sections' must be a JSON object.".to_string())?;

        if obj.is_empty() {
            return Err("Sections patch object must not be empty.".to_string());
        }

        // Reject unknown section fields
        for key in obj.keys() {
            if !KNOWN_USER_SECTION_FIELDS.contains(&key.as_str()) {
                return Err(format!(
                    "Unknown section field '{}'. Allowed: {}",
                    key,
                    KNOWN_USER_SECTION_FIELDS.join(", ")
                ));
            }
        }

        // Read current USER file
        let current_content = tokio::fs::read_to_string(&self.ctx.user_path)
            .await
            .map_err(|e| format!("Failed to read current USER file: {}", e))?;

        let mut doc = parse_user_markdown(&current_content)
            .map_err(|e| format!("Current USER file is invalid (cannot patch): {}", e))?;

        let mut modified = Vec::new();

        // Apply patches
        if let Some(v) = obj.get("identity") {
            let identity_obj = v
                .as_object()
                .ok_or_else(|| "sections.identity must be a JSON object".to_string())?;
            for (key, val) in identity_obj {
                let s = val
                    .as_str()
                    .ok_or_else(|| format!("identity.{} must be a string", key))?;
                if s.is_empty() {
                    doc.identity.remove(key);
                } else {
                    doc.identity.insert(key.clone(), s.to_string());
                }
            }
            modified.push("identity".to_string());
        }
        if let Some(v) = obj.get("communication_style") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.communication_style must be a string".to_string())?;
            doc.communication_style = s.to_string();
            modified.push("communication_style".to_string());
        }
        if let Some(v) = obj.get("expertise") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.expertise must be a string".to_string())?;
            doc.expertise = s.to_string();
            modified.push("expertise".to_string());
        }
        if let Some(v) = obj.get("projects") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.projects must be a string".to_string())?;
            doc.projects = s.to_string();
            modified.push("projects".to_string());
        }
        if let Some(v) = obj.get("preferences") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.preferences must be a string".to_string())?;
            doc.preferences = s.to_string();
            modified.push("preferences".to_string());
        }
        if let Some(v) = obj.get("notes") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.notes must be a string".to_string())?;
            doc.notes = s.to_string();
            modified.push("notes".to_string());
        }

        // Render and reparse for safety
        let rendered = render_user_markdown(&doc);
        parse_user_markdown(&rendered)
            .map_err(|e| format!("Patched USER failed re-validation: {}", e))?;

        Ok((rendered, modified))
    }
}

pub(super) fn update_user_tool(ctx: UserToolContext) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "update_user".to_string(),
            description: "Safely update the USER.md profile file. \
                Use mode=\"replace\" with content_b64 (base64-encoded full markdown) \
                for complete replacement, or mode=\"sections\" with a sections object \
                to patch individual sections (identity, communication_style, expertise, \
                projects, preferences, notes). All updates are validated before applying."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["replace", "sections"],
                        "description": "Update mode: 'replace' for full content, 'sections' for patch"
                    },
                    "content_b64": {
                        "type": "string",
                        "description": "Base64-encoded full USER markdown (required for 'replace' mode)"
                    },
                    "sections": {
                        "type": "object",
                        "description": "Section patches (required for 'sections' mode)",
                        "properties": {
                            "identity": {
                                "type": "object",
                                "description": "Key-value pairs for identity fields (Name, Timezone, etc.)"
                            },
                            "communication_style": { "type": "string" },
                            "expertise": { "type": "string" },
                            "projects": { "type": "string" },
                            "preferences": { "type": "string" },
                            "notes": { "type": "string" }
                        }
                    }
                },
                "required": ["mode"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(UserUpdateTool { ctx })),
    }
}

/// Generate a unique backup path for USER.md with nanosecond timestamp.
fn unique_user_backup_path(backup_dir: &std::path::Path) -> std::path::PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S.%9fZ");
    let base_name = format!("USER.{}.md", ts);
    let candidate = backup_dir.join(&base_name);
    if !candidate.exists() {
        return candidate;
    }
    for suffix in 1..1000 {
        let name = format!("USER.{}.{}.md", ts, suffix);
        let candidate = backup_dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }
    backup_dir.join(format!("USER.{}.{}.md", ts, uuid::Uuid::new_v4()))
}

/// Prune old USER backups, keeping at most `max` files.
async fn prune_user_backups(backup_dir: &std::path::Path, max: usize) {
    let mut entries: Vec<std::path::PathBuf> = match std::fs::read_dir(backup_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("USER.") && n.ends_with(".md"))
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => return,
    };

    if entries.len() <= max {
        return;
    }

    entries.sort();
    let to_remove = entries.len() - max;
    for path in entries.into_iter().take(to_remove) {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!("Failed to prune USER backup {}: {}", path.display(), e);
        }
    }
}
