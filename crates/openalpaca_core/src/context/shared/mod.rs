use crate::agent::registry::AgentRegistry;
use crate::runner::steering::SteeringInbox;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
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
    /// Steering inboxes for running workflows, keyed by task_id.
    steering_inboxes: DashMap<String, Arc<SteeringInbox>>,
    /// Active workflow task_ids per lane, keyed by lane_key.
    active_workflows_by_lane: DashMap<String, Vec<String>>,
}

impl SharedContext {
    pub fn new() -> Self {
        Self {
            task_registry: TaskRegistry::new(),
            agent_registry: Arc::new(AgentRegistry::new()),
            cancellation_tokens: Mutex::new(HashMap::new()),
            steering_inboxes: DashMap::new(),
            active_workflows_by_lane: DashMap::new(),
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

    /// Register a steering inbox for a running workflow.
    pub fn register_steering_inbox(&self, task_id: &str, inbox: Arc<SteeringInbox>) {
        self.steering_inboxes.insert(task_id.to_string(), inbox);
    }

    /// Look up the steering inbox for a workflow, if one is registered.
    pub fn steering_inbox(&self, task_id: &str) -> Option<Arc<SteeringInbox>> {
        self.steering_inboxes
            .get(task_id)
            .map(|entry| Arc::clone(entry.value()))
    }

    /// Deregister a workflow's steering inbox (cleanup at detach).
    /// Returns the inbox if it was registered.
    pub fn remove_steering_inbox(&self, task_id: &str) -> Option<Arc<SteeringInbox>> {
        self.steering_inboxes.remove(task_id).map(|(_, inbox)| inbox)
    }

    /// Record a workflow as active on a lane (deduplicated).
    pub fn register_workflow_for_lane(&self, lane_key: &str, task_id: &str) {
        let mut entry = self
            .active_workflows_by_lane
            .entry(lane_key.to_string())
            .or_default();
        if !entry.iter().any(|id| id == task_id) {
            entry.push(task_id.to_string());
        }
    }

    /// Remove a workflow from a lane; drops the lane entry once empty.
    pub fn deregister_workflow_for_lane(&self, lane_key: &str, task_id: &str) {
        let now_empty = match self.active_workflows_by_lane.get_mut(lane_key) {
            Some(mut entry) => {
                entry.retain(|id| id != task_id);
                entry.is_empty()
            }
            None => return,
        };
        if now_empty {
            self.active_workflows_by_lane
                .remove_if(lane_key, |_, ids| ids.is_empty());
        }
    }

    /// Task ids of workflows currently active on a lane.
    pub fn workflows_for_lane(&self, lane_key: &str) -> Vec<String> {
        self.active_workflows_by_lane
            .get(lane_key)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }
}

impl Default for SharedContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
