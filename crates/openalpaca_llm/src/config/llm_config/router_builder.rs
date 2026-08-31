use crate::LlmProvider;
use crate::error::LlmError;
use crate::keys::key_pool::{
    ApiKey, KeyPool, ProviderType, SelectionStrategy,
};
use crate::routing::cost_tracker::CostTracker;
use crate::routing::model_registry::ModelRegistry;
use crate::routing::router::{LlmRouter, ProviderEntry};
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;
use tracing;

use super::{LlmConfig, build_provider_with_runtime, parse_provider_type};
use super::router_config::LlmRouterConfig;
use super::runtime::LlmRuntimeConfig;

fn resolve_key_secret(env_var: &str) -> Option<String> {
    std::env::var(env_var).ok()
}

/// Build an LlmRouter from a config file path.
///
/// Auto-detects format:
/// - If `providers` key is present → new hierarchical format
/// - Otherwise → legacy flat format (wraps in single-provider router)
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

    // Try parsing as a generic Value to detect format
    let raw: toml::Value = toml::from_str(&content)
        .map_err(|e| LlmError::Config(format!("Failed to parse {}: {}", path.display(), e)))?;

    if raw.get("providers").is_some() {
        build_router_from_hierarchical(&content, secret_store)
    } else {
        build_router_from_legacy(&content)
    }
}

/// Build router from legacy flat format (single provider).
fn build_router_from_legacy(content: &str) -> Result<LlmRouter, LlmError> {
    let config: LlmConfig = toml::from_str(content)
        .map_err(|e| LlmError::Config(format!("Failed to parse legacy config: {}", e)))?;

    let runtime = LlmRuntimeConfig::default();
    let provider = build_provider_with_runtime(&config, Some(&runtime))?;
    let provider_type = parse_provider_type(&config.provider)
        .ok_or_else(|| LlmError::UnknownProvider(config.provider.clone()))?;

    let default_model = config.model.unwrap_or_else(|| {
        runtime
            .provider_defaults
            .get(config.provider.as_str())
            .map(|d| d.default_model.clone())
            .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string())
    });

    Ok(LlmRouter::single_provider(
        Arc::from(provider),
        provider_type,
        default_model,
    ))
}

/// Build router from new hierarchical format.
#[allow(unused_variables, unused_mut, unreachable_code)]
fn build_router_from_hierarchical(
    content: &str,
    secret_store: Option<&dyn crate::keys::secret_store::SecretStore>,
) -> Result<LlmRouter, LlmError> {
    let config: LlmRouterConfig = toml::from_str(content)
        .map_err(|e| LlmError::Config(format!("Failed to parse router config: {}", e)))?;

    let runtime_config = LlmRuntimeConfig::from(&config);
    let mut providers_map: HashMap<ProviderType, ProviderEntry> = HashMap::new();

    // Shared HTTP client for all providers (connection pool reuse).
    // Only built when at least one provider feature is enabled (requires reqwest).
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    let shared_client = reqwest::Client::builder()
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Build provider entries
    if let Some(ref providers) = config.providers {
        for (provider_name, provider_config) in providers {
            if provider_config.enabled == Some(false) {
                continue;
            }

            let provider_type = parse_provider_type(provider_name)
                .ok_or_else(|| LlmError::UnknownProvider(provider_name.clone()))?;

            // Collect keys
            let mut api_keys = Vec::new();
            if let Some(ref keys) = provider_config.keys {
                for key_config in keys {
                    // Resolution order: secret_env > secret_ref > secret_encrypted
                    let secret = if let Some(ref env_var) = key_config.secret_env {
                        // 1. Environment variable (highest priority, explicit)
                        let resolved = resolve_key_secret(env_var);
                        if resolved.is_none() {
                            tracing::warn!(
                                "Skipping key '{}': environment variable '{}' not set",
                                key_config.id,
                                env_var
                            );
                        }
                        resolved
                    } else if let Some(ref sref) = key_config.secret_ref {
                        // 2. OS keychain via secret_ref
                        if let Some(store) = secret_store {
                            match store.get(sref) {
                                Ok(Some(s)) => Some(s),
                                Ok(None) => {
                                    tracing::warn!(
                                        "Skipping key '{}': secret_ref '{}' not found in keychain",
                                        key_config.id,
                                        sref
                                    );
                                    None
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Skipping key '{}': keychain error: {e}",
                                        key_config.id
                                    );
                                    None
                                }
                            }
                        } else {
                            tracing::warn!(
                                "Skipping key '{}': secret_ref set but no secret store available",
                                key_config.id
                            );
                            None
                        }
                    } else if let Some(ref encrypted) = key_config.secret_encrypted {
                        // 3. Legacy encrypted (read-only, for pre-migration compat)
                        if crate::keys::key_encryption::KeyEncryptor::is_encrypted(encrypted) {
                            match crate::keys::key_encryption::KeyEncryptor::load_or_generate() {
                                Ok(enc) => match enc.decrypt(encrypted) {
                                    Ok(s) => Some(s),
                                    Err(e) => {
                                        tracing::warn!(
                                            "Skipping key '{}': decryption failed: {e}. \
                                             Re-add the key via Settings to fix.",
                                            key_config.id
                                        );
                                        None
                                    }
                                },
                                Err(e) => {
                                    tracing::warn!(
                                        "Skipping key '{}': failed to load master key: {e}",
                                        key_config.id
                                    );
                                    None
                                }
                            }
                        } else {
                            Some(encrypted.clone())
                        }
                    } else {
                        tracing::warn!("Skipping key '{}': no secret configured", key_config.id);
                        None
                    };

                    let secret = match secret {
                        Some(s) => s,
                        None => continue, // Skip this key, try next
                    };

                    let mut api_key =
                        ApiKey::new(key_config.id.clone(), provider_type.clone(), secret.clone());
                    crate::config::key_pool_builder::apply_key_config_metadata(
                        &mut api_key,
                        key_config,
                    );
                    api_keys.push(api_key);
                }
            }

            // Parse strategy from either field
            let strategy_str = provider_config
                .key_selection_strategy
                .as_deref()
                .or(provider_config.strategy.as_deref());
            let strategy = match strategy_str {
                Some("lru") | Some("least_recently_used") => SelectionStrategy::LeastRecentlyUsed,
                Some("primary_fallback") => SelectionStrategy::PrimaryFallback,
                _ => SelectionStrategy::RoundRobin,
            };

            // Use first successfully resolved key for the provider's default instance
            let first_secret = api_keys.first().map(|k| k.secret.clone());

            let key_pool = KeyPool::new(api_keys, strategy);

            // Build the actual provider.
            // Skip providers that require keys but have none resolved.
            let prov_defaults = runtime_config.provider_defaults.get(provider_name);
            let provider: Box<dyn LlmProvider> = match &provider_type {
                #[cfg(feature = "anthropic")]
                ProviderType::Anthropic => {
                    let Some(key) = first_secret else {
                        tracing::warn!(
                            "Skipping Anthropic provider: no valid API keys. \
                             Re-add your key via Settings to fix."
                        );
                        continue;
                    };
                    let model = provider_config
                        .default_model
                        .clone()
                        .or_else(|| prov_defaults.map(|d| d.default_model.clone()));
                    let max_tokens = provider_config
                        .default_max_tokens
                        .or_else(|| prov_defaults.map(|d| d.default_max_tokens));
                    Box::new(crate::providers::anthropic::AnthropicProvider::with_client(
                        shared_client.clone(),
                        key,
                        model,
                        max_tokens,
                    ))
                }
                #[cfg(feature = "openai")]
                ProviderType::OpenAI => {
                    let Some(key) = first_secret else {
                        tracing::warn!(
                            "Skipping OpenAI provider: no valid API keys. \
                             Re-add your key via Settings to fix."
                        );
                        continue;
                    };
                    let model = provider_config
                        .default_model
                        .clone()
                        .or_else(|| prov_defaults.map(|d| d.default_model.clone()));
                    let base_url = provider_config
                        .base_url
                        .clone()
                        .or_else(|| prov_defaults.and_then(|d| d.base_url.clone()));
                    let max_tokens = provider_config
                        .default_max_tokens
                        .or_else(|| prov_defaults.map(|d| d.default_max_tokens));
                    Box::new(crate::providers::openai::OpenAiProvider::with_client(
                        shared_client.clone(),
                        key,
                        model,
                        base_url,
                        max_tokens,
                    ))
                }
                #[cfg(feature = "ollama")]
                ProviderType::Ollama => {
                    let model = provider_config
                        .default_model
                        .clone()
                        .or_else(|| prov_defaults.map(|d| d.default_model.clone()))
                        .unwrap_or_else(|| "llama3".to_string());
                    let base_url = provider_config
                        .base_url
                        .clone()
                        .or_else(|| prov_defaults.and_then(|d| d.base_url.clone()));
                    Box::new(crate::providers::ollama::OllamaProvider::with_client(
                        shared_client.clone(),
                        model,
                        base_url,
                    ))
                }
                #[allow(unreachable_patterns)]
                _ => {
                    return Err(LlmError::UnknownProvider(provider_name.clone()));
                }
            };

            providers_map.insert(
                provider_type,
                ProviderEntry {
                    provider: Arc::from(provider),
                    key_pool: Arc::new(ArcSwap::from_pointee(key_pool)),
                },
            );
        }
    }

    // Build model registry (config models override compiled defaults)
    let model_registry = if let Some(ref models) = config.models {
        ModelRegistry::with_defaults_and_config(models)
    } else {
        ModelRegistry::with_defaults()
    };

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
