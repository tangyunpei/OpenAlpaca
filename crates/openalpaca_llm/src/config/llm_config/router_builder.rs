use crate::config::key_pool_builder::{KeyResolver, selection_strategy};
use crate::config::provider_builder::{ProviderBuildError, build_provider};
use crate::error::LlmError;
use crate::keys::key_pool::{KeyPool, ProviderType};
use crate::routing::cost_tracker::CostTracker;
use crate::routing::model_registry::ModelRegistry;
use crate::routing::router::{LlmRouter, ProviderEntry};
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;
use tracing;

use super::parse_provider_type;
use super::router_config::LlmRouterConfig;
use super::runtime::LlmRuntimeConfig;

/// Build an LlmRouter from a config file path.
pub fn build_router(path: &std::path::Path) -> Result<LlmRouter, LlmError> {
    build_router_with_secret_store(path, None)
}

/// Build an LlmRouter with an optional OS secret store for `secret_ref` resolution.
pub fn build_router_with_secret_store(
    path: &std::path::Path,
    secret_store: Option<&dyn crate::keys::secret_store::SecretStore>,
) -> Result<LlmRouter, LlmError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| LlmError::Config(format!("Failed to read {}: {}", path.display(), e)))?;

    build_router_from_hierarchical(&content, secret_store)
}

/// Build router from the hierarchical config format.
#[allow(unused_variables, unused_mut, unreachable_code)]
fn build_router_from_hierarchical(
    content: &str,
    secret_store: Option<&dyn crate::keys::secret_store::SecretStore>,
) -> Result<LlmRouter, LlmError> {
    let config: LlmRouterConfig = toml::from_str(content)
        .map_err(|e| LlmError::Config(format!("Failed to parse router config: {}", e)))?;

    let runtime_config = LlmRuntimeConfig::from(&config);
    // Read the ENABLE bit once, before anything is moved out of `config`: the
    // provider loop below skips a disabled provider, and so must the catalogue
    // (R58b).
    let disabled = super::disabled_providers(&config);
    let mut providers_map: HashMap<ProviderType, ProviderEntry> = HashMap::new();

    // Build provider entries
    if let Some(ref providers) = config.providers {
        for (provider_name, provider_config) in providers {
            if provider_config.enabled == Some(false) {
                continue;
            }

            let provider_type = parse_provider_type(provider_name)
                .ok_or_else(|| LlmError::UnknownProvider(provider_name.clone()))?;

            let mut resolver = KeyResolver::new(secret_store, None);
            let mut api_keys = Vec::new();
            for key_config in provider_config.keys.iter().flatten() {
                match resolver.resolve(key_config, &provider_type) {
                    Ok(key) => api_keys.push(key),
                    Err(error) => {
                        tracing::warn!(key_id = %key_config.id, %error, "Skipping invalid provider key; re-add it via Settings")
                    }
                }
            }
            let first_secret = api_keys.first().map(|k| k.secret.clone());
            let key_pool = KeyPool::new(api_keys, selection_strategy(Some(provider_config)));
            let provider = match build_provider(
                &provider_type,
                runtime_config
                    .provider_defaults
                    .get(provider_name)
                    .and_then(|d| d.base_url.clone()),
                &runtime_config,
                first_secret,
            ) {
                Ok(provider) => provider,
                #[cfg(any(feature = "anthropic", feature = "openai"))]
                Err(ProviderBuildError::MissingKey) => {
                    tracing::warn!(%provider_name, "Skipping provider: no valid API keys. Re-add your key via Settings to fix.");
                    continue;
                }
                Err(ProviderBuildError::Unavailable) => {
                    return Err(LlmError::UnknownProvider(provider_name.clone()));
                }
            };

            providers_map.insert(
                provider_type,
                ProviderEntry {
                    provider,
                    key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
                },
            );
        }
    }

    // Build model registry (config models override compiled defaults, and a
    // disabled provider contributes neither).
    let no_models = HashMap::new();
    let model_registry = ModelRegistry::with_defaults_and_config(
        config.models.as_ref().unwrap_or(&no_models),
        &disabled,
    );

    // Default model
    let default_model = config
        .orchestrator
        .as_ref()
        .map(|o| o.model.clone())
        .unwrap_or_else(|| {
            runtime_config
                .provider_defaults
                .get("anthropic")
                .map(|d| d.default_model.clone())
                .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string())
        });

    // Fallback chains
    let mut fallback_chains = config.fallback_chains.unwrap_or_default();
    if let Some(ref orch) = config.orchestrator
        && let Some(ref fallbacks) = orch.fallback_models
    {
        fallback_chains
            .entry(orch.model.clone())
            .or_insert_with(|| fallbacks.clone());
    }

    // The registry handed in here is a placeholder: every `LlmRouter`
    // constructor replaces it with the router's own, so pricing reads the same
    // catalogue routing does — `[models]` rows and discovered models included
    // (L8, `CostTracker::use_registry`).
    let cost_tracker = Arc::new(CostTracker::new(ModelRegistry::with_defaults()));
    let rate_limit_config = config.rate_limits.unwrap_or_default();

    let router = LlmRouter::new_with_runtime(
        providers_map,
        model_registry,
        fallback_chains,
        cost_tracker,
        default_model,
        runtime_config,
        rate_limit_config,
    );

    // Register CLI backends if detected
    let cli_config = config.cli_backends.unwrap_or_default();
    let cli_timeout_secs = router.runtime_config().timeouts.cli_backend_timeout_secs;

    if let Some(ref cc_config) = cli_config.claude_code {
        if let Some(provider) = crate::cli_backend::ClaudeCodeCliProvider::from_config(cc_config) {
            tracing::info!(
                "Registered Claude Code CLI backend at {:?}",
                provider.binary_path()
            );
            router.register_cli_backend(ProviderType::Anthropic, Arc::new(provider));
        }
    } else if let Some(path) = crate::cli_backend::ClaudeCodeCliProvider::detect() {
        tracing::info!("Auto-detected Claude Code CLI at {:?}", path);
        let provider = crate::cli_backend::ClaudeCodeCliProvider::new(
            path,
            std::time::Duration::from_secs(cli_timeout_secs),
        );
        router.register_cli_backend(ProviderType::Anthropic, Arc::new(provider));
    }

    if let Some(ref codex_config) = cli_config.codex {
        if let Some(provider) = crate::cli_backend::CodexCliProvider::from_config(codex_config) {
            tracing::info!(
                "Registered Codex CLI backend at {:?}",
                provider.binary_path()
            );
            router.register_cli_backend(ProviderType::OpenAI, Arc::new(provider));
        }
    } else if let Some(path) = crate::cli_backend::CodexCliProvider::detect() {
        tracing::info!("Auto-detected Codex CLI at {:?}", path);
        let provider = crate::cli_backend::CodexCliProvider::new(
            path,
            std::time::Duration::from_secs(cli_timeout_secs),
        );
        router.register_cli_backend(ProviderType::OpenAI, Arc::new(provider));
    }

    Ok(router)
}
