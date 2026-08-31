use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageFeedback {
    pub id: Option<i64>,
    pub message_id: i64,
    pub feedback: String, // "positive" | "negative"
    pub comment: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}
