use crate::agent::subagent::AgentConstraints;
use crate::security::capabilities::CapabilityManager;
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use openalpaca_llm::{
    ChatMessage, ChatRequest, FinishReason, LlmProvider, LlmRouter, LlmRouterError, RequestContext,
    RouterRequest, ToolDefinition,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Maximum tool result size before truncation (32 KB).
const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024;

/// Truncate tool result text if it exceeds the byte limit to prevent blowing
/// up the LLM context window. Uses byte-aware truncation at char boundaries.
fn truncate_tool_result(text: String) -> String {
    if text.len() <= MAX_TOOL_RESULT_SIZE {
        return text;
    }
    // Find the nearest char boundary at or before MAX_TOOL_RESULT_SIZE bytes
    let mut end = MAX_TOOL_RESULT_SIZE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[... truncated: showing first {} of {} bytes]",
        &text[..end],
        end,
        text.len()
    )
}

/// Format a tool error message with the standard `[tool_error]` prefix.
/// Centralizes the error format so the LLM always sees a consistent pattern.
fn format_tool_error(msg: &str) -> String {
    format!("[tool_error] {}", msg)
}

/// Estimate tokens for a single content part.
fn estimate_part_tokens(part: &openalpaca_llm::ContentPart) -> u32 {
    match part {
        openalpaca_llm::ContentPart::Text { text } => (text.len() / 4) as u32,
        openalpaca_llm::ContentPart::Image { detail, .. } => {
            match detail.as_deref() {
                Some("low") => 85,
                _ => 1590, // high/default — one Anthropic tile
            }
        }
        openalpaca_llm::ContentPart::Audio { data, .. } => {
            // ~25 tokens/sec; ensure non-empty audio gets at least 25 tokens
            ((data.len() as f64 / 4096.0) * 25.0).ceil().max(25.0) as u32
        }
        openalpaca_llm::ContentPart::Document { extracted_text, .. } => {
            extracted_text.as_ref().map_or(500, |t| (t.len() / 4) as u32)
        }
        openalpaca_llm::ContentPart::FileRef { .. } => 50,
    }
}

/// Estimate tokens in a message list using the 1 token ≈ 4 bytes heuristic.
/// When multimodal parts are present, estimates per-part tokens instead.
/// Consistent with `estimate_request_tokens` in the LLM router.
fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
    let bytes: usize = messages
        .iter()
        .map(|m| {
            let content_tokens = if let Some(ref parts) = m.parts {
                parts.iter().map(|p| estimate_part_tokens(p) as usize).sum()
            } else {
                m.content.len()
            };
            content_tokens
                + m.tool_calls.as_ref().map_or(0, |tcs| {
                    tcs.iter()
                        .map(|tc| tc.name.len() + tc.arguments.to_string().len())
                        .sum()
                })
        })
        .sum();
    (bytes / 4).max(100) as u32
}

/// Compress context by replacing older rounds with a compact summary.
///
/// Preserves:
/// - Message 0 (system prompt)
/// - Message 1 (initial user query)
/// - The last `tail_keep × 3` messages (most recent rounds)
///
/// Everything in between is replaced with a single user message summarizing
/// what happened in those earlier rounds (tool calls made, brief results).
fn compress_context(messages: &mut Vec<ChatMessage>, tail_keep: usize) {
    // Each "round" is roughly: 1 assistant message + N tool results ≈ 3 messages
    let keep_tail = tail_keep * 3;
    if messages.len() <= 2 + keep_tail {
        return; // Nothing to compress
    }

    let compress_end = messages.len() - keep_tail;

    // Build summary from messages[2..compress_end]
    let mut summary_parts = Vec::new();
    for msg in &messages[2..compress_end] {
        // Summarize multimodal parts when present
        if let Some(ref parts) = msg.parts {
            let role_label = match msg.role {
                openalpaca_llm::Role::User => "User",
                openalpaca_llm::Role::Assistant => "Assistant",
                openalpaca_llm::Role::System => "System",
                openalpaca_llm::Role::Tool => "Tool",
            };
            for part in parts {
                match part {
                    openalpaca_llm::ContentPart::Image { .. } => {
                        summary_parts.push(format!("- {role_label}: [sent an image]"));
                    }
                    openalpaca_llm::ContentPart::Audio { .. } => {
                        summary_parts.push(format!("- {role_label}: [sent audio]"));
                    }
                    openalpaca_llm::ContentPart::Document { filename, extracted_text, .. } => {
                        let excerpt = extracted_text
                            .as_ref()
                            .map(|t| truncate_for_summary(t, 200))
                            .unwrap_or_default();
                        summary_parts.push(format!("- {role_label}: [attached: {filename}] {excerpt}"));
                    }
                    openalpaca_llm::ContentPart::FileRef { filename, .. } => {
                        summary_parts.push(format!("- {role_label}: [attached: {filename}]"));
                    }
                    openalpaca_llm::ContentPart::Text { text } if !text.is_empty() => {
                        summary_parts.push(format!(
                            "- {role_label}: {}",
                            truncate_for_summary(text, 200)
                        ));
                    }
                    _ => {}
                }
            }
            continue;
        }

        match msg.role {
            openalpaca_llm::Role::Assistant => {
                if !msg.content.is_empty() {
                    summary_parts.push(format!(
                        "- Agent: {}",
                        truncate_for_summary(&msg.content, 200)
                    ));
                }
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        summary_parts.push(format!("- Called: {}", tc.name));
                    }
                }
            }
            openalpaca_llm::Role::Tool => {
                summary_parts.push(format!(
                    "- Result: {}",
                    truncate_for_summary(&msg.content, 300)
                ));
            }
            _ => {}
        }
    }

    let summary = format!(
        "[Context compressed: {} earlier messages summarized]\n{}",
        compress_end - 2,
        summary_parts.join("\n")
    );

    // Replace messages[2..compress_end] with a single user message
    messages.splice(
        2..compress_end,
        std::iter::once(ChatMessage::user(&summary)),
    );
}

/// Truncate text for inclusion in a compressed summary.
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(max_chars);
        format!("{}...", &text[..end])
    }
}

#[derive(Debug, Clone)]
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
    /// Input token rate ($ per 1M tokens) for cost estimation fallback.
    /// Set from model registry pricing when available.
    pub fallback_input_rate: f64,
    /// Output token rate ($ per 1M tokens) for cost estimation fallback.
    /// Set from model registry pricing when available.
    pub fallback_output_rate: f64,
    /// Maximum estimated input tokens before triggering context compression.
    /// When `> 0` and estimated tokens exceed this, older rounds are compressed
    /// into a summary, preserving the system prompt + initial query + recent rounds.
    /// Default: `0` (disabled — auto-set from model context window × 0.6 via
    /// `with_context_window()`).
    pub max_context_tokens: u32,
    /// Number of most recent conversation rounds to always preserve during
    /// context compression. Each "round" is roughly 3 messages (assistant +
    /// tool results). Default: `4`.
    pub context_tail_keep: usize,
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
            fallback_input_rate: FALLBACK_INPUT_RATE,
            fallback_output_rate: FALLBACK_OUTPUT_RATE,
            max_context_tokens: 0,
            context_tail_keep: 4,
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
            fallback_input_rate: FALLBACK_INPUT_RATE,
            fallback_output_rate: FALLBACK_OUTPUT_RATE,
            max_context_tokens: 0,
            context_tail_keep: 4,
        }
    }

    /// Set cost estimation rates from model registry pricing.
    /// If the model is found in the registry, uses its actual pricing.
    /// Otherwise keeps the default fallback rates (Sonnet-like).
    pub fn with_model_pricing(
        mut self,
        registry: &openalpaca_llm::ModelRegistry,
        model_id: Option<&str>,
    ) -> Self {
        if let Some(model) = model_id
            && let Some(pricing) = registry.get_pricing(model)
        {
            self.fallback_input_rate = pricing.input_price_per_million;
            self.fallback_output_rate = pricing.output_price_per_million;
        }
        self
    }

    /// Set context compression budget from model registry.
    /// Uses 60% of the model's context window as the compression trigger threshold.
    /// Only sets the budget if `max_context_tokens` is still 0 (not explicitly configured).
    pub fn with_context_window(
        mut self,
        registry: &openalpaca_llm::ModelRegistry,
        model_id: Option<&str>,
    ) -> Self {
        if self.max_context_tokens == 0
            && let Some(model) = model_id
            && let Some(info) = registry.get_model_info(model)
            && info.context_window > 0
        {
            self.max_context_tokens = (info.context_window as f64 * 0.6) as u32;
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopFinishReason {
    Complete,
    MaxRounds,
    CostExceeded,
    Cancelled,
    Error(String),
}

/// Run the agentic loop.
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
    cancel_token: Option<CancellationToken>,
) -> LoopResult {
    let start = Instant::now();
    let mut messages = initial_messages;
    let mut rounds = 0usize;
    let mut total_input = 0u32;
    let mut total_output = 0u32;
    let mut tool_calls_made = 0usize;
    let mut last_assistant_content = String::new();
    let mut last_model: Option<String> = None;

    tracing::info!(
        agent_id = agent_id,
        tools_count = tools.len(),
        max_rounds = config.max_rounds,
        max_cost = config.max_cost,
        "Agentic loop started (direct provider)"
    );

    loop {
        // Check cancellation before each round
        if let Some(ref token) = cancel_token
            && token.is_cancelled()
        {
            tracing::info!(
                agent_id = agent_id,
                rounds = rounds,
                "Agentic loop cancelled"
            );
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::Cancelled,
                model_used: last_model.clone(),
                elapsed: start.elapsed(),
            };
        }

        if rounds >= config.max_rounds {
            tracing::info!(
                agent_id = agent_id,
                rounds = rounds,
                "Agentic loop exiting: max rounds reached"
            );
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::MaxRounds,
                model_used: last_model.clone(),
                elapsed: start.elapsed(),
            };
        }

        let estimated_cost = estimate_cost(
            total_input,
            total_output,
            config.fallback_input_rate,
            config.fallback_output_rate,
        );
        if estimated_cost > config.max_cost {
            tracing::info!(
                agent_id = agent_id,
                rounds = rounds,
                "Agentic loop exiting: cost limit exceeded"
            );
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::CostExceeded,
                model_used: last_model.clone(),
                elapsed: start.elapsed(),
            };
        }

        // Context compression: if estimated tokens exceed the budget,
        // compress older rounds into a summary to reduce cost and latency.
        if config.max_context_tokens > 0 {
            let est = estimate_messages_tokens(&messages);
            if est > config.max_context_tokens {
                tracing::info!(
                    agent_id = agent_id,
                    estimated_tokens = est,
                    max_context_tokens = config.max_context_tokens,
                    messages_before = messages.len(),
                    "Compressing context: token budget exceeded"
                );
                compress_context(&mut messages, config.context_tail_keep);
                tracing::info!(
                    agent_id = agent_id,
                    messages_after = messages.len(),
                    estimated_tokens_after = estimate_messages_tokens(&messages),
                    "Context compressed"
                );
            }
        }

        let request = ChatRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            model: None,
            temperature: None,
            max_tokens: None,
        };

        tracing::debug!(
            agent_id = agent_id,
            round = rounds + 1,
            messages_count = messages.len(),
            "LLM call starting"
        );

        // Race LLM call against cancellation token (if present)
        let llm_result = if let Some(ref token) = cancel_token {
            tokio::select! {
                result = provider.chat(request) => result,
                _ = token.cancelled() => {
                    tracing::info!(agent_id = agent_id, round = rounds + 1, "LLM call interrupted by cancellation");
                    return LoopResult {
                        final_content: last_assistant_content,
                        rounds_used: rounds,
                        total_input_tokens: total_input,
                        total_output_tokens: total_output,
                        tool_calls_made,
                        finish_reason: LoopFinishReason::Cancelled,
                        model_used: last_model.clone(),
                        elapsed: start.elapsed(),
                    };
                }
            }
        } else {
            provider.chat(request).await
        };

        match llm_result {
            Ok(response) => {
                total_input += response.usage.input_tokens;
                total_output += response.usage.output_tokens;
                rounds += 1;
                last_model = Some(response.model.clone());

                tracing::debug!(
                    agent_id = agent_id,
                    round = rounds,
                    model = %response.model,
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    finish_reason = ?response.finish_reason,
                    "LLM call completed"
                );

                // Capture last content before any branching
                if !response.content.is_empty() {
                    last_assistant_content = response.content.clone();
                }

                if response.finish_reason == FinishReason::ToolUse
                    && !response.tool_calls.is_empty()
                {
                    // Record assistant message with tool calls
                    messages.push(ChatMessage::assistant_with_tools(&response));

                    // Enforce max_tools_per_round
                    let calls_this_round =
                        response.tool_calls.len().min(config.max_tools_per_round);

                    for tc in response.tool_calls.iter().take(calls_this_round) {
                        // Enforce max_tool_calls from sandbox policy
                        if let Some(policy) = sandbox_policy
                            && let Some(max_calls) = policy.max_tool_calls
                            && tool_calls_made >= max_calls as usize
                        {
                            let err = format_tool_error(
                                "max_tool_calls limit reached — no more tool calls allowed",
                            );
                            messages.push(ChatMessage::tool_result(&tc.id, &err));
                            continue;
                        }

                        tool_calls_made += 1;

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            tool_call_number = tool_calls_made,
                            "Executing tool"
                        );

                        // Tool error convention: errors use format_tool_error()
                        // so the LLM sees a consistent format regardless of the tool.
                        let result_text = if let (Some(sbx), Some(policy)) =
                            (sandbox, sandbox_policy)
                        {
                            // Route through sandbox
                            match sbx.execute_tool(agent_id, tc, policy).await {
                                Ok(output) => truncate_tool_result(output),
                                Err(err) => {
                                    truncate_tool_result(format_tool_error(&err.to_string()))
                                }
                            }
                        } else {
                            tracing::warn!(
                                agent_id = agent_id,
                                tool = tc.name,
                                "Sandbox not configured — returning stub for tool call (misconfiguration?)"
                            );
                            format_tool_error(&format!(
                                "tool '{}' not available — sandbox not configured",
                                tc.name
                            ))
                        };

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            success = !result_text.starts_with("[tool_error]"),
                            result_len = result_text.len(),
                            "Tool execution completed"
                        );

                        messages.push(ChatMessage::tool_result(&tc.id, &result_text));
                    }

                    // If we truncated, add error for remaining tool calls
                    for tc in response.tool_calls.iter().skip(calls_this_round) {
                        let err = format_tool_error("max tools per round exceeded");
                        messages.push(ChatMessage::tool_result(&tc.id, &err));
                    }

                    continue;
                }

                // No tool calls or stop -> done
                tracing::info!(
                    agent_id = agent_id,
                    rounds = rounds,
                    total_input_tokens = total_input,
                    total_output_tokens = total_output,
                    tool_calls = tool_calls_made,
                    content_len = response.content.len(),
                    "Agentic loop completed successfully"
                );
                return LoopResult {
                    final_content: response.content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Complete,
                    model_used: last_model.clone(),
                    elapsed: start.elapsed(),
                };
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = agent_id,
                    rounds = rounds,
                    error = %e,
                    "Agentic loop exiting: LLM error"
                );
                return LoopResult {
                    final_content: last_assistant_content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Error(e.to_string()),
                    model_used: last_model.clone(),
                    elapsed: start.elapsed(),
                };
            }
        }
    }
}

/// Run the agentic loop using the LlmRouter (multi-provider, multi-key).
///
/// Same bounded-loop logic as `run_agentic_loop`, but uses `RouterRequest`
/// and `router.complete()` with key rotation, fallback chains, and cost tracking.
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
    cancel_token: Option<CancellationToken>,
) -> LoopResult {
    let start = Instant::now();
    let mut messages = initial_messages;
    let mut rounds = 0usize;
    let mut total_input = 0u32;
    let mut total_output = 0u32;
    let mut tool_calls_made = 0usize;
    let mut last_assistant_content = String::new();
    let mut last_model: Option<String> = None;
    let mut consecutive_llm_errors: usize = 0;
    const MAX_LLM_RETRIES: usize = 3;

    let context = RequestContext {
        agent_id: Some(agent_id.to_string()),
        task_id: task_id.map(|s| s.to_string()),
    };

    // Wrap tools in Arc once — they don't change between rounds
    let tools_arc = Arc::new(tools);

    tracing::info!(
        agent_id = agent_id,
        tools_count = tools_arc.len(),
        max_rounds = config.max_rounds,
        max_cost = config.max_cost,
        "Agentic loop started"
    );

    loop {
        // Check cancellation before each round
        if let Some(ref token) = cancel_token
            && token.is_cancelled()
        {
            tracing::info!(
                agent_id = agent_id,
                rounds = rounds,
                "Agentic loop cancelled"
            );
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::Cancelled,
                model_used: last_model.clone(),
                elapsed: start.elapsed(),
            };
        }

        if rounds >= config.max_rounds {
            tracing::info!(
                agent_id = agent_id,
                rounds = rounds,
                "Agentic loop exiting: max rounds reached"
            );
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::MaxRounds,
                model_used: last_model.clone(),
                elapsed: start.elapsed(),
            };
        }

        // Check cost using local token tracking (avoids per-round async lock acquisition)
        let estimated_cost = estimate_cost(
            total_input,
            total_output,
            config.fallback_input_rate,
            config.fallback_output_rate,
        );
        if estimated_cost > config.max_cost {
            tracing::info!(
                agent_id = agent_id,
                rounds = rounds,
                estimated_cost,
                "Agentic loop exiting: cost limit exceeded"
            );
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::CostExceeded,
                model_used: last_model.clone(),
                elapsed: start.elapsed(),
            };
        }

        // Context compression: if estimated tokens exceed the budget,
        // compress older rounds into a summary to reduce cost and latency.
        if config.max_context_tokens > 0 {
            let est = estimate_messages_tokens(&messages);
            if est > config.max_context_tokens {
                tracing::info!(
                    agent_id = agent_id,
                    estimated_tokens = est,
                    max_context_tokens = config.max_context_tokens,
                    messages_before = messages.len(),
                    "Compressing context: token budget exceeded"
                );
                compress_context(&mut messages, config.context_tail_keep);
                tracing::info!(
                    agent_id = agent_id,
                    messages_after = messages.len(),
                    estimated_tokens_after = estimate_messages_tokens(&messages),
                    "Context compressed"
                );
            }
        }

        let request = RouterRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            tools: Arc::clone(&tools_arc),
            temperature: None,
            max_tokens: None,
            context: context.clone(),
        };

        tracing::debug!(
            agent_id = agent_id,
            round = rounds + 1,
            messages_count = messages.len(),
            "LLM call starting"
        );

        // Race LLM call against cancellation token (if present).
        // This allows interrupting long-running LLM calls (10-60s) mid-flight.
        let llm_result = if let Some(ref token) = cancel_token {
            tokio::select! {
                result = router.complete(request) => result,
                _ = token.cancelled() => {
                    tracing::info!(agent_id = agent_id, round = rounds + 1, "LLM call interrupted by cancellation");
                    return LoopResult {
                        final_content: last_assistant_content,
                        rounds_used: rounds,
                        total_input_tokens: total_input,
                        total_output_tokens: total_output,
                        tool_calls_made,
                        finish_reason: LoopFinishReason::Cancelled,
                        model_used: last_model.clone(),
                        elapsed: start.elapsed(),
                    };
                }
            }
        } else {
            router.complete(request).await
        };

        match llm_result {
            Ok(response) => {
                consecutive_llm_errors = 0;
                // Enforce model access constraints: if the router resolved to a
                // model the agent is not allowed to use (e.g., via fallback chain),
                // abort with an error rather than processing the response.
                if let Some(ref constraints) = config.agent_constraints
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
                    return LoopResult {
                        final_content: last_assistant_content,
                        rounds_used: rounds,
                        total_input_tokens: total_input,
                        total_output_tokens: total_output,
                        tool_calls_made,
                        finish_reason: LoopFinishReason::Error(format!(
                            "Model access denied: {}",
                            violation
                        )),
                        model_used: Some(response.model),
                        elapsed: start.elapsed(),
                    };
                }

                total_input += response.usage.input_tokens;
                total_output += response.usage.output_tokens;
                rounds += 1;
                last_model = Some(response.model.clone());

                tracing::debug!(
                    agent_id = agent_id,
                    round = rounds,
                    model = %response.model,
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    finish_reason = ?response.finish_reason,
                    "LLM call completed"
                );

                // Capture last content before any branching
                if !response.content.is_empty() {
                    last_assistant_content = response.content.clone();
                }

                if response.finish_reason == FinishReason::ToolUse
                    && !response.tool_calls.is_empty()
                {
                    messages.push(ChatMessage::assistant_with_tools(&response));

                    let calls_this_round =
                        response.tool_calls.len().min(config.max_tools_per_round);

                    for tc in response.tool_calls.iter().take(calls_this_round) {
                        // Enforce max_tool_calls from sandbox policy
                        if let Some(policy) = sandbox_policy
                            && let Some(max_calls) = policy.max_tool_calls
                            && tool_calls_made >= max_calls as usize
                        {
                            let err = format_tool_error(
                                "max_tool_calls limit reached — no more tool calls allowed",
                            );
                            messages.push(ChatMessage::tool_result(&tc.id, &err));
                            continue;
                        }

                        tool_calls_made += 1;

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            tool_call_number = tool_calls_made,
                            "Executing tool"
                        );

                        let result_text = if let (Some(sbx), Some(policy)) =
                            (sandbox, sandbox_policy)
                        {
                            match sbx.execute_tool(agent_id, tc, policy).await {
                                Ok(output) => truncate_tool_result(output),
                                Err(err) => {
                                    truncate_tool_result(format_tool_error(&err.to_string()))
                                }
                            }
                        } else {
                            tracing::warn!(
                                agent_id = agent_id,
                                tool = tc.name,
                                "Sandbox not configured — returning stub for tool call (misconfiguration?)"
                            );
                            format_tool_error(&format!(
                                "tool '{}' not available — sandbox not configured",
                                tc.name
                            ))
                        };

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            success = !result_text.starts_with("[tool_error]"),
                            result_len = result_text.len(),
                            "Tool execution completed"
                        );

                        messages.push(ChatMessage::tool_result(&tc.id, &result_text));
                    }

                    for tc in response.tool_calls.iter().skip(calls_this_round) {
                        let err = format_tool_error("max tools per round exceeded");
                        messages.push(ChatMessage::tool_result(&tc.id, &err));
                    }

                    continue;
                }

                tracing::info!(
                    agent_id = agent_id,
                    rounds = rounds,
                    total_input_tokens = total_input,
                    total_output_tokens = total_output,
                    tool_calls = tool_calls_made,
                    content_len = response.content.len(),
                    "Agentic loop completed successfully"
                );
                return LoopResult {
                    final_content: response.content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Complete,
                    model_used: last_model.clone(),
                    elapsed: start.elapsed(),
                };
            }
            Err(e) => {
                let is_transient = matches!(
                    e,
                    LlmRouterError::MaxRetriesExceeded
                        | LlmRouterError::AllKeysRateLimited
                        | LlmRouterError::AllFallbacksFailed
                ) || matches!(&e, LlmRouterError::Llm(inner) if inner.is_transient());

                if is_transient
                    && consecutive_llm_errors < MAX_LLM_RETRIES
                    && rounds < config.max_rounds
                {
                    consecutive_llm_errors += 1;
                    let backoff_secs = (1u64 << consecutive_llm_errors).min(30);
                    tracing::warn!(
                        agent_id = agent_id,
                        rounds = rounds,
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
                                tracing::info!(agent_id = agent_id, rounds = rounds, "Agentic loop cancelled during retry backoff");
                                return LoopResult {
                                    final_content: last_assistant_content,
                                    rounds_used: rounds,
                                    total_input_tokens: total_input,
                                    total_output_tokens: total_output,
                                    tool_calls_made,
                                    finish_reason: LoopFinishReason::Cancelled,
                                    model_used: last_model.clone(),
                                    elapsed: start.elapsed(),
                                };
                            }
                        }
                    } else {
                        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    }
                    continue;
                }

                tracing::warn!(
                    agent_id = agent_id,
                    rounds = rounds,
                    error = %e,
                    "Agentic loop exiting: LLM error"
                );
                return LoopResult {
                    final_content: last_assistant_content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Error(e.to_string()),
                    model_used: last_model.clone(),
                    elapsed: start.elapsed(),
                };
            }
        }
    }
}

/// Fallback cost rates for the non-routed (test) path.
/// Claude Sonnet pricing as conservative upper bound.
const FALLBACK_INPUT_RATE: f64 = 3.0; // $ per 1M tokens
const FALLBACK_OUTPUT_RATE: f64 = 15.0; // $ per 1M tokens

fn estimate_cost(input_tokens: u32, output_tokens: u32, input_rate: f64, output_rate: f64) -> f64 {
    (input_tokens as f64 * input_rate / 1_000_000.0)
        + (output_tokens as f64 * output_rate / 1_000_000.0)
}

#[cfg(test)]
mod tests;
