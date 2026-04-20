use chrono::{DateTime, Utc};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// Parse the canonical `"{user_id}:{source}"` format produced by
    /// `LaneKey::to_string()`. Returns None if the input doesn't contain
    /// exactly one `:` separator, or if either side is empty.
    ///
    /// Note: the `Option<Self>` return type is intentional — we don't want
    /// the `FromStr` trait's `Result`-based contract here; plain `Option`
    /// is clearer for a single-error-case parser.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        let (user_id, source) = s.split_once(':')?;
        if user_id.is_empty() || source.is_empty() {
            return None;
        }
        Some(Self::new(user_id, source))
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

/// Per-lane tier-2 memoization slots for the layered compose engine
/// (spec section Component 4). Layer 3 (dynamic context) and Layer 4
/// (history) each get one slot, protected by an independent mutex so they
/// do not serialize against each other within a single `compose()` call.
///
/// Phase 1: slots exist and are empty-initialized. Phase 3 wires the
/// hit/miss flow inside the layer computations.
pub struct LaneCaches {
    pub dynamic_context: Mutex<
        Option<(
            crate::compose::DynamicContextFingerprint,
            std::sync::Arc<crate::compose::DynamicContextOutput>,
        )>,
    >,
    pub history: Mutex<
        Option<(
            crate::compose::HistoryFingerprint,
            std::sync::Arc<crate::compose::HistoryOutput>,
        )>,
    >,
}

impl LaneCaches {
    pub fn new() -> Self {
        Self {
            dynamic_context: Mutex::new(None),
            history: Mutex::new(None),
        }
    }
}

impl Default for LaneCaches {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LaneCaches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaneCaches").finish_non_exhaustive()
    }
}

/// A conversation lane: tracks an ongoing multi-turn conversation.
pub struct ConversationLane {
    pub key: LaneKey,
    pub created_at: DateTime<Utc>,
    message_count: AtomicUsize,
    last_active_at: Mutex<DateTime<Utc>>,
    /// Tier-2 per-lane caches for the compose engine (spec Component 4).
    pub caches: LaneCaches,
}

impl std::fmt::Debug for ConversationLane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationLane")
            .field("key", &self.key)
            .field("created_at", &self.created_at)
            .field("message_count", &self.message_count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
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
            caches: LaneCaches::new(),
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

    /// Deterministic fingerprint of the lane's current tip state. Advances
    /// on every new message via `record_message`. Feeds
    /// `HistoryInput.lane_tip_fingerprint` so Layer 4's per-lane cache busts
    /// on turn advance. No lane-content info leaks out — hash covers only
    /// `lane_id + message_count`.
    pub fn compute_tip_fingerprint(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        let key_str = self.key.to_string();
        h.update(&(key_str.len() as u64).to_le_bytes());
        h.update(key_str.as_bytes());
        h.update(&(self.message_count() as u64).to_le_bytes());
        h.finalize().into()
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
mod tests;
