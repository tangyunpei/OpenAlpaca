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
    ApiKey, KeyGuard, KeyPool, KeyStatus, ProviderType, SelectionStrategy,
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
use dashmap::{DashMap, DashSet};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// The slot a keyless provider's calls are accounted to.
///
/// It is **not** a key: it carries no secret, it is never written to config,
/// and it never appears in [`LlmRouter::key_statuses`]. It exists so a provider
/// that needs no key still gets a rate limiter and a circuit-breaker report of
/// its own instead of being refused by an empty pool (L1).
pub(super) fn keyless_slot(provider_name: &str) -> KeyGuard {
    KeyGuard {
        id: format!("{provider_name}:keyless"),
        secret: String::new(),
        rate_limit: None,
    }
}

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
    /// What the last discovery pass learned about each provider, so an empty
    /// model list can be told apart from a provider that could not be reached.
    pub(super) discovery: DashMap<ProviderType, crate::routing::model_registry::ProviderDiscovery>,
    /// (requested → effective) pairs already announced, so a substitution is
    /// visible once per pair instead of once per round (L3).
    pub(super) substitutions_logged: DashSet<(String, String)>,
}

/// Append `candidate` unless it is the model we are already replacing, or is
/// already on the ladder.
fn push_unique(chain: &mut Vec<String>, requested: &str, candidate: &str) {
    if candidate != requested && !chain.iter().any(|m| m == candidate) {
        chain.push(candidate.to_string());
    }
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
            discovery: DashMap::new(),
            substitutions_logged: DashSet::new(),
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
            discovery: DashMap::new(),
            substitutions_logged: DashSet::new(),
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
            discovery: DashMap::new(),
            substitutions_logged: DashSet::new(),
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

    /// Clone a provider's entry out of the map.
    ///
    /// The one way a request may reach a provider: the returned entry owns its
    /// `Arc`s, so the `DashMap` shard's read lock is released here rather than
    /// at the end of the call. A `Ref` held across an `.await` blocks
    /// [`Self::deregister_provider`], whose `remove` is a synchronous write
    /// lock on that same shard (R59).
    pub(super) fn provider_entry(&self, provider_type: &ProviderType) -> Option<ProviderEntry> {
        self.providers.get(provider_type).map(|e| e.value().clone())
    }

    /// Is this provider loaded right now?
    ///
    /// The router keeps no `enabled` bit of its own — a disabled provider is
    /// simply not in the map — so this is what "the owner has it switched on
    /// and it loaded" looks like from inside (R58c, R60).
    pub fn has_provider(&self, provider_type: &ProviderType) -> bool {
        self.providers.contains_key(provider_type)
    }

    // ── L3: which model a request actually reaches ──────────────────────────

    /// Can a call naming this model be made at all — is the id in the
    /// catalogue *and* its provider loaded?
    pub fn is_routable(&self, model_id: &str) -> bool {
        self.model_registry
            .resolve_provider(model_id)
            .is_some_and(|pt| self.has_provider(&pt))
    }

    /// Loaded providers, in the order the ladder prefers them.
    ///
    /// `llm.toml`'s `[providers]` table parses into a `HashMap`, so the file's
    /// declaration order does not survive parsing. The built-in order is used
    /// instead — anthropic, openai, ollama — which is the order the seeded
    /// template declares. Anything else loaded (a plugin provider) follows,
    /// sorted by name, so the answer does not move between runs.
    fn provider_preference_order(&self) -> Vec<ProviderType> {
        let mut ordered: Vec<ProviderType> = ProviderType::all()
            .iter()
            .filter(|pt| self.has_provider(pt))
            .cloned()
            .collect();
        let mut rest: Vec<ProviderType> = self
            .providers
            .iter()
            .map(|e| e.key().clone())
            .filter(|pt| !ordered.contains(pt))
            .collect();
        rest.sort_by_key(|pt| pt.to_string());
        ordered.append(&mut rest);
        ordered
    }

    /// The model this provider would answer with, if it had to answer.
    ///
    /// Its configured `default_model` when that is routable; otherwise what the
    /// owner actually has — Ollama's `default_model` names a tag that may never
    /// have been pulled, and the catalogue knows which ones were.
    fn provider_default_model(&self, provider_type: &ProviderType) -> Option<String> {
        let configured = self
            .runtime_config()
            .provider_defaults
            .get(&provider_type.to_string())
            .map(|d| d.default_model.clone());
        if let Some(model) = configured
            && self.is_routable(&model)
        {
            return Some(model);
        }
        self.model_registry
            .first_model_for_provider(provider_type)
            .filter(|m| self.is_routable(m))
    }

    /// The model a request that names none will actually reach.
    ///
    /// `None` means nothing at all is routable — no enabled provider offers a
    /// model — which is the one condition [`LlmRouterError::NoRoutableModel`]
    /// reports. Callers that display configuration (status, the models route)
    /// show this beside the configured default, so "configured: X — not
    /// available, using Y" is sayable.
    pub fn effective_default_model(&self) -> Option<String> {
        let configured = self.default_model();
        if self.is_routable(&configured) {
            return Some(configured);
        }
        self.provider_preference_order()
            .into_iter()
            .find_map(|pt| self.provider_default_model(&pt))
    }

    /// Everything the ladder would try in place of `requested`, in order.
    ///
    /// The request's own chain, then the model's configured chain, then
    /// `[orchestrator] fallback_models` (stored under the configured default
    /// model), then the effective default. Deduped, and never containing
    /// `requested` itself.
    pub(super) fn substitution_ladder(
        &self,
        requested: &str,
        request_chain: &[String],
    ) -> Vec<String> {
        let mut chain: Vec<String> = Vec::new();
        for model in request_chain {
            push_unique(&mut chain, requested, model);
        }
        if let Some(models) = self.fallback_chains.get(requested) {
            for model in models {
                push_unique(&mut chain, requested, model);
            }
        }
        let configured_default = self.default_model();
        if let Some(models) = self.fallback_chains.get(&configured_default) {
            for model in models {
                push_unique(&mut chain, requested, model);
            }
        }
        if let Some(effective) = self.effective_default_model() {
            push_unique(&mut chain, requested, &effective);
        }
        chain
    }

    /// The model a request naming `requested` will be sent as.
    ///
    /// `requested` itself when it is routable — the ordinary case, unchanged.
    /// Otherwise the first rung of the ladder that is, announced once. `None`
    /// when nothing is routable.
    ///
    /// This is what keeps the shipped agent templates working: their Claude
    /// pins are right when Anthropic is configured, and fall through here when
    /// it is not, instead of dying on `UnknownModel` before the fallback chain
    /// was ever consulted.
    pub(super) fn resolve_routable(
        &self,
        requested: &str,
        request_chain: &[String],
    ) -> Option<String> {
        if self.is_routable(requested) {
            return Some(requested.to_string());
        }
        let effective = self
            .substitution_ladder(requested, request_chain)
            .into_iter()
            .find(|m| self.is_routable(m))?;
        self.note_substitution(requested, &effective);
        Some(effective)
    }

    /// Say, once per distinct pair, that an answer came from another model.
    ///
    /// A substitution is never silent: the owner asked for one model and got
    /// another, and the call log carries the model that actually answered.
    pub(super) fn note_substitution(&self, requested: &str, effective: &str) {
        if requested == effective {
            return;
        }
        if self
            .substitutions_logged
            .insert((requested.to_string(), effective.to_string()))
        {
            tracing::warn!(
                requested = requested,
                effective = effective,
                "Requested model is not available — answering with another model. \
                 Enable its provider, or pin a different model in the agent template \
                 or [orchestrator] model."
            );
        }
    }

    /// Does this provider need an API key? `None` when it is not loaded.
    ///
    /// The one honest answer for "key health" on a local provider: a `false`
    /// here means "no key needed", not "unhealthy" and not "no key configured"
    /// (L1). Callers that render key state ask this before they render a ✗.
    pub fn provider_requires_key(&self, provider_type: &ProviderType) -> Option<bool> {
        self.providers
            .get(provider_type)
            .map(|e| e.value().provider.requires_key())
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
        // Cloned out for the same reason as in `try_model` (R59): no shard lock
        // is held across the await.
        let entry = self.provider_entry(provider)?;
        let pool = entry.key_pool.load();
        Some(pool.key_statuses().await)
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "ollama", feature = "openai"))]
mod ollama_tests;
