use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Mutex;

/// Status of a task entry in the in-memory registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskEntryStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Paused,
}

impl TaskEntryStatus {
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

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// An in-memory task entry tracked by the registry.
#[derive(Debug, Clone)]
pub struct TaskEntry {
    pub task_id: String,
    pub title: String,
    pub status: TaskEntryStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Registry for tracking active tasks.
pub struct TaskRegistry {
    tasks: Mutex<HashMap<String, TaskEntry>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Register a task. Returns false if the task_id already exists.
    pub fn register(&self, task_id: String, title: String) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.contains_key(&task_id) {
            return false;
        }
        let now = Utc::now();
        tasks.insert(
            task_id.clone(),
            TaskEntry {
                task_id,
                title,
                status: TaskEntryStatus::Queued,
                created_at: now,
                updated_at: now,
            },
        );
        true
    }

    /// Update the status of a task. Returns false if the task doesn't exist.
    pub fn update_status(&self, task_id: &str, status: TaskEntryStatus) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(entry) = tasks.get_mut(task_id) {
            entry.status = status;
            entry.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    /// Get a task entry by ID.
    pub fn get(&self, task_id: &str) -> Option<TaskEntry> {
        self.tasks.lock().unwrap().get(task_id).cloned()
    }

    /// Remove a task by id. Returns true if it existed.
    pub fn remove(&self, task_id: &str) -> bool {
        self.tasks.lock().unwrap().remove(task_id).is_some()
    }

    /// Number of tracked tasks.
    pub fn count(&self) -> usize {
        self.tasks.lock().unwrap().len()
    }

    /// List all non-terminal (active) task entries.
    pub fn list_active(&self) -> Vec<TaskEntry> {
        self.tasks
            .lock()
            .unwrap()
            .values()
            .filter(|e| !e.status.is_terminal())
            .cloned()
            .collect()
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Store for agent runtime states.
pub struct AgentStateStore {
    states: Mutex<HashMap<String, String>>,
}

impl AgentStateStore {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    /// Set an agent's state.
    pub fn set(&self, agent_id: String, state: String) {
        self.states.lock().unwrap().insert(agent_id, state);
    }

    /// Get an agent's state.
    pub fn get(&self, agent_id: &str) -> Option<String> {
        self.states.lock().unwrap().get(agent_id).cloned()
    }
}

impl Default for AgentStateStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared context holding cross-cutting state for the gateway.
pub struct SharedContext {
    pub task_registry: TaskRegistry,
    pub agent_states: AgentStateStore,
}

impl SharedContext {
    pub fn new() -> Self {
        Self {
            task_registry: TaskRegistry::new(),
            agent_states: AgentStateStore::new(),
        }
    }
}

impl Default for SharedContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_context_creation() {
        let ctx = SharedContext::new();
        assert_eq!(ctx.task_registry.count(), 0);
        assert!(ctx.agent_states.get("any").is_none());
    }

    #[test]
    fn test_task_registry_empty() {
        let reg = TaskRegistry::new();
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_task_registry_register_and_remove() {
        let reg = TaskRegistry::new();
        assert!(reg.register("t1".into(), "task one".into()));
        assert!(!reg.register("t1".into(), "duplicate".into()));
        assert_eq!(reg.count(), 1);
        assert!(reg.remove("t1"));
        assert_eq!(reg.count(), 0);
        assert!(!reg.remove("t1"));
    }

    #[test]
    fn test_task_registry_update_status() {
        let reg = TaskRegistry::new();
        reg.register("t1".into(), "task one".into());

        assert!(reg.update_status("t1", TaskEntryStatus::Running));
        let entry = reg.get("t1").unwrap();
        assert_eq!(entry.status, TaskEntryStatus::Running);

        assert!(!reg.update_status("nope", TaskEntryStatus::Running));
    }

    #[test]
    fn test_task_registry_list_active() {
        let reg = TaskRegistry::new();
        reg.register("t1".into(), "queued".into());
        reg.register("t2".into(), "will run".into());
        reg.register("t3".into(), "will complete".into());

        reg.update_status("t2", TaskEntryStatus::Running);
        reg.update_status("t3", TaskEntryStatus::Completed);

        let active = reg.list_active();
        assert_eq!(active.len(), 2); // t1 (queued) and t2 (running)
    }

    #[test]
    fn test_task_entry_status_terminal() {
        assert!(!TaskEntryStatus::Queued.is_terminal());
        assert!(!TaskEntryStatus::Running.is_terminal());
        assert!(!TaskEntryStatus::Paused.is_terminal());
        assert!(TaskEntryStatus::Completed.is_terminal());
        assert!(TaskEntryStatus::Failed.is_terminal());
        assert!(TaskEntryStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_agent_state_store() {
        let store = AgentStateStore::new();
        assert!(store.get("a1").is_none());
        store.set("a1".into(), "idle".into());
        assert_eq!(store.get("a1").unwrap(), "idle");
        store.set("a1".into(), "running".into());
        assert_eq!(store.get("a1").unwrap(), "running");
    }
}
