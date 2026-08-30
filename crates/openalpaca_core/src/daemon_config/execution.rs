use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ExecutionConfig {
    pub agent_defaults: AgentDefaults,
    pub lead_agent_defaults: LeadAgentDefaults,
    pub skill_defaults: SkillDefaults,
    pub planner: PlannerConfig,
    pub dag: DagConfig,
    pub context: ContextBudgetConfig,
}

/// Fallback defaults for regular agents (when agent TOML `[constraints]` are absent).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentDefaults {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    pub max_tool_runtime_secs: u64,
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
            max_tool_runtime_secs: 60,
            max_cost: 1.00,
            confirmation_timeout_secs: default_confirmation_timeout(),
        }
    }
}

/// Fallback defaults for lead/orchestrating agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LeadAgentDefaults {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    pub max_tool_runtime_secs: u64,
    pub max_cost: f64,
    /// Maximum number of concurrent subagents a single lead agent can have running.
    pub max_concurrent_subagents: usize,
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
            max_tool_runtime_secs: 300,
            max_cost: 5.0,
            max_concurrent_subagents: 6,
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
    /// Default permission level for skills without explicit permissions.
    pub default_permission_level: String,
    /// Global tool deny list (applied to all skills in addition to per-skill deny).
    pub global_tool_deny: Vec<String>,
    /// Default tool rate limit (calls per minute) if not specified in skill.
    pub default_tool_rate_limit: u32,
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
            default_permission_level: "readonly".to_string(),
            global_tool_deny: Vec::new(),
            default_tool_rate_limit: 60,
            router_auto_select_threshold: 0.65,
            router_suggest_threshold: 0.45,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_deep_query_max_rounds() -> usize {
    10
}
fn default_deep_query_max_tools_per_round() -> usize {
    4
}

/// LLM planner configuration (classification + hierarchical planning).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlannerConfig {
    /// Timeout in seconds for a single LLM planning call.
    pub planning_timeout_secs: u64,
    /// Maximum retry attempts on malformed LLM responses before giving up.
    pub max_retries: usize,
    /// Maximum tokens for the planner LLM response.
    pub max_tokens: u32,
    /// When true, inject a system hint nudging the planner toward DAG
    /// when the user message contains predictable parallel structure.
    pub dag_prefer_predictable_enabled: bool,
    /// When true, the dispatcher produces a DispatchDecision before execution,
    /// emitting an event for observability. Phase 2 feature flag.
    #[serde(default)]
    pub dispatch_analysis_enabled: bool,
    /// When true, TaskPlan responses include execution_mode and predictability_score
    /// fields, and the planner prompt is extended with v2 schema. Phase 2 feature flag.
    #[serde(default)]
    pub plan_protocol_v2_enabled: bool,
    /// When true, an enhanced heuristic pre-screens messages classified as
    /// SimpleQuery to bypass the LLM planner for likely conversational queries.
    /// Phase 1 feature flag. Default: true.
    #[serde(default = "default_true")]
    pub enhanced_pre_screen_enabled: bool,
    /// When true, messages not caught by the enhanced pre-screen go through a
    /// lightweight LLM classification before the full planner. Default: false.
    #[serde(default)]
    pub two_phase_enabled: bool,
    /// Model ID for the lightweight triage classifier (Opt-12).
    /// Should be a cheap/fast model (e.g. "claude-haiku-4-5-20251001").
    /// If None, falls back to the router's default model.
    #[serde(default)]
    pub triage_model: Option<String>,
    /// Maximum rounds for `deep_query` tier (single-agent, expanded budget).
    /// Default: 10.
    #[serde(default = "default_deep_query_max_rounds")]
    pub deep_query_max_rounds: usize,
    /// Maximum tool calls per round for `deep_query` tier. Default: 4.
    #[serde(default = "default_deep_query_max_tools_per_round")]
    pub deep_query_max_tools_per_round: usize,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            planning_timeout_secs: 60,
            max_retries: 2,
            max_tokens: 2048,
            dag_prefer_predictable_enabled: true,
            dispatch_analysis_enabled: true,
            plan_protocol_v2_enabled: true,
            enhanced_pre_screen_enabled: true,
            two_phase_enabled: true,
            triage_model: None,
            deep_query_max_rounds: 10,
            deep_query_max_tools_per_round: 4,
        }
    }
}

/// DAG executor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DagConfig {
    pub max_concurrent_agents: usize,
    pub node_timeout_secs: u64,
    pub total_timeout_secs: u64,
    pub max_retries_per_node: usize,
    pub replan_after_every_n_nodes: usize,
    pub max_replans: usize,
    pub replan_enabled: bool,
    /// When true, ready nodes are prioritized by critical path length
    /// (longest remaining dependency chain first). Phase 2 feature flag.
    #[serde(default)]
    pub critical_path_scheduling_enabled: bool,
}

impl Default for DagConfig {
    fn default() -> Self {
        Self {
            max_concurrent_agents: 4,
            node_timeout_secs: 300,
            total_timeout_secs: 1800,
            max_retries_per_node: 1,
            replan_after_every_n_nodes: 5,
            max_replans: 3,
            replan_enabled: false,
            critical_path_scheduling_enabled: true,
        }
    }
}

/// Context budget and compaction configuration.
///
/// Controls autocompact buffer sizing, compaction target, and extraction limits.
/// Deserialized from `[execution.context]` in daemon.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextBudgetConfig {
    pub autocompact_buffer_ratio: f64,
    pub compaction_target_ratio: f64,
    pub compaction_model: Option<String>,
    pub max_extractions_per_compaction: usize,
    pub min_recent_messages: usize,
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            autocompact_buffer_ratio: 0.165,
            compaction_target_ratio: 0.50,
            compaction_model: None,
            max_extractions_per_compaction: 10,
            min_recent_messages: 4,
        }
    }
}
