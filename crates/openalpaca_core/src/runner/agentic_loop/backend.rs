use super::StreamCallback;
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
        /// Dedicated model for LLM-based context compaction (extraction + summarization).
        compaction_model: Option<String>,
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

// ── MemoryExtractor impl ─────────────────────────────────────────────

#[async_trait::async_trait]
impl<'a> crate::context_budget::compaction::MemoryExtractor for LlmBackend<'a> {
    async fn extract(
        &self,
        messages: &[ChatMessage],
    ) -> Result<Vec<crate::context_budget::compaction::ExtractedMemory>, String> {
        let (router, compaction_model, req_context) = match self {
            Self::Router {
                router,
                compaction_model,
                context,
                ..
            } => (*router, compaction_model.clone(), context.clone()),
            Self::Direct { .. } => {
                return Err("No router available for LLM extraction".into());
            }
        };

        let mut extract_messages = Vec::with_capacity(messages.len() + 1);
        extract_messages.push(ChatMessage::system(
            "You are a memory extraction assistant. Extract key facts, decisions, and important \
             information from the following conversation messages. Return each memory as a line \
             in the format:\n\
             KIND: content\n\n\
             Valid KINDs: fact, decision, preference, instruction, context\n\n\
             Only extract genuinely important information. Be concise. Maximum 10 entries.",
        ));
        let combined: String = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    openalpaca_llm::Role::User => "User",
                    openalpaca_llm::Role::Assistant => "Assistant",
                    openalpaca_llm::Role::System => "System",
                    openalpaca_llm::Role::Tool => "Tool",
                };
                format!("[{role}]: {}", m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        extract_messages.push(ChatMessage::user(&combined));

        let request = RouterRequest {
            messages: Arc::new(extract_messages),
            tools: Arc::new(vec![]),
            model: compaction_model,
            temperature: Some(0.0),
            max_tokens: Some(1024),
            context: req_context,
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
            context_management: None,
            fallback_models: Vec::new(),
            ephemeral_system_notice: None,
        };

        let response = router.complete(request).await.map_err(|e| e.to_string())?;

        let memories = response
            .content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                if let Some((kind, content)) = line.split_once(':') {
                    let kind = kind.trim().to_lowercase();
                    let content = content.trim().to_string();
                    if !content.is_empty() {
                        return Some(crate::context_budget::compaction::ExtractedMemory {
                            kind,
                            content,
                        });
                    }
                }
                None
            })
            .take(10)
            .collect();

        Ok(memories)
    }
}

// ── Summarizer impl ──────────────────────────────────────────────────

#[async_trait::async_trait]
impl<'a> crate::context_budget::compaction::Summarizer for LlmBackend<'a> {
    async fn summarize(&self, messages: &[ChatMessage]) -> Result<String, String> {
        let (router, compaction_model, req_context) = match self {
            Self::Router {
                router,
                compaction_model,
                context,
                ..
            } => (*router, compaction_model.clone(), context.clone()),
            Self::Direct { .. } => {
                return Err("No router available for LLM summarization".into());
            }
        };

        let mut sum_messages = Vec::with_capacity(2);
        sum_messages.push(ChatMessage::system(
            "You are a conversation summarizer. Summarize the following conversation messages \
             into a concise paragraph that captures the key points, decisions made, and important \
             context. Focus on information that would be needed to continue the conversation. \
             Be concise but complete.",
        ));
        let combined: String = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    openalpaca_llm::Role::User => "User",
                    openalpaca_llm::Role::Assistant => "Assistant",
                    openalpaca_llm::Role::System => "System",
                    openalpaca_llm::Role::Tool => "Tool",
                };
                format!("[{role}]: {}", m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        sum_messages.push(ChatMessage::user(&combined));

        let request = RouterRequest {
            messages: Arc::new(sum_messages),
            tools: Arc::new(vec![]),
            model: compaction_model,
            temperature: Some(0.0),
            max_tokens: Some(2048),
            context: req_context,
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
            context_management: None,
            fallback_models: Vec::new(),
            ephemeral_system_notice: None,
        };

        let response = router.complete(request).await.map_err(|e| e.to_string())?;
        Ok(response.content)
    }
}
