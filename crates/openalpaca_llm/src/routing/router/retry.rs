use super::*;
use super::capacity::estimate_request_tokens;
use crate::error::LlmError;
use crate::keys::key_pool::{CallResult, KeyPoolError};
use crate::routing::cost_tracker::CallRecord;
use crate::routing::rate_limiter::{CircuitState, backoff_with_jitter};
use crate::types::*;
use std::sync::Arc;

impl LlmRouter {
    pub(super) async fn execute_with_retry(
        &self,
        entry: &ProviderEntry,
        model: &str,
        request: &RouterRequest,
    ) -> Result<ChatResponse, LlmRouterError> {
        // 1. Check global circuit breaker
        if let Err(CircuitState::Open) = self.rate_limiter_registry.check_circuit().await {
            tracing::warn!(
                model = model,
                "Circuit breaker is Open — failing fast without calling API"
            );
            return Err(LlmRouterError::MaxRetriesExceeded);
        }

        let pool = entry.key_pool.load();
        let rate_config = self.rate_limiter_registry.config();
        let max_retries = pool.len().max(rate_config.max_transient_retries);
        let estimated_tokens = estimate_request_tokens(request);
        let backoff_base = std::time::Duration::from_millis(rate_config.backoff_base_ms);
        let backoff_cap = std::time::Duration::from_millis(rate_config.backoff_cap_ms);
        const OVERLOAD_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(10);

        // Allow up to 2 rate-limit wait cycles before giving up.
        let mut rate_limit_waits = 0;
        const MAX_RATE_LIMIT_WAITS: u32 = 2;
        const MAX_RATE_LIMIT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

        loop {
            for attempt in 0..max_retries {
                let key_guard = match pool.acquire().await {
                    Ok(guard) => guard,
                    Err(KeyPoolError::NoApiCompatibleKeys) => {
                        return Err(LlmRouterError::NoApiCompatibleKeys);
                    }
                    Err(_) => {
                        // AllKeysRateLimited — break to the wait-for-cooldown logic below
                        break;
                    }
                };

                // 2. Per-key rate limiting: concurrency + RPM + TPM token buckets
                let key_limiter = self
                    .rate_limiter_registry
                    .get_or_create(&key_guard.id, key_guard.rate_limit);
                let _key_permit = key_limiter.acquire(estimated_tokens).await;

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
                    .chat_with_key(&key_guard.secret, chat_request)
                    .await
                {
                    Ok(response) => {
                        pool.report_result(&key_guard.id, CallResult::Success).await;
                        self.rate_limiter_registry.report_success().await;

                        // Record cost (cache-aware: cache-read tokens must not be
                        // billed at the full input rate, or budgets abort early).
                        let cost = self.cost_tracker.calculate_cost_with_cache(
                            &response.model,
                            response.usage.input_tokens,
                            response.usage.output_tokens,
                            response.usage.cache_creation_input_tokens,
                            response.usage.cache_read_input_tokens,
                        );
                        let record = CallRecord {
                            agent_id: request
                                .context
                                .agent_id
                                .clone()
                                .unwrap_or_else(|| "unknown".to_string()),
                            task_id: request.context.task_id.clone(),
                            model: response.model.clone(),
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                            cost_usd: cost,
                            cache_creation_tokens: response.usage.cache_creation_input_tokens,
                            cache_read_tokens: response.usage.cache_read_input_tokens,
                        };
                        self.cost_tracker.record(&record).await;

                        return Ok(response);
                    }
                    Err(LlmError::RateLimited { retry_after_ms }) => {
                        tracing::warn!(
                            model = model,
                            key_id = %key_guard.id,
                            retry_after_ms,
                            attempt = attempt + 1,
                            max_attempts = max_retries,
                            error_kind = "rate_limited",
                            "Provider rate-limited key; applying key cooldown and rotating"
                        );
                        pool.report_result(
                            &key_guard.id,
                            CallResult::RateLimited { retry_after_ms },
                        )
                        .await;
                        self.rate_limiter_registry.report_failure().await;
                        // Try next key — token bucket naturally throttles re-requests
                        continue;
                    }
                    Err(LlmError::Overloaded {
                        status,
                        retry_after_ms,
                    }) => {
                        let overload_cap = backoff_cap.min(OVERLOAD_BACKOFF_CAP);
                        let recommended = retry_after_ms
                            .map(std::time::Duration::from_millis)
                            .map(|d| d.min(backoff_cap))
                            .unwrap_or_else(|| {
                                backoff_with_jitter(backoff_base, attempt as u32, overload_cap)
                            });
                        let jitter = backoff_with_jitter(
                            std::time::Duration::from_millis(50),
                            attempt as u32,
                            std::time::Duration::from_millis(750),
                        );
                        let wait = recommended.saturating_add(jitter).min(backoff_cap);

                        tracing::warn!(
                            model = model,
                            key_id = %key_guard.id,
                            status,
                            retry_after_ms = ?retry_after_ms,
                            wait_ms = wait.as_millis() as u64,
                            attempt = attempt + 1,
                            max_attempts = max_retries,
                            error_kind = "overloaded",
                            "Provider overloaded; retrying without key cooldown"
                        );

                        // Overload is provider-level transient pressure, not a per-key quota issue.
                        pool.report_result(
                            &key_guard.id,
                            CallResult::Error(format!(
                                "provider_overloaded(status={status}, retry_after_ms={:?})",
                                retry_after_ms
                            )),
                        )
                        .await;
                        self.rate_limiter_registry.report_failure().await;
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    Err(e) if e.is_auth_error() => {
                        tracing::warn!(
                            "Authentication error for key '{}', trying next key",
                            key_guard.id
                        );
                        pool.report_result(&key_guard.id, CallResult::Error(e.to_string()))
                            .await;
                        continue;
                    }
                    Err(e) if e.is_transient() => {
                        // Exponential backoff with full jitter
                        let backoff =
                            backoff_with_jitter(backoff_base, attempt as u32, backoff_cap);
                        tracing::warn!(
                            model = model,
                            key_id = %key_guard.id,
                            error = %e,
                            attempt = attempt,
                            backoff_ms = backoff.as_millis() as u64,
                            "Transient LLM error, retrying with exponential backoff"
                        );
                        pool.report_result(&key_guard.id, CallResult::Error(e.to_string()))
                            .await;
                        self.rate_limiter_registry.report_failure().await;
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    Err(e) => {
                        pool.report_result(&key_guard.id, CallResult::Error(e.to_string()))
                            .await;
                        self.rate_limiter_registry.report_failure().await;
                        return Err(LlmRouterError::Llm(e));
                    }
                }
            }

            // All keys exhausted or rate-limited — wait for cooldown if we haven't exceeded wait limit
            if rate_limit_waits >= MAX_RATE_LIMIT_WAITS {
                break;
            }

            if let Some(cooldown) = pool.shortest_cooldown().await {
                let wait = cooldown.min(MAX_RATE_LIMIT_WAIT);
                tracing::info!(
                    model = model,
                    wait_secs = wait.as_secs(),
                    attempt = rate_limit_waits + 1,
                    max_attempts = MAX_RATE_LIMIT_WAITS,
                    "All keys rate-limited, waiting for cooldown before retry"
                );
                tokio::time::sleep(wait).await;
                rate_limit_waits += 1;
                continue;
            } else {
                // No cooldowns active — keys are genuinely exhausted
                break;
            }
        }

        Err(LlmRouterError::MaxRetriesExceeded)
    }
}
