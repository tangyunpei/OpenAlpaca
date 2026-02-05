use std::collections::HashMap;
use std::sync::Mutex;

/// Registry for tracking active tasks.
pub struct TaskRegistry {
    tasks: Mutex<HashMap<String, String>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Register a task. Returns false if the task_id already exists.
    pub fn register(&self, task_id: String, description: String) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.contains_key(&task_id) {
            return false;
        }
        tasks.insert(task_id, description);
        true
    }

    /// Remove a task by id. Returns true if it existed.
    pub fn remove(&self, task_id: &str) -> bool {
        self.tasks.lock().unwrap().remove(task_id).is_some()
    }

    /// Number of active tasks.
    pub fn count(&self) -> usize {
        self.tasks.lock().unwrap().len()
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
    fn test_agent_state_store() {
        let store = AgentStateStore::new();
        assert!(store.get("a1").is_none());
        store.set("a1".into(), "idle".into());
        assert_eq!(store.get("a1").unwrap(), "idle");
        store.set("a1".into(), "running".into());
        assert_eq!(store.get("a1").unwrap(), "running");
    }
}
