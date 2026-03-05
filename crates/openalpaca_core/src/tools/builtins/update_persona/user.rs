use crate::bus::EventBus;
use crate::middleware::user::{parse_user_markdown, render_user_markdown};
use async_trait::async_trait;
use base64::Engine as _;
use std::path::{Path, PathBuf};

/// Known section-patch fields for USER documents.
const KNOWN_SECTION_FIELDS: &[&str] = &[
    "identity",
    "communication_style",
    "expertise",
    "projects",
    "preferences",
    "notes",
];

pub(super) struct UserHandler {
    pub user_path: PathBuf,
    pub backup_dir: PathBuf,
    pub bus: EventBus,
    pub max_backups: Option<usize>,
}

#[async_trait]
impl super::PersonaHandler for UserHandler {
    fn target_name(&self) -> &'static str {
        "user"
    }

    fn document_path(&self) -> &Path {
        &self.user_path
    }

    fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    fn backup_prefix(&self) -> &'static str {
        "USER"
    }

    fn max_backups(&self) -> Option<usize> {
        self.max_backups
    }

    fn tmp_filename(&self) -> &'static str {
        ".USER.md.tmp"
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
            return Err("Decoded USER content is empty.".to_string());
        }

        parse_user_markdown(&content)
            .map_err(|e| format!("USER validation failed: {}", e))?;

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

        let current_content = tokio::fs::read_to_string(&self.user_path)
            .await
            .map_err(|e| format!("Failed to read current USER file: {}", e))?;

        let mut doc = parse_user_markdown(&current_content)
            .map_err(|e| format!("Current USER file is invalid (cannot patch): {}", e))?;

        let mut modified = Vec::new();

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

        let rendered = render_user_markdown(&doc);
        parse_user_markdown(&rendered)
            .map_err(|e| format!("Patched USER failed re-validation: {}", e))?;

        Ok((rendered, Some(modified)))
    }

    fn publish_event(
        &self,
        mode: &str,
        hash: &str,
        backup_path: &Option<String>,
        modified_sections: Option<Vec<String>>,
    ) {
        self.bus
            .publish(crate::events::SystemEvent::UserProfileUpdated {
                actor: "agent".to_string(),
                mode: mode.to_string(),
                content_sha256: hash.to_string(),
                modified_sections: modified_sections.unwrap_or_else(|| vec!["all".to_string()]),
                backup_path: backup_path.clone(),
                timestamp: chrono::Utc::now(),
            });
    }
}
