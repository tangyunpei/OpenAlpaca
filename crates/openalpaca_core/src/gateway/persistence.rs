//! Gateway-level message persistence
//!
//! Persists user and assistant messages at the Gateway layer so ALL sources
//! (GUI, Telegram, CLI, etc.) get automatic conversation persistence.

use crate::gateway::router::ResolvedAttachment;
use anyhow::Result;
use openalpaca_storage::{
    ConversationMessage, ConversationRepository, Database, FileAssetRepository,
};

/// Handles persisting messages to the conversation_messages table.
pub struct GatewayPersistence {
    db: Database,
}

impl GatewayPersistence {
    const PERSISTED_ATTACHMENT_TEXT_CHARS: usize = 4000;

    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Access the underlying database (for cross-repository operations).
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Persist a user message, ensuring the conversation master record exists.
    pub fn persist_user_message(&self, lane_key: &str, content: &str, source: &str) -> Result<i64> {
        let repo = ConversationRepository::new(&self.db);
        repo.get_or_create_conversation(lane_key, source)?;
        let id = repo.insert(&ConversationMessage {
            id: 0,
            lane_key: lane_key.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            source: Some(source.to_string()),
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
            content_json: None,
            display_text: None,
        })?;
        repo.increment_message_count(lane_key)?;
        Ok(id)
    }

    /// Persist a user message with file attachments.
    pub fn persist_user_message_with_attachments(
        &self,
        lane_key: &str,
        content: &str,
        source: &str,
        attachments: &[ResolvedAttachment],
    ) -> Result<i64> {
        let repo = ConversationRepository::new(&self.db);
        repo.get_or_create_conversation(lane_key, source)?;

        // Build content_json
        let mut parts = Vec::with_capacity(attachments.len() + 1);
        if !content.trim().is_empty() {
            parts.push(serde_json::json!({"type": "text", "text": content}));
        }
        for att in attachments {
            let extracted = att
                .extracted_text
                .as_ref()
                .map(|t| {
                    t.chars()
                        .take(Self::PERSISTED_ATTACHMENT_TEXT_CHARS)
                        .collect::<String>()
                })
                .filter(|t| !t.trim().is_empty());

            if !att.mime_type.starts_with("image/")
                && !att.mime_type.starts_with("audio/")
                && extracted.is_some()
            {
                parts.push(serde_json::json!({
                    "type": "document",
                    "file_id": att.file_id,
                    "filename": att.filename,
                    "mime_type": att.mime_type,
                    "extracted_text": extracted,
                }));
            } else {
                parts.push(serde_json::json!({
                    "type": "file_ref",
                    "file_id": att.file_id,
                    "filename": att.filename,
                    "mime_type": att.mime_type,
                }));
            }
        }
        let content_json = serde_json::json!({"v": 1, "parts": parts}).to_string();

        // Build display_text
        let filenames: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
        let display_text = if filenames.is_empty() {
            content.to_string()
        } else if content.trim().is_empty() {
            format!("[Attachments: {}]", filenames.join(", "))
        } else {
            format!("{}\n[Attachments: {}]", content, filenames.join(", "))
        };

        let msg = ConversationMessage {
            id: 0,
            lane_key: lane_key.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            source: Some(source.to_string()),
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
            content_json: None,
            display_text: None,
        };

        let id = repo.insert_with_structured(&msg, &content_json, &display_text)?;

        // Link attachments
        let file_repo = FileAssetRepository::new(&self.db);
        for (i, att) in attachments.iter().enumerate() {
            if let Err(e) = file_repo.link_to_message(id, &att.file_id, i as i32, None) {
                tracing::warn!(
                    "Failed to link attachment {} to message {}: {e}",
                    att.file_id,
                    id
                );
            }
        }

        repo.increment_message_count(lane_key)?;
        Ok(id)
    }

    /// Persist an assistant message. Skips empty content to avoid polluting history.
    pub fn persist_assistant_message(
        &self,
        lane_key: &str,
        content: &str,
        duration_ms: Option<i64>,
        source: &str,
    ) -> Result<i64> {
        if content.trim().is_empty() {
            tracing::debug!("Skipping empty assistant message for lane {}", lane_key);
            return Ok(0);
        }
        let repo = ConversationRepository::new(&self.db);
        let id = repo.insert(&ConversationMessage {
            id: 0,
            lane_key: lane_key.to_string(),
            role: "assistant".to_string(),
            content: content.to_string(),
            source: Some(source.to_string()),
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms,
            created_at: String::new(),
            content_json: None,
            display_text: None,
        })?;
        repo.increment_message_count(lane_key)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::Database;

    fn make_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (dir, db)
    }

    fn sample_attachment() -> ResolvedAttachment {
        ResolvedAttachment {
            file_id: "file-1".to_string(),
            filename: "sample.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size_bytes: 123,
            extracted_text: None,
            storage_path: "/tmp/sample.pdf".to_string(),
        }
    }

    fn sample_attachment_with_text(text: &str) -> ResolvedAttachment {
        ResolvedAttachment {
            file_id: "file-doc".to_string(),
            filename: "resume.docx".to_string(),
            mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                .to_string(),
            size_bytes: text.len() as i64,
            extracted_text: Some(text.to_string()),
            storage_path: "/tmp/resume.docx".to_string(),
        }
    }

    #[test]
    fn persist_with_attachments_skips_empty_text_part() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let id = persistence
            .persist_user_message_with_attachments("user1:gui", "", "gui", &[sample_attachment()])
            .expect("persist message");
        assert!(id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user1:gui", 10)
            .expect("load recent messages");
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        let content_json = msg
            .content_json
            .as_deref()
            .expect("content_json should be present");
        let parsed: serde_json::Value =
            serde_json::from_str(content_json).expect("content_json should be valid json");
        let parts = parsed["parts"]
            .as_array()
            .expect("parts should be an array");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "file_ref");
        assert_eq!(
            msg.display_text.as_deref(),
            Some("[Attachments: sample.pdf]")
        );
    }

    #[test]
    fn persist_with_attachments_keeps_non_empty_text_part() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let id = persistence
            .persist_user_message_with_attachments(
                "user2:gui",
                "please analyze",
                "gui",
                &[sample_attachment()],
            )
            .expect("persist message");
        assert!(id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user2:gui", 10)
            .expect("load recent messages");
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        let content_json = msg
            .content_json
            .as_deref()
            .expect("content_json should be present");
        let parsed: serde_json::Value =
            serde_json::from_str(content_json).expect("content_json should be valid json");
        let parts = parsed["parts"]
            .as_array()
            .expect("parts should be an array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "please analyze");
        assert_eq!(parts[1]["type"], "file_ref");
    }

    #[test]
    fn persist_with_attachments_stores_document_part_when_text_available() {
        let (_tmp, db) = make_db();
        let persistence = GatewayPersistence::new(db.clone());

        let id = persistence
            .persist_user_message_with_attachments(
                "user3:gui",
                "review this resume",
                "gui",
                &[sample_attachment_with_text(
                    "Candidate has 5 years experience",
                )],
            )
            .expect("persist message");
        assert!(id > 0);

        let repo = ConversationRepository::new(&db);
        let msgs = repo
            .list_recent_by_lane("user3:gui", 10)
            .expect("load recent messages");
        assert_eq!(msgs.len(), 1);
        let msg = &msgs[0];
        let content_json = msg
            .content_json
            .as_deref()
            .expect("content_json should be present");
        let parsed: serde_json::Value =
            serde_json::from_str(content_json).expect("content_json should be valid json");
        let parts = parsed["parts"]
            .as_array()
            .expect("parts should be an array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "document");
        assert_eq!(
            parts[1]["extracted_text"],
            "Candidate has 5 years experience"
        );
    }
}
