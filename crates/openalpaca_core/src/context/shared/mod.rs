use crate::agent::registry::AgentRegistry;
use crate::runner::steering::SteeringInbox;
use crate::session_log::{SessionLogHandle, SessionLogService};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
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
    /// A run a previous incarnation left in flight (§5.6b). The registry is
    /// empty at boot, so nothing ever *enters* this state in memory — the
    /// variant exists because [`TaskEntryStatus`] is the total projection of
    /// `TaskStatus`, and a resurrected DB row must not be reported as
    /// something it is not.
    Interrupted,
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
            Self::Interrupted => "interrupted",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
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
    /// The session event log service, parked here at boot the way the event
    /// bus is (R28) so the runner can reach it without a global. `None` in
    /// every context that has no home store — most tests.
    session_log: OnceLock<Arc<SessionLogService>>,
    /// The session log handle of each running workflow, keyed by task_id —
    /// the exact analogue of `steering_inboxes` above, and for the same
    /// reason: a producer that only knows the task id (`push_steering`) must
    /// be able to narrate into the run's transcript.
    task_session_logs: DashMap<String, SessionLogHandle>,
}

impl SharedContext {
    pub fn new() -> Self {
        Self {
            task_registry: TaskRegistry::new(),
            agent_registry: Arc::new(AgentRegistry::new()),
            cancellation_tokens: Mutex::new(HashMap::new()),
            steering_inboxes: DashMap::new(),
            active_workflows_by_lane: DashMap::new(),
            session_log: OnceLock::new(),
            task_session_logs: DashMap::new(),
        }
    }

    /// Attach the session event log service. Called once, at daemon boot;
    /// a second call is ignored (the first service keeps its writers).
    pub fn set_session_log(&self, service: Arc<SessionLogService>) {
        if self.session_log.set(service).is_err() {
            tracing::warn!("Session log service already attached — keeping the first");
        }
    }

    /// The session event log service, if this daemon has one.
    pub fn session_log(&self) -> Option<&Arc<SessionLogService>> {
        self.session_log.get()
    }

    /// Remember a running workflow's session log handle, so a producer that
    /// only knows the task id can narrate into the right transcript.
    pub fn register_task_session_log(&self, task_id: &str, handle: SessionLogHandle) {
        self.task_session_logs.insert(task_id.to_string(), handle);
    }

    /// The session log of a running workflow, if one was registered.
    pub fn task_session_log(&self, task_id: &str) -> Option<SessionLogHandle> {
        self.task_session_logs.get(task_id).map(|e| e.value().clone())
    }

    /// Forget a workflow's session log (cleanup at detach, beside the
    /// steering inbox's).
    pub fn remove_task_session_log(&self, task_id: &str) {
        self.task_session_logs.remove(task_id);
    }

    /// Register a cancellation token for a task.
    ///
    /// A run takes its token from
    /// [`run_cancellation_token`](Self::run_cancellation_token) instead, so that
    /// a cancel which arrived against a claim is not replaced away.
    pub fn register_cancellation_token(&self, task_id: &str, token: CancellationToken) {
        let mut tokens = self
            .cancellation_tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        tokens.insert(task_id.to_string(), token);
    }

    /// Claim the run slot for `task_id`: register a token iff none is
    /// registered, and say whether the claim succeeded.
    ///
    /// The compare-and-set behind D5's `start` (`POST /v1/tasks/{id}/action
    /// {"action":"start"}`), which re-launches a stored row **under its own
    /// id**. A registered token is what "this id is already running" means
    /// everywhere else in the daemon, so it is also the lock: reading the map
    /// and then dispatching would let two simultaneous `start`s both pass, and
    /// two lead agents on one task id fight over its `state_version` and its
    /// run log.
    ///
    /// "Running" here means **through finalisation**, not "inside the agentic
    /// loop" (R45): a lead agent's background half keeps writing to the row
    /// long after its loop returns, and the terminal status and result land
    /// last. So a run releases its token only after
    /// `finalize_task_with_outcome` (`dispatcher/lead_agent.rs`) — otherwise
    /// this claim would succeed on a row that still says `running`, and the
    /// finishing run's result would land on the row the new one now owns.
    ///
    /// **The token this inserts is the run's own**, returned here and taken
    /// again by the dispatch through
    /// [`run_cancellation_token`](Self::run_cancellation_token). It used to be a
    /// placeholder the dispatch replaced a few lines later, which swallowed any
    /// `cancel` that landed in between — the row read `running`, `cancel_task`
    /// answered `true`, and the cancelled token was then dropped on the floor
    /// (final review, Minor). A caller that claims and then fails to dispatch
    /// must [`remove_cancellation_token`](Self::remove_cancellation_token), or
    /// the id stays claimed until the daemon restarts.
    pub fn claim_run_slot(&self, task_id: &str) -> Option<CancellationToken> {
        use std::collections::hash_map::Entry;
        let mut tokens = self
            .cancellation_tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match tokens.entry(task_id.to_string()) {
            Entry::Occupied(_) => None,
            Entry::Vacant(slot) => Some(slot.insert(CancellationToken::new()).clone()),
        }
    }

    /// The token the run under `task_id` must watch: whatever a claim already
    /// installed, or a fresh one registered now.
    ///
    /// This is the other half of [`claim_run_slot`](Self::claim_run_slot)'s
    /// contract. A dispatch that minted its own token and registered it over the
    /// claim's lost every cancel issued in the window between the two; taking
    /// the claimed token means such a cancel is already set on the token the
    /// agentic loop checks at the top of its first round.
    pub fn run_cancellation_token(&self, task_id: &str) -> CancellationToken {
        let mut tokens = self
            .cancellation_tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        tokens
            .entry(task_id.to_string())
            .or_insert_with(CancellationToken::new)
            .clone()
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
    ///
    /// "Finished" means the row is terminal, not that the agentic loop
    /// returned — see [`claim_run_slot`](Self::claim_run_slot) for why the
    /// difference matters.
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
