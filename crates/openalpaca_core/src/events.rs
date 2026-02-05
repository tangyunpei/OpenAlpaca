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
    /// A new task was created
    TaskCreated {
        task_id: String,
        title: String,
        created_by: String,
        timestamp: DateTime<Utc>,
    },
    /// A task was updated (status change, progress update)
    TaskUpdated {
        task_id: String,
        status: String,
        progress_current: Option<i32>,
        progress_total: Option<i32>,
        timestamp: DateTime<Utc>,
    },
    /// A task completed successfully
    TaskCompleted {
        task_id: String,
        result_summary: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// A task failed
    TaskFailed {
        task_id: String,
        error: String,
        timestamp: DateTime<Utc>,
    },
    /// An agent was registered or config updated
    AgentRegistered {
        agent_id: String,
        name: String,
        timestamp: DateTime<Utc>,
    },
    /// An agent's status changed
    AgentStatusChanged {
        agent_id: String,
        status: String,
        current_task_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// The Orchestrator classified a user's intent
    IntentClassified {
        request_id: Uuid,
        intent_type: String,
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeEvent {
    Timer { job_id: String, tag: String },
    FileChanged { path: String, change_type: String },
}
