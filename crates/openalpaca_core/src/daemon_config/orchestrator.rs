use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct OrchestratorConfig {
    pub memory: MemoryConfig,
    pub costs: CostsConfig,
    pub prompt_budgets: PromptBudgetsConfig,
    pub routing: RoutingConfig,
    pub sessions: SessionsConfig,
}

/// Session event log limits (`[orchestrator.sessions]`, plan §5.4).
///
/// The log is bounded, not unbounded history: a trim loses loop-interior
/// detail (rounds, tool payloads) but never the conversation, which lives in
/// SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsConfig {
    /// Per session, counting `log.jsonl` segments **plus** `results/` and
    /// `snapshots/`. On exceed the writer drops whole oldest segments (never
    /// the live one) and writes a `log_trimmed` record naming the dropped seq
    /// range.
    #[serde(default = "default_log_max_session_bytes")]
    pub log_max_session_bytes: u64,
    /// Across all sessions, evicting oldest-touched archived sessions first;
    /// an active session's log is never evicted. Read by the boot sweep.
    #[serde(default = "default_log_max_total_bytes")]
    pub log_max_total_bytes: u64,
    /// Age-based sweep of archived session logs. `0` disables it — the
    /// default, pending owner decision T12; it is not an oversight, and the
    /// sweep machinery ships regardless of the value (P-21).
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// The one threshold two consumers share (§5.4, P-16/C-2): a tool result
    /// larger than this is written once to the session's `results/` and the
    /// **model** is handed a stub naming it, instead of a head-only cut that
    /// destroyed the tail. Below it the result travels inline, unchanged.
    ///
    /// Only sessions can spill, so a loop with no session log falls back to
    /// the head-only cut at this same size — one number, never two.
    #[serde(default = "default_tool_result_inline_bytes")]
    pub tool_result_inline_bytes: usize,
    /// §5.7's pre-edit images: the largest file `file_write` will copy into
    /// the session's `snapshots/` before overwriting it.
    ///
    /// It is a **refusal** threshold, not a skip: a snapshot the session
    /// cannot afford means the write does not happen, because silently
    /// overwriting a file whose only copy this was is the outcome the tier
    /// exists to prevent. Default 10 MB — deliberately the same number as
    /// `file_write`'s own content bound (`MAX_FILE_WRITE_SIZE`,
    /// `tools/builtins/file_ops.rs`), so this tier costs nothing in
    /// reachability: no overwrite `file_write` could already produce is
    /// refused purely because S3 was added (Task 56 fix round 1, Important
    /// #2 / R68). Lower it only with that coupling in mind — a smaller value
    /// reopens the band of existing files `file_write` refuses to touch at
    /// all.
    #[serde(default = "default_snapshot_max_bytes")]
    pub snapshot_max_bytes: u64,
}

fn default_log_max_session_bytes() -> u64 {
    256 * 1024 * 1024
}
fn default_log_max_total_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}
fn default_log_retention_days() -> u32 {
    0
}
fn default_tool_result_inline_bytes() -> usize {
    32 * 1024
}
fn default_snapshot_max_bytes() -> u64 {
    // `file_write`'s own content bound (`MAX_FILE_WRITE_SIZE`,
    // `tools/builtins/file_ops.rs`) — kept equal on purpose, see the field
    // doc comment above.
    10 * 1024 * 1024
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            log_max_session_bytes: default_log_max_session_bytes(),
            log_max_total_bytes: default_log_max_total_bytes(),
            log_retention_days: default_log_retention_days(),
            tool_result_inline_bytes: default_tool_result_inline_bytes(),
            snapshot_max_bytes: default_snapshot_max_bytes(),
        }
    }
}

/// Routing V2 configuration (`[orchestrator.routing]`).
///
/// The main-loop tool ladder is the only routing ladder (the legacy planner
/// pre-classifier and its `mode` key were deleted in Phase 5).
///
/// Every field carries a named `#[serde(default = "...")]` function shared
/// with the `Default` impl, so a partially-specified table gets the same
/// values as an absent one (avoids the field-level `#[serde(default)]`
/// footgun where a bool silently defaults to `false`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Enable the mid-workflow steering rail (steering inboxes plus the
    /// `post_update`/`queue_followup` lead-agent tools).
    #[serde(default = "default_steering_enabled")]
    pub steering_enabled: bool,
    /// Maximum queued steering messages per workflow inbox.
    #[serde(default = "default_steering_inbox_cap")]
    pub steering_inbox_cap: usize,
    /// Maximum concurrent workflows per lane (enforced in `start_workflow`).
    #[serde(default = "default_max_workflows_per_lane")]
    pub max_workflows_per_lane: usize,
    /// Auto-start the next queued `followup` item when a workflow finalizes.
    /// `unprocessed_steering` items never auto-run.
    #[serde(default = "default_followup_autostart")]
    pub followup_autostart: bool,
    /// Max agentic-loop rounds for the tool-mode main loop.
    #[serde(default = "default_main_loop_max_rounds")]
    pub main_loop_max_rounds: usize,
    /// Max tool calls per round for the tool-mode main loop.
    #[serde(default = "default_main_loop_max_tools_per_round")]
    pub main_loop_max_tools_per_round: usize,
    /// Main-loop tool surface: "core_union" (default — core set ∪
    /// keyword-suggested tools ∪ MCP/plugin extension tools ∪ `invoke_skill`)
    /// or "full" (entire registry — escape hatch). Both drop the tools of an
    /// extension that is not `Enabled`; there is no per-tool opt-out — the
    /// ENABLE axis is one toggle per MCP server and per plugin.
    #[serde(default = "default_tool_selection")]
    pub tool_selection: String,
    /// Global kill switch for cron-scheduled skills (`invoke.cron` /
    /// `invoke.mode = "scheduled"` frontmatter). When false, no skill cron
    /// jobs are registered with the wake scheduler and pending timer fires
    /// are ignored.
    #[serde(default = "default_scheduled_skills_enabled")]
    pub scheduled_skills_enabled: bool,
    /// **Experimental (§5.6c, S2): replay resume.** With this on,
    /// `POST /v1/tasks/{id}/action {"action":"resume"}` re-enters an
    /// `interrupted` run under its own id, rebuilding the loop's history from
    /// the session log and continuing it with a synthetic interjection.
    ///
    /// Off by default, and deliberately so: the plan calls S2 its one
    /// speculative piece, `rerun` is the trusted fallback, and a replay that
    /// gets the history wrong hands the model a false account of its own
    /// work. Everything the feature needs is built and tested; what is not
    /// yet earned is the default.
    #[serde(default = "default_resume_enabled")]
    pub resume_enabled: bool,
}

fn default_steering_enabled() -> bool {
    true
}
fn default_steering_inbox_cap() -> usize {
    16
}
fn default_max_workflows_per_lane() -> usize {
    3
}
fn default_followup_autostart() -> bool {
    true
}
fn default_main_loop_max_rounds() -> usize {
    8
}
fn default_main_loop_max_tools_per_round() -> usize {
    4
}
fn default_tool_selection() -> String {
    "core_union".to_string()
}
fn default_scheduled_skills_enabled() -> bool {
    true
}
fn default_resume_enabled() -> bool {
    false
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            steering_enabled: default_steering_enabled(),
            steering_inbox_cap: default_steering_inbox_cap(),
            max_workflows_per_lane: default_max_workflows_per_lane(),
            followup_autostart: default_followup_autostart(),
            main_loop_max_rounds: default_main_loop_max_rounds(),
            main_loop_max_tools_per_round: default_main_loop_max_tools_per_round(),
            tool_selection: default_tool_selection(),
            scheduled_skills_enabled: default_scheduled_skills_enabled(),
            resume_enabled: default_resume_enabled(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Number of recent messages to include in prompt context.
    pub prompt_recent_messages: usize,
    /// Minimum new older messages before triggering summary.
    pub summary_min_new_older_messages: usize,
    /// Maximum characters in a summary.
    pub summary_max_chars: usize,
    /// Character limit for message truncation in summary input.
    pub msg_trunc_chars: usize,
    /// L2 distance threshold for semantic supersession (lower = stricter).
    /// Memories within this distance are considered "same topic" and will be superseded.
    pub supersession_distance_threshold: f64,
    /// Jaccard word-overlap threshold for FTS-based supersession fallback.
    /// Only memories with overlap >= this value are considered for supersession
    /// when the embedder is unavailable. Range: 0.0–1.0.
    pub fts_jaccard_threshold: f64,
    /// Decay and pruning configuration.
    pub decay: MemoryDecayConfig,
    /// Minimum confidence for profile trait extraction (set action). Range: 0.0–1.0.
    pub profile_confidence_threshold: f64,
    /// Minimum confidence for profile trait update action. Range: 0.0–1.0.
    pub profile_update_confidence_threshold: f64,
    /// Minimum confidence for memory item extraction. Range: 0.0–1.0.
    pub memory_confidence_threshold: f64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            prompt_recent_messages: 25,
            summary_min_new_older_messages: 12,
            summary_max_chars: 4000,
            msg_trunc_chars: 1500,
            supersession_distance_threshold: 1.0,
            fts_jaccard_threshold: 0.4,
            decay: MemoryDecayConfig::default(),
            profile_confidence_threshold: 0.8,
            profile_update_confidence_threshold: 0.9,
            memory_confidence_threshold: 0.65,
        }
    }
}

/// Configuration for memory importance decay and pruning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryDecayConfig {
    /// How often to run the decay task (seconds).
    pub poll_interval_secs: u64,
    /// Half-life in days: after this many days without access, importance drops by 50%.
    pub half_life_days: f64,
    /// Minimum importance floor. Memories below this are eligible for pruning.
    pub min_importance: f64,
    /// Soft cap on total non-KbChunk memories per owner. Excess is pruned by lowest importance.
    pub soft_cap: usize,
    /// Small importance boost applied each time a memory is accessed.
    /// Reinforces frequently-used memories. Capped at 1.0 to prevent unbounded growth.
    pub access_boost: f64,
}

impl Default for MemoryDecayConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 3600,
            half_life_days: 30.0,
            min_importance: 0.05,
            soft_cap: 500,
            access_boost: 0.05,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CostsConfig {
    /// Maximum daily cost (USD) for conversation summaries.
    pub summary_max_daily_cost_usd: f64,
    /// Maximum daily cost (USD) for memory extractions.
    pub extract_max_daily_cost_usd: f64,
    /// Run extraction every N user turns.
    pub extract_every_n_turns: usize,
    /// Minimum content length (chars) to trigger extraction.
    pub extract_min_content_len: usize,
    /// Whether task output memory extraction is enabled.
    pub task_extract_enabled: bool,
    /// Maximum daily cost (USD) for task output memory extractions.
    pub task_extract_max_daily_cost_usd: f64,
    /// Minimum content length (chars) for task output to trigger extraction.
    pub task_extract_min_content_len: usize,
    /// Model ID for background summary generation (e.g. claude-haiku-4-5-20251001).
    /// If None, falls back to the router's default model.
    #[serde(default)]
    pub summary_model: Option<String>,
    /// Model ID for background user trait extraction (e.g. claude-haiku-4-5-20251001).
    /// If None, falls back to the router's default model.
    #[serde(default)]
    pub extraction_model: Option<String>,
}

impl Default for CostsConfig {
    fn default() -> Self {
        Self {
            summary_max_daily_cost_usd: 0.50,
            extract_max_daily_cost_usd: 0.25,
            extract_every_n_turns: 5,
            extract_min_content_len: 20,
            task_extract_enabled: true,
            task_extract_max_daily_cost_usd: 0.50,
            task_extract_min_content_len: 100,
            summary_model: None,
            extraction_model: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptBudgetsConfig {
    /// Character budget for the identity prompt block.
    pub identity_budget: usize,
    /// Character budget for the user profile prompt block.
    pub user_profile_budget: usize,
}

impl Default for PromptBudgetsConfig {
    fn default() -> Self {
        Self {
            identity_budget: 300,
            user_profile_budget: 1000,
        }
    }
}
