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

            let backup_path = unique_backup_path(backup_dir);
            tokio::fs::copy(&self.ctx.soul_path, &backup_path)
                .await
                .map_err(|e| format!("Failed to create backup: {}", e))?;

            Some(backup_path.display().to_string())
        } else {
            None
        };

        // Prune old backups if retention limit is configured
        if let Some(max) = self.ctx.max_backups {
            prune_backups(&self.ctx.backup_dir, max).await;
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
        },
        backend: ToolBackend::BuiltIn(Arc::new(SoulUpdateTool { ctx })),
    }
}

#[cfg(test)]
mod tests {
    use super::super::helpers::{prune_backups, unique_backup_path};
    use super::*;
    use crate::bus::EventBus;

    /// Helper: create a SoulUpdateTool backed by a temp directory with a valid SOUL file.
    fn make_soul_tool() -> (SoulUpdateTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        let valid_soul = r#"---
title: "Test Soul"
summary: "A test soul"
read_when:
  - always
---

## Core Truths

Be helpful.

## Boundaries

- Stay safe.

## Vibe

Friendly and clear.

## Continuity

Remember everything.
"#;
        std::fs::write(&soul_path, valid_soul).unwrap();

        let ctx = SoulToolContext {
            soul_path,
            backup_dir: dir.path().join("backups"),
            bus: EventBus::new(16),
            max_backups: None,
        };
        (SoulUpdateTool { ctx }, dir)
    }

    #[tokio::test]
    async fn test_soul_update_replace_valid() {
        let (tool, _dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok(), "Valid replace should succeed: {:?}", result);
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "applied");
    }

    #[tokio::test]
    async fn test_soul_update_replace_invalid_base64() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": "not-valid-b64!!!"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid base64"));
    }

    #[tokio::test]
    async fn test_soul_update_replace_empty_content() {
        let (tool, _dir) = make_soul_tool();
        // Base64 of just whitespace
        let b64 = base64::engine::general_purpose::STANDARD.encode("   \n  ");
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[tokio::test]
    async fn test_soul_update_replace_invalid_schema() {
        let (tool, _dir) = make_soul_tool();
        // Missing ## Boundaries section
        let invalid = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Boundaries"));
    }

    #[tokio::test]
    async fn test_soul_update_unknown_field_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(
                &serde_json::json!({"mode": "replace", "content_b64": "x", "evil_field": true}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown field"));
    }

    #[tokio::test]
    async fn test_soul_update_missing_mode() {
        let (tool, _dir) = make_soul_tool();
        let result = tool.execute(&serde_json::json!({"content_b64": "x"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mode"));
    }

    #[tokio::test]
    async fn test_soul_update_sections_valid() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "vibe": "Pirate style, arr!" }
            }))
            .await;
        assert!(
            result.is_ok(),
            "Valid sections patch should succeed: {:?}",
            result
        );
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "applied");
    }

    #[tokio::test]
    async fn test_soul_update_sections_empty_patch_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": {}
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[tokio::test]
    async fn test_soul_update_sections_unknown_field_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "evil": "hi" }
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown section field"));
    }

    #[tokio::test]
    async fn test_soul_update_replace_empty_b64_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": ""}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[tokio::test]
    async fn test_soul_update_creates_backup() {
        let (tool, dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // Verify backup directory was created and contains a backup
        let backup_dir = dir.path().join("backups");
        assert!(backup_dir.exists(), "Backup directory should exist");
        let entries: Vec<_> = std::fs::read_dir(&backup_dir).unwrap().collect();
        assert_eq!(entries.len(), 1, "Should have exactly one backup");

        // Verify backup content matches original, not the new content
        let backup_path = entries[0].as_ref().unwrap().path();
        let backup_content = std::fs::read_to_string(&backup_path).unwrap();
        assert!(
            backup_content.contains("Test Soul"),
            "Backup should contain original title"
        );
    }

    #[tokio::test]
    async fn test_soul_update_atomic_write_applies_new_content() {
        let (tool, dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"Updated\"\nsummary: \"Updated soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // Verify the SOUL file now has the new content
        let current = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        assert!(
            current.contains("Updated"),
            "SOUL.md should have new content"
        );
        // Verify no temp file remains
        assert!(
            !dir.path().join(".SOUL.md.tmp").exists(),
            "Temp file should not remain"
        );
    }

    #[tokio::test]
    async fn test_soul_update_result_contains_backup_path() {
        let (tool, _dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nY.\n\n## Boundaries\n\n- Z.\n\n## Vibe\n\nV.\n\n## Continuity\n\nC.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(
            json["backup_path"].is_string(),
            "Result should contain backup_path"
        );
        assert!(
            json["backup_path"].as_str().unwrap().contains("SOUL."),
            "Backup path should contain timestamped name"
        );
    }

    #[tokio::test]
    async fn test_soul_update_validation_failure_does_not_write() {
        let (tool, dir) = make_soul_tool();
        let original = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        // Invalid SOUL - missing Boundaries
        let invalid = "---\ntitle: \"Bad\"\nsummary: \"Bad\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_err());

        // Original file should be untouched
        let after = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        assert_eq!(
            original, after,
            "Failed validation should not modify SOUL.md"
        );
        // No backup should be created for failed validation
        assert!(
            !dir.path().join("backups").exists(),
            "No backup for failed validation"
        );
    }

    #[tokio::test]
    async fn test_soul_update_publishes_soul_updated_event() {
        let (tool, _dir) = make_soul_tool();

        // Subscribe BEFORE executing the tool
        let mut rx = tool.ctx.bus.subscribe();

        let valid_soul = "---\ntitle: \"Evented\"\nsummary: \"Event test\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe evented.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nEventful.\n\n## Continuity\n\nRemember events.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // Verify the SoulUpdated event was published
        let event = rx
            .try_recv()
            .expect("Should have received SoulUpdated event");
        match event {
            crate::events::SystemEvent::SoulUpdated {
                actor,
                mode,
                content_sha256,
                ..
            } => {
                assert_eq!(actor, "agent");
                assert_eq!(mode, "replace");
                assert!(!content_sha256.is_empty(), "Hash should not be empty");
            }
            other => panic!("Expected SoulUpdated, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_soul_update_with_max_backups_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let valid_soul = "---\ntitle: \"Test\"\nsummary: \"Test soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe helpful.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nFriendly.\n\n## Continuity\n\nRemember.\n";
        std::fs::write(&soul_path, valid_soul).unwrap();

        // Pre-create 3 old backups
        for i in 1..=3 {
            std::fs::write(
                backup_dir.join(format!("SOUL.20250101T00000{}Z.md", i)),
                format!("old {}", i),
            )
            .unwrap();
        }

        let ctx = SoulToolContext {
            soul_path,
            backup_dir: backup_dir.clone(),
            bus: EventBus::new(16),
            max_backups: Some(2), // Keep only 2 backups
        };
        let tool = SoulUpdateTool { ctx };

        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // After creating 1 new backup + 3 old = 4 total, pruned to 2
        let count = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|e| e.file_name().to_str().map(|n| n.starts_with("SOUL.")))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count, 2, "Should have pruned to max_backups=2");
    }

    #[tokio::test]
    async fn test_sections_rejects_wrong_type_string_fields() {
        let (tool, _dir) = make_soul_tool();

        // vibe: number instead of string
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "vibe": 123 }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be a string"),
            "vibe=123 should report type error"
        );

        // summary: bool instead of string
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "summary": true }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be a string"),
            "summary=true should report type error"
        );

        // title: array instead of string
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "title": ["array"] }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be a string"),
            "title=[array] should report type error"
        );
    }

    #[tokio::test]
    async fn test_sections_rejects_wrong_type_array_fields() {
        let (tool, _dir) = make_soul_tool();

        // core_truths: string instead of array
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "core_truths": "not array" }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be an array"),
            "core_truths=string should report type error"
        );

        // boundaries: object instead of array
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "boundaries": {"obj": true} }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be an array"),
            "boundaries=object should report type error"
        );

        // continuity: number instead of array
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "continuity": 42 }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be an array"),
            "continuity=42 should report type error"
        );
    }

    #[tokio::test]
    async fn test_wrong_type_does_not_mutate_soul_or_create_backup() {
        let (tool, dir) = make_soul_tool();
        let original = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();

        // Attempt with wrong type for vibe
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "vibe": 999 }
            }))
            .await;
        assert!(result.is_err());

        // File should be unchanged
        let after = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        assert_eq!(
            original, after,
            "SOUL.md should not be modified on type error"
        );

        // No backup directory should be created
        assert!(
            !dir.path().join("backups").exists(),
            "No backup dir should exist for failed type validation"
        );
    }

    #[tokio::test]
    async fn test_rapid_updates_produce_distinct_backups() {
        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        let backup_dir = dir.path().join("backups");

        let valid_soul = "---\ntitle: \"Test\"\nsummary: \"Test soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe helpful.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nFriendly.\n\n## Continuity\n\nRemember.\n";
        std::fs::write(&soul_path, valid_soul).unwrap();

        let ctx = SoulToolContext {
            soul_path,
            backup_dir: backup_dir.clone(),
            bus: EventBus::new(16),
            max_backups: None,
        };
        let tool = SoulUpdateTool { ctx };

        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);

        // Perform 5 rapid updates
        for _ in 0..5 {
            let result = tool
                .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
                .await;
            assert!(result.is_ok(), "Update should succeed: {:?}", result);
        }

        // Count backup files
        let backup_count = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|e| {
                        e.file_name()
                            .to_str()
                            .map(|n| n.starts_with("SOUL.") && n.ends_with(".md"))
                    })
                    .unwrap_or(false)
            })
            .count();

        assert_eq!(
            backup_count, 5,
            "Should have 5 distinct backup files, no overwrites"
        );
    }
}
