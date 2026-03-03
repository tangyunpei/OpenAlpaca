//! LLM Router: routes requests to the correct provider with key rotation,
//! fallback chains, and cost tracking.

use crate::LlmProvider;
use crate::config::LlmRuntimeConfig;
use crate::error::LlmError;
use crate::keys::key_pool::{
    ApiKey, CallResult, KeyPool, KeyPoolError, KeyStatus, ProviderType, SelectionStrategy,
};
use crate::routing::cost_tracker::{CallRecord, CostTracker};
use crate::routing::model_registry::{ModelEntry, ModelInfo, ModelRegistry};
use crate::routing::rate_limiter::{
    CircuitState, RateLimitConfig, RateLimiterRegistry, backoff_with_jitter,
};
use crate::types::*;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing;

/// Context for a router request (agent/task identification).
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
}

/// A request to the LLM router.
pub struct RouterRequest {
    pub model: Option<String>,
    pub messages: Arc<Vec<ChatMessage>>,
    pub tools: Arc<Vec<ToolDefinition>>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub context: RequestContext,
    pub tool_choice: Option<ToolChoice>,
}

/// Errors from the LLM router.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmRouterError {
    #[error("Unknown model: {0}")]
    UnknownModel(String),

    #[error("Provider not configured: {0}")]
    ProviderNotConfigured(String),

    #[error("All keys are rate-limited for provider")]
    AllKeysRateLimited,

    #[error("No API-compatible keys configured for provider (only managed/OAuth keys found)")]
    NoApiCompatibleKeys,

    #[error("Max retries exceeded")]
    MaxRetriesExceeded,

    #[error("All fallback models failed")]
    AllFallbacksFailed,

    #[error("No fallback chain configured for model: {0}")]
    NoFallbackAvailable(String),

    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
}

/// Structured LLM capacity information for adaptive concurrency decisions.
///
/// Returned by [`LlmRouter::estimated_llm_capacity`] so callers can base
/// stagger delay on the raw key count and reserve slots for the lead agent.
#[derive(Debug, Clone)]
pub struct LlmCapacityInfo {
    /// Number of API-compatible keys currently available (not in cooldown).
    pub available_api_keys: usize,
    /// Max concurrent calls allowed per key (from rate limiter config).
    pub per_key_concurrency: usize,
    /// `available_api_keys * per_key_concurrency`.
    pub key_capacity: usize,
    /// Whether a CLI fallback backend is registered for this provider.
    pub has_cli_fallback: bool,
    /// Effective parallel capacity: `min(key_capacity, global_available)`.
    /// When `key_capacity == 0` and `has_cli_fallback`, this is 1 (fallback only).
    pub effective_capacity: usize,
}

/// A provider entry with its key pool (swappable for hot-reload).
pub struct ProviderEntry {
    pub provider: Arc<dyn LlmProvider>,
    pub key_pool: Arc<ArcSwap<KeyPool>>,
}

/// The LLM Router — routes requests to providers with key rotation and fallback.
pub struct LlmRouter {
    providers: DashMap<ProviderType, ProviderEntry>,
    model_registry: ModelRegistry,
    fallback_chains: HashMap<String, Vec<String>>,
    pub cost_tracker: Arc<CostTracker>,
    default_model: ArcSwap<String>,
    /// CLI backend providers for fallback (e.g. `claude` CLI, `codex` CLI).
    cli_backends: DashMap<ProviderType, Arc<dyn LlmProvider>>,
    /// Hot-swappable runtime config (timeouts, endpoints, env vars, provider defaults).
    runtime_config: ArcSwap<LlmRuntimeConfig>,
    /// Limits concurrent in-flight LLM API calls to prevent rate-limit stampedes
    /// when parallel subagents all call the API simultaneously.
    concurrency_limiter: Arc<Semaphore>,
    /// Per-key rate limiters (RPM/TPM token buckets + concurrency) and circuit breaker.
    rate_limiter_registry: Arc<RateLimiterRegistry>,
}

impl LlmRouter {
    pub fn new(
        providers: HashMap<ProviderType, ProviderEntry>,
        model_registry: ModelRegistry,
        fallback_chains: HashMap<String, Vec<String>>,
        cost_tracker: Arc<CostTracker>,
        default_model: String,
    ) -> Self {
        let rate_config = RateLimitConfig::default();
        let dm = DashMap::new();
        for (k, v) in providers {
            dm.insert(k, v);
        }
        Self {
            providers: dm,
            model_registry,
            fallback_chains,
            cost_tracker,
            default_model: ArcSwap::from_pointee(default_model),
            cli_backends: DashMap::new(),
            runtime_config: ArcSwap::from_pointee(LlmRuntimeConfig::default()),
            concurrency_limiter: Arc::new(Semaphore::new(rate_config.global_concurrency)),
            rate_limiter_registry: Arc::new(RateLimiterRegistry::new(rate_config)),
        }
    }

    /// Create a router with an explicit runtime config and rate limit config.
    pub fn new_with_runtime(
        providers: HashMap<ProviderType, ProviderEntry>,
        model_registry: ModelRegistry,
        fallback_chains: HashMap<String, Vec<String>>,
        cost_tracker: Arc<CostTracker>,
        default_model: String,
        runtime_config: LlmRuntimeConfig,
        rate_limit_config: RateLimitConfig,
    ) -> Self {
        let dm = DashMap::new();
        for (k, v) in providers {
            dm.insert(k, v);
        }
        Self {
            providers: dm,
            model_registry,
            fallback_chains,
            cost_tracker,
            default_model: ArcSwap::from_pointee(default_model),
            cli_backends: DashMap::new(),
            runtime_config: ArcSwap::from_pointee(runtime_config),
            concurrency_limiter: Arc::new(Semaphore::new(rate_limit_config.global_concurrency)),
            rate_limiter_registry: Arc::new(RateLimiterRegistry::new(rate_limit_config)),
        }
    }

    /// Convenience constructor for single-provider setups (legacy / tests).
    pub fn single_provider(
        provider: Arc<dyn LlmProvider>,
        provider_type: ProviderType,
        default_model: String,
    ) -> Self {
        let key_pool = KeyPool::new(
            vec![ApiKey::new(
                "default".to_string(),
                provider_type,
                String::new(),
            )],
            SelectionStrategy::RoundRobin,
        );

        let providers = DashMap::new();
        providers.insert(
            provider_type,
            ProviderEntry {
                provider,
                key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
            },
        );

        let model_registry = ModelRegistry::with_defaults();
        let cost_tracker = Arc::new(CostTracker::new(ModelRegistry::with_defaults()));
        let rate_config = RateLimitConfig::default();

        Self {
            providers,
            model_registry,
            fallback_chains: HashMap::new(),
            cost_tracker,
            default_model: ArcSwap::from_pointee(default_model),
            cli_backends: DashMap::new(),
            runtime_config: ArcSwap::from_pointee(LlmRuntimeConfig::default()),
            concurrency_limiter: Arc::new(Semaphore::new(rate_config.global_concurrency)),
            rate_limiter_registry: Arc::new(RateLimiterRegistry::new(rate_config)),
        }
    }

    /// Get the default model.
    pub fn default_model(&self) -> String {
        (**self.default_model.load()).clone()
    }

    /// Set the default model (hot-reload).
    pub fn set_default_model(&self, model: String) {
        self.default_model.store(Arc::new(model));
    }

    /// Get a snapshot of the runtime config.
    pub fn runtime_config(&self) -> arc_swap::Guard<Arc<LlmRuntimeConfig>> {
        self.runtime_config.load()
    }

    /// Hot-reload the runtime config (timeouts, endpoints, env vars, provider defaults).
    pub fn reload_runtime_config(&self, config: LlmRuntimeConfig) {
        self.runtime_config.store(Arc::new(config));
    }

    /// Batch-reload model registry entries from config.
    pub fn reload_model_registry(&self, models: HashMap<String, ModelInfo>) {
        for (model_id, info) in models {
            self.model_registry.register(model_id, info);
        }
    }

    /// Get fallback models for a given model.
    pub fn fallback_models(&self, model: &str) -> Option<&Vec<String>> {
        self.fallback_chains.get(model)
    }

    /// Get list of configured providers.
    pub fn configured_providers(&self) -> Vec<ProviderType> {
        self.providers.iter().map(|entry| *entry.key()).collect()
    }

    /// Hot-reload the key pool for a specific provider.
    /// Returns true if the provider was found and updated.
    pub fn reload_keys(&self, provider: ProviderType, new_pool: KeyPool) -> bool {
        if let Some(entry) = self.providers.get(&provider) {
            entry.key_pool.store(Arc::new(new_pool));
            true
        } else {
            false
        }
    }

    /// Register a provider that was not in the original config.
    /// Used when auto-discovered credentials exist but no provider was configured.
    pub fn register_provider(
        &self,
        provider_type: ProviderType,
        provider: Arc<dyn LlmProvider>,
        pool: KeyPool,
    ) -> bool {
        self.providers.insert(
            provider_type,
            ProviderEntry {
                provider,
                key_pool: Arc::new(ArcSwap::from_pointee(pool)),
            },
        );
        true
    }

    /// Register a CLI backend for fallback.
    pub fn register_cli_backend(&self, provider_type: ProviderType, backend: Arc<dyn LlmProvider>) {
        self.cli_backends.insert(provider_type, backend);
    }

    /// Get registered CLI backend provider types.
    pub fn cli_backend_providers(&self) -> Vec<ProviderType> {
        self.cli_backends.iter().map(|e| *e.key()).collect()
    }

    /// Check if a CLI backend is registered for a provider type.
    pub fn has_cli_backend(&self, provider_type: ProviderType) -> bool {
        self.cli_backends.contains_key(&provider_type)
    }

    /// Estimate the parallel LLM capacity given the current state of API keys
    /// and rate limiters.
    ///
    /// Returns a [`LlmCapacityInfo`] struct so callers can base stagger delay
    /// on the raw key count and reserve slots for the lead agent.
    ///
    /// CLI fallback is **not** counted as parallel bandwidth — it is only used
    /// when all API keys are exhausted (see `try_fallback()`). When
    /// `key_capacity == 0` and a CLI backend exists, `effective_capacity` is 1
    /// so at least one subagent can proceed via fallback.
    ///
    /// Used by `SpawnSubagentTool` to dynamically reduce parallelism when
    /// the number of available API keys cannot support `max_concurrent_subagents`.
    pub async fn estimated_llm_capacity(&self, model: Option<&str>) -> LlmCapacityInfo {
        let zero = LlmCapacityInfo {
            available_api_keys: 0,
            per_key_concurrency: 0,
            key_capacity: 0,
            has_cli_fallback: false,
            effective_capacity: 0,
        };

        let default = self.default_model();
        let model_id = model.unwrap_or(&default);

        let provider_type = match self.model_registry.resolve_provider(model_id) {
            Some(pt) => pt,
            None => return zero,
        };

        let available_keys = match self.providers.get(&provider_type) {
            Some(entry) => {
                let pool = entry.value().key_pool.load();
                pool.available_api_key_count().await
            }
            None => return zero,
        };

        let per_key = self.rate_limiter_registry.config().per_key_concurrency;
        let key_capacity = available_keys * per_key;
        let has_cli_fallback = self.cli_backends.contains_key(&provider_type);

        let effective_capacity = if key_capacity > 0 {
            let global_available = self.concurrency_limiter.available_permits();
            key_capacity.min(global_available)
        } else if has_cli_fallback {
            // All keys exhausted but CLI fallback can handle 1 request
            1
        } else {
            0
        };

        LlmCapacityInfo {
            available_api_keys: available_keys,
            per_key_concurrency: per_key,
            key_capacity,
            has_cli_fallback,
            effective_capacity,
        }
    }

    /// List models confirmed by provider API refresh (for GUI dropdowns).
    /// Returns only discovered models so the dropdown reflects real availability.
    pub fn available_models(&self) -> Vec<ModelEntry> {
        self.model_registry.list_discovered_models()
    }

    /// Get a reference to the model registry.
    pub fn model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }

    /// Refresh models by querying each configured provider's API.
    /// Discovered models are added to the registry (existing entries preserved).
    /// Falls back to hardcoded defaults when no API-compatible key is available
    /// or when the provider returns 0 models (e.g. managed/OAuth keys only).
    pub async fn refresh_models(&self) {
        for entry in self.providers.iter() {
            let provider_type = *entry.key();
            let prov_entry = entry.value();
            let pool = prov_entry.key_pool.load();

            let key_secret = match pool.acquire_api_compatible().await {
                Ok(guard) => guard.secret.clone(),
                Err(_) => {
                    // No API-compatible key — fall back to hardcoded defaults
                    let count = self
                        .model_registry
                        .mark_defaults_discovered_for_provider(provider_type);
                    if count > 0 {
                        tracing::info!(
                            "No API key for {:?}, marked {} default models as discovered",
                            provider_type,
                            count
                        );
                    }
                    continue;
                }
            };

            match prov_entry.provider.list_models_with_key(&key_secret).await {
                Ok(model_ids) => {
                    let count = model_ids.len();
                    if count == 0 {
                        let dc = self
                            .model_registry
                            .mark_defaults_discovered_for_provider(provider_type);
                        tracing::info!(
                            "Provider {:?} returned 0 models, marked {} defaults",
                            provider_type,
                            dc
                        );
                        continue;
                    }
                    for model_id in model_ids {
                        self.model_registry.register_discovered(
                            model_id,
                            ModelInfo {
                                provider: provider_type,
                                input_price_per_million: 0.0,
                                output_price_per_million: 0.0,
                                context_window: 0,
                                discovered: true,
                                supports_image: false,
                                supports_audio: false,
                                supports_document: false,
                            },
                        );
                    }
                    tracing::info!("Refreshed {} models from {:?}", count, provider_type);
                }
                Err(e) => {
                    tracing::warn!("Failed to list models from {:?}: {}", provider_type, e);
                    self.model_registry
                        .mark_defaults_discovered_for_provider(provider_type);
                }
            }
        }
    }

    /// List models available from a specific provider using the given key.
    /// Used during key validation to show what models the key can access.
    pub async fn list_models_for_provider(
        &self,
        provider_type: ProviderType,
        key: &str,
    ) -> Result<Vec<String>, LlmError> {
        let entry = self
            .providers
            .get(&provider_type)
            .ok_or(LlmError::NotConfigured)?;
        entry.value().provider.list_models_with_key(key).await
    }

    /// Get key statuses for a provider.
    pub async fn key_statuses(&self, provider: ProviderType) -> Option<Vec<KeyStatus>> {
        if let Some(entry) = self.providers.get(&provider) {
            let pool = entry.value().key_pool.load();
            Some(pool.key_statuses().await)
        } else {
            None
        }
    }

    /// Complete a request: resolve provider, acquire key, call, handle retries/fallbacks.
    pub async fn complete(&self, request: RouterRequest) -> Result<ChatResponse, LlmRouterError> {
        let default = self.default_model();
        let model = request.model.as_deref().unwrap_or(&default);

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

    async fn try_model(
        &self,
        model: &str,
        request: &RouterRequest,
    ) -> Result<ChatResponse, LlmRouterError> {
        // Resolve provider type for the model
        let provider_type = self
            .model_registry
            .resolve_provider(model)
            .ok_or_else(|| LlmRouterError::UnknownModel(model.to_string()))?;

        let entry = self
            .providers
            .get(&provider_type)
            .ok_or_else(|| LlmRouterError::ProviderNotConfigured(provider_type.to_string()))?;

        self.execute_with_retry(entry.value(), model, request).await
    }

    async fn execute_with_retry(
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
                };

                match entry
                    .provider
                    .chat_with_key(&key_guard.secret, chat_request)
                    .await
                {
                    Ok(response) => {
                        pool.report_result(&key_guard.id, CallResult::Success).await;
                        self.rate_limiter_registry.report_success().await;

                        // Record cost
                        let cost = self.cost_tracker.calculate_cost(
                            model,
                            response.usage.input_tokens,
                            response.usage.output_tokens,
                        );
                        let record = CallRecord {
                            agent_id: request
                                .context
                                .agent_id
                                .clone()
                                .unwrap_or_else(|| "unknown".to_string()),
                            task_id: request.context.task_id.clone(),
                            model: model.to_string(),
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                            cost_usd: cost,
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

    async fn try_fallback(
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

/// Rough estimate of tokens in a request.
///
/// Uses 1 token ≈ 4 bytes heuristic. Intentionally overestimates slightly,
/// which is the safe direction for rate limiting (better to be conservative
/// than to exceed TPM limits).
fn estimate_request_tokens(request: &RouterRequest) -> u32 {
    let msg_tokens: u32 = request
        .messages
        .iter()
        .map(|m| {
            if let Some(ref parts) = m.parts {
                parts.iter().map(estimate_content_part_tokens).sum::<u32>()
            } else {
                (m.content.len() / 4) as u32
            }
        })
        .sum();
    let tool_bytes: usize = request
        .tools
        .iter()
        .map(|t| t.description.len() + t.parameters.to_string().len())
        .sum();
    (msg_tokens + (tool_bytes / 4) as u32).max(100)
}

/// Estimate tokens for a single content part (mirrors agentic_loop logic).
fn estimate_content_part_tokens(part: &crate::ContentPart) -> u32 {
    match part {
        crate::ContentPart::Text { text } => (text.len() / 4) as u32,
        crate::ContentPart::Image { detail, .. } => match detail.as_deref() {
            Some("low") => 85,
            _ => 1590,
        },
        crate::ContentPart::Audio { data, .. } => {
            ((data.len() as f64 / 4096.0) * 25.0).ceil().max(25.0) as u32
        }
        crate::ContentPart::Document { extracted_text, .. } => extracted_text
            .as_ref()
            .map_or(500, |t| (t.len() / 4) as u32),
        crate::ContentPart::FileRef { .. } => 50,
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

#[cfg(test)]
mod tests;
