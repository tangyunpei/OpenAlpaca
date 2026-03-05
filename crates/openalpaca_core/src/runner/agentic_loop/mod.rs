use crate::agent::subagent::AgentConstraints;
use crate::security::capabilities::CapabilityManager;
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use openalpaca_llm::{
    ChatMessage, ChatRequest, ChatResponse, FinishReason, LlmProvider, LlmRouter, LlmRouterError,
    RequestContext, RouterRequest, StreamEvent, ThinkingConfig, ToolChoice, ToolDefinition,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Maximum tool result size before truncation (32 KB).
const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024;

/// Maximum retries when LLM response is truncated due to max_tokens.
const MAX_TOKENS_RETRIES: usize = 2;

/// Default context compression threshold (fraction of max_context_tokens).
const DEFAULT_CONTEXT_THRESHOLD: f64 = 0.6;

/// Truncate tool result text if it exceeds the byte limit to prevent blowing
/// up the LLM context window. Uses byte-aware truncation at char boundaries.
fn truncate_tool_result(text: String) -> String {
    if text.len() <= MAX_TOOL_RESULT_SIZE {
        return text;
    }

    // Find the nearest char boundary at or before the byte limit
    let mut end = MAX_TOOL_RESULT_SIZE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    let slice = &text[..end];

    // Try sentence boundary (last ". " or ".\n" or "! " or "!\n" or "? " or "?\n")
    let sentence_end = slice.rfind(". ")
        .or_else(|| slice.rfind(".\n"))
        .or_else(|| slice.rfind("! "))
        .or_else(|| slice.rfind("!\n"))
        .or_else(|| slice.rfind("? "))
        .or_else(|| slice.rfind("?\n"))
        .map(|pos| pos + 1); // Include the punctuation char

    // Try line boundary
    let line_end = slice.rfind('\n');

    // Try word boundary
    let word_end = slice.rfind(' ');

    // Don't cut more than 25% short — avoid distant sentence boundaries
    // discarding most of the content
    let min_cut = end * 3 / 4;

    // Pick best boundary: sentence (if recent enough) > line > word > char
    let cut = sentence_end
        .filter(|&p| p >= min_cut)
        .or_else(|| line_end.filter(|&p| p >= min_cut))
        .or_else(|| word_end.filter(|&p| p >= min_cut))
        .unwrap_or(end);

    format!(
        "{}\n\n[... truncated: showing first {} of {} bytes]",
        &text[..cut],
        cut,
        text.len()
    )
}

/// Format a tool error message with the standard `[tool_error]` prefix.
/// Centralizes the error format so the LLM always sees a consistent pattern.
fn format_tool_error(msg: &str) -> String {
    format!("[tool_error] {}", msg)
}

/// Tool-specific recovery suggestions for common error patterns.
fn tool_recovery_hint(tool_name: &str, error: &str) -> Option<&'static str> {
    if tool_name == "file_read" && (error.contains("not found") || error.contains("No such file")) {
        return Some("Hint: verify the path exists using shell_execute with `ls`.");
    }
    if tool_name == "file_write" && error.contains("Permission denied") {
        return Some("Hint: check file permissions or try a different output path.");
    }
    if tool_name == "web_fetch" && (error.contains("404") || error.contains("not found")) {
        return Some("Hint: use web_search to find the correct URL first.");
    }
    if tool_name == "web_fetch" && error.contains("timeout") {
        return Some("Hint: the URL may be unreachable. Try a different source.");
    }
    if tool_name == "shell_execute" && error.contains("timed out") {
        return Some("Hint: break the command into smaller steps or increase timeout.");
    }
    if tool_name == "shell_execute" && error.contains("not found") {
        return Some("Hint: check if the command is installed or use the full path.");
    }
    if tool_name == "memory_search" && error.contains("no results") {
        return Some("Hint: try broader search terms or check workspace_read for shared context.");
    }
    None
}

/// Format a tool error with an optional recovery hint appended.
fn format_tool_error_with_hint(tool_name: &str, msg: &str) -> String {
    let base = format_tool_error(msg);
    match tool_recovery_hint(tool_name, msg) {
        Some(hint) => format!("{}\n{}", base, hint),
        None => base,
    }
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
        openalpaca_llm::ContentPart::Document { extracted_text, .. } => extracted_text
            .as_ref()
            .map_or(500, |t| (t.len() / 4) as u32),
        openalpaca_llm::ContentPart::FileRef { .. } => 50,
    }
}

/// Estimate tokens in a message list using the 1 token ≈ 4 bytes heuristic.
/// When multimodal parts are present, estimates per-part tokens instead.
/// Consistent with `estimate_request_tokens` in the LLM router.
fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
    let tokens: u32 = messages
        .iter()
        .map(|m| {
            let content_tokens: u32 = if let Some(ref parts) = m.parts {
                parts.iter().map(estimate_part_tokens).sum()
            } else {
                (m.content.len() / 4) as u32
            };
            let tool_call_tokens: u32 = m.tool_calls.as_ref().map_or(0, |tcs| {
                tcs.iter()
                    .map(|tc| ((tc.name.len() + tc.arguments.to_string().len()) / 4) as u32)
                    .sum()
            });
            content_tokens + tool_call_tokens
        })
        .sum();
    tokens.max(100)
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
                    openalpaca_llm::ContentPart::Document {
                        filename,
                        extracted_text,
                        ..
                    } => {
                        let excerpt = extracted_text
                            .as_ref()
                            .map(|t| truncate_for_summary(t, 200))
                            .unwrap_or_default();
                        summary_parts
                            .push(format!("- {role_label}: [attached: {filename}] {excerpt}"));
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
    /// Input token rate ($ per 1M tokens) for cost estimation fallback.
    /// Set from model registry pricing when available.
    pub fallback_input_rate: f64,
    /// Output token rate ($ per 1M tokens) for cost estimation fallback.
    /// Set from model registry pricing when available.
    pub fallback_output_rate: f64,
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
            fallback_input_rate: self.fallback_input_rate,
            fallback_output_rate: self.fallback_output_rate,
            max_context_tokens: self.max_context_tokens,
            context_tail_keep: self.context_tail_keep,
            context_threshold: self.context_threshold,
            initial_tool_choice: self.initial_tool_choice.clone(),
            enable_caching: self.enable_caching,
            thinking: self.thinking.clone(),
            stream_callback: self.stream_callback.clone(),
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
            fallback_input_rate: FALLBACK_INPUT_RATE,
            fallback_output_rate: FALLBACK_OUTPUT_RATE,
            max_context_tokens: 0,
            context_tail_keep: 4,
            context_threshold: DEFAULT_CONTEXT_THRESHOLD,
            initial_tool_choice: None,
            enable_caching: true,
            thinking: None,
            stream_callback: None,
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
            context_threshold: DEFAULT_CONTEXT_THRESHOLD,
            initial_tool_choice: None,
            enable_caching: true,
            thinking: None,
            stream_callback: None,
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
    /// Uses `context_threshold` fraction of the model's context window as the compression trigger.
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
        }
    }
}

/// Abstraction over direct-provider vs router-based LLM completion.
/// Keeps the unified loop generic without dynamic dispatch overhead.
enum LlmBackend<'a> {
    /// Direct single-provider call (legacy/test path).
    Direct { provider: &'a dyn LlmProvider },
    /// Router-based call with key rotation, fallback, cost tracking.
    Router {
        router: &'a LlmRouter,
        context: RequestContext,
    },
}

impl<'a> LlmBackend<'a> {
    /// Complete a request via the appropriate backend.
    ///
    /// When `stream_callback` is `Some` and the backend is Router, attempts
    /// streaming first. On streaming failure, falls back to non-streaming.
    #[allow(clippy::too_many_arguments)]
    async fn complete(
        &self,
        messages: Arc<Vec<ChatMessage>>,
        tools: Arc<Vec<ToolDefinition>>,
        model: Option<String>,
        tool_choice: Option<ToolChoice>,
        tools_token_estimate: Option<u32>,
        enable_caching: bool,
        thinking: Option<ThinkingConfig>,
        stream_callback: Option<&StreamCallback>,
    ) -> Result<ChatResponse, LlmRouterError> {
        match self {
            LlmBackend::Direct { provider } => {
                let request = ChatRequest {
                    messages,
                    tools,
                    model: None,
                    temperature: None,
                    max_tokens: None,
                    tool_choice,
                    enable_caching,
                    thinking,
                };
                provider.chat(request).await.map_err(LlmRouterError::Llm)
            }
            LlmBackend::Router { router, context } => {
                // Try streaming if callback is set
                if let Some(callback) = stream_callback {
                    let stream_request = RouterRequest {
                        model: model.clone(),
                        messages: Arc::clone(&messages),
                        tools: Arc::clone(&tools),
                        temperature: None,
                        max_tokens: None,
                        context: context.clone(),
                        tool_choice: tool_choice.clone(),
                        tools_token_estimate,
                        enable_caching,
                        thinking: thinking.clone(),
                    };
                    match router.complete_streaming(stream_request).await {
                        Ok(stream) => {
                            // Forward events to callback while collecting
                            use futures_util::StreamExt;
                            let callback = Arc::clone(callback);
                            let forwarding_stream = stream.map(move |event| {
                                if let Ok(ref e) = event {
                                    callback(e);
                                }
                                event
                            });
                            let model_str = model.clone().unwrap_or_else(|| router.default_model());
                            match openalpaca_llm::collect_stream(
                                Box::pin(forwarding_stream),
                                model_str,
                            )
                            .await
                            {
                                Ok(response) => return Ok(response),
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Streaming collection failed, falling back to non-streaming"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Streaming request failed, falling back to non-streaming"
                            );
                        }
                    }
                }

                // Non-streaming path (or streaming fallback)
                let request = RouterRequest {
                    model,
                    messages,
                    tools,
                    temperature: None,
                    max_tokens: None,
                    context: context.clone(),
                    tool_choice,
                    tools_token_estimate,
                    enable_caching,
                    thinking,
                };
                router.complete(request).await
            }
        }
    }

    /// Whether this backend supports transient-error retry with backoff.
    fn supports_retry(&self) -> bool {
        matches!(self, LlmBackend::Router { .. })
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
    cancel_token: Option<CancellationToken>,
) -> LoopResult {
    run_agentic_loop_inner(
        LlmBackend::Direct { provider },
        initial_messages,
        tools,
        config,
        sandbox,
        agent_id,
        sandbox_policy,
        cancel_token,
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
    cancel_token: Option<CancellationToken>,
) -> LoopResult {
    let context = RequestContext {
        agent_id: Some(agent_id.to_string()),
        task_id: task_id.map(|s| s.to_string()),
    };
    run_agentic_loop_inner(
        LlmBackend::Router { router, context },
        initial_messages,
        tools,
        config,
        sandbox,
        agent_id,
        sandbox_policy,
        cancel_token,
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
    cancel_token: Option<CancellationToken>,
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

    tracing::info!(
        agent_id = agent_id,
        tools_count = tools_arc.len(),
        max_rounds = config.max_rounds,
        max_cost = config.max_cost,
        backend = if backend.supports_retry() { "router" } else { "direct" },
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

        // ── 3. Cost check ──────────────────────────────────────────
        // Uses local token accumulation + model registry pricing.
        // Equivalent to router.cost_tracker but avoids per-round async
        // lock and is correctly scoped to this single invocation.
        let estimated_cost = estimate_cost(
            state.total_input,
            state.total_output,
            config.fallback_input_rate,
            config.fallback_output_rate,
        );
        let cost_ratio = estimated_cost / config.max_cost;
        if cost_ratio >= 0.8 && !state.cost_warning_emitted {
            state.cost_warning_emitted = true;
            let warning = format!(
                "[system] Budget {:.0}% consumed (${:.4}/{:.2}). \
                 Prioritize completing the current task efficiently.",
                cost_ratio * 100.0,
                estimated_cost,
                config.max_cost,
            );
            Arc::make_mut(&mut messages).push(ChatMessage::user(&warning));
            tracing::info!(
                agent_id,
                cost_ratio,
                estimated_cost,
                "Cost warning emitted at {:.0}%",
                cost_ratio * 100.0,
            );
        }
        if estimated_cost > config.max_cost {
            tracing::info!(
                agent_id = agent_id,
                rounds = state.rounds,
                estimated_cost,
                "Agentic loop exiting: cost limit exceeded"
            );
            return state.result(LoopFinishReason::CostExceeded);
        }

        // ── 4. Context compression ─────────────────────────────────
        if config.max_context_tokens > 0 && known_token_count > config.max_context_tokens {
            tracing::info!(
                agent_id = agent_id,
                estimated_tokens = known_token_count,
                max_context_tokens = config.max_context_tokens,
                messages_before = messages.len(),
                "Compressing context: token budget exceeded"
            );
            compress_context(Arc::make_mut(&mut messages), config.context_tail_keep);
            known_token_count = estimate_messages_tokens(&messages);
            tracing::info!(
                agent_id = agent_id,
                messages_after = messages.len(),
                estimated_tokens_after = known_token_count,
                "Context compressed"
            );
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

                    let tool_futures = executable.iter().map(|&tc| async move {
                        if let (Some(sbx), Some(policy)) = (sandbox, sandbox_policy) {
                            match sbx.execute_tool(agent_id, tc, policy).await {
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
