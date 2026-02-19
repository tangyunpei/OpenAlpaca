//! LLM Router: routes requests to the correct provider with key rotation,
//! fallback chains, and cost tracking.

use crate::config::LlmRuntimeConfig;
use crate::cost_tracker::{CallRecord, CostTracker};
use crate::error::LlmError;
use crate::key_pool::{ApiKey, CallResult, KeyPool, KeyPoolError, KeyStatus, ProviderType, SelectionStrategy};
use crate::model_registry::{ModelEntry, ModelInfo, ModelRegistry};
use crate::types::*;
use crate::LlmProvider;
use tracing;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Maximum concurrent LLM API calls to prevent rate-limit stampedes.
/// When parallel subagents all call the API simultaneously, this limits
/// how many can be in-flight at once. Others queue with backpressure.
const DEFAULT_MAX_CONCURRENT_LLM_CALLS: usize = 4;

/// Context for a router request (agent/task identification).
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
}

/// A request to the LLM router.
pub struct RouterRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub context: RequestContext,
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
}

impl LlmRouter {
    pub fn new(
        providers: HashMap<ProviderType, ProviderEntry>,
        model_registry: ModelRegistry,
        fallback_chains: HashMap<String, Vec<String>>,
        cost_tracker: Arc<CostTracker>,
        default_model: String,
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
            runtime_config: ArcSwap::from_pointee(LlmRuntimeConfig::default()),
            concurrency_limiter: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_LLM_CALLS)),
        }
    }

    /// Create a router with an explicit runtime config.
    pub fn new_with_runtime(
        providers: HashMap<ProviderType, ProviderEntry>,
        model_registry: ModelRegistry,
        fallback_chains: HashMap<String, Vec<String>>,
        cost_tracker: Arc<CostTracker>,
        default_model: String,
        runtime_config: LlmRuntimeConfig,
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
            concurrency_limiter: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_LLM_CALLS)),
        }
    }

    /// Convenience constructor for single-provider setups (legacy / tests).
    pub fn single_provider(
        provider: Arc<dyn LlmProvider>,
        provider_type: ProviderType,
        default_model: String,
    ) -> Self {
        let key_pool = KeyPool::new(
            vec![ApiKey::new("default".to_string(), provider_type, String::new())],
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

        Self {
            providers,
            model_registry,
            fallback_chains: HashMap::new(),
            cost_tracker,
            default_model: ArcSwap::from_pointee(default_model),
            cli_backends: DashMap::new(),
            runtime_config: ArcSwap::from_pointee(LlmRuntimeConfig::default()),
            concurrency_limiter: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT_LLM_CALLS)),
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
    pub fn register_cli_backend(
        &self,
        provider_type: ProviderType,
        backend: Arc<dyn LlmProvider>,
    ) {
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
                    let count = self.model_registry
                        .mark_defaults_discovered_for_provider(provider_type);
                    if count > 0 {
                        tracing::info!(
                            "No API key for {:?}, marked {} default models as discovered",
                            provider_type, count
                        );
                    }
                    continue;
                }
            };

            match prov_entry.provider.list_models_with_key(&key_secret).await {
                Ok(model_ids) => {
                    let count = model_ids.len();
                    if count == 0 {
                        let dc = self.model_registry
                            .mark_defaults_discovered_for_provider(provider_type);
                        tracing::info!(
                            "Provider {:?} returned 0 models, marked {} defaults",
                            provider_type, dc
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
        let entry = self.providers.get(&provider_type).ok_or(LlmError::NotConfigured)?;
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
        let _permit = self.concurrency_limiter.acquire().await
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
        let pool = entry.key_pool.load();
        let max_retries = pool.len().max(1);

        // Allow up to 2 rate-limit wait cycles before giving up.
        // This handles parallel agents that burn through keys simultaneously.
        let mut rate_limit_waits = 0;
        const MAX_RATE_LIMIT_WAITS: u32 = 2;
        const MAX_RATE_LIMIT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

        loop {
            for _attempt in 0..max_retries {
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

                let chat_request = ChatRequest {
                    messages: request.messages.clone(),
                    tools: request.tools.clone(),
                    model: Some(model.to_string()),
                    temperature: request.temperature,
                    max_tokens: request.max_tokens,
                };

                match entry
                    .provider
                    .chat_with_key(&key_guard.secret, chat_request)
                    .await
                {
                    Ok(response) => {
                        pool.report_result(&key_guard.id, CallResult::Success)
                            .await;

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
                        pool.report_result(
                            &key_guard.id,
                            CallResult::RateLimited { retry_after_ms },
                        )
                        .await;
                        // Try next key
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
                        tracing::warn!(
                            model = model,
                            error = %e,
                            "Transient LLM error, retrying with next key"
                        );
                        pool.report_result(&key_guard.id, CallResult::Error(e.to_string()))
                            .await;
                        // Brief backoff before retry to avoid hammering an overloaded service
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        continue;
                    }
                    Err(e) => {
                        pool.report_result(&key_guard.id, CallResult::Error(e.to_string()))
                            .await;
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
        if let Some(pt) = provider_type {
            if let Some(cli_backend) = self.cli_backends.get(&pt) {
                tracing::info!("Falling back to CLI backend for {:?}", pt);
                let truncated = truncate_messages_for_cli(&request.messages);
                let flattened = flatten_messages(&truncated);
                let cli_request = ChatRequest {
                    messages: vec![ChatMessage::user(&flattened)],
                    tools: vec![],
                    model: Some(original_model.to_string()),
                    temperature: request.temperature,
                    max_tokens: request.max_tokens,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_pool::{ApiKey, SelectionStrategy};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockProvider {
        name_str: String,
        responses: Vec<Result<ChatResponse, LlmError>>,
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn new(name: &str, responses: Vec<Result<ChatResponse, LlmError>>) -> Self {
            Self {
                name_str: name.to_string(),
                responses,
                call_count: AtomicUsize::new(0),
            }
        }

        fn ok_response(model: &str) -> ChatResponse {
            ChatResponse {
                content: format!("Response from {}", model),
                tool_calls: vec![],
                model: model.to_string(),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                finish_reason: FinishReason::Stop,
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            &self.name_str
        }
        fn supports_tools(&self) -> bool {
            false
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx < self.responses.len() {
                self.responses[idx].clone()
            } else {
                self.responses.last().cloned().unwrap_or(Err(LlmError::NotConfigured))
            }
        }
        async fn chat_with_key(&self, _key: &str, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.chat(request).await
        }
    }

    fn make_request(model: Option<&str>) -> RouterRequest {
        RouterRequest {
            model: model.map(|m| m.to_string()),
            messages: vec![ChatMessage::user("test")],
            tools: vec![],
            temperature: None,
            max_tokens: None,
            context: RequestContext::default(),
        }
    }

    fn make_router_with_mock(
        provider: Arc<dyn LlmProvider>,
        provider_type: ProviderType,
        model: &str,
    ) -> LlmRouter {
        LlmRouter::single_provider(provider, provider_type, model.to_string())
    }

    #[tokio::test]
    async fn test_routes_to_correct_provider() {
        let provider = Arc::new(MockProvider::new(
            "anthropic",
            vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
        ));
        let router = make_router_with_mock(
            provider,
            ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929",
        );

        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.model, "claude-sonnet-4-5-20250929");
    }

    #[tokio::test]
    async fn test_key_rotation_on_rate_limit() {
        let provider = Arc::new(MockProvider::new(
            "anthropic",
            vec![
                Err(LlmError::RateLimited { retry_after_ms: 1000 }),
                Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929")),
            ],
        ));

        let key_pool = KeyPool::new(
            vec![
                ApiKey::new("k1".to_string(), ProviderType::Anthropic, "sk-1".to_string()),
                ApiKey::new("k2".to_string(), ProviderType::Anthropic, "sk-2".to_string()),
            ],
            SelectionStrategy::RoundRobin,
        );

        let mut providers = HashMap::new();
        providers.insert(
            ProviderType::Anthropic,
            ProviderEntry {
                provider: provider,
                key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
            },
        );

        let router = LlmRouter::new(
            providers,
            ModelRegistry::with_defaults(),
            HashMap::new(),
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fallback_chain() {
        // Anthropic provider always rate-limits
        let anthropic = Arc::new(MockProvider::new(
            "anthropic",
            vec![Err(LlmError::RateLimited { retry_after_ms: 1000 })],
        ));
        let openai = Arc::new(MockProvider::new(
            "openai",
            vec![Ok(MockProvider::ok_response("gpt-5.2"))],
        ));

        let mut providers = HashMap::new();
        providers.insert(
            ProviderType::Anthropic,
            ProviderEntry {
                provider: anthropic,
                key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                    vec![ApiKey::new("k1".to_string(), ProviderType::Anthropic, "sk-1".to_string())],
                    SelectionStrategy::RoundRobin,
                ))),
            },
        );
        providers.insert(
            ProviderType::OpenAI,
            ProviderEntry {
                provider: openai,
                key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                    vec![ApiKey::new("k1".to_string(), ProviderType::OpenAI, "sk-1".to_string())],
                    SelectionStrategy::RoundRobin,
                ))),
            },
        );

        let mut fallback_chains = HashMap::new();
        fallback_chains.insert(
            "claude-sonnet-4-5-20250929".to_string(),
            vec!["gpt-5.2".to_string()],
        );

        let router = LlmRouter::new(
            providers,
            ModelRegistry::with_defaults(),
            fallback_chains,
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().model, "gpt-5.2");
    }

    #[tokio::test]
    async fn test_cost_recording() {
        let provider = Arc::new(MockProvider::new(
            "anthropic",
            vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
        ));
        let router = make_router_with_mock(
            provider,
            ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929",
        );

        let mut req = make_request(None);
        req.context.agent_id = Some("agent1".to_string());

        router.complete(req).await.unwrap();

        let usage = router.cost_tracker.get_agent_usage("agent1").await;
        assert!(usage.is_some());
        assert_eq!(usage.unwrap().total_requests, 1);
    }

    #[tokio::test]
    async fn test_unknown_model_error() {
        let provider = Arc::new(MockProvider::new("anthropic", vec![]));
        let router = make_router_with_mock(
            provider,
            ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929",
        );

        let result = router
            .complete(make_request(Some("nonexistent-model-xyz")))
            .await;
        assert!(matches!(result, Err(LlmRouterError::UnknownModel(_))));
    }

    #[tokio::test]
    async fn test_all_failed_no_fallback() {
        let provider = Arc::new(MockProvider::new(
            "anthropic",
            vec![Err(LlmError::RateLimited { retry_after_ms: 1000 })],
        ));

        let key_pool = KeyPool::new(
            vec![ApiKey::new("k1".to_string(), ProviderType::Anthropic, "sk-1".to_string())],
            SelectionStrategy::RoundRobin,
        );

        let mut providers = HashMap::new();
        providers.insert(
            ProviderType::Anthropic,
            ProviderEntry {
                provider: provider,
                key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
            },
        );

        let router = LlmRouter::new(
            providers,
            ModelRegistry::with_defaults(),
            HashMap::new(), // No fallback chains
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        let result = router.complete(make_request(None)).await;
        assert!(matches!(result, Err(LlmRouterError::AllFallbacksFailed)));
    }

    #[tokio::test]
    async fn test_hot_reload_keys() {
        let provider = Arc::new(MockProvider::new(
            "anthropic",
            vec![
                Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929")),
                Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929")),
            ],
        ));

        let key_pool = KeyPool::new(
            vec![ApiKey::new("k1".to_string(), ProviderType::Anthropic, "sk-1".to_string())],
            SelectionStrategy::RoundRobin,
        );

        let mut providers = HashMap::new();
        providers.insert(
            ProviderType::Anthropic,
            ProviderEntry {
                provider: provider,
                key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
            },
        );

        let router = LlmRouter::new(
            providers,
            ModelRegistry::with_defaults(),
            HashMap::new(),
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        // First call works
        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());

        // Hot-reload with new key pool
        let new_pool = KeyPool::new(
            vec![
                ApiKey::new("k_new_1".to_string(), ProviderType::Anthropic, "sk-new-1".to_string()),
                ApiKey::new("k_new_2".to_string(), ProviderType::Anthropic, "sk-new-2".to_string()),
            ],
            SelectionStrategy::RoundRobin,
        );
        assert!(router.reload_keys(ProviderType::Anthropic, new_pool));

        // Second call works with new pool
        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());

        // Verify reload returns false for unconfigured provider
        let empty_pool = KeyPool::new(vec![], SelectionStrategy::RoundRobin);
        assert!(!router.reload_keys(ProviderType::OpenAI, empty_pool));
    }

    #[tokio::test]
    async fn test_register_provider_new() {
        let provider = Arc::new(MockProvider::new(
            "openai",
            vec![Ok(MockProvider::ok_response("gpt-5.2"))],
        ));
        let pool = KeyPool::new(
            vec![ApiKey::new("k1".to_string(), ProviderType::OpenAI, "sk-1".to_string())],
            SelectionStrategy::RoundRobin,
        );

        let router = LlmRouter::new(
            HashMap::new(),
            ModelRegistry::with_defaults(),
            HashMap::new(),
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "gpt-5.2".to_string(),
        );

        assert!(!router.reload_keys(ProviderType::OpenAI, KeyPool::new(vec![], SelectionStrategy::RoundRobin)));
        assert!(router.register_provider(ProviderType::OpenAI, provider, pool));
        assert!(router.configured_providers().contains(&ProviderType::OpenAI));
    }

    #[tokio::test]
    async fn test_try_fallback_to_cli() {
        // API provider always rate-limits
        let api_provider = Arc::new(MockProvider::new(
            "anthropic",
            vec![Err(LlmError::RateLimited { retry_after_ms: 1000 })],
        ));
        let cli_provider = Arc::new(MockProvider::new(
            "claude_cli",
            vec![Ok(MockProvider::ok_response("claude_cli"))],
        ));

        let mut providers = HashMap::new();
        providers.insert(
            ProviderType::Anthropic,
            ProviderEntry {
                provider: api_provider,
                key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                    vec![ApiKey::new("k1".to_string(), ProviderType::Anthropic, "sk-1".to_string())],
                    SelectionStrategy::RoundRobin,
                ))),
            },
        );

        let router = LlmRouter::new(
            providers,
            ModelRegistry::with_defaults(),
            HashMap::new(), // No model-level fallback chains
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        router.register_cli_backend(ProviderType::Anthropic, cli_provider);

        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().model, "claude_cli");
    }

    #[tokio::test]
    async fn test_cli_fallback_not_called_when_api_succeeds() {
        let api_provider = Arc::new(MockProvider::new(
            "anthropic",
            vec![Ok(MockProvider::ok_response("claude-sonnet-4-5-20250929"))],
        ));
        let cli_provider = Arc::new(MockProvider::new(
            "claude_cli",
            vec![Ok(MockProvider::ok_response("claude_cli"))],
        ));

        let router = LlmRouter::single_provider(
            api_provider,
            ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        router.register_cli_backend(ProviderType::Anthropic, cli_provider);

        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());
        // Should use API, not CLI
        assert_eq!(result.unwrap().model, "claude-sonnet-4-5-20250929");
    }

    // ── Key-aware mock provider ────────────────────────────────────

    struct KeyAwareMockProvider;

    #[async_trait]
    impl LlmProvider for KeyAwareMockProvider {
        fn name(&self) -> &str { "key_aware_mock" }
        fn supports_tools(&self) -> bool { false }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
            Err(LlmError::NotConfigured)
        }
        async fn chat_with_key(&self, key: &str, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
            if key.starts_with("sk-ant-oat") {
                Err(LlmError::AuthenticationFailed("managed token cannot auth against HTTP API".into()))
            } else {
                Ok(MockProvider::ok_response("claude-haiku"))
            }
        }
    }

    #[tokio::test]
    async fn test_no_api_keys_triggers_cli_fallback() {
        // Pool with only a managed key (source=ClaudeCode)
        let mut managed_key = ApiKey::new(
            "managed1".to_string(),
            ProviderType::Anthropic,
            "sk-ant-oat01-managed-token-placeholder-very-long".to_string(),
        );
        managed_key.source = crate::key_pool::KeySource::ClaudeCode;

        let key_pool = KeyPool::new(
            vec![managed_key],
            SelectionStrategy::RoundRobin,
        );

        let key_aware = Arc::new(KeyAwareMockProvider);
        let cli_provider = Arc::new(MockProvider::new(
            "claude_cli",
            vec![Ok(MockProvider::ok_response("claude_cli"))],
        ));

        let mut providers = HashMap::new();
        providers.insert(
            ProviderType::Anthropic,
            ProviderEntry {
                provider: key_aware,
                key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
            },
        );

        let router = LlmRouter::new(
            providers,
            ModelRegistry::with_defaults(),
            HashMap::new(),
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        router.register_cli_backend(ProviderType::Anthropic, cli_provider);

        // Should fall through to CLI backend since no API-compatible keys
        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().model, "claude_cli");
    }

    #[tokio::test]
    async fn test_mixed_pool_uses_api_key_only() {
        // Pool with managed key + real API key
        let mut managed_key = ApiKey::new(
            "managed1".to_string(),
            ProviderType::Anthropic,
            "sk-ant-oat01-managed-token-placeholder-very-long".to_string(),
        );
        managed_key.source = crate::key_pool::KeySource::ClaudeCode;

        let mut api_key = ApiKey::new(
            "api1".to_string(),
            ProviderType::Anthropic,
            format!("sk-ant-api03-{}", "x".repeat(30)),
        );
        api_key.source = crate::key_pool::KeySource::ApiConsole;

        let key_pool = KeyPool::new(
            vec![managed_key, api_key],
            SelectionStrategy::RoundRobin,
        );

        let key_aware = Arc::new(KeyAwareMockProvider);

        let mut providers = HashMap::new();
        providers.insert(
            ProviderType::Anthropic,
            ProviderEntry {
                provider: key_aware,
                key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
            },
        );

        let router = LlmRouter::new(
            providers,
            ModelRegistry::with_defaults(),
            HashMap::new(),
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        // Should succeed using the API key, not the managed key
        let result = router.complete(make_request(None)).await;
        assert!(result.is_ok());
        // KeyAwareMockProvider returns "claude-haiku" for valid API keys
        assert_eq!(result.unwrap().model, "claude-haiku");
    }

    #[test]
    fn test_flatten_messages() {
        let messages = vec![
            ChatMessage { role: Role::System, content: "You are helpful.".to_string(), tool_calls: None, tool_call_id: None },
            ChatMessage::user("Hello"),
            ChatMessage { role: Role::Assistant, content: "Hi!".to_string(), tool_calls: None, tool_call_id: None },
        ];
        let flattened = flatten_messages(&messages);
        assert!(flattened.contains("[System] You are helpful."));
        assert!(flattened.contains("Hello"));
        assert!(flattened.contains("[Assistant] Hi!"));
    }

    #[test]
    fn test_truncate_messages_for_cli_small_input() {
        let messages = vec![
            ChatMessage { role: Role::System, content: "System prompt".to_string(), tool_calls: None, tool_call_id: None },
            ChatMessage::user("Hello"),
            ChatMessage { role: Role::Assistant, content: "Hi!".to_string(), tool_calls: None, tool_call_id: None },
        ];
        let result = truncate_messages_for_cli(&messages);
        // Small input — no truncation needed
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "System prompt");
        assert_eq!(result[1].content, "Hello");
        assert_eq!(result[2].content, "Hi!");
    }

    #[test]
    fn test_truncate_messages_for_cli_large_input() {
        let big_content = "x".repeat(8 * 1024); // 8KB per message
        let messages = vec![
            ChatMessage { role: Role::System, content: "System prompt".to_string(), tool_calls: None, tool_call_id: None },
            ChatMessage::user(&big_content),          // middle — should be dropped
            ChatMessage { role: Role::Assistant, content: big_content.clone(), tool_calls: None, tool_call_id: None }, // middle — should be dropped
            ChatMessage::user(&big_content),          // middle — should be dropped
            ChatMessage { role: Role::Tool, content: "tool result".to_string(), tool_calls: None, tool_call_id: None }, // second-to-last — kept
            ChatMessage::user("What next?"),           // last — kept
        ];
        // Total > 16KB, so truncation should fire
        let result = truncate_messages_for_cli(&messages);
        // Should keep: system (1) + omission notice (1) + last 2 (2) = 4
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].content, "System prompt");
        assert!(result[1].content.contains("earlier messages omitted"));
        assert_eq!(result[2].content, "tool result");
        assert_eq!(result[3].content, "What next?");
    }

    #[test]
    fn test_truncate_messages_for_cli_empty() {
        let result = truncate_messages_for_cli(&[]);
        assert!(result.is_empty());
    }
}
