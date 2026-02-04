use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Examples of system-wide events that flow through the EventBus.
/// This replaces the loose JSON and separate API types for internal logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SystemEvent {
    /// A heartbeat to keep connections alive or signal healthy status
    Heartbeat { timestamp: DateTime<Utc> },
    /// An event triggered by the scheduler or file watcher
    Wake(WakeEvent),
    /// A raw log message from the system
    Log {
        level: String,
        message: String,
        timestamp: DateTime<Utc>,
    },
    /// A structured request from a user (via Connector or API)
    UserRequest {
        request_id: Uuid,
        source: String, // e.g. "telegram", "http"
        content: String,
        timestamp: DateTime<Utc>,
    },
    /// A response from an Agent
    AgentResponse {
        request_id: Uuid,
        agent_id: String,
        content: String,
        timestamp: DateTime<Utc>,
    },
    /// A system error
    Error {
        code: String,
        message: String,
        timestamp: DateTime<Utc>,
    },
    /// A connector status change
    ConnectorStatus {
        id: String,
        status: String,
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeEvent {
    Timer { job_id: String, tag: String },
    FileChanged { path: String, change_type: String },
}
