use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ExecutionConfig {
    pub agent_defaults: AgentDefaults,
    pub lead_agent_defaults: LeadAgentDefaults,
    pub skill_defaults: SkillDefaults,
    pub context: ContextBudgetConfig,
    pub artifacts: ArtifactsConfig,
}

/// The highest `max_tool_runtime_ceiling_secs` may be set to (N-TOOL-1b): the
/// range the retired `[execution.lead_agent_defaults] max_tool_runtime_secs`
/// had, so no figure an owner could configure before is out of reach.
/// `shell_execute`'s own bound is this, so the sandbox's cut — at most the
/// ceiling — always comes first.
pub const MAX_TOOL_RUNTIME_CEILING_LIMIT_SECS: u64 = 3600;

/// Fallback defaults for regular agents (when agent TOML `[constraints]` are absent).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentDefaults {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    /// The per-tool bound every loop reads (N-TOOL-1b, B-11) — the main
    /// loop, the lead, every subagent and both skill tiers: a tool call still
    /// running after this many seconds is cut (and a command killed with
    /// what it started, R45). A template's `timeout_seconds` does not move
    /// it.
    pub max_tool_runtime_secs: u64,
    /// The most a call's own `timeout_secs` may ask for, on `shell_execute`
    /// and the `Command`-backend tools: a larger figure is clamped to this,
    /// never refused. Never below `max_tool_runtime_secs` (`validate` raises
    /// it to that).
    pub max_tool_runtime_ceiling_secs: u64,
    pub max_cost: f64,
    /// Timeout for interactive tool confirmation prompts (seconds).
    #[serde(default = "default_confirmation_timeout")]
    pub confirmation_timeout_secs: u64,
}

fn default_confirmation_timeout() -> u64 {
    300
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            max_rounds: 15,
            max_tools_per_round: 5,
            max_tool_runtime_secs: 120,
            max_tool_runtime_ceiling_secs: 600,
            max_cost: 1.00,
            confirmation_timeout_secs: default_confirmation_timeout(),
        }
    }
}

/// Fallback defaults for lead/orchestrating agents. The lead's per-tool bound
/// is `AgentDefaults`' (N-TOOL-1b): this table's `max_tool_runtime_secs`
/// reached no policy and is gone, warned about at boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LeadAgentDefaults {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    pub max_cost: f64,
    /// Maximum number of concurrent subagents a single lead agent can have running.
    pub max_concurrent_subagents: usize,
    /// The most subagents one run may spawn in all (N-LEAD-6) — queued,
    /// running and finished, retries and a resumed run's earlier ones (those
    /// its session log still records) included; the next spawn is refused,
    /// naming this figure, which the lead's prompt states. A lifetime
    /// backstop far above normal use: each spawn pays an instance, a lane
    /// and a task, and since N-LEAD-2 a spawn past `max_concurrent_subagents`
    /// queues instead of waiting. Its default, 48, is
    /// `max_concurrent_subagents`' (6) times the batch cap
    /// (`spawn_subagents_batch`, at most 8): re-derived by hand when either
    /// moves, which `daemon_config`'s tests check.
    pub max_subagents_per_run: usize,
    /// When true, lead agents get the `spawn_subagents_batch` tool for
    /// spawning multiple subagents in a single tool call. Default: true
    /// (Routing V2 — batch spawn is the parallel fan-out primitive).
    #[serde(default = "default_batch_spawn_enabled")]
    pub batch_spawn_enabled: bool,
}

fn default_batch_spawn_enabled() -> bool {
    true
}

impl Default for LeadAgentDefaults {
    fn default() -> Self {
        Self {
            max_rounds: 18,
            max_tools_per_round: 3,
            max_cost: 5.0,
            max_concurrent_subagents: 6,
            // 6 × 8: see the field.
            max_subagents_per_run: 48,
            batch_spawn_enabled: true,
        }
    }
}

/// Fallback defaults for skill invocations (agentic loop during /skill commands).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillDefaults {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    /// Auto-select score threshold for the skill router.
    pub router_auto_select_threshold: f64,
    /// Suggest score threshold for the skill router.
    pub router_suggest_threshold: f64,
}

impl Default for SkillDefaults {
    fn default() -> Self {
        Self {
            max_rounds: 6,
            max_tools_per_round: 3,
            router_auto_select_threshold: 0.65,
            router_suggest_threshold: 0.45,
        }
    }
}

/// Caps on artifacts produced by agents (`artifact_write`, and the
/// `workspace_write(entry_type="artifact")` spill). Deserialized from
/// `[execution.artifacts]` in daemon.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactsConfig {
    /// Largest single artifact body accepted, in bytes. Defaults to 10 MB —
    /// `file_write`'s cap, not the 50 MB upload cap: this content comes out of
    /// a context window, not off a disk.
    #[serde(default = "default_max_artifact_bytes")]
    pub max_artifact_bytes: u64,
    /// Versions retained per artifact. On write, the oldest superseded version
    /// beyond this bound is pruned (file + row); the head is never pruned.
    #[serde(default = "default_max_versions_per_artifact")]
    pub max_versions_per_artifact: u32,
}

fn default_max_artifact_bytes() -> u64 {
    10 * 1024 * 1024
}

fn default_max_versions_per_artifact() -> u32 {
    20
}

impl Default for ArtifactsConfig {
    fn default() -> Self {
        Self {
            max_artifact_bytes: default_max_artifact_bytes(),
            max_versions_per_artifact: default_max_versions_per_artifact(),
        }
    }
}

/// Context budget and compaction configuration.
///
/// Controls autocompact buffer sizing, the compaction target and the recent
/// tail compaction keeps. Deserialized from `[execution.context]` in
/// daemon.toml. Compaction's model is `[orchestrator.costs] summary_model`
/// (design 11, CQ-3); a `compaction_model` left in a hand-edited file is
/// ignored like any stale key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextBudgetConfig {
    pub autocompact_buffer_ratio: f64,
    pub compaction_target_ratio: f64,
    pub min_recent_messages: usize,
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            autocompact_buffer_ratio: 0.165,
            compaction_target_ratio: 0.50,
            min_recent_messages: 4,
        }
    }
}
