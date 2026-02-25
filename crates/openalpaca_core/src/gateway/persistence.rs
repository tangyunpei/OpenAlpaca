//! Gateway-level message persistence
//!
//! Persists user and assistant messages at the Gateway layer so ALL sources
//! (GUI, Telegram, CLI, etc.) get automatic conversation persistence.

use crate::gateway::router::ResolvedAttachment;
use anyhow::Result;
use openalpaca_storage::{ConversationMessage, ConversationRepository, Database, FileAssetRepository};

/// Handles persisting messages to the conversation_messages table.
pub struct GatewayPersistence {
    db: Database,
}

impl GatewayPersistence {
    pub fn new(db: Database) -> Self {
        Self { db }
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
        let mut parts = vec![serde_json::json!({"type": "text", "text": content})];
        for att in attachments {
            parts.push(serde_json::json!({
                "type": "file_ref",
                "file_id": att.file_id,
                "filename": att.filename,
                "mime_type": att.mime_type,
            }));
        }
        let content_json = serde_json::json!({"v": 1, "parts": parts}).to_string();

        // Build display_text
        let filenames: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
        let display_text = if filenames.is_empty() {
            content.to_string()
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
                tracing::warn!("Failed to link attachment {} to message {}: {e}", att.file_id, id);
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
