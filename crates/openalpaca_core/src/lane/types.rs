use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

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
pub struct ConversationLane {
    pub key: LaneKey,
    pub created_at: DateTime<Utc>,
    message_count: AtomicUsize,
    last_active_at: Mutex<DateTime<Utc>>,
}

impl std::fmt::Debug for ConversationLane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationLane")
            .field("key", &self.key)
            .field("created_at", &self.created_at)
            .field("message_count", &self.message_count.load(Ordering::Relaxed))
            .finish()
    }
}

impl ConversationLane {
    pub fn new(key: LaneKey) -> Self {
        let now = Utc::now();
        Self {
            key,
            created_at: now,
            message_count: AtomicUsize::new(0),
            last_active_at: Mutex::new(now),
        }
    }

    /// Record that a message was processed on this lane.
    pub fn record_message(&self) {
        self.message_count.fetch_add(1, Ordering::Relaxed);
        *self.last_active_at.lock().unwrap() = Utc::now();
    }

    /// Get the number of messages processed on this lane.
    pub fn message_count(&self) -> usize {
        self.message_count.load(Ordering::Relaxed)
    }

    /// Get the last time a message was processed on this lane.
    pub fn last_active_at(&self) -> DateTime<Utc> {
        *self.last_active_at.lock().unwrap()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_lane_message_tracking() {
        let lane = ConversationLane::new(LaneKey::new("u1", "cli"));
        assert_eq!(lane.message_count(), 0);

        lane.record_message();
        assert_eq!(lane.message_count(), 1);

        lane.record_message();
        lane.record_message();
        assert_eq!(lane.message_count(), 3);
    }

    #[test]
    fn test_conversation_lane_last_active_at() {
        let lane = ConversationLane::new(LaneKey::new("u1", "cli"));
        let initial = lane.last_active_at();

        // Small sleep to ensure time advances
        std::thread::sleep(std::time::Duration::from_millis(10));
        lane.record_message();

        assert!(lane.last_active_at() >= initial);
    }

    #[test]
    fn test_lane_key_equality() {
        let k1 = LaneKey::new("u1", "cli");
        let k2 = LaneKey::new("u1", "cli");
        let k3 = LaneKey::new("u1", "gui");
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }
}
