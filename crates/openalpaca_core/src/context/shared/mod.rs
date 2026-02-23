use crate::agent::registry::AgentRegistry;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

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

    /// Acquire the tasks lock, recovering from poisoning if necessary.
    fn lock_tasks(&self) -> std::sync::MutexGuard<'_, HashMap<String, TaskEntry>> {
        match self.tasks.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("TaskRegistry mutex poisoned — recovering");
                poisoned.into_inner()
            }
        }
    }

    /// Register a task. Returns false if the task_id already exists.
    pub fn register(&self, task_id: String, title: String) -> bool {
        let mut tasks = self.lock_tasks();
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
        let mut tasks = self.lock_tasks();
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
        self.lock_tasks().get(task_id).cloned()
    }

    /// Remove a task by id. Returns true if it existed.
    pub fn remove(&self, task_id: &str) -> bool {
        self.lock_tasks().remove(task_id).is_some()
    }

    /// Number of tracked tasks.
    pub fn count(&self) -> usize {
        self.lock_tasks().len()
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
        let mut tasks = self.lock_tasks();
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
        self.lock_tasks()
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
    /// Cancellation tokens for running tasks, keyed by task_id.
    cancellation_tokens: Mutex<HashMap<String, CancellationToken>>,
}

impl SharedContext {
    pub fn new() -> Self {
        Self {
            task_registry: TaskRegistry::new(),
            agent_registry: Arc::new(AgentRegistry::new()),
            cancellation_tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Register a cancellation token for a task.
    pub fn register_cancellation_token(&self, task_id: &str, token: CancellationToken) {
        let mut tokens = self
            .cancellation_tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        tokens.insert(task_id.to_string(), token);
    }

    /// Trigger cancellation for a task. Returns `true` if the token was found.
    pub fn cancel_task(&self, task_id: &str) -> bool {
        let tokens = self
            .cancellation_tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(token) = tokens.get(task_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Remove a cancellation token after the task has finished (cleanup).
    pub fn remove_cancellation_token(&self, task_id: &str) {
        let mut tokens = self
            .cancellation_tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        tokens.remove(task_id);
    }
}

impl Default for SharedContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
