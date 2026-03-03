use crate::middleware::soul::{parse_soul_markdown, render_soul_markdown};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use base64::Engine as _;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

use super::SoulToolContext;
use super::helpers::{prune_backups, unique_backup_path};

struct SoulUpdateTool {
    ctx: SoulToolContext,
}

/// Known top-level fields in the update_soul tool arguments.
const KNOWN_SOUL_FIELDS: &[&str] = &["mode", "content_b64", "sections"];
/// Known section-patch fields inside the `sections` object.
const KNOWN_SECTION_FIELDS: &[&str] = &[
    "title",
    "summary",
    "core_truths",
    "boundaries",
    "vibe",
    "continuity",
];

#[async_trait]
impl BuiltInTool for SoulUpdateTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Reject unknown top-level fields
        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
                if key == "owner_id" {
                    continue; // Injected by ContextualToolExecutor
                }
                if !KNOWN_SOUL_FIELDS.contains(&key.as_str()) {
                    return Err(format!(
                        "Unknown field '{}'. Allowed fields: {}",
                        key,
                        KNOWN_SOUL_FIELDS.join(", ")
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

        // -- Backup current SOUL file (if it exists) --
        let backup_path = if self.ctx.soul_path.exists() {
            let backup_dir = &self.ctx.backup_dir;

            tokio::fs::create_dir_all(backup_dir)
                .await
                .map_err(|e| format!("Failed to create backup directory: {}", e))?;

            let backup_path = unique_backup_path(backup_dir, "SOUL");
            tokio::fs::copy(&self.ctx.soul_path, &backup_path)
                .await
                .map_err(|e| format!("Failed to create backup: {}", e))?;

            Some(backup_path.display().to_string())
        } else {
            None
        };

        // Prune old backups if retention limit is configured
        if let Some(max) = self.ctx.max_backups {
            prune_backups(&self.ctx.backup_dir, max, "SOUL").await;
        }

        // -- Atomic write: temp file → fsync → rename --
        let soul_dir = self
            .ctx
            .soul_path
            .parent()
            .ok_or_else(|| "SOUL path has no parent directory".to_string())?;

        let tmp_path = soul_dir.join(".SOUL.md.tmp");

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
        tokio::fs::rename(&tmp_path, &self.ctx.soul_path)
            .await
            .map_err(|e| format!("Atomic rename failed: {}", e))?;

        // Publish SoulUpdated event for synchronous activation
        self.ctx
            .bus
            .publish(crate::events::SystemEvent::SoulUpdated {
                actor: "agent".to_string(),
                mode: mode.to_string(),
                content_sha256: hash.clone(),
                backup_path: backup_path.clone(),
                timestamp: chrono::Utc::now(),
            });

        let mut result = serde_json::json!({
            "status": "applied",
            "mode": mode,
            "path": self.ctx.soul_path.display().to_string(),
            "content_sha256": hash,
            "content_length": validated_markdown.len(),
            "message": "SOUL.md updated successfully."
        });

        if let Some(bp) = backup_path {
            result["backup_path"] = serde_json::Value::String(bp);
        }

        Ok(result.to_string())
    }
}

impl SoulUpdateTool {
    /// Validate the "replace" mode: decode base64, parse as SOUL markdown.
    fn validate_replace(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let b64 = arguments
            .get("content_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Missing required parameter: content_b64 (base64-encoded SOUL markdown)".to_string()
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
            return Err("Decoded SOUL content is empty.".to_string());
        }

        // Strict schema validation
        parse_soul_markdown(&content).map_err(|e| format!("SOUL validation failed: {}", e))?;

        Ok(content)
    }

    /// Validate the "sections" mode: read current SOUL, apply patches, render, reparse.
    async fn validate_sections(&self, arguments: &serde_json::Value) -> Result<String, String> {
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
            if !KNOWN_SECTION_FIELDS.contains(&key.as_str()) {
                return Err(format!(
                    "Unknown section field '{}'. Allowed: {}",
                    key,
                    KNOWN_SECTION_FIELDS.join(", ")
                ));
            }
        }

        // Read current SOUL file
        let current_content = tokio::fs::read_to_string(&self.ctx.soul_path)
            .await
            .map_err(|e| format!("Failed to read current SOUL file: {}", e))?;

        let mut doc = parse_soul_markdown(&current_content)
            .map_err(|e| format!("Current SOUL file is invalid (cannot patch): {}", e))?;

        // Apply patches
        if let Some(v) = obj.get("title") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.title must be a string".to_string())?;
            doc.frontmatter.title = s.to_string();
        }
        if let Some(v) = obj.get("summary") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.summary must be a string".to_string())?;
            doc.frontmatter.summary = s.to_string();
        }
        if let Some(v) = obj.get("core_truths") {
            let arr = v
                .as_array()
                .ok_or_else(|| "sections.core_truths must be an array of strings".to_string())?;
            let truths: Result<Vec<String>, String> = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "core_truths items must be strings".to_string())
                })
                .collect();
            let truths = truths?;
            if truths.is_empty() {
                return Err("core_truths must not be empty.".to_string());
            }
            doc.core_truths = truths;
        }
        if let Some(v) = obj.get("boundaries") {
            let arr = v
                .as_array()
                .ok_or_else(|| "sections.boundaries must be an array of strings".to_string())?;
            let bounds: Result<Vec<String>, String> = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "boundaries items must be strings".to_string())
                })
                .collect();
            let bounds = bounds?;
            if bounds.is_empty() {
                return Err("boundaries must not be empty.".to_string());
            }
            doc.boundaries = bounds;
        }
        if let Some(v) = obj.get("vibe") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.vibe must be a string".to_string())?;
            if s.trim().is_empty() {
                return Err("vibe must not be empty.".to_string());
            }
            doc.vibe = s.to_string();
        }
        if let Some(v) = obj.get("continuity") {
            let arr = v
                .as_array()
                .ok_or_else(|| "sections.continuity must be an array of strings".to_string())?;
            let cont: Result<Vec<String>, String> = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "continuity items must be strings".to_string())
                })
                .collect();
            let cont = cont?;
            if cont.is_empty() {
                return Err("continuity must not be empty.".to_string());
            }
            doc.continuity = cont;
        }

        // Render and reparse for safety
        let rendered = render_soul_markdown(&doc);
        parse_soul_markdown(&rendered)
            .map_err(|e| format!("Patched SOUL failed re-validation: {}", e))?;

        Ok(rendered)
    }
}

pub(super) fn update_soul_tool(ctx: SoulToolContext) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "update_soul".to_string(),
            description: "Safely update the system's SOUL.md personality file. \
                Use mode=\"replace\" with content_b64 (base64-encoded full markdown) \
                for complete replacement, or mode=\"sections\" with a sections object \
                to patch individual sections (core_truths, boundaries, vibe, continuity, \
                title, summary). All updates are validated before applying."
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
                        "description": "Base64-encoded full SOUL markdown (required for 'replace' mode)"
                    },
                    "sections": {
                        "type": "object",
                        "description": "Section patches (required for 'sections' mode)",
                        "properties": {
                            "title": { "type": "string" },
                            "summary": { "type": "string" },
                            "core_truths": { "type": "array", "items": { "type": "string" } },
                            "boundaries": { "type": "array", "items": { "type": "string" } },
                            "vibe": { "type": "string" },
                            "continuity": { "type": "array", "items": { "type": "string" } }
                        }
                    }
                },
                "required": ["mode"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(SoulUpdateTool { ctx })),
    }
}

#[cfg(test)]
mod tests;
