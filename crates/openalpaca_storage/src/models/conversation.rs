//! Conversation message model for chat persistence

use serde::{Deserialize, Serialize};

/// A single message in a conversation, persisted to SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub id: i64,
    pub lane_key: String,
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}
