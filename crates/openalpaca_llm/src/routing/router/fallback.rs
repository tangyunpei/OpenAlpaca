use super::*;
use crate::routing::cost_tracker::CallRecord;
use crate::types::*;
use std::sync::Arc;

impl LlmRouter {
    pub(super) async fn try_fallback(
        &self,
        original_model: &str,
        request: &RouterRequest,
    ) -> Result<ChatResponse, LlmRouterError> {
        // 1. Try model-level fallback chains (existing behavior)
        if let Some(fallback_chain) = self.fallback_chains.get(original_model) {
            for fallback_model in fallback_chain {
                match self.try_model(fallback_model, request).await {
                    Ok(response) => return Ok(response),
                    Err(_) => continue,
                }
            }
        }

        // 2. Try CLI backend fallback
        let provider_type = self.model_registry.resolve_provider(original_model);
        if let Some(pt) = provider_type
            && let Some(cli_backend) = self.cli_backends.get(&pt)
        {
            tracing::info!("Falling back to CLI backend for {:?}", pt);
            let truncated = truncate_messages_for_cli(&request.messages);
            let flattened = flatten_messages(&truncated);
            let cli_request = ChatRequest {
                messages: Arc::new(vec![ChatMessage::user(&flattened)]),
                tools: Arc::new(vec![]),
                model: Some(original_model.to_string()),
                temperature: request.temperature,
                max_tokens: request.max_tokens,
                tool_choice: None,
                enable_caching: false,
                thinking: None,
                context_management: None,
            };
            match cli_backend.chat(cli_request).await {
                Ok(response) => {
                    // Record with zero cost (CLI fallback)
                    let record = CallRecord {
                        agent_id: request
                            .context
                            .agent_id
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        task_id: request.context.task_id.clone(),
                        model: format!("{}_cli", original_model),
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    };
                    self.cost_tracker.record(&record).await;
                    return Ok(response);
                }
                Err(e) => {
                    tracing::warn!("CLI backend fallback failed for {:?}: {}", pt, e);
                }
            }
        }

        Err(LlmRouterError::AllFallbacksFailed)
    }
}

/// Maximum prompt size (in bytes) for CLI backend fallback.
/// CLI backends shell out to `claude -p "..."` which is slow on large prompts.
const CLI_MAX_PROMPT_BYTES: usize = 16 * 1024;

/// Truncate a message history for CLI fallback to avoid timeouts.
///
/// Keeps the first message (system prompt) and the last 2 messages (most recent context),
/// dropping middle messages when the total flattened size exceeds `CLI_MAX_PROMPT_BYTES`.
pub fn truncate_messages_for_cli(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    if messages.is_empty() {
        return vec![];
    }

    let total_size: usize = messages.iter().map(|m| m.content.len() + 20).sum();
    if total_size <= CLI_MAX_PROMPT_BYTES {
        return messages.to_vec();
    }

    let mut result = Vec::new();

    // Always keep the first message (system prompt)
    result.push(messages[0].clone());

    // Keep the last 2 messages (most recent context)
    let kept_tail = 2.min(messages.len().saturating_sub(1));
    let dropped = messages.len().saturating_sub(1 + kept_tail);

    if dropped > 0 {
        result.push(ChatMessage::user(&format!(
            "[... {} earlier messages omitted for CLI fallback (total history was {}KB) ...]",
            dropped,
            total_size / 1024,
        )));
    }

    for msg in messages.iter().rev().take(kept_tail).rev() {
        result.push(msg.clone());
    }

    result
}

/// Flatten messages into a single prompt string for CLI backends.
pub fn flatten_messages(messages: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        match msg.role {
            Role::System => parts.push(format!("[System] {}", msg.content)),
            Role::User => parts.push(msg.content.clone()),
            Role::Assistant => parts.push(format!("[Assistant] {}", msg.content)),
            Role::Tool => parts.push(format!("[Tool] {}", msg.content)),
        }
    }
    parts.join("\n")
}
