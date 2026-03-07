mod migration;
mod router_builder;
pub mod router_config;
pub mod runtime;

// Re-export public API (unchanged from before the split)
pub use migration::{
    collect_secret_refs, migrate_llm_secrets, resolve_key_from_config, reverse_migrate_llm_secrets,
};
pub use router_builder::{build_router, build_router_with_secret_store};
pub use router_config::{
    EmbeddingsConfig, KeyConfig, LimitsConfig, LlmRouterConfig, ModelConfigEntry,
    OrchestratorLlmConfig, ProviderConfig, SecurityConfig, WebSearchConfig,
};
pub use runtime::{
    EndpointsConfig, EnvVarsConfig, LlmRuntimeConfig, ProviderDefaults, TimeoutsConfig,
};

use crate::LlmProvider;
use crate::error::LlmError;
use crate::keys::key_pool::ProviderType;
use runtime::EnvVarsConfig as EnvVarsCfg;
use serde::Deserialize;

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
        let defaults = EnvVarsCfg::default();
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
mod tests;
