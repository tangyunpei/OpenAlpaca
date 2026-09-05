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
    EmbeddingsConfig, KeyConfig, LlmRouterConfig, ModelConfigEntry,
    OrchestratorLlmConfig, ProviderConfig, SecurityConfig, WebSearchConfig,
};
pub use runtime::{
    EndpointsConfig, EnvVarsConfig, LlmRuntimeConfig, ProviderDefaults, TimeoutsConfig,
};

use crate::error::LlmError;
use crate::keys::key_pool::ProviderType;

pub(crate) fn parse_provider_type(name: &str) -> Option<ProviderType> {
    match name {
        "anthropic" => Some(ProviderType::Anthropic),
        "openai" => Some(ProviderType::OpenAI),
        "ollama" => Some(ProviderType::Ollama),
        other if other.starts_with("plugin:") => {
            Some(ProviderType::Plugin(other.strip_prefix("plugin:").unwrap().to_string()))
        }
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
