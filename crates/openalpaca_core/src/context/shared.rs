use crate::agent::registry::AgentRegistry;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

/// Lightweight summary of DAG execution state for in-memory tracking.
#[derive(Debug, Clone)]
pub struct DagSummary {
    pub total_nodes: usize,
    pub completed_nodes: usize,
    pub running_nodes: usize,
    pub failed_nodes: usize,
}

/// An in-memory task entry tracked by the registry.
#[derive(Debug, Clone)]
pub struct TaskEntry {
    pub task_id: String,
    pub title: String,
    pub status: TaskEntryStatus,
    pub progress_current: Option<i32>,
    pub progress_total: Option<i32>,
    pub dag_summary: Option<DagSummary>,
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
                progress_current: None,
                progress_total: None,
                dag_summary: None,
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

    /// Update progress counters and optional DAG summary for a task.
    /// Returns false if the task doesn't exist.
    pub fn update_progress(
        &self,
        task_id: &str,
        progress_current: i32,
        progress_total: i32,
        dag_summary: Option<DagSummary>,
    ) -> bool {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(entry) = tasks.get_mut(task_id) {
            entry.progress_current = Some(progress_current);
            entry.progress_total = Some(progress_total);
            entry.dag_summary = dag_summary;
            entry.updated_at = Utc::now();
            true
        } else {
            false
        }
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

/// Shared context holding cross-cutting state for the gateway.
pub struct SharedContext {
    pub task_registry: TaskRegistry,
    pub agent_registry: Arc<AgentRegistry>,
}

impl SharedContext {
    pub fn new() -> Self {
        Self {
            task_registry: TaskRegistry::new(),
            agent_registry: Arc::new(AgentRegistry::new()),
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
        assert_eq!(ctx.agent_registry.count(), 0);
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
    fn test_agent_registry_in_shared_context() {
        use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent};

        let ctx = SharedContext::new();
        let agent = SubAgent {
            id: "a1".to_string(),
            name: "Test Agent".to_string(),
            description: None,
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: vec![],
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        };
        assert!(ctx.agent_registry.register(agent));
        assert_eq!(ctx.agent_registry.count(), 1);
        assert!(ctx.agent_registry.get("a1").is_some());
    }
}
