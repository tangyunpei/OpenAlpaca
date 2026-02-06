//! Gateway-level message persistence
//!
//! Persists user and assistant messages at the Gateway layer so ALL sources
//! (GUI, Telegram, CLI, etc.) get automatic conversation persistence.

use anyhow::Result;
use openalpaca_storage::{ConversationMessage, ConversationRepository, Database};

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
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
        })?;
        repo.increment_message_count(lane_key)?;
        Ok(id)
    }

    /// Persist an assistant message.
    pub fn persist_assistant_message(
        &self,
        lane_key: &str,
        content: &str,
        duration_ms: Option<i64>,
        _source: &str,
    ) -> Result<i64> {
        let repo = ConversationRepository::new(&self.db);
        let id = repo.insert(&ConversationMessage {
            id: 0,
            lane_key: lane_key.to_string(),
            role: "assistant".to_string(),
            content: content.to_string(),
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms,
            created_at: String::new(),
        })?;
        repo.increment_message_count(lane_key)?;
        Ok(id)
    }
}
