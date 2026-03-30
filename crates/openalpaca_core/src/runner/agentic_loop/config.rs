use crate::agent::subagent::AgentConstraints;
use crate::bus::EventBus;
use crate::security::capabilities::CapabilityManager;
use openalpaca_llm::{ModelRegistry, StreamEvent, ThinkingConfig, ToolChoice};
use std::sync::Arc;
use std::time::Duration;

/// Default context compression threshold (fraction of max_context_tokens).
pub(super) const DEFAULT_CONTEXT_THRESHOLD: f64 = 0.6;

/// Optional callback for streaming events (UI progress, logging).
pub type StreamCallback = Arc<dyn Fn(&StreamEvent) + Send + Sync>;

/// Configuration for the agentic loop.
///
/// `Clone` is implemented manually because `StreamCallback` (`Arc<dyn Fn>`)
/// implements `Clone` but not `Debug`.
pub struct LoopConfig {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    pub max_tool_runtime: Duration,
    pub max_cost: f64,
    /// Override model for this loop (used by LlmRouter).
    pub model: Option<String>,
    /// Fallback models (informational — fallback is handled by router's fallback chain).
    pub fallback_models: Vec<String>,
    /// Agent model constraints for access control enforcement.
    pub agent_constraints: Option<AgentConstraints>,
    /// Maximum estimated input tokens before triggering context compression.
    /// When `> 0` and estimated tokens exceed this, older rounds are compressed
    /// into a summary, preserving the system prompt + initial query + recent rounds.
    /// Default: `0` (disabled — auto-set from model context window × `context_threshold` via
    /// `with_context_window()`).
    pub max_context_tokens: u32,
    /// Number of most recent conversation rounds to always preserve during
    /// context compression. Each "round" is roughly 3 messages (assistant +
    /// tool results). Default: `4`.
    pub context_tail_keep: usize,
    /// Fraction of model context window to use as compression trigger (0.0–1.0).
    /// Only used by `with_context_window()`. Default: `0.6`.
    pub context_threshold: f64,
    /// Tool choice to force on the first round only (`rounds == 0`).
    /// After the first round, reverts to `None` (auto).
    pub initial_tool_choice: Option<ToolChoice>,
    /// Enable Anthropic prompt caching for system prompt and tools.
    pub enable_caching: bool,
    /// Extended thinking configuration (Anthropic only).
    pub thinking: Option<ThinkingConfig>,
    /// Optional streaming callback for real-time event forwarding.
    /// When set and the backend is Router, `LlmBackend::complete()` attempts
    /// streaming first, forwarding each event to this callback, then falls
    /// back to non-streaming on failure.
    pub stream_callback: Option<StreamCallback>,
    /// Maximum wall-clock duration for streaming collection.
    /// If `collect_stream()` exceeds this, it is cancelled and the loop
    /// falls back to non-streaming. Default: 10 minutes.
    pub max_stream_duration: Duration,
    /// Model to use for LLM-based context compaction (extraction + summarization).
    /// When `None`, falls back to heuristic-only compaction.
    pub compaction_model: Option<String>,
    /// Optional event bus for emitting compaction telemetry.
    /// When set, the agentic loop publishes `CompactionTriggered` events.
    pub event_bus: Option<EventBus>,
}

impl Clone for LoopConfig {
    fn clone(&self) -> Self {
        Self {
            max_rounds: self.max_rounds,
            max_tools_per_round: self.max_tools_per_round,
            max_tool_runtime: self.max_tool_runtime,
            max_cost: self.max_cost,
            model: self.model.clone(),
            fallback_models: self.fallback_models.clone(),
            agent_constraints: self.agent_constraints.clone(),
            max_context_tokens: self.max_context_tokens,
            context_tail_keep: self.context_tail_keep,
            context_threshold: self.context_threshold,
            initial_tool_choice: self.initial_tool_choice.clone(),
            enable_caching: self.enable_caching,
            thinking: self.thinking.clone(),
            stream_callback: self.stream_callback.clone(),
            max_stream_duration: self.max_stream_duration,
            compaction_model: self.compaction_model.clone(),
            event_bus: self.event_bus.clone(),
        }
    }
}

impl std::fmt::Debug for LoopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopConfig")
            .field("max_rounds", &self.max_rounds)
            .field("max_tools_per_round", &self.max_tools_per_round)
            .field("max_cost", &self.max_cost)
            .field("model", &self.model)
            .field("max_context_tokens", &self.max_context_tokens)
            .field("context_threshold", &self.context_threshold)
            .field("context_tail_keep", &self.context_tail_keep)
            .field("enable_caching", &self.enable_caching)
            .field("thinking", &self.thinking)
            .field("stream_callback", &self.stream_callback.is_some())
            .field("max_stream_duration", &self.max_stream_duration)
            .field("compaction_model", &self.compaction_model)
            .field("event_bus", &self.event_bus.is_some())
            .finish()
    }
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_rounds: 15,
            max_tools_per_round: 5,
            max_tool_runtime: Duration::from_secs(60),
            max_cost: 1.00,
            model: None,
            fallback_models: Vec::new(),
            agent_constraints: None,
            max_context_tokens: 0,
            context_tail_keep: 4,
            context_threshold: DEFAULT_CONTEXT_THRESHOLD,
            initial_tool_choice: None,
            enable_caching: true,
            thinking: None,
            stream_callback: None,
            max_stream_duration: Duration::from_secs(600),
            compaction_model: None,
            event_bus: None,
        }
    }
}

impl LoopConfig {
    /// Build from daemon defaults + agent-level overrides.
    /// Works with both `AgentDefaults` and `LeadAgentDefaults` since they share the
    /// same field names. Pass the raw field values to keep the factory generic.
    ///
    /// Validates the agent's configured model against its model access constraints.
    /// If the model is denied, falls back to `None` (router default) with a warning.
    pub fn from_defaults(
        max_rounds: usize,
        max_tools_per_round: usize,
        max_tool_runtime_secs: u64,
        max_cost: f64,
        agent: &crate::agent::subagent::SubAgent,
    ) -> Self {
        // Validate the agent's configured model against its model access constraints
        let model = if let Some(ref model_id) = agent.llm_config.model {
            match CapabilityManager::check_model_access(&agent.id, model_id, &agent.constraints) {
                Ok(()) => Some(model_id.clone()),
                Err(violation) => {
                    tracing::warn!(
                        agent_id = %agent.id,
                        model = %model_id,
                        "Model access denied for agent, falling back to router default: {}",
                        violation,
                    );
                    None
                }
            }
        } else {
            None
        };

        Self {
            max_rounds: agent.constraints.max_rounds.unwrap_or(max_rounds),
            max_tools_per_round,
            max_tool_runtime: Duration::from_secs(
                agent
                    .constraints
                    .timeout_seconds
                    .unwrap_or(max_tool_runtime_secs),
            ),
            max_cost: agent.constraints.max_cost_per_task.unwrap_or(max_cost),
            model,
            fallback_models: agent.llm_config.fallback_models.clone(),
            agent_constraints: Some(agent.constraints.clone()),
            max_context_tokens: 0,
            context_tail_keep: 4,
            context_threshold: DEFAULT_CONTEXT_THRESHOLD,
            initial_tool_choice: None,
            enable_caching: true,
            thinking: None,
            stream_callback: None,
            max_stream_duration: Duration::from_secs(600),
            compaction_model: None,
            event_bus: None,
        }
    }

    /// Set context compression budget from model registry.
    /// Uses `context_threshold` fraction of the model's context window as the compression trigger.
    /// Only sets the budget if `max_context_tokens` is still 0 (not explicitly configured).
    pub fn with_context_window(
        mut self,
        registry: &ModelRegistry,
        model_id: Option<&str>,
    ) -> Self {
        if self.max_context_tokens == 0
            && let Some(model) = model_id
            && let Some(info) = registry.get_model_info(model)
            && info.context_window > 0
        {
            self.max_context_tokens = (info.context_window as f64 * self.context_threshold) as u32;
        }
        self
    }

    /// Build from `AgentDefaults` + agent constraint overrides.
    pub fn from_agent(
        defaults: &crate::daemon_config::AgentDefaults,
        agent: &crate::agent::subagent::SubAgent,
    ) -> Self {
        Self::from_defaults(
            defaults.max_rounds,
            defaults.max_tools_per_round,
            defaults.max_tool_runtime_secs,
            defaults.max_cost,
            agent,
        )
    }

    /// Build from `LeadAgentDefaults` + agent constraint overrides.
    pub fn from_lead_agent(
        defaults: &crate::daemon_config::LeadAgentDefaults,
        agent: &crate::agent::subagent::SubAgent,
    ) -> Self {
        Self::from_defaults(
            defaults.max_rounds,
            defaults.max_tools_per_round,
            defaults.max_tool_runtime_secs,
            defaults.max_cost,
            agent,
        )
    }
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub final_content: String,
    pub rounds_used: usize,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub tool_calls_made: usize,
    pub finish_reason: LoopFinishReason,
    /// Actual model from API response (may differ from requested due to fallback).
    pub model_used: Option<String>,
    /// Wall-clock time for the entire loop execution.
    pub elapsed: Duration,
    /// Accumulated cost for this loop invocation (from CostTracker for Router, local estimate for Direct).
    pub estimated_cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopFinishReason {
    Complete,
    MaxRounds,
    CostExceeded,
    Truncated,
    Cancelled,
    Error(String),
}
