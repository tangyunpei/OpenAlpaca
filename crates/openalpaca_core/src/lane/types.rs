use chrono::{DateTime, Utc};

/// A key that uniquely identifies a conversation lane.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaneKey {
    pub user_id: String,
    pub source: String,
}

impl LaneKey {
    pub fn new(user_id: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            source: source.into(),
        }
    }
}

/// The type of lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneType {
    Conversation,
    Task,
}

/// A conversation lane: tracks an ongoing multi-turn conversation.
#[derive(Debug)]
pub struct ConversationLane {
    pub key: LaneKey,
    pub created_at: DateTime<Utc>,
}

impl ConversationLane {
    pub fn new(key: LaneKey) -> Self {
        Self {
            key,
            created_at: Utc::now(),
        }
    }
}

/// A task lane: tracks a single background task.
#[derive(Debug)]
pub struct TaskLane {
    pub task_id: String,
    pub created_at: DateTime<Utc>,
}

impl TaskLane {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            created_at: Utc::now(),
        }
    }
}
