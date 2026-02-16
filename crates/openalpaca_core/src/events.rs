use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Re-export WakeEvent from API so downstream code can still use
// `openalpaca_core::events::WakeEvent` without breaking.
pub use openalpaca_api::events::WakeEvent;

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
    /// A security policy was violated
    SecurityViolation {
        agent_id: String,
        tool_name: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// A tool was executed by an agent
    ToolExecuted {
        agent_id: String,
        tool_name: String,
        success: bool,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    /// An LLM call completed
    LlmCallCompleted {
        agent_id: String,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: f64,
        timestamp: DateTime<Utc>,
    },
    /// An agent was denied access to a model
    ModelAccessDenied {
        agent_id: String,
        model_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// The SOUL.md personality file was updated
    SoulUpdated {
        /// Who initiated the update: "agent" or "user" (via file watcher)
        actor: String,
        /// Update mode: "replace" or "sections"
        mode: String,
        /// SHA-256 hash of the new content for deduplication
        content_sha256: String,
        /// Path to the timestamped backup (if created)
        backup_path: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// The USER.md profile file was updated
    UserProfileUpdated {
        /// Who initiated the update: "agent", "user" (via file watcher), or "extraction"
        actor: String,
        /// Update mode: "replace", "sections", or "remember_command"
        mode: String,
        /// SHA-256 hash of the new content for deduplication
        content_sha256: String,
        /// Which sections were modified (for sections mode)
        modified_sections: Vec<String>,
        /// Path to the timestamped backup (if created)
        backup_path: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// The skill catalog was updated (skill added, removed, or reloaded)
    SkillCatalogUpdated {
        /// Skill name that changed
        skill_name: String,
        /// Action taken: "added", "removed", or "reloaded"
        action: String,
        timestamp: DateTime<Utc>,
    },
    /// The IDENTITY.md file was updated
    IdentityUpdated {
        /// Who initiated the update: "agent" or "user" (via file watcher)
        actor: String,
        /// Update mode: "replace" or "sections"
        mode: String,
        /// SHA-256 hash of the new content for deduplication
        content_sha256: String,
        /// Path to the timestamped backup (if created)
        backup_path: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// Bootstrap onboarding completed — BOOTSTRAP.md has been consumed and deleted
    BootstrapCompleted {
        /// Whether the agent identity was populated during bootstrap
        identity_populated: bool,
        /// Whether the user profile was populated during bootstrap
        user_populated: bool,
        timestamp: DateTime<Utc>,
    },
    /// A DAG node started execution
    DagNodeStarted {
        task_id: String,
        node_id: String,
        node_title: String,
        agent_id: String,
        timestamp: DateTime<Utc>,
    },
    /// A DAG node completed execution (success or failure)
    DagNodeCompleted {
        task_id: String,
        node_id: String,
        node_title: String,
        agent_id: String,
        success: bool,
        duration_ms: u64,
        /// First 200 chars of the node's output (for quick preview)
        output_preview: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// A tool's circuit breaker was tripped due to repeated transient failures.
    /// The tool will be blocked for `reset_after_secs` before a probe call is allowed.
    CircuitBreakerTripped {
        agent_id: String,
        tool_name: String,
        consecutive_failures: usize,
        reset_after_secs: u64,
        timestamp: DateTime<Utc>,
    },
    /// A task DAG was replanned during execution
    TaskReplanned {
        task_id: String,
        /// Which replan iteration this was (1-based)
        replan_number: usize,
        /// The decision taken: "continue", "modify", or "abort"
        decision: String,
        /// How many nodes were added in the new DAG
        nodes_added: usize,
        /// How many nodes were removed (replaced) from the old DAG
        nodes_removed: usize,
        timestamp: DateTime<Utc>,
    },
}
