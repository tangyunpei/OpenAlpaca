use crate::bus::EventBus;
use crate::middleware::soul::{parse_soul_markdown, render_soul_markdown};
use async_trait::async_trait;
use base64::Engine as _;
use std::path::{Path, PathBuf};

/// Known section-patch fields for SOUL documents.
const KNOWN_SECTION_FIELDS: &[&str] = &[
    "title",
    "summary",
    "core_truths",
    "boundaries",
    "vibe",
    "continuity",
];

pub(super) struct SoulHandler {
    pub soul_path: PathBuf,
    pub backup_dir: PathBuf,
    pub bus: EventBus,
    pub max_backups: Option<usize>,
}

#[async_trait]
impl super::PersonaHandler for SoulHandler {
    fn target_name(&self) -> &'static str {
        "soul"
    }

    fn document_path(&self) -> &Path {
        &self.soul_path
    }

    fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    fn backup_prefix(&self) -> &'static str {
        "SOUL"
    }

    fn max_backups(&self) -> Option<usize> {
        self.max_backups
    }

    fn tmp_filename(&self) -> &'static str {
        ".SOUL.md.tmp"
    }

    fn validate_replace(&self, content_b64: &str) -> Result<String, String> {
        if content_b64.trim().is_empty() {
            return Err("content_b64 must not be empty.".to_string());
        }

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(content_b64.trim())
            .map_err(|e| format!("Invalid base64 encoding: {}", e))?;

        let content = String::from_utf8(decoded)
            .map_err(|e| format!("Decoded content is not valid UTF-8: {}", e))?;

        if content.trim().is_empty() {
            return Err("Decoded SOUL content is empty.".to_string());
        }

        parse_soul_markdown(&content)
            .map_err(|e| format!("SOUL validation failed: {}", e))?;

        Ok(content)
    }

    async fn validate_sections(
        &self,
        sections: &serde_json::Value,
    ) -> Result<(String, Option<Vec<String>>), String> {
        let obj = sections
            .as_object()
            .ok_or_else(|| "Parameter 'sections' must be a JSON object.".to_string())?;

        if obj.is_empty() {
            return Err("Sections patch object must not be empty.".to_string());
        }

        for key in obj.keys() {
            if !KNOWN_SECTION_FIELDS.contains(&key.as_str()) {
                return Err(format!(
                    "Unknown section field '{}'. Allowed: {}",
                    key,
                    KNOWN_SECTION_FIELDS.join(", ")
                ));
            }
        }

        let current_content = tokio::fs::read_to_string(&self.soul_path)
            .await
            .map_err(|e| format!("Failed to read current SOUL file: {}", e))?;

        let mut doc = parse_soul_markdown(&current_content)
            .map_err(|e| format!("Current SOUL file is invalid (cannot patch): {}", e))?;

        let mut modified = Vec::new();

        if let Some(v) = obj.get("title") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.title must be a string".to_string())?;
            doc.frontmatter.title = s.to_string();
            modified.push("title".to_string());
        }
        if let Some(v) = obj.get("summary") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.summary must be a string".to_string())?;
            doc.frontmatter.summary = s.to_string();
            modified.push("summary".to_string());
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
            modified.push("core_truths".to_string());
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
            modified.push("boundaries".to_string());
        }
        if let Some(v) = obj.get("vibe") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.vibe must be a string".to_string())?;
            if s.trim().is_empty() {
                return Err("vibe must not be empty.".to_string());
            }
            doc.vibe = s.to_string();
            modified.push("vibe".to_string());
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
            modified.push("continuity".to_string());
        }

        let rendered = render_soul_markdown(&doc);
        parse_soul_markdown(&rendered)
            .map_err(|e| format!("Patched SOUL failed re-validation: {}", e))?;

        Ok((rendered, Some(modified)))
    }

    fn publish_event(
        &self,
        mode: &str,
        hash: &str,
        backup_path: &Option<String>,
        _modified_sections: Option<Vec<String>>,
    ) {
        self.bus
            .publish(crate::events::SystemEvent::SoulUpdated {
                actor: "agent".to_string(),
                mode: mode.to_string(),
                content_sha256: hash.to_string(),
                backup_path: backup_path.clone(),
                timestamp: chrono::Utc::now(),
            });
    }
}
