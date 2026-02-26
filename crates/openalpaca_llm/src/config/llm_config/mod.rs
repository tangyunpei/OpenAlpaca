use crate::LlmProvider;
use crate::error::LlmError;
use crate::keys::key_pool::{
    ApiKey, KeyPool, KeyPriority, KeySource, ProviderType, SelectionStrategy,
};
use crate::routing::cost_tracker::CostTracker;
use crate::routing::model_registry::ModelRegistry;
use crate::routing::router::{LlmRouter, ProviderEntry};
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing;

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
}

impl LlmConfig {
    pub fn from_file(path: &std::path::Path) -> Result<Self, LlmError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| LlmError::Config(format!("Failed to read {}: {}", path.display(), e)))?;
        toml::from_str(&content)
            .map_err(|e| LlmError::Config(format!("Failed to parse {}: {}", path.display(), e)))
    }

    pub fn resolve_api_key(&self) -> Option<String> {
        self.resolve_api_key_with_env_config(None)
    }

    pub fn resolve_api_key_with_env_config(
        &self,
        env_config: Option<&EnvVarsConfig>,
    ) -> Option<String> {
        if let Some(ref key) = self.api_key {
            return Some(key.clone());
        }
        let defaults = EnvVarsConfig::default();
        let env_cfg = env_config.unwrap_or(&defaults);
        let env_var = match self.provider.as_str() {
            "anthropic" => &env_cfg.anthropic_api_key,
            "openai" => &env_cfg.openai_api_key,
            _ => return None,
        };
        std::env::var(env_var).ok()
    }
}

pub fn build_provider(config: &LlmConfig) -> Result<Box<dyn LlmProvider>, LlmError> {
    build_provider_with_runtime(config, None)
}

pub fn build_provider_with_runtime(
    config: &LlmConfig,
    runtime: Option<&LlmRuntimeConfig>,
) -> Result<Box<dyn LlmProvider>, LlmError> {
    let defaults = LlmRuntimeConfig::default();
    let rt = runtime.unwrap_or(&defaults);
    let _provider_defaults = rt.provider_defaults.get(config.provider.as_str());

    match config.provider.as_str() {
        #[cfg(feature = "anthropic")]
        "anthropic" => {
            let api_key = config
                .resolve_api_key_with_env_config(Some(&rt.env_vars))
                .ok_or(LlmError::Config("Anthropic API key not configured. Set api_key in config or ANTHROPIC_API_KEY env var.".into()))?;
            let model = config
                .model
                .clone()
                .or_else(|| _provider_defaults.map(|d| d.default_model.clone()));
            let max_tokens = config
                .max_tokens
                .or_else(|| _provider_defaults.map(|d| d.default_max_tokens));
            let provider =
                crate::providers::anthropic::AnthropicProvider::new(api_key, model, max_tokens);
            Ok(Box::new(provider))
        }
        #[cfg(feature = "openai")]
        "openai" => {
            let api_key = config
                .resolve_api_key_with_env_config(Some(&rt.env_vars))
                .ok_or(LlmError::Config(
                "OpenAI API key not configured. Set api_key in config or OPENAI_API_KEY env var."
                    .into(),
            ))?;
            let model = config
                .model
                .clone()
                .or_else(|| _provider_defaults.map(|d| d.default_model.clone()));
            let base_url = config
                .base_url
                .clone()
                .or_else(|| _provider_defaults.and_then(|d| d.base_url.clone()));
            let max_tokens = config
                .max_tokens
                .or_else(|| _provider_defaults.map(|d| d.default_max_tokens));
            let provider =
                crate::providers::openai::OpenAiProvider::new(api_key, model, base_url, max_tokens);
            Ok(Box::new(provider))
        }
        #[cfg(feature = "ollama")]
        "ollama" => {
            let model = config
                .model
                .clone()
                .or_else(|| _provider_defaults.map(|d| d.default_model.clone()))
                .unwrap_or_else(|| "llama3".to_string());
            let base_url = config
                .base_url
                .clone()
                .or_else(|| _provider_defaults.and_then(|d| d.base_url.clone()));
            let provider = crate::providers::ollama::OllamaProvider::new(model, base_url);
            Ok(Box::new(provider))
        }
        other => Err(LlmError::UnknownProvider(other.to_string())),
    }
}

// ── Hierarchical Router Config ────────────────────────────────────────

/// Top-level config for the LLM router (new hierarchical format).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmRouterConfig {
    pub orchestrator: Option<OrchestratorLlmConfig>,
    pub providers: Option<HashMap<String, ProviderConfig>>,
    pub models: Option<HashMap<String, ModelConfigEntry>>,
    pub fallback_chains: Option<HashMap<String, Vec<String>>>,
    pub limits: Option<LimitsConfig>,
    pub rate_limits: Option<crate::routing::rate_limiter::RateLimitConfig>,
    pub credential_discovery: Option<crate::keys::credential_discovery::CredentialDiscoveryConfig>,
    pub cli_backends: Option<crate::cli_backend::CliBackendsConfig>,
    pub embeddings: Option<EmbeddingsConfig>,
    pub security: Option<SecurityConfig>,
    pub timeouts: Option<TimeoutsConfig>,
    pub endpoints: Option<EndpointsConfig>,
    pub env_vars: Option<EnvVarsConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Use OS keychain (macOS Keychain / Windows Credential Manager) for API key storage.
    /// Default: false — uses AES-encrypted local storage (`secret_encrypted`) instead.
    #[serde(default)]
    pub use_keychain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    #[serde(default)]
    pub enabled: bool,
    pub provider: String,
    pub model: Option<String>,
    pub dimensions: Option<u32>,
    pub batch_size: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorLlmConfig {
    pub model: String,
    pub fallback_models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub enabled: Option<bool>,
    pub base_url: Option<String>,
    pub strategy: Option<String>,
    #[serde(alias = "key_selection_strategy")]
    pub key_selection_strategy: Option<String>,
    pub keys: Option<Vec<KeyConfig>>,
    pub default_model: Option<String>,
    pub default_max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyConfig {
    pub id: String,
    #[serde(default)]
    pub secret_env: Option<String>,
    /// OS keychain pointer (e.g. "llm/anthropic/<uuid>").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    /// Legacy encrypted secret (read-only after migration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_encrypted: Option<String>,
    pub tier: Option<String>,
    pub monthly_budget: Option<f64>,
    pub priority: Option<String>,
    pub source: Option<String>,
    pub notes: Option<String>,
    pub rate_limit: Option<u32>,
    pub allowed_models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfigEntry {
    pub provider: String,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub context: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_image: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_document: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    pub max_cost_per_task: Option<f64>,
    pub max_cost_per_agent: Option<f64>,
}

// ── Externalized Config Structs ───────────────────────────────────────

fn default_usage_cache_ttl() -> u64 {
    300
}
fn default_usage_fetch_timeout() -> u64 {
    10
}
fn default_cli_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutsConfig {
    #[serde(default = "default_usage_cache_ttl")]
    pub usage_cache_ttl_secs: u64,
    #[serde(default = "default_usage_fetch_timeout")]
    pub usage_fetch_timeout_secs: u64,
    #[serde(default = "default_cli_timeout")]
    pub cli_backend_timeout_secs: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            usage_cache_ttl_secs: default_usage_cache_ttl(),
            usage_fetch_timeout_secs: default_usage_fetch_timeout(),
            cli_backend_timeout_secs: default_cli_timeout(),
        }
    }
}

fn default_anthropic_usage_url() -> String {
    "https://api.anthropic.com/api/oauth/usage".to_string()
}
fn default_openai_usage_url() -> String {
    "https://api.openai.com/dashboard/billing/usage".to_string()
}
fn default_openai_embeddings_url() -> String {
    "https://api.openai.com/v1/embeddings".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointsConfig {
    #[serde(default = "default_anthropic_usage_url")]
    pub anthropic_usage: String,
    #[serde(default = "default_openai_usage_url")]
    pub openai_usage: String,
    #[serde(default = "default_openai_embeddings_url")]
    pub openai_embeddings: String,
}

impl Default for EndpointsConfig {
    fn default() -> Self {
        Self {
            anthropic_usage: default_anthropic_usage_url(),
            openai_usage: default_openai_usage_url(),
            openai_embeddings: default_openai_embeddings_url(),
        }
    }
}

fn default_anthropic_env() -> String {
    "ANTHROPIC_API_KEY".to_string()
}
fn default_openai_env() -> String {
    "OPENAI_API_KEY".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVarsConfig {
    #[serde(default = "default_anthropic_env")]
    pub anthropic_api_key: String,
    #[serde(default = "default_openai_env")]
    pub openai_api_key: String,
}

impl Default for EnvVarsConfig {
    fn default() -> Self {
        Self {
            anthropic_api_key: default_anthropic_env(),
            openai_api_key: default_openai_env(),
        }
    }
}

/// Per-provider default settings resolved from config.
#[derive(Debug, Clone)]
pub struct ProviderDefaults {
    pub default_model: String,
    pub default_max_tokens: u32,
    pub base_url: Option<String>,
}

/// Runtime representation of all externalized LLM configuration.
/// Wrapped in ArcSwap for lock-free hot-reload.
#[derive(Debug, Clone)]
pub struct LlmRuntimeConfig {
    pub timeouts: TimeoutsConfig,
    pub endpoints: EndpointsConfig,
    pub env_vars: EnvVarsConfig,
    pub provider_defaults: HashMap<String, ProviderDefaults>,
}

impl Default for LlmRuntimeConfig {
    fn default() -> Self {
        let mut provider_defaults = HashMap::new();
        provider_defaults.insert(
            "anthropic".to_string(),
            ProviderDefaults {
                default_model: "claude-sonnet-4-5-20250929".to_string(),
                default_max_tokens: 4096,
                base_url: None,
            },
        );
        provider_defaults.insert(
            "openai".to_string(),
            ProviderDefaults {
                default_model: "gpt-4o".to_string(),
                default_max_tokens: 4096,
                base_url: Some("https://api.openai.com/v1".to_string()),
            },
        );
        provider_defaults.insert(
            "ollama".to_string(),
            ProviderDefaults {
                default_model: "llama3".to_string(),
                default_max_tokens: 4096,
                base_url: Some("http://localhost:11434/v1".to_string()),
            },
        );
        Self {
            timeouts: TimeoutsConfig::default(),
            endpoints: EndpointsConfig::default(),
            env_vars: EnvVarsConfig::default(),
            provider_defaults,
        }
    }
}

impl From<&LlmRouterConfig> for LlmRuntimeConfig {
    fn from(config: &LlmRouterConfig) -> Self {
        let timeouts = config.timeouts.clone().unwrap_or_default();
        let endpoints = config.endpoints.clone().unwrap_or_default();
        let env_vars = config.env_vars.clone().unwrap_or_default();

        let mut provider_defaults = LlmRuntimeConfig::default().provider_defaults;

        if let Some(ref providers) = config.providers {
            for (name, pc) in providers {
                let existing = provider_defaults.get(name);
                let defaults = ProviderDefaults {
                    default_model: pc
                        .default_model
                        .clone()
                        .or_else(|| existing.map(|e| e.default_model.clone()))
                        .unwrap_or_else(|| "unknown".to_string()),
                    default_max_tokens: pc
                        .default_max_tokens
                        .or_else(|| existing.map(|e| e.default_max_tokens))
                        .unwrap_or(4096),
                    base_url: pc
                        .base_url
                        .clone()
                        .or_else(|| existing.and_then(|e| e.base_url.clone())),
                };
                provider_defaults.insert(name.clone(), defaults);
            }
        }

        Self {
            timeouts,
            endpoints,
            env_vars,
            provider_defaults,
        }
    }
}

pub(crate) fn parse_provider_type(name: &str) -> Option<ProviderType> {
    match name {
        "anthropic" => Some(ProviderType::Anthropic),
        "openai" => Some(ProviderType::OpenAI),
        "ollama" => Some(ProviderType::Ollama),
        _ => None,
    }
}

/// Public version of `parse_provider_type` for use by other modules.
pub fn parse_provider_type_pub(name: &str) -> Option<ProviderType> {
    parse_provider_type(name)
}

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
                        ApiKey::new(key_config.id.clone(), provider_type, secret.clone());
                    api_key.tier = key_config.tier.clone();
                    api_key.monthly_budget = key_config.monthly_budget;
                    api_key.rate_limit = key_config.rate_limit;
                    if let Some(ref models) = key_config.allowed_models {
                        api_key.allowed_models = models.clone();
                    }

                    // Parse priority
                    api_key.priority = match key_config.priority.as_deref() {
                        Some("fallback") => KeyPriority::Fallback,
                        _ => KeyPriority::Primary,
                    };

                    // Parse source
                    api_key.source = match key_config.source.as_deref() {
                        Some("api_console") => KeySource::ApiConsole,
                        Some("claude_code") => KeySource::ClaudeCode,
                        Some("claude_max_pro") => KeySource::ClaudeMaxPro,
                        Some("codex") => KeySource::Codex,
                        Some("environment") => KeySource::Environment,
                        _ => KeySource::Other,
                    };

                    api_key.notes = key_config.notes.clone();
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
            let provider: Box<dyn LlmProvider> = match provider_type {
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

/// Read a hierarchical LLM config from a TOML file.
pub fn read_config(path: &std::path::Path) -> Result<LlmRouterConfig, LlmError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| LlmError::Config(format!("Failed to read {}: {}", path.display(), e)))?;
    toml::from_str(&content)
        .map_err(|e| LlmError::Config(format!("Failed to parse {}: {}", path.display(), e)))
}

/// Write a hierarchical LLM config to a TOML file.
pub fn write_config(path: &std::path::Path, config: &LlmRouterConfig) -> Result<(), LlmError> {
    let content = toml::to_string_pretty(config)
        .map_err(|e| LlmError::Config(format!("Failed to serialize config: {}", e)))?;
    std::fs::write(path, content)
        .map_err(|e| LlmError::Config(format!("Failed to write {}: {}", path.display(), e)))?;
    Ok(())
}

/// Resolve a secret from a `KeyConfig` using the given secret store.
///
/// Resolution order: `secret_env` > `secret_ref` (keychain) > `secret_encrypted` (legacy).
pub fn resolve_key_from_config(
    key_config: &KeyConfig,
    secret_store: Option<&dyn crate::keys::secret_store::SecretStore>,
) -> Option<String> {
    if let Some(ref env_var) = key_config.secret_env {
        return resolve_key_secret(env_var);
    }
    if let Some(ref sref) = key_config.secret_ref
        && let Some(store) = secret_store
        && let Ok(Some(s)) = store.get(sref)
    {
        return Some(s);
    }
    if let Some(ref encrypted) = key_config.secret_encrypted {
        if crate::keys::key_encryption::KeyEncryptor::is_encrypted(encrypted) {
            if let Ok(enc) = crate::keys::key_encryption::KeyEncryptor::load_or_generate() {
                return enc.decrypt(encrypted).ok();
            }
        } else {
            return Some(encrypted.clone());
        }
    }
    None
}

/// Auto-migrate `secret_encrypted` → OS keychain (`secret_ref`).
///
/// For each key with `secret_encrypted`, decrypts the secret and stores it
/// in the OS keychain via `SecretStore`, then replaces `secret_encrypted`
/// with `secret_ref` in the config. Writes the updated config to disk.
///
/// Returns the number of secrets migrated.
pub fn migrate_llm_secrets(
    config_path: &std::path::Path,
    secret_store: &dyn crate::keys::secret_store::SecretStore,
) -> Result<u32, String> {
    // Check path is writable
    if config_path.exists() {
        let metadata = std::fs::metadata(config_path)
            .map_err(|e| format!("Cannot stat {}: {e}", config_path.display()))?;
        if metadata.permissions().readonly() {
            tracing::warn!(
                "Config at {} is read-only; skipping secret migration",
                config_path.display()
            );
            return Ok(0);
        }
    } else {
        return Ok(0);
    }

    let mut config = read_config(config_path)
        .map_err(|e| format!("Failed to read config for migration: {e}"))?;

    let encryptor = match crate::keys::key_encryption::KeyEncryptor::load_or_generate() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Cannot load master key for migration: {e}");
            return Ok(0);
        }
    };

    let mut migrated = 0u32;

    if let Some(ref mut providers) = config.providers {
        for (provider_name, provider) in providers.iter_mut() {
            if let Some(ref mut keys) = provider.keys {
                for key in keys.iter_mut() {
                    // Skip if already has secret_ref or no secret_encrypted
                    if key.secret_ref.is_some() {
                        continue;
                    }
                    let Some(ref encrypted) = key.secret_encrypted else {
                        continue;
                    };
                    if !crate::keys::key_encryption::KeyEncryptor::is_encrypted(encrypted) {
                        continue;
                    }

                    let plaintext = match encryptor.decrypt(encrypted) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(
                                "Migration: cannot decrypt key '{}' for {}: {e}",
                                key.id,
                                provider_name
                            );
                            continue;
                        }
                    };

                    let sref = format!("llm/{}/{}", provider_name, uuid::Uuid::new_v4());

                    if let Err(e) = secret_store.set(&sref, &plaintext) {
                        tracing::warn!("Migration: cannot store key '{}' in keychain: {e}", key.id);
                        continue;
                    }

                    key.secret_ref = Some(sref);
                    key.secret_encrypted = None;
                    migrated += 1;
                }
            }
        }
    }

    if migrated > 0 {
        let _lock = crate::keys::key_encryption::acquire_config_write_lock()
            .map_err(|e| format!("Failed to acquire config lock for migration: {e}"))?;
        write_config(config_path, &config)
            .map_err(|e| format!("Failed to write migrated config: {e}"))?;
        tracing::info!("Migrated {migrated} secret(s) from llm.toml to OS keychain");
    }

    Ok(migrated)
}

/// Reverse-migrate `secret_ref` keys back to `secret_encrypted`.
///
/// For each key that has `secret_ref` (but no `secret_encrypted`), reads
/// the plaintext from the keychain, encrypts it locally, writes
/// `secret_encrypted`, and clears `secret_ref`.
///
/// Used when switching from keychain storage to local encrypted storage
/// (`[security] use_keychain = false`).
pub fn reverse_migrate_llm_secrets(
    config_path: &std::path::Path,
    secret_store: &dyn crate::keys::secret_store::SecretStore,
) -> Result<u32, String> {
    if !config_path.exists() {
        return Ok(0);
    }
    if let Ok(metadata) = std::fs::metadata(config_path)
        && metadata.permissions().readonly()
    {
        return Ok(0);
    }

    let mut config = read_config(config_path)
        .map_err(|e| format!("Failed to read config for reverse migration: {e}"))?;

    let encryptor = match crate::keys::key_encryption::KeyEncryptor::load_or_generate() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Cannot load master key for reverse migration: {e}");
            return Ok(0);
        }
    };

    let mut migrated = 0u32;

    if let Some(ref mut providers) = config.providers {
        for (provider_name, provider) in providers.iter_mut() {
            if let Some(ref mut keys) = provider.keys {
                for key in keys.iter_mut() {
                    // Skip if no secret_ref or already has secret_encrypted
                    let Some(ref sref) = key.secret_ref else {
                        continue;
                    };
                    if key.secret_encrypted.is_some() {
                        continue;
                    }

                    // Read plaintext from keychain
                    let plaintext = match secret_store.get(sref) {
                        Ok(Some(p)) => p,
                        Ok(None) => {
                            tracing::warn!(
                                "Reverse migration: secret_ref '{}' for key '{}' ({}) not found in keychain, skipping",
                                sref,
                                key.id,
                                provider_name
                            );
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Reverse migration: cannot read '{}' from keychain for key '{}': {e}",
                                sref,
                                key.id
                            );
                            continue;
                        }
                    };

                    // Encrypt locally
                    let encrypted = match encryptor.encrypt(&plaintext) {
                        Ok(enc) => enc,
                        Err(e) => {
                            tracing::warn!("Failed to encrypt key '{}': {e}", key.id);
                            continue;
                        }
                    };
                    key.secret_encrypted = Some(encrypted);
                    key.secret_ref = None;
                    migrated += 1;
                }
            }
        }
    }

    if migrated > 0 {
        let _lock = crate::keys::key_encryption::acquire_config_write_lock()
            .map_err(|e| format!("Failed to acquire config lock for reverse migration: {e}"))?;
        write_config(config_path, &config)
            .map_err(|e| format!("Failed to write reverse-migrated config: {e}"))?;
        tracing::info!(
            "Reverse-migrated {migrated} secret(s) from OS keychain to local encrypted storage"
        );
    }

    Ok(migrated)
}

/// Collect all unique `secret_ref` values from the config.
///
/// Used at startup to pre-fetch every keychain key in a single batch,
/// so that all macOS Keychain password prompts happen at once.
pub fn collect_secret_refs(config: &LlmRouterConfig) -> Vec<String> {
    let mut refs = Vec::new();
    if let Some(ref providers) = config.providers {
        for provider in providers.values() {
            if let Some(ref keys) = provider.keys {
                for key in keys {
                    if let Some(ref sref) = key.secret_ref
                        && !refs.contains(sref)
                    {
                        refs.push(sref.clone());
                    }
                }
            }
        }
    }
    refs
}

#[cfg(test)]
mod tests;
