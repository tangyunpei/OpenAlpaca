use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ExecutionConfig {
    pub agent_defaults: AgentDefaults,
    pub lead_agent_defaults: LeadAgentDefaults,
    pub skill_defaults: SkillDefaults,
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
