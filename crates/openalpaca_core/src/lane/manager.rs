use super::types::{ConversationLane, LaneKey, TaskLane};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Manages conversation and task lanes.
pub struct LaneManager {
    conversations: Mutex<HashMap<LaneKey, Arc<ConversationLane>>>,
    tasks: Mutex<HashMap<String, Arc<TaskLane>>>,
}

impl LaneManager {
    pub fn new() -> Self {
        Self {
            conversations: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Get or create a conversation lane for the given key (idempotent).
    pub fn get_or_create_conversation(&self, key: LaneKey) -> Arc<ConversationLane> {
        let mut convs = self.conversations.lock().unwrap();
        convs
            .entry(key.clone())
            .or_insert_with(|| Arc::new(ConversationLane::new(key)))
            .clone()
    }

    /// Create a new task lane.
    pub fn create_task_lane(&self, task_id: impl Into<String>) -> Arc<TaskLane> {
        let task_id = task_id.into();
        let lane = Arc::new(TaskLane::new(task_id.clone()));
        self.tasks.lock().unwrap().insert(task_id, lane.clone());
        lane
    }

    /// Get a task lane by ID.
    pub fn get_task_lane(&self, task_id: &str) -> Option<Arc<TaskLane>> {
        self.tasks.lock().unwrap().get(task_id).cloned()
    }

    /// Remove a task lane by ID.
    pub fn remove_task_lane(&self, task_id: &str) -> Option<Arc<TaskLane>> {
        self.tasks.lock().unwrap().remove(task_id)
    }

    /// Number of active conversation lanes.
    pub fn conversation_count(&self) -> usize {
        self.conversations.lock().unwrap().len()
    }

    /// Number of active task lanes.
    pub fn task_count(&self) -> usize {
        self.tasks.lock().unwrap().len()
    }
}

impl Default for LaneManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_lane_creation() {
        let mgr = LaneManager::new();
        let key = LaneKey::new("user1", "telegram");
        let lane = mgr.get_or_create_conversation(key);
        assert_eq!(lane.key.user_id, "user1");
        assert_eq!(lane.key.source, "telegram");
        assert_eq!(mgr.conversation_count(), 1);
    }

    #[test]
    fn test_get_or_create_idempotent() {
        let mgr = LaneManager::new();
        let key = LaneKey::new("user1", "cli");
        let lane1 = mgr.get_or_create_conversation(key.clone());
        let lane2 = mgr.get_or_create_conversation(key);
        assert!(Arc::ptr_eq(&lane1, &lane2));
        assert_eq!(mgr.conversation_count(), 1);
    }

    #[test]
    fn test_task_lane_creation() {
        let mgr = LaneManager::new();
        let lane = mgr.create_task_lane("task-abc");
        assert_eq!(lane.task_id, "task-abc");
        assert_eq!(mgr.task_count(), 1);
    }

    #[test]
    fn test_get_task_lane() {
        let mgr = LaneManager::new();
        assert!(mgr.get_task_lane("nope").is_none());

        mgr.create_task_lane("t1");
        let lane = mgr.get_task_lane("t1").unwrap();
        assert_eq!(lane.task_id, "t1");
    }

    #[test]
    fn test_remove_task_lane() {
        let mgr = LaneManager::new();
        mgr.create_task_lane("t1");
        assert_eq!(mgr.task_count(), 1);

        let removed = mgr.remove_task_lane("t1");
        assert!(removed.is_some());
        assert_eq!(mgr.task_count(), 0);
        assert!(mgr.remove_task_lane("t1").is_none());
    }

    #[test]
    fn test_counts() {
        let mgr = LaneManager::new();
        assert_eq!(mgr.conversation_count(), 0);
        assert_eq!(mgr.task_count(), 0);

        mgr.get_or_create_conversation(LaneKey::new("u1", "cli"));
        mgr.get_or_create_conversation(LaneKey::new("u2", "gui"));
        mgr.create_task_lane("t1");

        assert_eq!(mgr.conversation_count(), 2);
        assert_eq!(mgr.task_count(), 1);
    }
}
