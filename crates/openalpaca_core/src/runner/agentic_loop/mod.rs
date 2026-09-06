mod backend;
mod cost;

pub use cost::LoopCostAccumulator;
mod config;
mod context;
mod tool_helpers;

// Public API (unchanged from before the split)
pub use config::{LoopConfig, LoopFinishReason, LoopResult, StreamCallback};

// Internal re-exports so the core loop and tests can access submodule items
use backend::LlmBackend;
pub(crate) use context::{compress_context, estimate_messages_tokens, estimate_tools_tokens};
use tool_helpers::{format_tool_error, format_tool_error_with_hint, truncate_tool_result};
#[cfg(test)]
use tool_helpers::MAX_TOOL_RESULT_SIZE;

use chrono::Utc;
use crate::runner::steering::SteeringMsg;
use crate::security::capabilities::CapabilityManager;
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::session_log::{Record, RecordType};
use crate::tools::registry::ToolContext;
use serde_json::{Value, json};
use openalpaca_llm::{
    ChatMessage, FinishReason, LlmProvider, LlmRouter, LlmRouterError, RequestContext,
    ToolDefinition,
};
#[cfg(test)]
use openalpaca_llm::ChatRequest;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

/// Maximum retries when LLM response is truncated due to max_tokens.
const MAX_TOKENS_RETRIES: usize = 2;

/// Extra rounds granted per non-empty steering drain. The effective round
/// budget is capped at `2 * max_rounds` regardless of how many drains occur.
const STEERING_ROUNDS_BONUS: usize = 5;

/// Accumulates loop execution state and builds `LoopResult` on exit.
/// Private to this module — avoids repeating the 8-field struct construction
/// at every exit point.
struct LoopState {
    start: Instant,
    rounds: usize,
    total_input: u32,
    total_output: u32,
    tool_calls_made: usize,
    last_assistant_content: String,
    last_model: Option<String>,
    max_tokens_retries: usize,
    last_cost: f64,
}

impl LoopState {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            rounds: 0,
            total_input: 0,
            total_output: 0,
            tool_calls_made: 0,
            last_assistant_content: String::new(),
            last_model: None,
            max_tokens_retries: 0,
            last_cost: 0.0,
        }
    }

    /// Build a `LoopResult` using accumulated state.
    fn result(&self, finish_reason: LoopFinishReason) -> LoopResult {
        LoopResult {
            final_content: self.last_assistant_content.clone(),
            rounds_used: self.rounds,
            total_input_tokens: self.total_input,
            total_output_tokens: self.total_output,
            tool_calls_made: self.tool_calls_made,
            finish_reason,
            model_used: self.last_model.clone(),
            elapsed: self.start.elapsed(),
            estimated_cost: self.last_cost,
        }
    }

    /// Build a `LoopResult` with custom final content.
    fn result_with_content(
        &self,
        content: String,
        finish_reason: LoopFinishReason,
    ) -> LoopResult {
        LoopResult {
            final_content: content,
            rounds_used: self.rounds,
            total_input_tokens: self.total_input,
            total_output_tokens: self.total_output,
            tool_calls_made: self.tool_calls_made,
            finish_reason,
            model_used: self.last_model.clone(),
            elapsed: self.start.elapsed(),
            estimated_cost: self.last_cost,
        }
    }
}

/// Legacy/test entry point — direct provider, no retry.
///
/// When `sandbox` is `Some`, tool calls are routed through the SandboxManager
/// with capability checks, input sanitization, and timeout enforcement.
/// When `sandbox` is `None`, falls back to stub behavior (backward compat).
#[allow(clippy::too_many_arguments)]
pub async fn run_agentic_loop(
    provider: &dyn LlmProvider,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    context_budget: Option<&crate::context_budget::ContextBudgetManager>,
    cancel_token: Option<CancellationToken>,
    tool_context: Option<&ToolContext>,
) -> LoopResult {
    run_agentic_loop_inner(
        LlmBackend::Direct { provider },
        initial_messages,
        tools,
        config,
        sandbox,
        agent_id,
        sandbox_policy,
        context_budget,
        cancel_token,
        tool_context,
        None,
        None,
    )
    .await
}

/// Production entry point — router with key rotation, fallback, cost tracking.
#[allow(clippy::too_many_arguments)]
pub async fn run_agentic_loop_routed(
    router: &LlmRouter,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    task_id: Option<&str>,
    context_budget: Option<&crate::context_budget::ContextBudgetManager>,
    cancel_token: Option<CancellationToken>,
    tool_context: Option<&ToolContext>,
    cost_accumulator: Option<LoopCostAccumulator>,
) -> LoopResult {
    let context = RequestContext {
        agent_id: Some(agent_id.to_string()),
        task_id: task_id.map(|s| s.to_string()),
    };
    run_agentic_loop_inner(
        LlmBackend::Router { router, context, compaction_model: config.compaction_model.clone(), fallback_models: config.fallback_models.clone() },
        initial_messages,
        tools,
        config,
        sandbox,
        agent_id,
        sandbox_policy,
        context_budget,
        cancel_token,
        tool_context,
        cost_accumulator,
        task_id,
    )
    .await
}

/// Narrate one event into the turn's session log (§5.5).
///
/// A no-op when the loop has no log — every non-session caller, and every
/// test that does not care. `emit` itself is a non-blocking `try_send`, so
/// this never stalls a round.
fn log_event(
    config: &LoopConfig,
    task_id: Option<&str>,
    agent_id: &str,
    kind: RecordType,
    data: Value,
) {
    if let Some(ref log) = config.session_log {
        log.emit(
            Record::new(kind)
                .task(task_id)
                .span(config.span_id.as_deref())
                .agent(Some(agent_id))
                .with_data(data),
        );
    }
}

/// `ext {kind, id, generation}` for a tool that belongs to an extension
/// (§5.4, P-17) — what makes the S4 refusal auditable per session. `None`
/// for builtins, which are never on the ENABLE axis.
fn tool_extension(sandbox: Option<&SandboxManager>, tool_name: &str) -> Option<Value> {
    let tool = sandbox?.registry().get(tool_name)?;
    let id = tool.extension_id()?;
    Some(json!({
        "kind": id.kind.as_str(),
        "id": id.name,
        "generation": tool.incarnation(),
    }))
}

/// Return drained-but-unsent steering messages to the inbox on a loop exit
/// that never delivered them to an LLM call (budget, cancellation, or error
/// exits), so the cleanup path can convert them to follow-ups.
fn return_pending_steering(config: &LoopConfig, pending: &mut Vec<SteeringMsg>) {
    if let Some(ref inbox) = config.steering
        && !pending.is_empty()
    {
        inbox.push_front_all(std::mem::take(pending));
    }
}

/// Run the loop and narrate its exit (§5.5: "at every exit").
///
/// The exit record lives here rather than at the nine `return`s inside the
/// core so no future exit can be added without one.
#[allow(clippy::too_many_arguments)]
async fn run_agentic_loop_inner(
    backend: LlmBackend<'_>,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    context_budget: Option<&crate::context_budget::ContextBudgetManager>,
    cancel_token: Option<CancellationToken>,
    tool_context: Option<&ToolContext>,
    cost_accumulator: Option<LoopCostAccumulator>,
    task_id: Option<&str>,
) -> LoopResult {
    let result = run_agentic_loop_core(
        backend,
        initial_messages,
        tools,
        config,
        sandbox,
        agent_id,
        sandbox_policy,
        context_budget,
        cancel_token,
        tool_context,
        cost_accumulator,
        task_id,
    )
    .await;

    if config.session_log.is_some() {
        if let LoopFinishReason::Error(ref message) = result.finish_reason {
            log_event(
                config,
                task_id,
                agent_id,
                RecordType::Error,
                json!({ "where": "agentic_loop", "message": message }),
            );
        }
        log_event(
            config,
            task_id,
            agent_id,
            RecordType::WorkflowDone,
            json!({
                "finish_reason": finish_reason_str(&result.finish_reason),
                "rounds_used": result.rounds_used,
                "tool_calls_made": result.tool_calls_made,
                "input_tokens": result.total_input_tokens,
                "output_tokens": result.total_output_tokens,
                "estimated_cost_usd": result.estimated_cost,
                "elapsed_ms": result.elapsed.as_millis() as u64,
                "model": result.model_used,
            }),
        );
    }
    result
}

/// The exit word a reader sees, stable across refactors of the enum's Debug.
fn finish_reason_str(reason: &LoopFinishReason) -> &'static str {
    match reason {
        LoopFinishReason::Complete => "complete",
        LoopFinishReason::MaxRounds => "max_rounds",
        LoopFinishReason::CostExceeded => "cost_exceeded",
        LoopFinishReason::Truncated => "truncated",
        LoopFinishReason::Cancelled => "cancelled",
        LoopFinishReason::Error(_) => "error",
    }
}

/// Core agentic loop implementation shared by both Direct and Router backends.
#[allow(clippy::too_many_arguments)]
async fn run_agentic_loop_core(
    backend: LlmBackend<'_>,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    context_budget: Option<&crate::context_budget::ContextBudgetManager>,
    cancel_token: Option<CancellationToken>,
    tool_context: Option<&ToolContext>,
    cost_accumulator: Option<LoopCostAccumulator>,
    task_id: Option<&str>,
) -> LoopResult {
    let mut state = LoopState::new();
    // Baseline the agent-scoped cumulative cost BEFORE round 0's LLM call.
    // `agent_cost()` (Router backend) returns the cost tracker's cumulative
    // total for `agent_id`, not this turn's spend — the main loop reuses a
    // literal id ("orchestrator") across every turn with no accumulator, so
    // without this baseline round 0's delta would equal the agent's entire
    // lifetime spend, tripping CostExceeded before the first LLM call once
    // that total passes max_cost (A5, bug-main-loop-cost-lockout.md option 1).
    // Only the baseline changes; the delta arithmetic below is unchanged.
    state.last_cost = backend.agent_cost(0, 0).await;
    let cost_acc = cost_accumulator.unwrap_or_default();
    let mut messages: Arc<Vec<ChatMessage>> = Arc::new(initial_messages);
    let tools_arc = Arc::new(tools);
    let mut consecutive_llm_errors: usize = 0;
    const MAX_LLM_RETRIES: usize = 3;

    // ── Steering state (Routing V2) ────────────────────────────────
    // `steering_bonus_rounds` extends the round budget by
    // `STEERING_ROUNDS_BONUS` per non-empty drain, capped at 2× max_rounds.
    // `pending_steering` holds drained messages until an LLM call consumes
    // them, so budget exits can re-append unsent messages to the inbox for
    // follow-up conversion.
    let mut steering_bonus_rounds: usize = 0;
    // P-14: what this run's compactions have taken out of its context so far,
    // in tokens. Cumulative across every compaction the loop performs, which
    // is what makes a later `compaction` record readable on its own.
    let mut cumulative_dropped_tokens: u64 = 0;
    let mut pending_steering: Vec<SteeringMsg> = Vec::new();

    // Pre-compute tool token estimate once — avoids re-serializing tool JSON
    // schemas on every Router retry attempt. `None` for Direct backend because
    // it uses `ChatRequest` which doesn't pass through `estimate_request_tokens`.
    let tools_token_estimate: Option<u32> = if backend.supports_retry() {
        Some(estimate_tools_tokens(&tools_arc) as u32)
    } else {
        None
    };

    // Build context_management from budget manager (Phase D)
    let context_management = context_budget.map(|budget| {
        openalpaca_llm::context_management::ContextManagement::from_budget(
            budget.compaction_trigger(),
            5, // keep 5 recent tool-use blocks
            2, // keep 2 recent thinking turns
        )
    });

    tracing::info!(
        agent_id = agent_id,
        tools_count = tools_arc.len(),
        max_rounds = config.max_rounds,
        max_cost = config.max_cost,
        backend = if backend.supports_retry() { "router" } else { "direct" },
        context_management = context_management.is_some(),
        "Agentic loop started"
    );

    loop {
        // ── 1. Cancellation check ──────────────────────────────────
        {
            let _span = tracing::info_span!(
                "loop.step.cancellation_check",
                agent_id = agent_id,
                round = state.rounds,
            )
            .entered();
            tracing::trace!("cancellation_check step entered");
            if let Some(ref token) = cancel_token
                && token.is_cancelled()
            {
                tracing::info!(
                    agent_id = agent_id,
                    rounds = state.rounds,
                    "Agentic loop cancelled"
                );
                // A prior failed call may have left drained-but-unsent
                // steering messages pending — return them for follow-up
                // conversion (Routing V2, widened from the budget exits).
                return_pending_steering(config, &mut pending_steering);
                return state.result(LoopFinishReason::Cancelled);
            }
        }

        // ── 2. Max rounds check ────────────────────────────────────
        // Steering drains extend the budget by STEERING_ROUNDS_BONUS each,
        // capped at 2× max_rounds. With no steering the bonus is 0 and this
        // is exactly `state.rounds >= config.max_rounds`.
        let effective_max_rounds = (config.max_rounds + steering_bonus_rounds)
            .min(config.max_rounds.saturating_mul(2));
        {
            let _span = tracing::info_span!(
                "loop.step.max_rounds_check",
                agent_id = agent_id,
                round = state.rounds,
            )
            .entered();
            tracing::trace!("max_rounds_check step entered");
            if state.rounds >= effective_max_rounds {
                tracing::info!(
                    agent_id = agent_id,
                    rounds = state.rounds,
                    "Agentic loop exiting: max rounds reached"
                );
                // Return drained-but-unsent steering messages to the inbox
                // so the cleanup path can convert them to follow-ups.
                return_pending_steering(config, &mut pending_steering);
                return state.result(LoopFinishReason::MaxRounds);
            }
        }

        // ── 3. Cost check (CostTracker for Router, local estimate for Direct) ──
        // agent-scoped: the per-agent budget must not inherit other agents' spend.
        let round_cost = backend.agent_cost(state.total_input, state.total_output).await;
        let cost_delta = (round_cost - state.last_cost).max(0.0);
        if cost_delta > 0.0 {
            cost_acc.add_usd(cost_delta);
        }
        state.last_cost = round_cost;
        let accumulated_cost = cost_acc.total_usd();
        let cost_ratio = accumulated_cost / config.max_cost;
        {
            let _span = tracing::info_span!(
                "loop.step.cost_check",
                agent_id = agent_id,
                round = state.rounds,
                cost_ratio = cost_ratio,
                accumulated_cost = accumulated_cost,
            )
            .entered();
            tracing::trace!("cost_check step entered");
            if accumulated_cost > config.max_cost {
                tracing::info!(
                    agent_id = agent_id,
                    rounds = state.rounds,
                    accumulated_cost,
                    "Agentic loop exiting: cost limit exceeded"
                );
                // Return drained-but-unsent steering messages to the inbox
                // so the cleanup path can convert them to follow-ups.
                return_pending_steering(config, &mut pending_steering);
                return state.result(LoopFinishReason::CostExceeded);
            }
        }

        // ── Ephemeral budget-pressure notice (spec P0b) ────────────
        // Stateless: recomputed every iteration, passed to `backend.complete`
        // as `ephemeral_system_notice`, never appended to `messages`. Fires
        // when either the cost ratio or rounds ratio reaches 0.8.
        let ephemeral_notice = if config.experimental_ephemeral_pressure {
            let rounds_ratio = (state.rounds as f64) / (config.max_rounds as f64).max(1.0);
            let fire = cost_ratio.max(rounds_ratio) >= 0.8;
            if fire {
                let rp = (rounds_ratio * 100.0).round() as u32;
                let cp = (cost_ratio * 100.0).round() as u32;
                Some(format!(
                    "[budget_notice]\n\
                     Budget status: {}/{} rounds ({}%), ${:.2}/${:.2} spent ({}%).\n\
                     Prefer concluding the current task over opening new tool calls. \
                     If you need more than 2 additional tool calls to finish, \
                     summarize the partial result instead.\n\
                     [/budget_notice]",
                    state.rounds,
                    config.max_rounds,
                    rp,
                    accumulated_cost,
                    config.max_cost,
                    cp,
                ))
            } else {
                None
            }
        } else {
            None
        };

        // ── 4. Context compression (budget-aware) ──────────────────
        let msg_tokens_estimate = estimate_messages_tokens(&messages) as usize;
        let compaction_span = tracing::info_span!(
            "loop.step.compaction",
            agent_id = agent_id,
            round = state.rounds,
            msg_tokens = msg_tokens_estimate,
        );
        compaction_span.in_scope(|| {
            tracing::trace!("compaction step entered");
        });
        if let Some(budget) = context_budget {
            let msg_tokens = msg_tokens_estimate;
            if budget.should_compact(msg_tokens) {
                let messages_before = messages.len();
                let tier = budget.compaction_tier(msg_tokens);

                compaction_span.in_scope(|| {
                    tracing::info!(
                        agent_id = agent_id,
                        msg_tokens,
                        trigger = budget.compaction_trigger(),
                        messages_before,
                        ?tier,
                        "Graduated compaction triggered"
                    );
                });

                let compactor = crate::prompt_ctx::compaction::GraduatedCompactor::new(
                    budget, &backend, &backend,
                );
                let report = compactor
                    .compact(
                        Arc::make_mut(&mut messages),
                        config.context_tail_keep,
                        cancel_token.clone(),
                    )
                    .instrument(compaction_span.clone())
                    .await;

                compaction_span.in_scope(|| {
                    tracing::info!(
                        agent_id = agent_id,
                        messages_before,
                        messages_after = messages.len(),
                        tiers_applied = ?report.tiers_applied,
                        initial_tokens = report.initial_tokens,
                        final_tokens = report.final_tokens,
                        "Graduated compaction completed"
                    );
                });

                cumulative_dropped_tokens += report
                    .initial_tokens
                    .saturating_sub(report.final_tokens)
                    as u64;

                // §5.5: the loop narrates its compaction into the session
                // log. `preserved_from_seq` (P-14) is stamped by the writer,
                // which is the only party that knows what the log already
                // holds. `dropped_from_seq` and `summary_msg_id` need a
                // message→log-seq map that does not exist yet (T55) and are
                // explicit nulls rather than guesses; every other value below
                // is the report's own.
                log_event(
                    config,
                    task_id,
                    agent_id,
                    RecordType::Compaction,
                    json!({
                        "tier": format!("{tier:?}"),
                        "trigger": "auto",
                        "pre_tokens": report.initial_tokens,
                        "post_tokens": report.final_tokens,
                        "messages_before": report.messages_before,
                        "messages_after": report.messages_after,
                        "messages_discarded": report.messages_discarded,
                        "memories_extracted": report.memories_extracted,
                        "tiers_applied": format!("{:?}", report.tiers_applied),
                        "cumulative_dropped_tokens": cumulative_dropped_tokens,
                        "dropped_from_seq": Value::Null,
                        "summary_msg_id": Value::Null,
                    }),
                );

                // Emit CompactionTriggered telemetry
                if let Some(ref bus) = config.event_bus {
                    bus.publish(crate::events::SystemEvent::CompactionTriggered {
                        request_id: uuid::Uuid::new_v4(),
                        utilization_pct: report.initial_tokens as f64
                            / budget.model_context_window() as f64
                            * 100.0,
                        messages_before: report.messages_before,
                        messages_after: report.messages_after,
                        memories_extracted: report.memories_extracted,
                        messages_discarded: report.messages_discarded,
                        summary_tokens: report.initial_tokens
                            .saturating_sub(report.final_tokens),
                        timestamp: Utc::now(),
                    });
                }
            }
        }

        // ── Steering drain (Routing V2) ────────────────────────────
        // Placed after the cancellation/budget checks and compaction,
        // immediately before the request is built — draining any earlier
        // would lose messages on MaxRounds/CostExceeded exits.
        if let Some(ref inbox) = config.steering {
            let drained = inbox.drain_all();
            if !drained.is_empty() {
                steering_bonus_rounds += STEERING_ROUNDS_BONUS;
                tracing::info!(
                    agent_id = agent_id,
                    round = state.rounds,
                    interjections = drained.len(),
                    "Steering drain: injecting user interjections"
                );
                log_event(
                    config,
                    task_id,
                    agent_id,
                    RecordType::SteeringDrained,
                    json!({
                        "at": "round_boundary",
                        "round": state.rounds,
                        "count": drained.len(),
                        "request_ids": drained
                            .iter()
                            .map(|m| m.request_id.to_string())
                            .collect::<Vec<_>>(),
                    }),
                );
                for msg in &drained {
                    Arc::make_mut(&mut messages)
                        .push(ChatMessage::user(&msg.to_interjection()));
                }
                pending_steering.extend(drained);
            }
        }

        // ── 5. Build request (pre-LLM-call assembly) ──────────────
        let tool_choice = {
            let _span = tracing::info_span!(
                "loop.step.build_request",
                agent_id = agent_id,
                round = state.rounds,
                messages_count = messages.len(),
            )
            .entered();
            tracing::trace!("build_request step entered");

            // ── 6. Pressure-layer ephemeral notice ─────────────────
            // `ephemeral_notice` is computed above (after the cost_check
            // step). The span fires only when a notice is present, so the
            // telemetry signal tracks real injections rather than no-ops.
            if let Some(ref n) = ephemeral_notice {
                let _ps = tracing::info_span!(
                    "loop.step.pressure_layer",
                    agent_id = agent_id,
                    round = state.rounds,
                )
                .entered();
                tracing::trace!(notice_len = n.len(), "pressure_layer step entered");
            }

            let tc = if state.rounds == 0 {
                config.initial_tool_choice.clone()
            } else {
                None
            };

            tracing::debug!(
                agent_id = agent_id,
                round = state.rounds + 1,
                messages_count = messages.len(),
                "LLM call starting"
            );
            tc
        };

        // ── 8. LLM call ───────────────────────────────────────────
        let llm_call_span = tracing::info_span!(
            "loop.step.llm_call",
            agent_id = agent_id,
            round = state.rounds + 1,
            model = config.model.as_deref().unwrap_or("default"),
            messages_count = messages.len(),
        );
        llm_call_span.in_scope(|| {
            tracing::trace!("llm_call step entered");
        });
        let llm_result = if let Some(ref token) = cancel_token {
            tokio::select! {
                result = backend.complete(
                    Arc::clone(&messages),
                    Arc::clone(&tools_arc),
                    config.model.clone(),
                    tool_choice,
                    tools_token_estimate,
                    config.enable_caching,
                    config.thinking.clone(),
                    config.stream_callback.as_ref(),
                    config.max_stream_duration,
                    context_management.clone(),
                    ephemeral_notice.clone(),
                ).instrument(llm_call_span.clone()) => result,
                _ = token.cancelled() => {
                    tracing::info!(agent_id = agent_id, round = state.rounds + 1, "LLM call interrupted by cancellation");
                    // The interrupted call never delivered this round's
                    // drained steering messages — return them to the inbox
                    // for follow-up conversion.
                    return_pending_steering(config, &mut pending_steering);
                    return state.result(LoopFinishReason::Cancelled);
                }
            }
        } else {
            backend
                .complete(
                    Arc::clone(&messages),
                    Arc::clone(&tools_arc),
                    config.model.clone(),
                    tool_choice,
                    tools_token_estimate,
                    config.enable_caching,
                    config.thinking.clone(),
                    config.stream_callback.as_ref(),
                    config.max_stream_duration,
                    context_management.clone(),
                    ephemeral_notice.clone(),
                )
                .instrument(llm_call_span)
                .await
        };

        // ── 9/10. Parse response / persist or execute tools ────────
        match llm_result {
            Ok(response) => {
                consecutive_llm_errors = 0;
                // Any injected steering messages were consumed by this call.
                pending_steering.clear();

                // ── 9. Response parse ──────────────────────────────
                {
                    let _span = tracing::info_span!(
                        "loop.step.response_parse",
                        agent_id = agent_id,
                        round = state.rounds,
                    )
                    .entered();
                    tracing::trace!("response_parse step entered");

                    // Record usage first — the API call already happened, tokens
                    // were consumed regardless of whether we accept the response.
                    state.total_input += response.usage.input_tokens;
                    state.total_output += response.usage.output_tokens;
                    state.rounds += 1;
                    state.last_model = Some(response.model.clone());

                    // Model access check (Router only): the router may fallback to a
                    // different model than requested. Verify the agent is allowed to
                    // use the actual model.
                    if backend.supports_retry()
                        && let Some(ref constraints) = config.agent_constraints
                        && let Err(violation) = CapabilityManager::check_model_access(
                            agent_id,
                            &response.model,
                            constraints,
                        )
                    {
                        tracing::warn!(
                            agent_id = agent_id,
                            model = %response.model,
                            "Model access denied at runtime: {}",
                            violation,
                        );
                        if let Some(ref bus) = config.event_bus {
                            bus.publish(crate::events::SystemEvent::ModelAccessDenied {
                                agent_id: agent_id.to_string(),
                                model_id: response.model.clone(),
                                reason: violation.to_string(),
                                timestamp: Utc::now(),
                            });
                        }
                        return state.result(LoopFinishReason::Error(format!(
                            "Model access denied: {}",
                            violation
                        )));
                    }

                    tracing::debug!(
                        agent_id = agent_id,
                        round = state.rounds,
                        model = %response.model,
                        input_tokens = response.usage.input_tokens,
                        output_tokens = response.usage.output_tokens,
                        finish_reason = ?response.finish_reason,
                        "LLM call completed"
                    );

                    if response.usage.cache_read_input_tokens > 0 {
                        tracing::debug!(
                            agent_id = agent_id,
                            round = state.rounds,
                            cache_read_tokens = response.usage.cache_read_input_tokens,
                            cache_creation_tokens = response.usage.cache_creation_input_tokens,
                            "Prompt cache hit"
                        );
                    }

                    if let Some(ref thinking_text) = response.thinking {
                        tracing::debug!(
                            agent_id = agent_id,
                            round = state.rounds,
                            thinking_len = thinking_text.len(),
                            "Extended thinking produced"
                        );
                    }

                    // Capture last content before any branching
                    if !response.content.is_empty() {
                        state.last_assistant_content = response.content.clone();
                    }
                }

                // §5.5: one `round` record per LLM response. It carries the
                // `tool_use` blocks **verbatim** — id, name, full input — so
                // both halves of the assistant(tool_use)/user(tool_result)
                // alternation are reconstructible bit-for-bit, which is what
                // makes replay-resume possible (§5.4). The `context` block is
                // `ContextBudgetManager::section_breakdown()`, so a per-turn
                // `/context` bar needs no new endpoint (P-19).
                if config.session_log.is_some() {
                    let context = context_budget.map(|budget| {
                        let sections: std::collections::HashMap<&str, usize> =
                            budget.section_breakdown().into_iter().collect();
                        json!({
                            "window": budget.model_context_window(),
                            "system_prompt": sections.get("system_prompt").copied().unwrap_or(0),
                            "tools": sections.get("tools").copied().unwrap_or(0),
                            "messages": msg_tokens_estimate,
                            "free": budget.free_zone_capacity(),
                        })
                    });
                    log_event(
                        config,
                        task_id,
                        agent_id,
                        RecordType::Round,
                        json!({
                            "round": state.rounds,
                            "model": response.model,
                            "input_tokens": response.usage.input_tokens,
                            "output_tokens": response.usage.output_tokens,
                            "cache_read_input_tokens": response.usage.cache_read_input_tokens,
                            "stop_reason": format!("{:?}", response.finish_reason),
                            "text": response.content,
                            "tool_use": response
                                .tool_calls
                                .iter()
                                .map(|tc| json!({
                                    "id": tc.id,
                                    "name": tc.name,
                                    "input": tc.arguments,
                                }))
                                .collect::<Vec<_>>(),
                            "context": context,
                        }),
                    );
                }

                // ── 10. Persist or execute tools ──────────────────
                let persist_span = tracing::info_span!(
                    "loop.step.persist_or_tools",
                    agent_id = agent_id,
                    round = state.rounds,
                );
                persist_span.in_scope(|| {
                    tracing::trace!("persist_or_tools step entered");
                });

                // ── Tool execution ─────────────────────────────────
                // IMPORTANT: Thinking blocks are NOT included in conversation history.
                // Only the text content and tool calls are pushed as assistant messages.
                // Anthropic API strips thinking from re-sent messages automatically,
                // but we also omit response.thinking from the ChatMessage to be explicit.
                if response.finish_reason == FinishReason::ToolUse
                    && !response.tool_calls.is_empty()
                {
                    Arc::make_mut(&mut messages)
                        .push(ChatMessage::assistant_with_tools(&response));

                    let calls_this_round =
                        response.tool_calls.len().min(config.max_tools_per_round);

                    // Pre-compute budget before spawning futures
                    let remaining_budget = sandbox_policy
                        .and_then(|p| p.max_tool_calls)
                        .map(|max| (max as usize).saturating_sub(state.tool_calls_made));

                    // Partition: executable vs over-budget
                    let (executable, over_budget): (Vec<_>, Vec<_>) = response
                        .tool_calls
                        .iter()
                        .take(calls_this_round)
                        .enumerate()
                        .partition(|(i, _)| remaining_budget.is_none_or(|budget| *i < budget));
                    let executable: Vec<_> = executable.into_iter().map(|(_, tc)| tc).collect();
                    let over_budget: Vec<_> = over_budget.into_iter().map(|(_, tc)| tc).collect();

                    persist_span.in_scope(|| {
                        if sandbox.is_none() {
                            tracing::warn!(
                                agent_id = agent_id,
                                round = state.rounds,
                                tools = executable.len(),
                                "Sandbox not configured — returning stub for tool calls (misconfiguration?)"
                            );
                        }
                        tracing::debug!(
                            agent_id = agent_id,
                            round = state.rounds,
                            tools = executable.len(),
                            "Executing tools in parallel"
                        );
                    });

                    let mut effective_ctx = tool_context.cloned().unwrap_or_else(|| ToolContext {
                        agent_id: Some(agent_id.to_string()),
                        ..Default::default()
                    });
                    // The session whose log carries this call's records — and
                    // therefore its `tool_execution_log` index row. Set from
                    // the loop's own handle so the two can never disagree:
                    // where this is `Some`, the sandbox leaves the row to the
                    // session writer instead of writing a bare one.
                    effective_ctx.session_id = config
                        .session_log
                        .as_ref()
                        .map(|log| log.session_id().to_string());

                    // §5.5: the call is announced before dispatch, so a
                    // `tool_call` with no matching `tool_result` is exactly
                    // what a crash or a cancellation looks like in the log.
                    for tc in &executable {
                        log_event(
                            config,
                            task_id,
                            agent_id,
                            RecordType::ToolCall,
                            json!({
                                "tool_use_id": tc.id,
                                "name": tc.name,
                                "input": tc.arguments,
                                "ext": tool_extension(sandbox, &tc.name),
                            }),
                        );
                    }
                    let tool_started = Instant::now();

                    let effective_ctx = effective_ctx;
                    // The **untruncated** output. §5.4 makes the JSONL the
                    // source of truth for tool payloads, so the cut to
                    // `MAX_TOOL_RESULT_SIZE` happens once, below, on the copy
                    // that goes into the model's context — never on the copy
                    // the log records. The envelope cap still bounds what
                    // lands inline in the record.
                    let tool_futures = executable.iter().map(|&tc| {
                        let ctx_ref = &effective_ctx;
                        async move {
                            if let (Some(sbx), Some(policy)) = (sandbox, sandbox_policy) {
                                match sbx.execute_tool(tc, policy, ctx_ref).await {
                                    Ok(output) => output,
                                    Err(err) => {
                                        format_tool_error_with_hint(&tc.name, &err.to_string())
                                    }
                                }
                            } else {
                                format_tool_error(&format!(
                                    "tool '{}' not available — sandbox not configured",
                                    tc.name
                                ))
                            }
                        }
                    });

                    // Race tool execution against cancellation; instrument
                    // the join_all future so events/spans inside tool
                    // execution nest under persist_or_tools.
                    let results = if let Some(ref token) = cancel_token {
                        tokio::select! {
                            results = futures_util::future::join_all(tool_futures)
                                .instrument(persist_span.clone()) => results,
                            _ = token.cancelled() => {
                                persist_span.in_scope(|| {
                                    tracing::info!(
                                        agent_id = agent_id,
                                        round = state.rounds,
                                        "Cancelled during parallel tool execution"
                                    );
                                });
                                return state.result(LoopFinishReason::Cancelled);
                            }
                        }
                    } else {
                        futures_util::future::join_all(tool_futures)
                            .instrument(persist_span.clone())
                            .await
                    };

                    // The batch shares one wall-clock: `join_all` runs them
                    // together, so per-call durations would be fiction.
                    let batch_duration_ms = tool_started.elapsed().as_millis() as i64;

                    // Collect results in order (join_all preserves input order)
                    for (tc, result_text) in executable.iter().zip(results.iter()) {
                        state.tool_calls_made += 1;
                        let ok = !result_text.starts_with("[tool_error]");
                        log_event(
                            config,
                            task_id,
                            agent_id,
                            RecordType::ToolResult,
                            json!({
                                "tool_use_id": tc.id,
                                "name": tc.name,
                                "ok": ok,
                                "duration_ms": batch_duration_ms,
                                // On a refusal this is the S4 string the gate
                                // answered with, which is what makes a
                                // withheld capability auditable per session.
                                "error": (!ok).then(|| result_text.clone()),
                                "result": result_text,
                                "ext": tool_extension(sandbox, &tc.name),
                            }),
                        );
                        // The model's copy: head-only at
                        // `MAX_TOOL_RESULT_SIZE` so a long result cannot blow
                        // the context window. The record above kept the whole
                        // thing — that split is §5.4's, and it is what lets
                        // T42 spill the tail instead of destroying it.
                        let model_text = truncate_tool_result(result_text.clone());
                        persist_span.in_scope(|| {
                            tracing::debug!(
                                agent_id = agent_id,
                                round = state.rounds,
                                tool = %tc.name,
                                tool_call_number = state.tool_calls_made,
                                success = !result_text.starts_with("[tool_error]"),
                                result_len = result_text.len(),
                                model_visible_len = model_text.len(),
                                "Tool execution completed"
                            );
                        });
                        Arc::make_mut(&mut messages)
                            .push(ChatMessage::tool_result(&tc.id, &model_text));
                    }

                    // Over-budget tools get error
                    for tc in &over_budget {
                        let err = format_tool_error(
                            "max_tool_calls limit reached — no more tool calls allowed",
                        );
                        Arc::make_mut(&mut messages)
                            .push(ChatMessage::tool_result(&tc.id, &err));
                    }

                    // Overflow tools (exceeding max_tools_per_round) get error
                    for tc in response.tool_calls.iter().skip(calls_this_round) {
                        let err = format_tool_error("max tools per round exceeded");
                        Arc::make_mut(&mut messages)
                            .push(ChatMessage::tool_result(&tc.id, &err));
                    }

                    state.max_tokens_retries = 0;
                    continue;
                }

                // MaxTokens — retry with continuation prompt
                if response.finish_reason == FinishReason::MaxTokens {
                    state.max_tokens_retries += 1;
                    if state.max_tokens_retries <= MAX_TOKENS_RETRIES {
                        Arc::make_mut(&mut messages)
                            .push(ChatMessage::assistant(&response.content));
                        Arc::make_mut(&mut messages).push(ChatMessage::user(
                            "Your previous response was truncated due to length limits. \
                             Continue from where you left off.",
                        ));
                        persist_span.in_scope(|| {
                            tracing::warn!(
                                agent_id,
                                round = state.rounds,
                                retry = state.max_tokens_retries,
                                "MaxTokens hit — injecting continuation prompt"
                            );
                        });
                        continue;
                    }
                    persist_span.in_scope(|| {
                        tracing::warn!(agent_id, "MaxTokens retries exhausted, returning partial");
                    });
                    return state.result_with_content(
                        response.content,
                        LoopFinishReason::Truncated,
                    );
                }

                // ── Steering completion guard (Routing V2) ─────────
                // A message that arrived during the final round would be
                // silently dropped by returning Complete — instead, keep the
                // assistant's answer in history, inject the interjections,
                // and run another round (budget checks still apply above).
                if let Some(ref inbox) = config.steering
                    && !inbox.is_closed()
                {
                    let drained = inbox.drain_all();
                    if !drained.is_empty() {
                        steering_bonus_rounds += STEERING_ROUNDS_BONUS;
                        persist_span.in_scope(|| {
                            tracing::info!(
                                agent_id = agent_id,
                                round = state.rounds,
                                interjections = drained.len(),
                                "Steering completion guard: continuing loop for late interjections"
                            );
                        });
                        log_event(
                            config,
                            task_id,
                            agent_id,
                            RecordType::SteeringDrained,
                            json!({
                                "at": "completion_guard",
                                "round": state.rounds,
                                "count": drained.len(),
                                "request_ids": drained
                                    .iter()
                                    .map(|m| m.request_id.to_string())
                                    .collect::<Vec<_>>(),
                            }),
                        );
                        Arc::make_mut(&mut messages)
                            .push(ChatMessage::assistant(&response.content));
                        for msg in &drained {
                            Arc::make_mut(&mut messages)
                                .push(ChatMessage::user(&msg.to_interjection()));
                        }
                        pending_steering.extend(drained);
                        continue;
                    }
                }

                // No tool calls → done
                persist_span.in_scope(|| {
                    tracing::info!(
                        agent_id = agent_id,
                        rounds = state.rounds,
                        total_input_tokens = state.total_input,
                        total_output_tokens = state.total_output,
                        tool_calls = state.tool_calls_made,
                        content_len = response.content.len(),
                        "Agentic loop completed successfully"
                    );
                });
                return state.result_with_content(response.content, LoopFinishReason::Complete);
            }

            // ── 7. Error handling ──────────────────────────────────
            Err(e) => {
                // Router backend: retry transient errors with exponential backoff.
                // Direct backend: return error immediately (no retry).
                if backend.supports_retry() {
                    let is_transient = matches!(
                        e,
                        LlmRouterError::MaxRetriesExceeded
                            | LlmRouterError::AllKeysRateLimited
                            | LlmRouterError::AllFallbacksFailed
                    ) || matches!(&e, LlmRouterError::Llm(inner) if inner.is_transient());

                    if is_transient
                        && consecutive_llm_errors < MAX_LLM_RETRIES
                        && state.rounds < config.max_rounds
                    {
                        consecutive_llm_errors += 1;
                        state.rounds += 1; // count retry against round budget
                        let backoff_secs = (1u64 << consecutive_llm_errors).min(30);
                        tracing::warn!(
                            agent_id = agent_id,
                            rounds = state.rounds,
                            error = %e,
                            attempt = consecutive_llm_errors,
                            max_attempts = MAX_LLM_RETRIES,
                            backoff_secs = backoff_secs,
                            "Transient LLM error, retrying after backoff"
                        );
                        if let Some(ref token) = cancel_token {
                            tokio::select! {
                                () = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                                () = token.cancelled() => {
                                    tracing::info!(agent_id = agent_id, rounds = state.rounds, "Agentic loop cancelled during retry backoff");
                                    return_pending_steering(config, &mut pending_steering);
                                    return state.result(LoopFinishReason::Cancelled);
                                }
                            }
                        } else {
                            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                        }
                        continue;
                    }
                }

                tracing::warn!(
                    agent_id = agent_id,
                    rounds = state.rounds,
                    error = %e,
                    "Agentic loop exiting: LLM error"
                );
                // The failed call never delivered this round's drained
                // steering messages — return them for follow-up conversion.
                return_pending_steering(config, &mut pending_steering);
                return state.result(LoopFinishReason::Error(e.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests;
