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
        /// The task's title. Empty for post-restart DB-only tasks whose
        /// in-memory registry entry (and thus title) was lost (GAP-07).
        #[serde(default)]
        title: String,
        status: String,
        progress_current: Option<i32>,
        progress_total: Option<i32>,
        timestamp: DateTime<Utc>,
    },
    /// A task completed successfully
    TaskCompleted {
        task_id: String,
        /// The task's title. Empty for post-restart DB-only tasks whose
        /// in-memory registry entry (and thus title) was lost (GAP-07).
        #[serde(default)]
        title: String,
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
        /// The task's title. Empty for post-restart DB-only tasks whose
        /// in-memory registry entry (and thus title) was lost (GAP-07).
        #[serde(default)]
        title: String,
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
        /// The agent's human-readable display name (e.g. "Code Agent").
        /// Empty when the instance could not be resolved (GAP-07).
        #[serde(default)]
        name: String,
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
    /// Dispatch decision record (Routing V2 tool path). Historical rows may
    /// carry retired planner-era mode/reason strings.
    DispatchDecision {
        request_id: String,
        task_id: Option<String>,
        /// "lead_agent"
        mode: String,
        /// "model_tool_call"
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
        /// "task_ops" | "steered" | "skill_command" | "bootstrap" |
        /// "forced_simple_query" | "social_fast_path" | "main_loop"
        mode: String,
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
    /// Emitted when a compose-engine layer retrieved its output from cache
    /// (spec section Component 4).
    ComposeLayerCacheHit {
        layer: crate::compose::LayerId,
        fingerprint: [u8; 32],
        lane_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// Emitted when a compose-engine layer rebuilt its output (cache miss).
    ComposeLayerCacheMiss {
        layer: crate::compose::LayerId,
        fingerprint: [u8; 32],
        reason: crate::compose::MissReason,
        lane_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// A background workflow was started for a lane (Routing V2)
    WorkflowStarted {
        request_id: Uuid,
        task_id: String,
        lane_key: String,
        title: String,
        timestamp: DateTime<Utc>,
    },
    /// A steering message was accepted into a running workflow's inbox (Routing V2)
    WorkflowSteered {
        task_id: String,
        lane_key: String,
        request_id: Uuid,
        timestamp: DateTime<Utc>,
    },
    /// A running workflow posted a user-facing progress update (Routing V2)
    WorkflowProgress {
        task_id: String,
        lane_key: String,
        message: String,
        timestamp: DateTime<Utc>,
    },
    /// A follow-up item was queued for a lane (Routing V2)
    FollowupQueued {
        lane_key: String,
        followup_id: i64,
        /// "followup" | "unprocessed_steering"
        kind: String,
        timestamp: DateTime<Utc>,
    },
    /// An extension's observed state changed — T5, E5, `mark_failed`, T5-deny,
    /// T5-gone and §3.7's tool-list refresh (extension design ADR-030).
    ///
    /// Declared here, in C1, because the plugin supervisor publishes it and
    /// `openalpaca_plugins` cannot see a variant the daemon crate would add.
    ExtensionStateChanged {
        extension: crate::tools::extensions::ExtensionId,
        /// The record's new state word, or `"removed"` when the declaration is
        /// gone and the row simply disappears.
        state: String,
        /// The load the change belongs to, so the event log stays unambiguous
        /// when a late crash notice arrives after a newer load's events.
        generation: u64,
        /// Set only by a server-driven `tools/list_changed` refresh.
        #[serde(default)]
        tools_changed: bool,
        timestamp: DateTime<Utc>,
    },
    /// **S4 moment 1 and 2** — a capability was withheld from a caller
    /// (extension design §7.1, §7.2, §6.2 #13).
    ///
    /// Published by [`ExtensionLedger`](crate::tools::extensions::ExtensionLedger)
    /// itself, beside the `warn!`, and governed by the same 10-minute dedup
    /// (§7.4): the **announcement** is deduped, never the error.
    ExtensionCapabilityWithheld {
        extension: crate::tools::extensions::ExtensionId,
        /// The tool name (`AttemptedUse`), the capability or allowed tool name
        /// (`SurfaceAssembly`) or the skill id (`ScheduledSkip`) the
        /// withholding is about.
        subject: String,
        moment: crate::tools::extensions::Moment,
        /// The record's state word at the moment of the refusal, or
        /// `"unrecorded"` when there is no record (design §6.2a).
        state: String,
        /// The dedup `ScopeKey`: `task_id` → `request_id` → `agent_id` →
        /// `"global"`, or the **skill id** for `ScheduledSkip`, which is exempt
        /// from dedup (design §7.4, §6.2 #13).
        scope: String,
        agent_id: Option<String>,
        task_id: Option<String>,
        /// The caller held a *previous load*'s handle (design §3.0 Fact 3).
        stale: bool,
        timestamp: DateTime<Utc>,
    },
    /// **S4 moment 3** — the transition the owner is looking at: T1 step 3's
    /// dependent scan (extension design §3.2 T1, §7.3).
    ///
    /// One per transition, never deduped. `cause` — not the transient state —
    /// is what the `warn!` and the owner notice are worded from.
    ExtensionCapabilityWithdrawn {
        extension: crate::tools::extensions::ExtensionId,
        /// `Disabling` on the route / watcher / deny / reload paths,
        /// `Failed{Crashed,..}` from the reaper and the residue exits, `Enabled`
        /// from §3.7's server-driven list change.
        state: crate::tools::extensions::ExtensionState,
        cause: crate::tools::extensions::WithdrawalCause,
        /// The withdrawn set — T1 step 1's and T2 step 1's tombstones.
        capabilities: Vec<String>,
        /// The withdrawn tool **names**, which the legacy `tools.allow` scan
        /// matches on.
        tools: Vec<String>,
        affected_templates: Vec<String>,
        /// Skills now unsatisfiable — at least one required capability wholly
        /// withheld, or (legacy `tools.allow`) every allowed name withdrawn.
        affected_skills: Vec<String>,
        /// The subset of `affected_skills` that carry `invoke.cron`. The owner
        /// notice fires only when this is non-empty (design §7.3).
        affected_cron_skills: Vec<String>,
        /// The daemon's default lane, `{local_user_id}:gui`.
        notice_lane: String,
        timestamp: DateTime<Utc>,
    },
}
