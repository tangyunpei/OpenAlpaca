mod backend;
mod config;
mod context;
mod tool_helpers;

// Public API (unchanged from before the split)
pub use config::{LoopConfig, LoopFinishReason, LoopResult, StreamCallback};

// Internal re-exports so the core loop and tests can access submodule items
use backend::LlmBackend;
pub(crate) use context::{compress_context, estimate_messages_tokens};
use tool_helpers::{format_tool_error, format_tool_error_with_hint, truncate_tool_result};
#[cfg(test)]
use tool_helpers::MAX_TOOL_RESULT_SIZE;

use crate::security::capabilities::CapabilityManager;
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::registry::ToolContext;
use openalpaca_llm::{
    ChatMessage, FinishReason, LlmProvider, LlmRouter, LlmRouterError, RequestContext,
    ToolDefinition,
};
#[cfg(test)]
use openalpaca_llm::ChatRequest;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Maximum retries when LLM response is truncated due to max_tokens.
const MAX_TOKENS_RETRIES: usize = 2;

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
    cost_warning_emitted: bool,
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
            cost_warning_emitted: false,
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
) -> LoopResult {
    let context = RequestContext {
        agent_id: Some(agent_id.to_string()),
        task_id: task_id.map(|s| s.to_string()),
    };
    run_agentic_loop_inner(
        LlmBackend::Router { router, context, compaction_model: config.compaction_model.clone() },
        initial_messages,
        tools,
        config,
        sandbox,
        agent_id,
        sandbox_policy,
        context_budget,
        cancel_token,
        tool_context,
    )
    .await
}

/// Core agentic loop implementation shared by both Direct and Router backends.
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
) -> LoopResult {
    let mut state = LoopState::new();
    let mut messages: Arc<Vec<ChatMessage>> = Arc::new(initial_messages);
    let tools_arc = Arc::new(tools);
    let mut known_token_count: u32 = estimate_messages_tokens(&messages);
    let mut consecutive_llm_errors: usize = 0;
    const MAX_LLM_RETRIES: usize = 3;

    // Pre-compute tool token estimate once — avoids re-serializing tool JSON
    // schemas on every Router retry attempt. `None` for Direct backend because
    // it uses `ChatRequest` which doesn't pass through `estimate_request_tokens`.
    let tools_token_estimate: Option<u32> = if backend.supports_retry() {
        let tool_bytes: usize = tools_arc
            .iter()
            .map(|t| {
                let base = t.description.len() + t.parameters.to_string().len();
                let examples = t.input_examples.as_ref().map_or(0, |ex| {
                    ex.iter().map(|e| e.to_string().len()).sum()
                });
                base + examples
            })
            .sum();
        Some((tool_bytes / 4) as u32)
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
        if let Some(ref token) = cancel_token
            && token.is_cancelled()
        {
            tracing::info!(
                agent_id = agent_id,
                rounds = state.rounds,
                "Agentic loop cancelled"
            );
            return state.result(LoopFinishReason::Cancelled);
        }

        // ── 2. Max rounds check ────────────────────────────────────
        if state.rounds >= config.max_rounds {
            tracing::info!(
                agent_id = agent_id,
                rounds = state.rounds,
                "Agentic loop exiting: max rounds reached"
            );
            return state.result(LoopFinishReason::MaxRounds);
        }

        // ── 3. Cost check (CostTracker for Router, local estimate for Direct) ──
        let accumulated_cost = backend.task_cost(state.total_input, state.total_output).await;
        state.last_cost = accumulated_cost;
        let cost_ratio = accumulated_cost / config.max_cost;
        if cost_ratio >= 0.8 && !state.cost_warning_emitted {
            state.cost_warning_emitted = true;
            let warning = format!(
                "[system] Budget {:.0}% consumed (${:.4}/{:.2}). \
                 Prioritize completing the current task efficiently.",
                cost_ratio * 100.0,
                accumulated_cost,
                config.max_cost,
            );
            Arc::make_mut(&mut messages).push(ChatMessage::user(&warning));
            tracing::info!(
                agent_id,
                cost_ratio,
                accumulated_cost,
                "Cost warning emitted at {:.0}%",
                cost_ratio * 100.0,
            );
        }
        if accumulated_cost > config.max_cost {
            tracing::info!(
                agent_id = agent_id,
                rounds = state.rounds,
                accumulated_cost,
                "Agentic loop exiting: cost limit exceeded"
            );
            return state.result(LoopFinishReason::CostExceeded);
        }

        // ── 4. Context compression (budget-aware) ──────────────────
        if let Some(budget) = context_budget {
            let msg_tokens = estimate_messages_tokens(&messages) as usize;
            if budget.should_compact(msg_tokens) {
                let messages_before = messages.len();

                // Try LLM-based compaction if compaction model is available
                let can_llm_compact = matches!(&backend, LlmBackend::Router { compaction_model: Some(_), .. });

                if can_llm_compact {
                    tracing::info!(
                        agent_id = agent_id,
                        msg_tokens,
                        trigger = budget.compaction_trigger(),
                        messages_before,
                        "LLM compaction triggered"
                    );

                    // Extract messages from Arc for CompactionPipeline (takes Vec by value)
                    let owned = Arc::try_unwrap(messages)
                        .unwrap_or_else(|arc| (*arc).clone());

                    let result = crate::context_budget::compaction::CompactionPipeline::compact(
                        owned,
                        budget.min_recent_messages(),
                        &backend,
                        &backend,
                    )
                    .await;

                    // Log extracted memories (telemetry only — no DB storage)
                    for mem in &result.extracted_memories {
                        tracing::info!(
                            kind = %mem.kind,
                            preview = %crate::runner::agentic_loop::context::truncate_for_summary(&mem.content, 100),
                            "Compaction: extracted memory"
                        );
                    }

                    tracing::info!(
                        agent_id = agent_id,
                        messages_before,
                        messages_after = result.compacted_messages.len(),
                        memories_extracted = result.extracted_memories.len(),
                        messages_discarded = result.messages_discarded,
                        error = ?result.error,
                        "LLM compaction completed"
                    );

                    messages = Arc::new(result.compacted_messages);
                } else {
                    // Heuristic fallback
                    tracing::info!(
                        agent_id = agent_id,
                        msg_tokens,
                        messages_before,
                        "Heuristic compaction triggered (no compaction model)"
                    );
                    compress_context(Arc::make_mut(&mut messages), config.context_tail_keep, Some(budget));
                    tracing::info!(
                        agent_id = agent_id,
                        messages_after = messages.len(),
                        "Heuristic compaction completed"
                    );
                }

                known_token_count = estimate_messages_tokens(&messages);
            }
        } else if config.max_context_tokens > 0 && known_token_count > config.max_context_tokens {
            // Legacy fallback (no budget manager)
            tracing::debug!(
                agent_id = agent_id,
                tokens = known_token_count,
                max = config.max_context_tokens,
                "Legacy compression triggered"
            );
            compress_context(Arc::make_mut(&mut messages), config.context_tail_keep, None);
            known_token_count = estimate_messages_tokens(&messages);
        }

        let prev_msg_len = messages.len();

        // ── 5. LLM call ───────────────────────────────────────────
        let tool_choice = if state.rounds == 0 {
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
                ) => result,
                _ = token.cancelled() => {
                    tracing::info!(agent_id = agent_id, round = state.rounds + 1, "LLM call interrupted by cancellation");
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
                )
                .await
        };

        // ── 6. Handle response ─────────────────────────────────────
        match llm_result {
            Ok(response) => {
                consecutive_llm_errors = 0;

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
                    return state.result(LoopFinishReason::Error(format!(
                        "Model access denied: {}",
                        violation
                    )));
                }

                // Use actual input tokens as ground truth for our estimate
                if response.usage.input_tokens > 0 {
                    known_token_count = response.usage.input_tokens;
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

                    let effective_ctx = tool_context.cloned().unwrap_or_else(|| ToolContext {
                        agent_id: Some(agent_id.to_string()),
                        ..Default::default()
                    });
                    let tool_futures = executable.iter().map(|&tc| {
                        let ctx_ref = &effective_ctx;
                        async move {
                            if let (Some(sbx), Some(policy)) = (sandbox, sandbox_policy) {
                                match sbx.execute_tool(tc, policy, ctx_ref).await {
                                    Ok(output) => truncate_tool_result(output),
                                    Err(err) => {
                                        truncate_tool_result(format_tool_error_with_hint(&tc.name, &err.to_string()))
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

                    // Race tool execution against cancellation
                    let results = if let Some(ref token) = cancel_token {
                        tokio::select! {
                            results = futures_util::future::join_all(tool_futures) => results,
                            _ = token.cancelled() => {
                                tracing::info!(
                                    agent_id = agent_id,
                                    round = state.rounds,
                                    "Cancelled during parallel tool execution"
                                );
                                return state.result(LoopFinishReason::Cancelled);
                            }
                        }
                    } else {
                        futures_util::future::join_all(tool_futures).await
                    };

                    // Collect results in order (join_all preserves input order)
                    for (tc, result_text) in executable.iter().zip(results.iter()) {
                        state.tool_calls_made += 1;
                        tracing::debug!(
                            agent_id = agent_id,
                            round = state.rounds,
                            tool = %tc.name,
                            tool_call_number = state.tool_calls_made,
                            success = !result_text.starts_with("[tool_error]"),
                            result_len = result_text.len(),
                            "Tool execution completed"
                        );
                        Arc::make_mut(&mut messages)
                            .push(ChatMessage::tool_result(&tc.id, result_text));
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

                    // Update token estimate incrementally for newly-appended messages
                    if messages.len() > prev_msg_len {
                        known_token_count +=
                            estimate_messages_tokens(&messages[prev_msg_len..]);
                    }

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
                        tracing::warn!(
                            agent_id,
                            round = state.rounds,
                            retry = state.max_tokens_retries,
                            "MaxTokens hit — injecting continuation prompt"
                        );
                        continue;
                    }
                    tracing::warn!(agent_id, "MaxTokens retries exhausted, returning partial");
                    return state.result_with_content(
                        response.content,
                        LoopFinishReason::Truncated,
                    );
                }

                // No tool calls → done
                tracing::info!(
                    agent_id = agent_id,
                    rounds = state.rounds,
                    total_input_tokens = state.total_input,
                    total_output_tokens = state.total_output,
                    tool_calls = state.tool_calls_made,
                    content_len = response.content.len(),
                    "Agentic loop completed successfully"
                );
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
                return state.result(LoopFinishReason::Error(e.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests;
