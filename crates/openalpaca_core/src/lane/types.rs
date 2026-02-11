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

impl std::fmt::Display for LaneKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.user_id, self.source)
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

/// Status of a task lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLaneStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Paused,
}

impl TaskLaneStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
        }
    }
}

/// A task lane: tracks a single background task.
pub struct TaskLane {
    pub task_id: String,
    pub source_lane: Option<LaneKey>,
    pub created_at: DateTime<Utc>,
    status: Mutex<TaskLaneStatus>,
    assigned_agents: Mutex<Vec<String>>,
}

impl std::fmt::Debug for TaskLane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskLane")
            .field("task_id", &self.task_id)
            .field("source_lane", &self.source_lane)
            .field("created_at", &self.created_at)
            .field("status", &*self.status.lock().unwrap())
            .finish()
    }
}

impl TaskLane {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            source_lane: None,
            created_at: Utc::now(),
            status: Mutex::new(TaskLaneStatus::Queued),
            assigned_agents: Mutex::new(Vec::new()),
        }
    }

    /// Set the source lane that originated this task.
    pub fn with_source(mut self, source: LaneKey) -> Self {
        self.source_lane = Some(source);
        self
    }

    /// Get the current status.
    pub fn status(&self) -> TaskLaneStatus {
        *self.status.lock().unwrap()
    }

    /// Set the status.
    pub fn set_status(&self, status: TaskLaneStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// Assign an agent to this task.
    pub fn assign_agent(&self, agent_id: String) {
        self.assigned_agents.lock().unwrap().push(agent_id);
    }

    /// Get the list of assigned agents.
    pub fn assigned_agents(&self) -> Vec<String> {
        self.assigned_agents.lock().unwrap().clone()
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
    fn test_lane_key_display() {
        assert_eq!(LaneKey::new("user1", "telegram").to_string(), "user1:telegram");
        assert_eq!(LaneKey::new("abc", "gui").to_string(), "abc:gui");
    }

    #[test]
    fn test_lane_key_equality() {
        let k1 = LaneKey::new("u1", "cli");
        let k2 = LaneKey::new("u1", "cli");
        let k3 = LaneKey::new("u1", "gui");
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn test_task_lane_status() {
        let lane = TaskLane::new("task-1");
        assert_eq!(lane.status(), TaskLaneStatus::Queued);

        lane.set_status(TaskLaneStatus::Running);
        assert_eq!(lane.status(), TaskLaneStatus::Running);

        lane.set_status(TaskLaneStatus::Completed);
        assert_eq!(lane.status(), TaskLaneStatus::Completed);
    }

    #[test]
    fn test_task_lane_with_source() {
        let lane = TaskLane::new("task-1").with_source(LaneKey::new("u1", "cli"));
        assert_eq!(
            lane.source_lane,
            Some(LaneKey::new("u1", "cli"))
        );
    }

    #[test]
    fn test_task_lane_agents() {
        let lane = TaskLane::new("task-1");
        assert!(lane.assigned_agents().is_empty());

        lane.assign_agent("agent-1".into());
        lane.assign_agent("agent-2".into());

        let agents = lane.assigned_agents();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0], "agent-1");
        assert_eq!(agents[1], "agent-2");
    }
}
