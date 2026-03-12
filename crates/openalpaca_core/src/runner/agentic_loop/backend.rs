use super::StreamCallback;
use openalpaca_llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, LlmRouter, LlmRouterError,
    RequestContext, RouterRequest, ThinkingConfig, ToolChoice, ToolDefinition,
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
                    context_management: None,
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
                        context_management: None,
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
                    context_management: None,
                };
                router.complete(request).await
            }
        }
    }

    /// Whether this backend supports transient-error retry with backoff.
    pub(super) fn supports_retry(&self) -> bool {
        matches!(self, LlmBackend::Router { .. })
    }

    /// Get the current accumulated cost for this task/agent.
    /// For the Direct backend (tests), falls back to local token-based estimate.
    /// For the Router backend, reads from the global CostTracker.
    pub(super) async fn task_cost(&self, total_input: u32, total_output: u32) -> f64 {
        match self {
            LlmBackend::Direct { .. } => {
                estimate_cost(total_input, total_output, FALLBACK_INPUT_RATE, FALLBACK_OUTPUT_RATE)
            }
            LlmBackend::Router { router, context } => {
                if let Some(ref task_id) = context.task_id {
                    router.cost_tracker
                        .get_task_usage(task_id).await
                        .map(|u| u.total_cost_usd)
                        .unwrap_or(0.0)
                } else if let Some(ref agent_id) = context.agent_id {
                    router.cost_tracker
                        .get_agent_usage(agent_id).await
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
