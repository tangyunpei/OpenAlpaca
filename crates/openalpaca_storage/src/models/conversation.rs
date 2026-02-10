//! Conversation message model for chat persistence

use serde::{Deserialize, Serialize};

/// A single message in a conversation, persisted to SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub id: i64,
    pub lane_key: String,
    pub role: String,
    pub content: String,
    pub source: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}

/// A conversation master record, tracking all conversations across sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub lane_key: String,
    pub source: String,
    pub title: String,
    pub message_count: i64,
    pub last_message_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub summary: String,
    pub summary_version: i64,
    pub last_summarized_message_id: i64,
    pub summary_updated_at: Option<String>,
}
