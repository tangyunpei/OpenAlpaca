use super::StreamCallback;
use crate::orchestrator::working_state::{self, TranscriptLine, WorkingState, WriterConfig};
use openalpaca_llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, LlmRouter, LlmRouterError,
    RequestContext, RouterRequest, ThinkingConfig, ToolChoice, ToolDefinition,
    context_management::ContextManagement,
};
use std::sync::Arc;
use std::time::Duration;

/// Abstraction over direct-provider vs router-based LLM completion.
/// Keeps the unified loop generic without dynamic dispatch overhead.
pub(super) enum LlmBackend<'a> {
    /// Direct single-provider call (legacy/test path).
    Direct { provider: &'a dyn LlmProvider },
    /// Router-based call with key rotation, fallback, cost tracking.
    Router {
        router: &'a LlmRouter,
        context: RequestContext,
        /// What compaction's working-state writer is called with (design 11
        /// §3.1): `summary_model`, its daily ceiling, the line cut.
        writer: WriterConfig,
        /// Per-agent fallback model chain (forwarded to `RouterRequest`).
        fallback_models: Vec<String>,
    },
}

impl<'a> LlmBackend<'a> {
    /// Complete a request via the appropriate backend.
    ///
    /// When `stream_callback` is `Some` and the backend is Router, attempts
    /// streaming first. On streaming failure, falls back to non-streaming.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn complete(
        &self,
        messages: Arc<Vec<ChatMessage>>,
        tools: Arc<Vec<ToolDefinition>>,
        model: Option<String>,
        tool_choice: Option<ToolChoice>,
        tools_token_estimate: Option<u32>,
        enable_caching: bool,
        thinking: Option<ThinkingConfig>,
        stream_callback: Option<&StreamCallback>,
        max_stream_duration: Duration,
        context_management: Option<ContextManagement>,
        ephemeral_system_notice: Option<String>,
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
                    context_management,
                    ephemeral_system_notice,
                };
                provider.chat(request).await.map_err(LlmRouterError::Llm)
            }
            LlmBackend::Router { router, context, fallback_models, .. } => {
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
                        context_management: context_management.clone(),
                        fallback_models: fallback_models.clone(),
                        ephemeral_system_notice: ephemeral_system_notice.clone(),
                    };
                    match router.complete_streaming(stream_request).await {
                        Ok(routed) => {
                            // Forward events to callback while collecting
                            use futures_util::StreamExt;
                            let callback = Arc::clone(callback);
                            let forwarding_stream = routed.stream.map(move |event| {
                                if let Ok(ref e) = event {
                                    callback(e);
                                }
                                event
                            });
                            // V4: the model the router actually called, not the
                            // one this call asked for. A stream carries no
                            // `model` of its own, so this used to be
                            // `request.model` or the daemon default — which
                            // labelled every locally-answered turn with the
                            // Anthropic default and priced it against that
                            // model's rates two statements below.
                            let model_str = routed.model;
                            match tokio::time::timeout(
                                max_stream_duration,
                                openalpaca_llm::collect_stream(
                                    Box::pin(forwarding_stream),
                                    model_str,
                                ),
                            )
                            .await
                            {
                                Ok(Ok(response)) => {
                                    // Record streaming cost in CostTracker (H1 fix)
                                    router.cost_tracker.record_usage(
                                        context.agent_id.as_deref().unwrap_or("unknown"),
                                        context.task_id.as_deref(),
                                        &response.model,
                                        &response.usage,
                                    ).await;
                                    return Ok(response);
                                }
                                Ok(Err(e)) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Streaming collection failed, falling back to non-streaming"
                                    );
                                }
                                Err(_elapsed) => {
                                    tracing::warn!(
                                        max_stream_duration_secs = max_stream_duration.as_secs(),
                                        "Stream wall-clock deadline exceeded, falling back to non-streaming"
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
                    context_management,
                    fallback_models: fallback_models.clone(),
                    ephemeral_system_notice,
                };
                router.complete(request).await
            }
        }
    }

    /// Whether this backend supports transient-error retry with backoff.
    pub(super) fn supports_retry(&self) -> bool {
        matches!(self, LlmBackend::Router { .. })
    }

    /// Accumulated cost attributable to THIS agent (not the whole task).
    ///
    /// The agentic loop's budget (`config.max_cost`) is per-agent, so it must be
    /// charged only this agent's spend. Using the task-wide total makes every
    /// agent after the first inherit all prior agents' spend and exit
    /// `CostExceeded` before its first LLM call. Prefer the agent-scoped total;
    /// fall back to task-scoped, then a local estimate (Direct/test backend).
    pub(super) async fn agent_cost(&self, total_input: u32, total_output: u32) -> f64 {
        match self {
            LlmBackend::Direct { .. } => {
                estimate_cost(total_input, total_output, FALLBACK_INPUT_RATE, FALLBACK_OUTPUT_RATE)
            }
            LlmBackend::Router { router, context, .. } => {
                if let Some(ref agent_id) = context.agent_id {
                    router.cost_tracker
                        .get_agent_usage(agent_id).await
                        .map(|u| u.total_cost_usd)
                        .unwrap_or(0.0)
                } else if let Some(ref task_id) = context.task_id {
                    router.cost_tracker
                        .get_task_usage(task_id).await
                        .map(|u| u.total_cost_usd)
                        .unwrap_or(0.0)
                } else {
                    0.0
                }
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

// ── Summarizer impl ──────────────────────────────────────────────────

/// Compaction's one call (design 11 §3.3, CQ-1, CQ-3): the working-state
/// writer over the older messages, on `summary_model`, under the summariser's
/// label and its daily ceiling — read before the call, as at every site of
/// the writer (§3.1). A spent ceiling, a failed call and an answer that is not
/// a state are all an `Err`: the pipeline falls back to its heuristic.
#[async_trait::async_trait]
impl<'a> crate::context_budget::compaction::Summarizer for LlmBackend<'a> {
    async fn summarize(
        &self,
        previous: Option<&WorkingState>,
        messages: &[ChatMessage],
    ) -> Result<WorkingState, String> {
        let (router, writer) = match self {
            Self::Router { router, writer, .. } => (*router, writer),
            Self::Direct { .. } => {
                return Err("No router available for LLM summarization".into());
            }
        };

        let spent = router
            .cost_tracker
            .get_agent_usage(working_state::USAGE_LABEL)
            .await
            .map(|s| s.total_cost_usd)
            .unwrap_or(0.0);
        if spent > writer.max_daily_cost_usd {
            return Err(format!(
                "the summariser's daily ceiling is spent (${spent:.2})"
            ));
        }

        let lines: Vec<TranscriptLine<'_>> = messages
            .iter()
            .map(|m| TranscriptLine {
                role: match m.role {
                    openalpaca_llm::Role::User => "user",
                    openalpaca_llm::Role::Assistant => "assistant",
                    openalpaca_llm::Role::System => "system",
                    openalpaca_llm::Role::Tool => "tool",
                },
                text: &m.content,
            })
            .collect();
        let request = working_state::update(
            previous,
            &lines,
            writer.model.clone(),
            writer.msg_trunc_chars,
        );
        let response = router.complete(request).await.map_err(|e| e.to_string())?;
        WorkingState::parse(&response).map_err(|why| why.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_budget::compaction::Summarizer;
    use crate::test_util::{RecordingProvider, router_recording};

    fn state() -> WorkingState {
        WorkingState {
            v: working_state::VERSION,
            topic: "Store layout".into(),
            goal: "Know where files go".into(),
            done: Vec::new(),
            decisions: Vec::new(),
            open: Vec::new(),
            next: String::new(),
            refs: Vec::new(),
        }
    }

    /// **M2**, CQ-3: compaction's one call is the working-state writer — no
    /// reasoning, 1 536 output tokens, on `summary_model`, under the
    /// summariser's label — and it reads the previous state and the older
    /// messages as `[role]:` lines.
    #[tokio::test]
    async fn compaction_summary_asks_for_no_reasoning() {
        let provider = RecordingProvider::new(serde_json::to_string(&state()).unwrap());
        let router = router_recording(provider.clone());
        let backend = LlmBackend::Router {
            router: &router,
            context: RequestContext::default(),
            writer: WriterConfig {
                model: Some("claude-haiku-4-5-20251001".into()),
                ..WriterConfig::default()
            },
            fallback_models: Vec::new(),
        };
        let previous = WorkingState {
            next: "Ask about uploads".into(),
            ..state()
        };

        let written = backend
            .summarize(Some(&previous), &[ChatMessage::user("where do files go?")])
            .await
            .expect("summary");
        assert_eq!(written, state());

        let request = provider.first_request();
        assert!(
            matches!(request.thinking, Some(ThinkingConfig::Disabled)),
            "the utility call must ask for no reasoning, got {:?}",
            request.thinking
        );
        assert_eq!(request.max_tokens, Some(working_state::MAX_TOKENS));
        assert_eq!(request.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        let user = &request.messages[1].content;
        assert!(
            user.contains(&serde_json::to_string(&previous).unwrap()),
            "{user}"
        );
        assert!(user.contains("[user]: where do files go?"), "{user}");
        assert!(
            router
                .cost_tracker
                .get_agent_usage(working_state::USAGE_LABEL)
                .await
                .is_some(),
            "the call is made and counted under the summariser's label"
        );
    }

    /// The daily ceiling is read before the call: once the summariser has
    /// spent it, compaction makes no call and the pipeline falls back.
    #[tokio::test]
    async fn compaction_makes_no_call_over_the_daily_ceiling() {
        let provider = RecordingProvider::new(serde_json::to_string(&state()).unwrap());
        let router = router_recording(provider.clone());
        let backend = LlmBackend::Router {
            router: &router,
            context: RequestContext::default(),
            writer: WriterConfig {
                max_daily_cost_usd: 0.0,
                ..WriterConfig::default()
            },
            fallback_models: Vec::new(),
        };
        let older = [ChatMessage::user("where do files go?")];
        backend
            .summarize(None, &older)
            .await
            .expect("nothing spent yet");
        assert_eq!(provider.request_count(), 1);

        let why = backend.summarize(None, &older).await.unwrap_err();
        assert!(why.contains("daily ceiling"), "{why}");
        assert_eq!(provider.request_count(), 1, "over the ceiling: no call");
    }
}
