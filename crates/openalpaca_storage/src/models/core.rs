//! Persisted audit event model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Event log entry for auditing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLog {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub agent_id: Option<String>,
    /// The run this happened inside, or `None` for an event that belongs to no
    /// run (migration 037, GAP-10). Written by
    /// `EventLogRepository::log_for_task`.
    pub task_id: Option<String>,
    pub event_type: String,
    pub detail: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
}
