//! LLM Router: routes requests to the correct provider with key rotation,
//! fallback chains, and cost tracking.

mod capacity;
mod completion;
mod fallback;
mod retry;
mod types;

pub use fallback::{flatten_messages, truncate_messages_for_cli};
pub use types::{
    LlmCapacityInfo, LlmRouterError, ProviderEntry, RequestContext, RouterRequest,
};

use crate::LlmProvider;
use crate::config::LlmRuntimeConfig;
use crate::keys::key_pool::{
    ApiKey, KeyPool, KeyStatus, ProviderType, SelectionStrategy,
};
use crate::routing::cost_tracker::CostTracker;
use crate::routing::model_registry::ModelRegistry;
use crate::routing::rate_limiter::{RateLimitConfig, RateLimiterRegistry};
use arc_swap::ArcSwap;

// Re-export types used by tests via `use super::*;`
#[cfg(test)]
use crate::error::LlmError;
#[cfg(test)]
use crate::keys::key_pool::CallResult;
#[cfg(test)]
use crate::types::*;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// The LLM Router — routes requests to providers with key rotation and fallback.
pub struct LlmRouter {
    pub(super) providers: DashMap<ProviderType, ProviderEntry>,
    pub(super) model_registry: ModelRegistry,
    pub(super) fallback_chains: HashMap<String, Vec<String>>,
    pub cost_tracker: Arc<CostTracker>,
    default_model: ArcSwap<String>,
    /// CLI backend providers for fallback (e.g. `claude` CLI, `codex` CLI).
    pub(super) cli_backends: DashMap<ProviderType, Arc<dyn LlmProvider>>,
    /// Hot-swappable runtime config (timeouts, endpoints, env vars, provider defaults).
    runtime_config: ArcSwap<LlmRuntimeConfig>,
    /// Limits concurrent in-flight LLM API calls to prevent rate-limit stampedes
    /// when parallel subagents all call the API simultaneously.
    pub(super) concurrency_limiter: Arc<Semaphore>,
    /// Per-key rate limiters (RPM/TPM token buckets + concurrency) and circuit breaker.
    pub(super) rate_limiter_registry: Arc<RateLimiterRegistry>,
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
                provider_type.clone(),
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
    pub fn reload_model_registry(
        &self,
        models: HashMap<String, crate::routing::model_registry::ModelInfo>,
    ) {
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
        self.providers.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Hot-reload the key pool for a specific provider.
    /// Returns true if the provider was found and updated.
    pub fn reload_keys(&self, provider: &ProviderType, new_pool: KeyPool) -> bool {
        if let Some(entry) = self.providers.get(provider) {
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

    /// Remove a provider and all its registered models.
    /// Returns the list of model IDs that were removed.
    pub fn deregister_provider(&self, provider_type: &ProviderType) -> Vec<String> {
        self.providers.remove(provider_type);
        self.model_registry.remove_by_provider(provider_type)
    }

    /// Register a CLI backend for fallback.
    pub fn register_cli_backend(&self, provider_type: ProviderType, backend: Arc<dyn LlmProvider>) {
        self.cli_backends.insert(provider_type, backend);
    }

    /// Get registered CLI backend provider types.
    pub fn cli_backend_providers(&self) -> Vec<ProviderType> {
        self.cli_backends.iter().map(|e| e.key().clone()).collect()
    }

    /// Check if a CLI backend is registered for a provider type.
    pub fn has_cli_backend(&self, provider_type: &ProviderType) -> bool {
        self.cli_backends.contains_key(provider_type)
    }

    /// Get a reference to the model registry.
    pub fn model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }

    /// Get key statuses for a provider.
    pub async fn key_statuses(&self, provider: &ProviderType) -> Option<Vec<KeyStatus>> {
        if let Some(entry) = self.providers.get(provider) {
            let pool = entry.value().key_pool.load();
            Some(pool.key_statuses().await)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests;
