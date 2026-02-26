use crate::middleware::identity::{parse_identity_markdown, render_identity_markdown};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use base64::Engine as _;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

use super::IdentityToolContext;

struct IdentityUpdateTool {
    ctx: IdentityToolContext,
}

/// Known top-level fields in the update_identity tool arguments.
const KNOWN_IDENTITY_FIELDS: &[&str] = &["mode", "content_b64", "sections"];
/// Known section-patch fields inside the `sections` object.
const KNOWN_IDENTITY_SECTION_FIELDS: &[&str] = &["name", "creature", "vibe", "emoji", "avatar"];

#[async_trait]
impl BuiltInTool for IdentityUpdateTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Reject unknown top-level fields
        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
                if key == "owner_id" {
                    continue; // Injected by ContextualToolExecutor
                }
                if !KNOWN_IDENTITY_FIELDS.contains(&key.as_str()) {
                    return Err(format!(
                        "Unknown field '{}'. Allowed fields: {}",
                        key,
                        KNOWN_IDENTITY_FIELDS.join(", ")
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

        let validated_markdown = match mode {
            "replace" => self.validate_replace(arguments)?,
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

        // -- Backup current IDENTITY file (if it exists) --
        let backup_path = if self.ctx.identity_path.exists() {
            let backup_dir = &self.ctx.backup_dir;

            tokio::fs::create_dir_all(backup_dir)
                .await
                .map_err(|e| format!("Failed to create backup directory: {}", e))?;

            let backup_path = super::helpers::unique_backup_path(backup_dir, "IDENTITY");
            tokio::fs::copy(&self.ctx.identity_path, &backup_path)
                .await
                .map_err(|e| format!("Failed to create backup: {}", e))?;

            Some(backup_path.display().to_string())
        } else {
            None
        };

        // Prune old backups if retention limit is configured
        if let Some(max) = self.ctx.max_backups {
            super::helpers::prune_backups(&self.ctx.backup_dir, max, "IDENTITY").await;
        }

        // -- Atomic write: temp file -> fsync -> rename --
        let identity_dir = self
            .ctx
            .identity_path
            .parent()
            .ok_or_else(|| "IDENTITY path has no parent directory".to_string())?;

        let tmp_path = identity_dir.join(".IDENTITY.md.tmp");

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
        tokio::fs::rename(&tmp_path, &self.ctx.identity_path)
            .await
            .map_err(|e| format!("Atomic rename failed: {}", e))?;

        // Publish IdentityUpdated event for synchronous activation
        self.ctx
            .bus
            .publish(crate::events::SystemEvent::IdentityUpdated {
                actor: "agent".to_string(),
                mode: mode.to_string(),
                content_sha256: hash.clone(),
                backup_path: backup_path.clone(),
                timestamp: chrono::Utc::now(),
            });

        let mut result = serde_json::json!({
            "status": "applied",
            "mode": mode,
            "path": self.ctx.identity_path.display().to_string(),
            "content_sha256": hash,
            "content_length": validated_markdown.len(),
            "message": "IDENTITY.md updated successfully."
        });

        if let Some(bp) = backup_path {
            result["backup_path"] = serde_json::Value::String(bp);
        }

        Ok(result.to_string())
    }
}

impl IdentityUpdateTool {
    /// Validate the "replace" mode: decode base64, parse as IDENTITY markdown.
    fn validate_replace(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let b64 = arguments
            .get("content_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Missing required parameter: content_b64 (base64-encoded IDENTITY markdown)"
                    .to_string()
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
            return Err("Decoded IDENTITY content is empty.".to_string());
        }

        // Validate frontmatter (lenient: body fields are optional)
        parse_identity_markdown(&content)
            .map_err(|e| format!("IDENTITY validation failed: {}", e))?;

        Ok(content)
    }

    /// Validate the "sections" mode: read current IDENTITY, apply patches, render, reparse.
    async fn validate_sections(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let sections = arguments.get("sections").ok_or_else(|| {
            "Missing required parameter: sections (object with field patches)".to_string()
        })?;

        let obj = sections
            .as_object()
            .ok_or_else(|| "sections must be a JSON object".to_string())?;

        // Reject unknown section fields
        for key in obj.keys() {
            if !KNOWN_IDENTITY_SECTION_FIELDS.contains(&key.as_str()) {
                return Err(format!(
                    "Unknown section field '{}'. Allowed: {}",
                    key,
                    KNOWN_IDENTITY_SECTION_FIELDS.join(", ")
                ));
            }
        }

        if obj.is_empty() {
            return Err("sections object must contain at least one field to update".to_string());
        }

        // Read current file
        let current_content = tokio::fs::read_to_string(&self.ctx.identity_path)
            .await
            .map_err(|e| format!("Failed to read current IDENTITY.md: {}", e))?;

        let mut doc = parse_identity_markdown(&current_content)
            .map_err(|e| format!("Failed to parse current IDENTITY.md: {}", e))?;

        // Apply patches
        if let Some(v) = obj.get("name").and_then(|v| v.as_str()) {
            doc.name = v.to_string();
        }
        if let Some(v) = obj.get("creature").and_then(|v| v.as_str()) {
            doc.creature = v.to_string();
        }
        if let Some(v) = obj.get("vibe").and_then(|v| v.as_str()) {
            doc.vibe = v.to_string();
        }
        if let Some(v) = obj.get("emoji").and_then(|v| v.as_str()) {
            doc.emoji = v.to_string();
        }
        if let Some(v) = obj.get("avatar").and_then(|v| v.as_str()) {
            doc.avatar = v.to_string();
        }

        // Render and re-parse to validate
        let rendered = render_identity_markdown(&doc);
        parse_identity_markdown(&rendered)
            .map_err(|e| format!("Post-patch IDENTITY validation failed: {}", e))?;

        Ok(rendered)
    }
}

pub(super) fn update_identity_tool(ctx: IdentityToolContext) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "update_identity".to_string(),
            description: "Safely update the IDENTITY.md file (your self-identity). \
                Use mode=\"replace\" with content_b64 (base64-encoded full markdown) \
                for complete replacement, or mode=\"sections\" with a sections object \
                to patch individual fields (name, creature, vibe, emoji, avatar). \
                All updates are validated before applying."
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
                        "description": "Base64-encoded full IDENTITY markdown (required for 'replace' mode)"
                    },
                    "sections": {
                        "type": "object",
                        "description": "Field patches (required for 'sections' mode)",
                        "properties": {
                            "name": { "type": "string", "description": "The orchestrator's chosen name" },
                            "creature": { "type": "string", "description": "What kind of entity it is" },
                            "vibe": { "type": "string", "description": "How it comes across" },
                            "emoji": { "type": "string", "description": "Signature emoji" },
                            "avatar": { "type": "string", "description": "Avatar path, URL, or data URI" }
                        }
                    }
                },
                "required": ["mode"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(IdentityUpdateTool { ctx })),
    }
}
