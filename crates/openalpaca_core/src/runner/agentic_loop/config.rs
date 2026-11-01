use crate::agent::subagent::AgentConstraints;
use crate::bus::EventBus;
use crate::orchestrator::working_state::{Cursor, WorkingState, WriterConfig};
use crate::runner::agentic_loop::AnswerGuard;
use crate::runner::steering::SteeringInbox;
use crate::session_log::SessionLogHandle;
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
    /// **Deprecated:** Use `ContextBudgetManager` instead. This field is ignored
    /// when `context_budget` is provided (which is always the case in production).
    /// Retained only for `with_context_window()` seed calculation.
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
    /// What compaction's working-state writer is called with (design 11
    /// §3.1, CQ-3): `[orchestrator.costs] summary_model` (`None` the
    /// router's default), its daily ceiling, the line cut and the block's
    /// limit. Every loop that compacts sets it from the daemon config.
    pub writer: WriterConfig,
    /// The conversation's working state when the loop begins (design 11
    /// §3.3): what the first `LlmSummary` compaction folds the older messages
    /// into. The main loop sets it from the session row, the lead from the
    /// conversation its run was started from (CQ-6); `None` elsewhere.
    pub working_state: Option<Arc<WorkingState>>,
    /// The §3.2 cursor over the rows the loop was handed — the main loop's
    /// only; what a compaction's `compaction` record names as `state_through`,
    /// and the rows behind them that the loop's first compaction reads first.
    pub working_state_cursor: Option<Cursor>,
    /// Optional event bus for emitting compaction telemetry.
    /// When set, the agentic loop publishes `CompactionTriggered` events.
    pub event_bus: Option<EventBus>,
    /// When true, the loop injects an ephemeral system notice into each
    /// request once cost or round ratios cross 0.8 (spec P0).
    pub experimental_ephemeral_pressure: bool,
    /// Steering inbox for mid-workflow user interjections (Routing V2).
    /// When `Some`, the loop drains queued messages at its round boundary
    /// and injects them as `<user_interjection>` user messages. `None`
    /// (default) disables steering entirely.
    pub steering: Option<Arc<SteeringInbox>>,
    /// The session event log this loop narrates into (§5.5) — rounds, tool
    /// calls and their payloads, steering drains, compaction, and the exit.
    /// The identical pattern to `steering` above: `None` (default) means the
    /// loop writes nothing, which is what every non-session caller wants.
    pub session_log: Option<SessionLogHandle>,
    /// The 037 span every record of this loop belongs to (P-20): the lead's
    /// `lead::<task_id>` or a subagent's node id. `None` for a main-loop turn,
    /// which has no span.
    pub span_id: Option<String>,
    /// `[orchestrator.sessions] tool_result_inline_bytes` (§5.4, P-16/C-2):
    /// above it a tool result is spilled to the session's `results/` and the
    /// model is handed the stub that names it. With no `session_log` there is
    /// nowhere to spill, so the same number is the head-only cut instead —
    /// one threshold, never two.
    pub tool_result_inline_bytes: usize,
    /// H3 — the last look at the answer before the loop returns `Complete`.
    ///
    /// `Some` only on the Routing V2 main loop, whose guard objects to an
    /// answer stating a task id no run of this turn's owner answers to. The
    /// loop grants the guard exactly one corrective round and then **appends**
    /// the guard's own line to whatever the model said (N3) — the answer is
    /// never taken away. `None` (every other caller: the lead, subagents,
    /// skills, compaction) reviews nothing and costs nothing.
    pub answer_guard: Option<Arc<dyn AnswerGuard>>,
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
            writer: self.writer.clone(),
            working_state: self.working_state.clone(),
            working_state_cursor: self.working_state_cursor.clone(),
            event_bus: self.event_bus.clone(),
            experimental_ephemeral_pressure: self.experimental_ephemeral_pressure,
            steering: self.steering.clone(),
            session_log: self.session_log.clone(),
            span_id: self.span_id.clone(),
            tool_result_inline_bytes: self.tool_result_inline_bytes,
            answer_guard: self.answer_guard.clone(),
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
            .field("writer", &self.writer)
            .field("working_state", &self.working_state.is_some())
            .field("working_state_cursor", &self.working_state_cursor)
            .field("event_bus", &self.event_bus.is_some())
            .field(
                "experimental_ephemeral_pressure",
                &self.experimental_ephemeral_pressure,
            )
            .field("steering", &self.steering.is_some())
            .field("session_log", &self.session_log.is_some())
            .field("span_id", &self.span_id)
            .field("tool_result_inline_bytes", &self.tool_result_inline_bytes)
            .field("answer_guard", &self.answer_guard.is_some())
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
            writer: WriterConfig::default(),
            working_state: None,
            working_state_cursor: None,
            event_bus: None,
            experimental_ephemeral_pressure: false,
            steering: None,
            session_log: None,
            span_id: None,
            tool_result_inline_bytes: super::tool_helpers::MAX_TOOL_RESULT_SIZE,
            answer_guard: None,
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
                    // NOTE: no SystemEvent::ModelAccessDenied here — this is a
                    // static factory with no EventBus in scope (event_bus is
                    // attached to the LoopConfig later). The runtime denial in
                    // the agentic loop publishes the event; this path only
                    // downgrades to the router default and logs.
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
            writer: WriterConfig::default(),
            working_state: None,
            working_state_cursor: None,
            event_bus: None,
            experimental_ephemeral_pressure: false,
            steering: None,
            session_log: None,
            span_id: None,
            tool_result_inline_bytes: super::tool_helpers::MAX_TOOL_RESULT_SIZE,
            answer_guard: None,
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
    /// What this loop's cost cap was last checked against: the spend
    /// accumulated since the loop began (everything a shared accumulator
    /// holds, where one was passed) — the figure a `CostExceeded` exit went
    /// past. `estimated_cost` reads the agent's whole total in the cost
    /// tracker, which for a singleton template's reused instance id includes
    /// its earlier runs (N-LEAD-5).
    pub cap_spend: f64,
    /// The last tool result that came back an error, verbatim (V3).
    ///
    /// A turn that ends with no answer usually ends that way because a tool
    /// kept failing, and "I stopped after 8 rounds" is only half an answer
    /// without the reason. `None` when no tool failed — the loop tracks the
    /// last one, not a list: the reader wants the one it stopped on.
    pub last_tool_error: Option<String>,
    /// The working state this loop's last `LlmSummary` compaction wrote, and
    /// its boundary: `messages[2..boundary]` of the list the loop was handed
    /// lie behind it (design 11 §3.3). Counted in the list as the compaction
    /// found it, so once an earlier tier or compaction had shortened the list
    /// it falls short of the truth, never past it. `None` when the loop never
    /// wrote one.
    pub working_state: Option<(WorkingState, usize)>,
}

impl LoopResult {
    /// The line a turn shows when it produced no answer text of its own (V3).
    ///
    /// `None` when there is genuine content, and for `Cancelled` — a turn the
    /// user stopped is not a turn that failed to speak, and it keeps whatever
    /// its caller already does.
    ///
    /// The vocabulary is the lead's ([`completion_status_line`]), rewritten
    /// for a chat turn: the same four reasons, named the way the person who
    /// asked the question would name them.
    ///
    /// [`completion_status_line`]: crate::orchestrator::dispatcher::completion_status_line
    pub fn no_answer_line(&self) -> Option<String> {
        if !self.final_content.trim().is_empty() {
            return None;
        }
        let reason = match &self.finish_reason {
            LoopFinishReason::Cancelled => return None,
            LoopFinishReason::MaxRounds => format!(
                "I stopped {} without reaching an answer.",
                rounds_spent(self.rounds_used)
            ),
            LoopFinishReason::CostExceeded => {
                format!("I {STOPPED_SHORT}: this turn {HIT_COST_LIMIT}.")
            }
            LoopFinishReason::Truncated => format!("I {STOPPED_SHORT}: {HIT_OUTPUT_LIMIT}."),
            LoopFinishReason::Error(e) => {
                format!("I could not finish this turn: {e}")
            }
            LoopFinishReason::Complete => {
                "I finished this turn without writing an answer.".to_string()
            }
        };
        Some(self.with_last_tool_error(reason))
    }

    /// How a subagent that did not succeed is reported to its lead
    /// (N-LEAD-5): the cause in the words [`Self::no_answer_line`] uses, an
    /// error as written — never its `Debug` form — and the cost cap with its
    /// amount. Where a retry cannot change the outcome — the cost cap,
    /// credentials the provider refused, no model the agent can use, a model
    /// it may not use — it says so, because the lead's prompt tells it to
    /// retry a failed subagent. The last tool error follows, as there.
    ///
    /// `max_cost` is the subagent's own cap (its template's
    /// `max_cost_per_task`, else `[execution.agent_defaults] max_cost`); the
    /// spend beside it is [`Self::cap_spend`], what the cap was checked
    /// against, not the agent's lifetime total.
    pub fn subagent_failure(&self, agent: &str, max_cost: f64) -> String {
        let reason = match &self.finish_reason {
            LoopFinishReason::MaxRounds => format!(
                "Agent '{agent}' stopped {} without reaching an answer.",
                rounds_spent(self.rounds_used)
            ),
            LoopFinishReason::CostExceeded => format!(
                "Agent '{agent}' {STOPPED_SHORT}: it {HIT_COST_LIMIT} of {} (it spent {}). A \
                 retry will not help: every attempt has the same limit — give the work a \
                 narrower objective, or do this part yourself.",
                usd(max_cost),
                usd(self.cap_spend)
            ),
            LoopFinishReason::Truncated => {
                format!("Agent '{agent}' {STOPPED_SHORT}: {HIT_OUTPUT_LIMIT}.")
            }
            LoopFinishReason::Cancelled => {
                format!("Agent '{agent}' was cancelled before it finished.")
            }
            LoopFinishReason::Error(e) => {
                let e = e.trim_end();
                match retry_cannot_help(e) {
                    Some(why) => {
                        let stop = if e.ends_with(['.', '!', '?']) {
                            ""
                        } else {
                            "."
                        };
                        format!(
                            "Agent '{agent}' could not finish: {e}{stop} A retry will not \
                             help: {why}."
                        )
                    }
                    None => format!("Agent '{agent}' could not finish: {e}"),
                }
            }
            LoopFinishReason::Complete => {
                format!("Agent '{agent}' finished without writing an answer.")
            }
        };
        self.with_last_tool_error(reason)
    }

    /// `reason`, then the last tool error when there was one (V3, W2).
    fn with_last_tool_error(&self, reason: String) -> String {
        match self.last_tool_error {
            Some(ref err) => format!("{reason} The last tool error was: {}", user_facing(err)),
            None => reason,
        }
    }
}

// The words a loop's stop is told in, shared by its two readers: the person
// a chat turn answers (`no_answer_line`, V3) and the lead a subagent reports
// to (`subagent_failure`, N-LEAD-5).
const STOPPED_SHORT: &str = "stopped before reaching an answer";
const HIT_COST_LIMIT: &str = "hit its cost limit";
const HIT_OUTPUT_LIMIT: &str = "the reply hit the model's output limit";

fn rounds_spent(rounds: usize) -> String {
    format!(
        "after {rounds} tool {}",
        if rounds == 1 { "round" } else { "rounds" }
    )
}

/// A dollar amount as a cap or a spend reads: cents, or four places below a
/// cent so a small cap is never shown as `$0.00`.
fn usd(amount: f64) -> String {
    if amount.abs() >= 0.01 || amount == 0.0 {
        format!("${amount:.2}")
    } else {
        format!("${amount:.4}")
    }
}

/// Why an `Error` exit is one a retry cannot change, or `None` when a retry
/// may help (N-LEAD-5).
///
/// `LoopFinishReason::Error` keeps the router's message, not its type, so the
/// message is matched against the router's own `Display` of each permanent
/// error — built here from the types themselves, so a rewording there moves
/// both sides at once — plus the loop's own model-access refusal.
///
/// Credentials refused reach a routed loop as `AllKeysRefused` (the router
/// rotates past a refused key and names the end when every attempt was
/// refused) or `NoApiCompatibleKeys`, each kept past the fallback ladder; the
/// provider's own `AuthenticationFailed` and an HTTP 401 or 403 arrive
/// unflattened only on the direct, unrouted backend.
fn retry_cannot_help(error: &str) -> Option<&'static str> {
    use openalpaca_llm::{LlmError, LlmRouterError};
    let is = |e: LlmRouterError| error == e.to_string();
    let starts = |e: LlmRouterError| error.starts_with(&e.to_string());
    let llm = |e: LlmError| LlmRouterError::Llm(e);

    if is(LlmRouterError::NoRoutableModel)
        || starts(LlmRouterError::UnknownModel(String::new()))
        || starts(LlmRouterError::ProviderNotConfigured(String::new()))
        || is(llm(LlmError::NotConfigured))
    {
        return Some(
            "no model this agent can use is available — a provider must be turned on, or the \
             agent's model changed, first",
        );
    }
    if is(LlmRouterError::AllKeysRefused)
        || is(LlmRouterError::NoApiCompatibleKeys)
        || starts(llm(LlmError::AuthenticationFailed(String::new())))
        || starts(llm(LlmError::TokenRefresh(String::new())))
        || [401, 403].into_iter().any(|status| {
            starts(llm(LlmError::Api {
                status,
                message: String::new(),
            }))
        })
    {
        return Some("the provider refused the credentials — the key must be fixed first");
    }
    if error.starts_with("Model access denied:") {
        return Some("this agent may not use the model that answered");
    }
    None
}

/// A tool error as the person reading the turn should see it (W2).
///
/// `[tool_error] ` is the loop's own marker: it is how the transcript shows a
/// failed call on a wire with no error field (whether the call failed is its
/// own `Err`, carried as `ChatMessage::is_error`, N-TOOL-4), and it belongs in
/// the transcript the model reads, not in a sentence addressed to a human.
/// Only the leading marker goes — the message itself, hint and all, is what
/// the reader needs.
fn user_facing(err: &str) -> &str {
    let err = err.trim();
    match err.strip_prefix("[tool_error]") {
        Some(rest) => rest.trim_start(),
        None => err,
    }
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
