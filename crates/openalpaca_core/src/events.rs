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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_count: Option<i32>,
        /// First 500 chars of the outcome summary (may differ from result_summary).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome_summary: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// A task failed
    TaskFailed {
        task_id: String,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome_kind: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// An agent instance's lifecycle status changed.
    ///
    /// Status values form a full lifecycle:
    ///   "spawned"   — instance created from template
    ///   "busy"      — instance claimed/re-claimed for a task step
    ///   "idle"      — singleton released back to the reuse pool
    ///   "waiting"   — instance paused awaiting user action
    ///   "error"     — instance entered error state
    ///   "destroyed" — non-singleton instance removed from registry
    AgentStatusChanged {
        /// Kept for backward compat (same as instance_id).
        agent_id: String,
        /// The runtime instance identifier (e.g. "code_agent::a1b2c3d4").
        instance_id: String,
        /// The template this instance was spawned from (e.g. "code_agent").
        template_id: String,
        /// Lifecycle status string.
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
    /// An agent configuration was created, updated, or deleted
    AgentConfigChanged {
        agent_id: String,
        /// "created" | "updated" | "deleted"
        action: String,
        config_version: u64,
        timestamp: DateTime<Utc>,
    },
    /// The orchestrator configuration was changed (e.g. default model)
    OrchestratorConfigChanged {
        model: String,
        timestamp: DateTime<Utc>,
    },
    /// The daemon configuration was changed (e.g. execution limits, server settings)
    DaemonConfigChanged { timestamp: DateTime<Utc> },
    /// An LLM API key status changed (add/remove/reorder/priority)
    KeyStatusChanged {
        provider: String,
        key_id: String,
        /// "added" | "removed" | "reordered" | "priority_changed"
        status: String,
        timestamp: DateTime<Utc>,
    },
    /// A chat stream started
    ChatStreamStarted {
        stream_id: String,
        lane_key: String,
        timestamp: DateTime<Utc>,
    },
    /// A chat stream ended
    ChatStreamEnded {
        stream_id: String,
        lane_key: String,
        status: String,
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
    /// The LLM planner was bypassed (fast path, bootstrap, no router)
    PlannerBypassed {
        request_id: Uuid,
        /// "fast_path" | "bootstrap" | "no_llm_router"
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// Dispatcher analysis decision (Phase 2: decoupled analysis from execution)
    DispatchDecision {
        request_id: String,
        task_id: Option<String>,
        /// "lead_agent" | "dag_parallel" | "sequential_pipeline"
        mode: String,
        /// "planner_explicit" | "execution_mode_field" | "empty_assignments_fallback" | "heuristic_fallback" | "heuristic_match_failed"
        reason: String,
        agent_count: usize,
        dag_node_count: Option<usize>,
        predictability_score: Option<f64>,
        error_message: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// Request-level orchestration stage metrics
    OrchestrationStage {
        request_id: Uuid,
        /// "fast_path" | "planner_simple_query" | "planner_complex_task" |
        /// "heuristic_simple_query" | "heuristic_complex_task" | "bootstrap" | "no_llm"
        mode: String,
        planner_ms: u64,
        dispatch_ms: u64,
        ack_ms: u64,
        fallback_reason: Option<String>,
        auto_promotion_reason: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// A new skill was discovered during catalog scanning
    SkillDiscovered {
        /// Skill ID (directory name)
        skill_id: String,
        /// Human-readable skill name from frontmatter
        skill_name: String,
        /// "project" | "user"
        scope: String,
        timestamp: DateTime<Utc>,
    },
    /// The skill router auto-selected a skill for a user query
    SkillSelected {
        /// Skill ID that was selected
        skill_id: String,
        /// The routing score that triggered selection
        score: f64,
        /// User query that triggered the selection (truncated)
        query_preview: String,
        timestamp: DateTime<Utc>,
    },
    /// A skill invocation started
    SkillInvocationStarted {
        request_id: Uuid,
        /// Skill ID being invoked
        skill_id: String,
        /// User query (truncated)
        query_preview: String,
        timestamp: DateTime<Utc>,
    },
    /// Skill context was injected into the prompt
    SkillContextInjected {
        request_id: Uuid,
        /// Skill ID being invoked
        skill_id: String,
        /// Number of bytes of context injected
        context_bytes: usize,
        timestamp: DateTime<Utc>,
    },
    /// A skill invocation completed successfully
    SkillCompleted {
        request_id: Uuid,
        /// Skill ID that completed
        skill_id: String,
        /// Duration of the skill invocation in milliseconds
        duration_ms: u64,
        /// First 200 chars of the output (for quick preview)
        output_preview: String,
        timestamp: DateTime<Utc>,
    },
    /// A skill invocation failed
    SkillFailed {
        request_id: Uuid,
        /// Skill ID that failed
        skill_id: String,
        /// Error message
        error: String,
        timestamp: DateTime<Utc>,
    },
    /// A tool requires interactive human confirmation before execution
    ToolConfirmationRequested {
        request_id: String,
        agent_id: String,
        tool_name: String,
        tool_arguments: serde_json::Value,
        /// SSE stream ID for routing to the active chat stream
        stream_id: Option<String>,
        /// Lane key for routing to connectors (e.g. "telegram:12345")
        lane_key: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// Context budget was computed for a request (Phase A observability)
    ContextBudgetComputed {
        request_id: Uuid,
        model: String,
        window_size: usize,
        fixed_zone_tokens: usize,
        free_zone_tokens: usize,
        buffer_size: usize,
        section_breakdown: Vec<(String, usize)>,
        timestamp: DateTime<Utc>,
    },
    /// Context compaction was triggered and completed
    CompactionTriggered {
        request_id: Uuid,
        utilization_pct: f64,
        messages_before: usize,
        messages_after: usize,
        memories_extracted: usize,
        messages_discarded: usize,
        summary_tokens: usize,
        timestamp: DateTime<Utc>,
    },
    /// A single compaction phase completed
    CompactionPhaseCompleted {
        request_id: Uuid,
        phase: String,
        duration_ms: u64,
        items_processed: usize,
        timestamp: DateTime<Utc>,
    },
}
