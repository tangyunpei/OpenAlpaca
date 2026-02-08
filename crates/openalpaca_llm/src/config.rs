use crate::error::LlmError;
use crate::key_pool::{ApiKey, KeyPool, KeyPriority, KeySource, ProviderType, SelectionStrategy};
use crate::model_registry::{ModelInfo, ModelRegistry};
use crate::cost_tracker::CostTracker;
use crate::router::{LlmRouter, ProviderEntry};
use crate::LlmProvider;
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
        if let Some(ref key) = self.api_key {
            return Some(key.clone());
        }
        let env_var = match self.provider.as_str() {
            "anthropic" => "ANTHROPIC_API_KEY",
            "openai" => "OPENAI_API_KEY",
            _ => return None,
        };
        std::env::var(env_var).ok()
    }
}

pub fn build_provider(config: &LlmConfig) -> Result<Box<dyn LlmProvider>, LlmError> {
    match config.provider.as_str() {
        #[cfg(feature = "anthropic")]
        "anthropic" => {
            let api_key = config
                .resolve_api_key()
                .ok_or(LlmError::Config("Anthropic API key not configured. Set api_key in config or ANTHROPIC_API_KEY env var.".into()))?;
            let provider = crate::providers::anthropic::AnthropicProvider::new(
                api_key,
                config.model.clone(),
                config.max_tokens,
            );
            Ok(Box::new(provider))
        }
        #[cfg(feature = "openai")]
        "openai" => {
            let api_key = config
                .resolve_api_key()
                .ok_or(LlmError::Config("OpenAI API key not configured. Set api_key in config or OPENAI_API_KEY env var.".into()))?;
            let provider = crate::providers::openai::OpenAiProvider::new(
                api_key,
                config.model.clone(),
                config.base_url.clone(),
                config.max_tokens,
            );
            Ok(Box::new(provider))
        }
        #[cfg(feature = "ollama")]
        "ollama" => {
            let provider = crate::providers::ollama::OllamaProvider::new(
                config.model.clone().unwrap_or_else(|| "llama3".to_string()),
                config.base_url.clone(),
            );
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
    pub credential_discovery: Option<crate::credential_discovery::CredentialDiscoveryConfig>,
    pub cli_backends: Option<crate::cli_backend::CliBackendsConfig>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyConfig {
    pub id: String,
    #[serde(default)]
    pub secret_env: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    pub max_cost_per_task: Option<f64>,
    pub max_cost_per_agent: Option<f64>,
}

fn parse_provider_type(name: &str) -> Option<ProviderType> {
    match name {
        "anthropic" => Some(ProviderType::Anthropic),
        "openai" => Some(ProviderType::OpenAI),
        "ollama" => Some(ProviderType::Ollama),
        _ => None,
    }
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
    let content = std::fs::read_to_string(path)
        .map_err(|e| LlmError::Config(format!("Failed to read {}: {}", path.display(), e)))?;

    // Try parsing as a generic Value to detect format
    let raw: toml::Value = toml::from_str(&content)
        .map_err(|e| LlmError::Config(format!("Failed to parse {}: {}", path.display(), e)))?;

    if raw.get("providers").is_some() {
        build_router_from_hierarchical(&content)
    } else {
        build_router_from_legacy(&content)
    }
}

/// Build router from legacy flat format (single provider).
fn build_router_from_legacy(content: &str) -> Result<LlmRouter, LlmError> {
    let config: LlmConfig = toml::from_str(content)
        .map_err(|e| LlmError::Config(format!("Failed to parse legacy config: {}", e)))?;

    let provider = build_provider(&config)?;
    let provider_type = parse_provider_type(&config.provider)
        .ok_or_else(|| LlmError::UnknownProvider(config.provider.clone()))?;

    let default_model = config.model.unwrap_or_else(|| match provider_type {
        ProviderType::Anthropic => "claude-sonnet-4-5-20250929".to_string(),
        ProviderType::OpenAI => "gpt-5.2".to_string(),
        ProviderType::Ollama => "llama3".to_string(),
    });

    Ok(LlmRouter::single_provider(
        Arc::from(provider),
        provider_type,
        default_model,
    ))
}

/// Build router from new hierarchical format.
#[allow(unused_variables, unused_mut, unreachable_code)]
fn build_router_from_hierarchical(content: &str) -> Result<LlmRouter, LlmError> {
    let config: LlmRouterConfig = toml::from_str(content)
        .map_err(|e| LlmError::Config(format!("Failed to parse router config: {}", e)))?;

    let mut providers_map: HashMap<ProviderType, ProviderEntry> = HashMap::new();

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
                    // Resolve secret: try encrypted first, then env var.
                    // On failure, skip this key so other keys/providers still work.
                    let secret = if let Some(ref encrypted) = key_config.secret_encrypted {
                        if crate::key_encryption::KeyEncryptor::is_encrypted(encrypted) {
                            match crate::key_encryption::KeyEncryptor::load_or_generate() {
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
                    } else if let Some(ref env_var) = key_config.secret_env {
                        let resolved = resolve_key_secret(env_var);
                        if resolved.is_none() {
                            tracing::warn!(
                                "Skipping key '{}': environment variable '{}' not set",
                                key_config.id, env_var
                            );
                        }
                        resolved
                    } else {
                        tracing::warn!(
                            "Skipping key '{}': no secret_encrypted or secret_env configured",
                            key_config.id
                        );
                        None
                    };

                    let secret = match secret {
                        Some(s) => s,
                        None => continue, // Skip this key, try next
                    };

                    let mut api_key = ApiKey::new(
                        key_config.id.clone(),
                        provider_type,
                        secret.clone(),
                    );
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
            let strategy_str = provider_config.key_selection_strategy.as_deref()
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
                    Box::new(crate::providers::anthropic::AnthropicProvider::new(
                        key, None, None,
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
                    Box::new(crate::providers::openai::OpenAiProvider::new(
                        key,
                        None,
                        provider_config.base_url.clone(),
                        None,
                    ))
                }
                #[cfg(feature = "ollama")]
                ProviderType::Ollama => {
                    Box::new(crate::providers::ollama::OllamaProvider::new(
                        "llama3".to_string(),
                        provider_config.base_url.clone(),
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

    // Build model registry
    let model_registry = ModelRegistry::with_defaults();
    if let Some(ref models) = config.models {
        for (model_id, model_config) in models {
            if let Some(pt) = parse_provider_type(&model_config.provider) {
                model_registry.register(
                    model_id.clone(),
                    ModelInfo {
                        provider: pt,
                        input_price_per_million: model_config.input_price.unwrap_or(3.0),
                        output_price_per_million: model_config.output_price.unwrap_or(15.0),
                        context_window: model_config.context.unwrap_or(200_000),
                        discovered: false,
                    },
                );
            }
        }
    }

    // Default model
    let default_model = config
        .orchestrator
        .as_ref()
        .map(|o| o.model.clone())
        .unwrap_or_else(|| "claude-sonnet-4-5-20250929".to_string());

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

    let router = LlmRouter::new(
        providers_map,
        model_registry,
        fallback_chains,
        cost_tracker,
        default_model,
    );

    // Register CLI backends if detected
    let cli_config = config.cli_backends.unwrap_or_default();

    if let Some(ref cc_config) = cli_config.claude_code {
        if let Some(provider) = crate::cli_backend::ClaudeCodeCliProvider::from_config(cc_config) {
            tracing::info!("Registered Claude Code CLI backend at {:?}", provider.binary_path());
            router.register_cli_backend(ProviderType::Anthropic, Arc::new(provider));
        }
    } else if let Some(path) = crate::cli_backend::ClaudeCodeCliProvider::detect() {
        tracing::info!("Auto-detected Claude Code CLI at {:?}", path);
        let provider = crate::cli_backend::ClaudeCodeCliProvider::new(
            path,
            std::time::Duration::from_secs(120),
        );
        router.register_cli_backend(ProviderType::Anthropic, Arc::new(provider));
    }

    if let Some(ref codex_config) = cli_config.codex {
        if let Some(provider) = crate::cli_backend::CodexCliProvider::from_config(codex_config) {
            tracing::info!("Registered Codex CLI backend at {:?}", provider.binary_path());
            router.register_cli_backend(ProviderType::OpenAI, Arc::new(provider));
        }
    } else if let Some(path) = crate::cli_backend::CodexCliProvider::detect() {
        tracing::info!("Auto-detected Codex CLI at {:?}", path);
        let provider = crate::cli_backend::CodexCliProvider::new(
            path,
            std::time::Duration::from_secs(120),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_toml() {
        let toml_str = r#"
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
max_tokens = 4096
"#;
        let config: LlmConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
        assert_eq!(config.max_tokens, Some(4096));
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_resolve_api_key_env() {
        let config = LlmConfig {
            provider: "anthropic".to_string(),
            model: None,
            api_key: None,
            base_url: None,
            max_tokens: None,
        };
        // Without env var set, resolve returns None (unless env is set externally)
        // We just verify the method doesn't panic
        let _ = config.resolve_api_key();
    }

    #[test]
    fn test_resolve_api_key_config_value() {
        let config = LlmConfig {
            provider: "anthropic".to_string(),
            model: None,
            api_key: Some("sk-test-key".to_string()),
            base_url: None,
            max_tokens: None,
        };
        assert_eq!(config.resolve_api_key(), Some("sk-test-key".to_string()));
    }

    #[test]
    fn test_build_provider_unknown() {
        let config = LlmConfig {
            provider: "unknown_provider".to_string(),
            model: None,
            api_key: None,
            base_url: None,
            max_tokens: None,
        };
        let result = build_provider(&config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        match err {
            LlmError::UnknownProvider(name) => assert_eq!(name, "unknown_provider"),
            other => panic!("Expected UnknownProvider, got: {:?}", other),
        }
    }

    #[test]
    fn test_detect_legacy_format() {
        let toml_str = r#"
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
max_tokens = 4096
"#;
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        assert!(raw.get("providers").is_none());
    }

    #[test]
    fn test_detect_hierarchical_format() {
        let toml_str = r#"
[orchestrator]
model = "claude-sonnet-4-5-20250929"

[providers.anthropic]
enabled = true

[[providers.anthropic.keys]]
id = "key1"
secret_env = "ANTHROPIC_API_KEY"
"#;
        let raw: toml::Value = toml::from_str(toml_str).unwrap();
        assert!(raw.get("providers").is_some());

        // Verify it parses as LlmRouterConfig
        let config: LlmRouterConfig = toml::from_str(toml_str).unwrap();
        let key = &config.providers.as_ref().unwrap()["anthropic"]
            .keys.as_ref().unwrap()[0];
        assert_eq!(key.secret_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn test_parse_router_config() {
        let toml_str = r#"
[orchestrator]
model = "claude-sonnet-4-5-20250929"
fallback_models = ["gpt-4o"]

[providers.anthropic]
enabled = true
strategy = "round_robin"

[[providers.anthropic.keys]]
id = "key1"
secret_env = "ANTHROPIC_API_KEY"
tier = "tier1"
monthly_budget = 100.0

[models.custom-model]
provider = "anthropic"
input_price = 5.0
output_price = 25.0
context = 100000

[fallback_chains]
"claude-sonnet-4-5-20250929" = ["gpt-4o"]
"#;
        let config: LlmRouterConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.orchestrator.as_ref().unwrap().model, "claude-sonnet-4-5-20250929");
        assert!(config.providers.as_ref().unwrap().contains_key("anthropic"));
        assert!(config.models.as_ref().unwrap().contains_key("custom-model"));

        let key = &config.providers.as_ref().unwrap()["anthropic"]
            .keys.as_ref().unwrap()[0];
        assert_eq!(key.id, "key1");
        assert_eq!(key.tier.as_deref(), Some("tier1"));
    }

    #[test]
    fn test_parse_provider_type_fn() {
        assert_eq!(parse_provider_type("anthropic"), Some(ProviderType::Anthropic));
        assert_eq!(parse_provider_type("openai"), Some(ProviderType::OpenAI));
        assert_eq!(parse_provider_type("ollama"), Some(ProviderType::Ollama));
        assert_eq!(parse_provider_type("unknown"), None);
    }
}
