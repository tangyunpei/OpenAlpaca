use crate::bus::EventBus;
use crate::middleware::identity::{parse_identity_markdown, render_identity_markdown};
use async_trait::async_trait;
use base64::Engine as _;
use std::path::{Path, PathBuf};

/// Known section-patch fields for IDENTITY documents.
const KNOWN_SECTION_FIELDS: &[&str] = &["name", "creature", "vibe", "emoji", "avatar"];

pub(super) struct IdentityHandler {
    pub identity_path: PathBuf,
    pub backup_dir: PathBuf,
    pub bus: EventBus,
    pub max_backups: Option<usize>,
}

#[async_trait]
impl super::PersonaHandler for IdentityHandler {
    fn target_name(&self) -> &'static str {
        "identity"
    }

    fn document_path(&self) -> &Path {
        &self.identity_path
    }

    fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    fn backup_prefix(&self) -> &'static str {
        "IDENTITY"
    }

    fn max_backups(&self) -> Option<usize> {
        self.max_backups
    }

    fn tmp_filename(&self) -> &'static str {
        ".IDENTITY.md.tmp"
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
            return Err("Decoded IDENTITY content is empty.".to_string());
        }

        parse_identity_markdown(&content)
            .map_err(|e| format!("IDENTITY validation failed: {}", e))?;

        Ok(content)
    }

    async fn validate_sections(
        &self,
        sections: &serde_json::Value,
    ) -> Result<(String, Option<Vec<String>>), String> {
        let obj = sections
            .as_object()
            .ok_or_else(|| "sections must be a JSON object".to_string())?;

        if obj.is_empty() {
            return Err("sections object must contain at least one field to update".to_string());
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

        let current_content = tokio::fs::read_to_string(&self.identity_path)
            .await
            .map_err(|e| format!("Failed to read current IDENTITY.md: {}", e))?;

        let mut doc = parse_identity_markdown(&current_content)
            .map_err(|e| format!("Failed to parse current IDENTITY.md: {}", e))?;

        let mut modified = Vec::new();

        if let Some(v) = obj.get("name").and_then(|v| v.as_str()) {
            doc.name = v.to_string();
            modified.push("name".to_string());
        }
        if let Some(v) = obj.get("creature").and_then(|v| v.as_str()) {
            doc.creature = v.to_string();
            modified.push("creature".to_string());
        }
        if let Some(v) = obj.get("vibe").and_then(|v| v.as_str()) {
            doc.vibe = v.to_string();
            modified.push("vibe".to_string());
        }
        if let Some(v) = obj.get("emoji").and_then(|v| v.as_str()) {
            doc.emoji = v.to_string();
            modified.push("emoji".to_string());
        }
        if let Some(v) = obj.get("avatar").and_then(|v| v.as_str()) {
            doc.avatar = v.to_string();
            modified.push("avatar".to_string());
        }

        let rendered = render_identity_markdown(&doc);
        parse_identity_markdown(&rendered)
            .map_err(|e| format!("Post-patch IDENTITY validation failed: {}", e))?;

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
            .publish(crate::events::SystemEvent::IdentityUpdated {
                actor: "agent".to_string(),
                mode: mode.to_string(),
                content_sha256: hash.to_string(),
                backup_path: backup_path.clone(),
                timestamp: chrono::Utc::now(),
            });
    }
}
