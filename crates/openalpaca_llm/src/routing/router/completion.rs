use super::*;
use crate::error::LlmError;
use crate::keys::key_pool::KeyPoolError;
use crate::types::*;
use std::sync::Arc;

impl LlmRouter {
    /// Streaming completion: resolve provider, acquire key, start streaming.
    ///
    /// Retries on rate-limit or auth errors by rotating to the next key
    /// (up to `pool.len().min(3)` attempts). Does NOT fall back to other
    /// models — the caller (agentic loop) handles streaming→non-streaming
    /// fallback on final failure.
    ///
    /// Returns the stream **and the model it is being served by** (V4): the
    /// ladder below may answer a request for one id with another, and only
    /// this function knows which.
    pub async fn complete_streaming(
        &self,
        request: RouterRequest,
    ) -> Result<RoutedStream, LlmRouterError> {
        let permit = Arc::clone(&self.concurrency_limiter)
            .acquire_owned()
            .await
            .map_err(|_| LlmRouterError::MaxRetriesExceeded)?;

        let default = self.default_model();
        let requested = request.model.as_deref().unwrap_or(&default);
        // L3: an id nothing can serve no longer dies here. Resolve it against
        // the ladder first — the caller's chain, the model's, the
        // orchestrator's, the effective default — and say so when it moves.
        let model = self
            .resolve_routable(requested, &request.fallback_models)
            .ok_or(LlmRouterError::NoRoutableModel)?;
        let model = model.as_str();

        let provider_type = self
            .model_registry
            .resolve_provider(model)
            .ok_or_else(|| LlmRouterError::UnknownModel(model.to_string()))?;

        // As in `try_model`: the `Ref` must not outlive the lookup (R59).
        let entry = self
            .provider_entry(&provider_type)
            .ok_or_else(|| LlmRouterError::ProviderNotConfigured(provider_type.to_string()))?;

        let pool = entry.key_pool.load();
        // Same as the non-streaming ladder: an empty pool on a provider that
        // needs no key is served through the synthetic slot, and there is
        // exactly one of those to rotate through (L1). Without this the loop
        // body never ran — `pool.len().min(3)` is 0 — and the owner saw
        // "All keys are rate-limited" for a provider that has no keys to limit.
        let keyless = (pool.is_empty() && !entry.provider.requires_key())
            .then(|| super::keyless_slot(entry.provider.name()));
        let max_attempts = if keyless.is_some() {
            1
        } else {
            pool.len().min(3)
        };

        for attempt in 0..max_attempts {
            let key_guard = match pool.acquire().await {
                Ok(guard) => guard,
                Err(_) if keyless.is_some() => keyless.clone().expect("checked above"),
                Err(KeyPoolError::NoApiCompatibleKeys) => {
                    return Err(LlmRouterError::NoApiCompatibleKeys);
                }
                Err(_) => {
                    return Err(LlmRouterError::AllKeysRateLimited);
                }
            };

            let chat_request = ChatRequest {
                messages: Arc::clone(&request.messages),
                tools: Arc::clone(&request.tools),
                model: Some(model.to_string()),
                temperature: request.temperature,
                max_tokens: request.max_tokens,
                tool_choice: request.tool_choice.clone(),
                enable_caching: request.enable_caching,
                thinking: request.thinking.clone(),
                context_management: request.context_management.clone(),
                ephemeral_system_notice: request.ephemeral_system_notice.clone(),
            };

            match entry
                .provider
                .chat_streaming_with_key(&key_guard.secret, chat_request)
                .await
            {
                Ok(stream) => {
                    return Ok(RoutedStream {
                        model: model.to_string(),
                        stream: Box::pin(crate::streaming::PermitStream::new(stream, permit)),
                    });
                }
                Err(LlmError::RateLimited { retry_after_ms }) => {
                    tracing::warn!(
                        model = model,
                        key_id = %key_guard.id,
                        retry_after_ms,
                        attempt = attempt + 1,
                        max_attempts,
                        "Streaming key rate-limited, rotating"
                    );
                    pool.report_result(
                        &key_guard.id,
                        crate::keys::key_pool::CallResult::RateLimited { retry_after_ms },
                    )
                    .await;
                    continue;
                }
                Err(e) if e.is_auth_error() => {
                    tracing::warn!(
                        model = model,
                        key_id = %key_guard.id,
                        attempt = attempt + 1,
                        max_attempts,
                        "Streaming key auth error, rotating"
                    );
                    pool.report_result(
                        &key_guard.id,
                        crate::keys::key_pool::CallResult::Error(e.to_string()),
                    )
                    .await;
                    continue;
                }
                Err(e) => return Err(LlmRouterError::Llm(e)),
            }
        }

        Err(LlmRouterError::AllKeysRateLimited)
    }

    /// Complete a request: resolve provider, acquire key, call, handle retries/fallbacks.
    pub async fn complete(&self, request: RouterRequest) -> Result<ChatResponse, LlmRouterError> {
        let default = self.default_model();
        let requested = request.model.as_deref().unwrap_or(&default);
        // See `complete_streaming`: the ladder runs before the call, so an
        // unroutable pin is answered by a routable model instead of failing
        // past the fallback chain entirely (L3).
        let model = self
            .resolve_routable(requested, &request.fallback_models)
            .ok_or(LlmRouterError::NoRoutableModel)?;
        let model = model.as_str();

        // Acquire concurrency permit — limits parallel in-flight API calls
        // to prevent rate-limit stampedes from parallel subagents.
        let _permit = self
            .concurrency_limiter
            .acquire()
            .await
            .map_err(|_| LlmRouterError::MaxRetriesExceeded)?;

        match self.try_model(model, &request).await {
            Ok(response) => Ok(response),
            Err(LlmRouterError::NoApiCompatibleKeys) => {
                tracing::warn!(
                    model = model,
                    "No API-compatible keys configured (only managed/OAuth tokens). \
                     Add an API key (sk-ant-api*) to avoid CLI fallback. Trying fallback chain."
                );
                self.try_fallback(model, &request).await
            }
            Err(LlmRouterError::AllKeysRateLimited) => {
                tracing::warn!(
                    model = model,
                    "All API keys are rate-limited. Trying fallback chain."
                );
                self.try_fallback(model, &request).await
            }
            Err(LlmRouterError::MaxRetriesExceeded) => {
                tracing::warn!(
                    model = model,
                    "Max retries exceeded across all keys. Trying fallback chain."
                );
                self.try_fallback(model, &request).await
            }
            // The provider was unloaded between the ladder and the call — a
            // disable landing mid-request. Take the ladder's next rung rather
            // than handing the caller a bare "Unknown model".
            Err(LlmRouterError::UnknownModel(_)) | Err(LlmRouterError::ProviderNotConfigured(_)) => {
                tracing::warn!(
                    model = model,
                    "Model became unroutable during the call. Trying fallback chain."
                );
                self.try_fallback(model, &request).await
            }
            // Transient errors (529 Overloaded, 500+) should also try fallback models
            Err(LlmRouterError::Llm(ref llm_err)) if llm_err.is_transient() => {
                tracing::warn!(
                    model = model,
                    error = %llm_err,
                    "Transient API error. Trying fallback chain."
                );
                self.try_fallback(model, &request).await
            }
            Err(e) => Err(e),
        }
    }

    pub(super) async fn try_model(
        &self,
        model: &str,
        request: &RouterRequest,
    ) -> Result<ChatResponse, LlmRouterError> {
        // Resolve provider type for the model
        let provider_type = self
            .model_registry
            .resolve_provider(model)
            .ok_or_else(|| LlmRouterError::UnknownModel(model.to_string()))?;

        // Take the entry *out* of the map — two `Arc` clones — and drop the
        // `Ref` before awaiting anything. Holding it across the call would make
        // `deregister_provider`'s synchronous shard write lock wait for the
        // network, parking the worker thread that issued the disable (R59).
        let entry = self
            .provider_entry(&provider_type)
            .ok_or_else(|| LlmRouterError::ProviderNotConfigured(provider_type.to_string()))?;

        self.execute_with_retry(&entry, model, request).await
    }
}
