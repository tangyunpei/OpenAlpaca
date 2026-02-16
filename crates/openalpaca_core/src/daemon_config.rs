//! Daemon-level configuration loaded from `config/daemon.toml`.
//!
//! All fields have serde defaults matching the previously hardcoded constants,
//! so an empty file or missing sections produce identical behavior to before.

use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Top-level ────────────────────────────────────────────────────────

/// Root config loaded from `config/daemon.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub orchestrator: OrchestratorConfig,
    pub execution: ExecutionConfig,
    pub security: SecurityConfig,
    pub server: ServerConfig,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            orchestrator: OrchestratorConfig::default(),
            execution: ExecutionConfig::default(),
            security: SecurityConfig::default(),
            server: ServerConfig::default(),
        }
    }
}

// ── Orchestrator ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OrchestratorConfig {
    pub memory: MemoryConfig,
    pub costs: CostsConfig,
    pub prompt_budgets: PromptBudgetsConfig,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            memory: MemoryConfig::default(),
            costs: CostsConfig::default(),
            prompt_budgets: PromptBudgetsConfig::default(),
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
            prompt_recent_messages: 40,
            summary_min_new_older_messages: 12,
            summary_max_chars: 4000,
            msg_trunc_chars: 1500,
            supersession_distance_threshold: 1.0,
            fts_jaccard_threshold: 0.4,
            decay: MemoryDecayConfig::default(),
            profile_confidence_threshold: 0.8,
            profile_update_confidence_threshold: 0.9,
            memory_confidence_threshold: 0.5,
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

// ── Execution ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutionConfig {
    pub agent_defaults: AgentDefaults,
    pub lead_agent_defaults: LeadAgentDefaults,
    pub skill_defaults: SkillDefaults,
    pub dag: DagConfig,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            agent_defaults: AgentDefaults::default(),
            lead_agent_defaults: LeadAgentDefaults::default(),
            skill_defaults: SkillDefaults::default(),
            dag: DagConfig::default(),
        }
    }
}

/// Fallback defaults for regular agents (when agent TOML `[constraints]` are absent).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentDefaults {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    pub max_tool_runtime_secs: u64,
    pub max_cost: f64,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            max_rounds: 15,
            max_tools_per_round: 5,
            max_tool_runtime_secs: 60,
            max_cost: 1.00,
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
}

impl Default for LeadAgentDefaults {
    fn default() -> Self {
        Self {
            max_rounds: 30,
            max_tools_per_round: 3,
            max_tool_runtime_secs: 300,
            max_cost: 5.0,
        }
    }
}

/// Fallback defaults for skill invocations (agentic loop during /skill commands).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillDefaults {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
}

impl Default for SkillDefaults {
    fn default() -> Self {
        Self {
            max_rounds: 6,
            max_tools_per_round: 3,
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
}

impl Default for DagConfig {
    fn default() -> Self {
        Self {
            max_concurrent_agents: 3,
            node_timeout_secs: 300,
            total_timeout_secs: 1800,
            max_retries_per_node: 1,
            replan_after_every_n_nodes: 2,
            max_replans: 3,
        }
    }
}

// ── Security ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Maximum input length in bytes.
    pub max_input_length: usize,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            max_input_length: 32 * 1024,
        }
    }
}

// ── Server ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Capacity for the EventBus broadcast channel (system-wide event distribution).
    pub event_bus_capacity: usize,
    /// Capacity for the WebSocket event broadcaster channel.
    pub event_broadcaster_capacity: usize,
    /// Capacity for the wake event channel.
    pub wake_channel_capacity: usize,
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,
    /// SSE keep-alive interval in seconds.
    pub sse_keep_alive_secs: u64,
    pub chat_streams: ChatStreamsConfig,
    pub embedding_indexer: EmbeddingIndexerConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            event_bus_capacity: 1024,
            event_broadcaster_capacity: 64,
            wake_channel_capacity: 256,
            heartbeat_interval_secs: 5,
            sse_keep_alive_secs: 15,
            chat_streams: ChatStreamsConfig::default(),
            embedding_indexer: EmbeddingIndexerConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatStreamsConfig {
    /// Interval in seconds to run the stale stream cleanup.
    pub cleanup_interval_secs: u64,
    /// Seconds after which a stream is considered stale.
    pub stale_timeout_secs: u64,
}

impl Default for ChatStreamsConfig {
    fn default() -> Self {
        Self {
            cleanup_interval_secs: 60,
            stale_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingIndexerConfig {
    /// Interval in seconds between embedding indexer runs.
    pub poll_interval_secs: u64,
    /// Number of missing embeddings to process per batch.
    pub batch_size: usize,
}

impl Default for EmbeddingIndexerConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            batch_size: 50,
        }
    }
}

// ── Validation helpers ───────────────────────────────────────────────

fn clamp_usize(val: &mut usize, min: usize, max: usize, name: &str) {
    if *val < min {
        tracing::warn!("Config '{name}': {val} below minimum {min}, clamping");
        *val = min;
    } else if *val > max {
        tracing::warn!("Config '{name}': {val} above maximum {max}, clamping");
        *val = max;
    }
}

fn clamp_u64(val: &mut u64, min: u64, max: u64, name: &str) {
    if *val < min {
        tracing::warn!("Config '{name}': {val} below minimum {min}, clamping");
        *val = min;
    } else if *val > max {
        tracing::warn!("Config '{name}': {val} above maximum {max}, clamping");
        *val = max;
    }
}

fn clamp_f64(val: &mut f64, min: f64, max: f64, name: &str) {
    if *val < min {
        tracing::warn!("Config '{name}': {val} below minimum {min}, clamping");
        *val = min;
    } else if *val > max {
        tracing::warn!("Config '{name}': {val} above maximum {max}, clamping");
        *val = max;
    }
}

impl DaemonConfig {
    /// Validate and clamp all fields to their allowed ranges.
    ///
    /// Ranges are sourced from `config_schema.rs` `ConfigKeyDef` entries.
    /// Invalid values are clamped to the nearest valid boundary with a warning log.
    pub fn validate(&mut self) {
        // ── Orchestrator > Memory ──
        clamp_usize(&mut self.orchestrator.memory.prompt_recent_messages, 1, 200, "prompt_recent_messages");
        clamp_usize(&mut self.orchestrator.memory.summary_min_new_older_messages, 1, 200, "summary_min_new_older_messages");
        clamp_usize(&mut self.orchestrator.memory.summary_max_chars, 100, 32000, "summary_max_chars");
        clamp_usize(&mut self.orchestrator.memory.msg_trunc_chars, 100, 32000, "msg_trunc_chars");
        clamp_f64(&mut self.orchestrator.memory.supersession_distance_threshold, 0.0, 10.0, "supersession_distance_threshold");
        clamp_f64(&mut self.orchestrator.memory.fts_jaccard_threshold, 0.0, 1.0, "fts_jaccard_threshold");
        clamp_f64(&mut self.orchestrator.memory.profile_confidence_threshold, 0.0, 1.0, "profile_confidence_threshold");
        clamp_f64(&mut self.orchestrator.memory.profile_update_confidence_threshold, 0.0, 1.0, "profile_update_confidence_threshold");
        clamp_f64(&mut self.orchestrator.memory.memory_confidence_threshold, 0.0, 1.0, "memory_confidence_threshold");
        // ── Orchestrator > Memory > Decay ──
        clamp_u64(&mut self.orchestrator.memory.decay.poll_interval_secs, 60, 86400, "decay.poll_interval_secs");
        clamp_f64(&mut self.orchestrator.memory.decay.half_life_days, 1.0, 365.0, "decay.half_life_days");
        clamp_f64(&mut self.orchestrator.memory.decay.min_importance, 0.0, 1.0, "decay.min_importance");
        clamp_usize(&mut self.orchestrator.memory.decay.soft_cap, 10, 100_000, "decay.soft_cap");
        clamp_f64(&mut self.orchestrator.memory.decay.access_boost, 0.0, 1.0, "decay.access_boost");
        // ── Orchestrator > Costs ──
        clamp_f64(&mut self.orchestrator.costs.summary_max_daily_cost_usd, 0.0, 100.0, "summary_max_daily_cost_usd");
        clamp_f64(&mut self.orchestrator.costs.extract_max_daily_cost_usd, 0.0, 100.0, "extract_max_daily_cost_usd");
        clamp_usize(&mut self.orchestrator.costs.extract_every_n_turns, 1, 100, "extract_every_n_turns");
        clamp_f64(&mut self.orchestrator.costs.task_extract_max_daily_cost_usd, 0.0, 100.0, "task_extract_max_daily_cost_usd");
        // ── Orchestrator > Prompt Budgets ──
        clamp_usize(&mut self.orchestrator.prompt_budgets.identity_budget, 50, 5000, "identity_budget");
        clamp_usize(&mut self.orchestrator.prompt_budgets.user_profile_budget, 100, 10000, "user_profile_budget");
        // ── Execution > Agent Defaults ──
        clamp_usize(&mut self.execution.agent_defaults.max_rounds, 1, 100, "agent_defaults.max_rounds");
        clamp_usize(&mut self.execution.agent_defaults.max_tools_per_round, 1, 50, "agent_defaults.max_tools_per_round");
        clamp_u64(&mut self.execution.agent_defaults.max_tool_runtime_secs, 1, 600, "agent_defaults.max_tool_runtime_secs");
        clamp_f64(&mut self.execution.agent_defaults.max_cost, 0.0, 1000.0, "agent_defaults.max_cost");
        // ── Execution > Lead Agent Defaults ──
        clamp_usize(&mut self.execution.lead_agent_defaults.max_rounds, 1, 200, "lead_agent_defaults.max_rounds");
        clamp_usize(&mut self.execution.lead_agent_defaults.max_tools_per_round, 1, 50, "lead_agent_defaults.max_tools_per_round");
        clamp_u64(&mut self.execution.lead_agent_defaults.max_tool_runtime_secs, 1, 3600, "lead_agent_defaults.max_tool_runtime_secs");
        clamp_f64(&mut self.execution.lead_agent_defaults.max_cost, 0.0, 1000.0, "lead_agent_defaults.max_cost");
        // ── Execution > Skill Defaults ──
        clamp_usize(&mut self.execution.skill_defaults.max_rounds, 1, 100, "skill_defaults.max_rounds");
        clamp_usize(&mut self.execution.skill_defaults.max_tools_per_round, 1, 50, "skill_defaults.max_tools_per_round");
        // ── Execution > DAG ──
        clamp_usize(&mut self.execution.dag.max_concurrent_agents, 1, 32, "dag.max_concurrent_agents");
        clamp_u64(&mut self.execution.dag.node_timeout_secs, 10, 3600, "dag.node_timeout_secs");
        clamp_u64(&mut self.execution.dag.total_timeout_secs, 60, 7200, "dag.total_timeout_secs");
        clamp_usize(&mut self.execution.dag.max_retries_per_node, 0, 10, "dag.max_retries_per_node");
        clamp_usize(&mut self.execution.dag.replan_after_every_n_nodes, 1, 50, "dag.replan_after_every_n_nodes");
        clamp_usize(&mut self.execution.dag.max_replans, 0, 50, "dag.max_replans");
        // ── Security ──
        clamp_usize(&mut self.security.max_input_length, 1024, 1_048_576, "security.max_input_length");
        // ── Server ──
        clamp_usize(&mut self.server.event_bus_capacity, 64, 65536, "server.event_bus_capacity");
        clamp_usize(&mut self.server.event_broadcaster_capacity, 8, 4096, "server.event_broadcaster_capacity");
        clamp_usize(&mut self.server.wake_channel_capacity, 8, 4096, "server.wake_channel_capacity");
        clamp_u64(&mut self.server.heartbeat_interval_secs, 1, 300, "server.heartbeat_interval_secs");
        clamp_u64(&mut self.server.sse_keep_alive_secs, 1, 300, "server.sse_keep_alive_secs");
    }
}

// ── Loader ───────────────────────────────────────────────────────────

/// Load daemon config from a TOML file. Returns defaults if file is missing or unparseable.
pub fn load_daemon_config(path: &Path) -> DaemonConfig {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<DaemonConfig>(&content) {
            Ok(mut config) => {
                config.validate();
                tracing::info!("Daemon config loaded from {}", path.display());
                config
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to parse daemon config {}: {e}; using defaults",
                    path.display()
                );
                DaemonConfig::default()
            }
        },
        Err(_) => {
            tracing::info!(
                "No daemon config at {}; using defaults",
                path.display()
            );
            DaemonConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DaemonConfig::default();
        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 40);
        assert_eq!(config.orchestrator.memory.summary_max_chars, 4000);
        assert_eq!(config.orchestrator.costs.summary_max_daily_cost_usd, 0.50);
        assert_eq!(config.execution.agent_defaults.max_rounds, 15);
        assert_eq!(config.execution.lead_agent_defaults.max_rounds, 30);
        assert_eq!(config.execution.dag.max_concurrent_agents, 3);
        assert_eq!(config.security.max_input_length, 32768);
        assert_eq!(config.server.heartbeat_interval_secs, 5);
    }

    #[test]
    fn test_empty_toml_gives_defaults() {
        let config: DaemonConfig = toml::from_str("").unwrap();
        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 40);
        assert_eq!(config.execution.dag.total_timeout_secs, 1800);
    }

    #[test]
    fn test_partial_override() {
        let toml_str = r#"
[orchestrator.memory]
prompt_recent_messages = 60

[execution.dag]
max_concurrent_agents = 8
"#;
        let config: DaemonConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 60);
        assert_eq!(config.orchestrator.memory.summary_max_chars, 4000); // still default
        assert_eq!(config.execution.dag.max_concurrent_agents, 8);
        assert_eq!(config.execution.dag.node_timeout_secs, 300); // still default
    }

    #[test]
    fn test_validate_clamps_out_of_range_values() {
        let mut config = DaemonConfig::default();
        // Set some values out of range
        config.orchestrator.memory.prompt_recent_messages = 0; // min is 1
        config.orchestrator.memory.summary_max_chars = 999_999; // max is 32000
        config.orchestrator.memory.fts_jaccard_threshold = 2.5; // max is 1.0
        config.orchestrator.memory.decay.half_life_days = 0.0; // min is 1.0
        config.execution.dag.max_concurrent_agents = 100; // max is 32
        config.security.max_input_length = 0; // min is 1024
        config.server.event_bus_capacity = 1; // min is 64

        config.validate();

        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 1);
        assert_eq!(config.orchestrator.memory.summary_max_chars, 32000);
        assert_eq!(config.orchestrator.memory.fts_jaccard_threshold, 1.0);
        assert_eq!(config.orchestrator.memory.decay.half_life_days, 1.0);
        assert_eq!(config.execution.dag.max_concurrent_agents, 32);
        assert_eq!(config.security.max_input_length, 1024);
        assert_eq!(config.server.event_bus_capacity, 64);
    }

    #[test]
    fn test_validate_leaves_valid_values_unchanged() {
        let mut config = DaemonConfig::default();
        let original = config.clone();
        config.validate();

        // All defaults should be within valid ranges
        assert_eq!(config.orchestrator.memory.prompt_recent_messages, original.orchestrator.memory.prompt_recent_messages);
        assert_eq!(config.orchestrator.memory.summary_max_chars, original.orchestrator.memory.summary_max_chars);
        assert_eq!(config.execution.dag.max_concurrent_agents, original.execution.dag.max_concurrent_agents);
        assert_eq!(config.security.max_input_length, original.security.max_input_length);
        assert_eq!(config.server.event_bus_capacity, original.server.event_bus_capacity);
    }
}
