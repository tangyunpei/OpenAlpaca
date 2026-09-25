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
    pub learning: LearningConfig,
}

/// Background learning's cadence (`[orchestrator.learning]`, Learning v2,
/// design 09 §1.10, owner decision MD-10).
///
/// Every turn is queued; a session's unlearned turns are extracted together
/// when it has been idle about two minutes, when ten new turns have
/// accumulated, when the user switches conversations, and before the daemon
/// stops. Only MD-10's two numbers and the off switch are keys: the byte
/// budget, the chunk and retry bounds and the quiet period are engineering
/// constants (`memory::learning`), not owner-facing choices.
///
/// Named `#[serde(default = "...")]` functions shared with `Default`, as in
/// [`RoutingConfig`], so a partial table cannot default `enabled` to `false`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
    /// The off switch. `false` queues nothing and flushes nothing; the
    /// evaluation's question phase uses it.
    #[serde(default = "default_learning_enabled")]
    pub enabled: bool,
    /// MD-10's "idle about 2 minutes": a session with unlearned turns whose
    /// last turn is this old becomes due. `0` switches the idle trigger off.
    /// Range 0..=3600.
    #[serde(default = "default_learning_idle_secs")]
    pub idle_secs: u64,
    /// MD-10's "10 new turns": the Nth user-authored turn since the cursor
    /// makes the session due at once. Range 1..=100.
    #[serde(default = "default_learning_turns_per_flush")]
    pub turns_per_flush: u32,
    /// Conversation summaries a day (design 09 §13.3, MD-16): at most this
    /// many `episode` calls per UTC day, read durably from
    /// `llm_usage_daily`, on top of the extraction's own daily caps, which
    /// the summaries count under too. `0` writes none. Range 0..=1000.
    #[serde(default = "default_learning_episode_max_per_day")]
    pub episode_max_per_day: u32,
}

fn default_learning_enabled() -> bool {
    true
}
fn default_learning_idle_secs() -> u64 {
    120
}
fn default_learning_turns_per_flush() -> u32 {
    10
}
fn default_learning_episode_max_per_day() -> u32 {
    20
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            enabled: default_learning_enabled(),
            idle_secs: default_learning_idle_secs(),
            turns_per_flush: default_learning_turns_per_flush(),
            episode_max_per_day: default_learning_episode_max_per_day(),
        }
    }
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
    /// Age-based sweep of archived session logs, in days — **reserved, with no
    /// consumer yet**.
    ///
    /// The key is parsed, clamped by `validate()` and served by
    /// `GET /v1/status`; nothing reads it to expire anything. Only the two byte
    /// caps above are enforced, so setting this to `90` bounds nothing today:
    /// plan §5.4's age pass was not built, and whether it should be (and at
    /// what default) is owner decision T12. `load_daemon_config` warns at boot
    /// when the value is non-default, so the key cannot read as working.
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
    /// The extractor's acceptance floor (design §6): an item the model
    /// scores under this is dropped and counted; the number is never stored.
    /// Range: 0.0–1.0.
    pub memory_confidence_threshold: f64,
    /// How each turn's automatic recall delivers what it finds (design 09
    /// §13.2, MD-15): `index` (the default) or `excerpts`.
    pub recall_mode: RecallMode,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            prompt_recent_messages: 25,
            summary_min_new_older_messages: 12,
            summary_max_chars: 4000,
            msg_trunc_chars: 1500,
            memory_confidence_threshold: 0.65,
            recall_mode: RecallMode::default(),
        }
    }
}

/// `[orchestrator.memory] recall_mode` (design 09 §13.2, MD-15): what the
/// automatic per-turn recall puts in front of the model, always inside the
/// same 8 KiB and always as quoted reference data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecallMode {
    /// The pinned notes, the top 2 recalled notes as excerpts, then up to 8
    /// further notes listed one line each, for the model to read with
    /// `memory_read` — the owner-adopted default.
    #[default]
    Index,
    /// The pinned notes and up to 8 recalled notes, every one as an excerpt:
    /// the delivery before index-first recall.
    Excerpts,
}

impl RecallMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RecallMode::Index => "index",
            RecallMode::Excerpts => "excerpts",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CostsConfig {
    /// Maximum daily cost (USD) for conversation summaries.
    pub summary_max_daily_cost_usd: f64,
    /// Maximum cost (USD) per UTC day for chat learning's extraction calls,
    /// read durably from `llm_usage_daily` (design 09 §1.8).
    pub extract_max_daily_cost_usd: f64,
    /// A learning chunk with less of the user's own text than this many bytes
    /// is not sent, and the cursor moves past it (design 09 §1.6).
    pub extract_min_content_len: usize,
    /// Whether task output memory extraction is enabled.
    pub task_extract_enabled: bool,
    /// Maximum daily cost (USD) for task output memory extractions.
    pub task_extract_max_daily_cost_usd: f64,
    /// Minimum content length (chars) for task output to trigger extraction.
    pub task_extract_min_content_len: usize,
    /// Notes one chat-learning extraction call may write at most (design 09
    /// §1.10; the per-turn 5 of design 03 §4 before it). The model's items
    /// past this count are dropped and counted, never stored, and a small
    /// model's output budget can lower it further (§1.6).
    pub extract_max_items_per_call: usize,
    /// Notes one task run's extraction may write at most (design §4: 10).
    pub task_extract_max_items_per_run: usize,
    /// Chat-learning extraction calls per UTC day (Learning v2, design 09
    /// §1.8), read durably from `llm_usage_daily`. This is what bounds a $0
    /// local model, where the cost cap never binds. `0` = no cap. Range
    /// 0..=10000.
    pub extract_max_calls_per_day: u32,
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
            extract_min_content_len: 20,
            task_extract_enabled: true,
            task_extract_max_daily_cost_usd: 0.50,
            task_extract_min_content_len: 100,
            extract_max_items_per_call: 12,
            task_extract_max_items_per_run: 10,
            extract_max_calls_per_day: 200,
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
