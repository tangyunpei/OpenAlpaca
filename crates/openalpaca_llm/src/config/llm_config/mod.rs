mod edit;
mod migration;
mod router_builder;
pub mod router_config;
pub mod runtime;

// Re-export public API (unchanged from before the split)
pub use edit::render_config_preserving;
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

/// The providers `llm.toml` says are off.
///
/// One disable has to reach three places, or the daemon holds three different
/// answers for one on-disk state (R58): the router does not build the provider,
/// the model registry takes neither its `[models]` rows nor its compiled
/// defaults, and the CLI-backend fallback will not stand in for it. This is the
/// single reading of the bit that all three use.
pub fn disabled_providers(config: &LlmRouterConfig) -> std::collections::HashSet<ProviderType> {
    config
        .providers
        .iter()
        .flatten()
        .filter(|(_, pc)| pc.enabled == Some(false))
        .filter_map(|(name, _)| parse_provider_type(name))
        .collect()
}

/// Read a hierarchical LLM config from a TOML file.
pub fn read_config(path: &std::path::Path) -> Result<LlmRouterConfig, LlmError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| LlmError::Config(format!("Failed to read {}: {}", path.display(), e)))?;
    toml::from_str(&content)
        .map_err(|e| LlmError::Config(format!("Failed to parse {}: {}", path.display(), e)))
}

/// Render a config as TOML text, **deterministically**.
///
/// Serializing the struct directly walks its `HashMap`s, whose iteration order
/// differs between instances, so two writes of the same content can reorder
/// half the file. `toml::Value`'s tables are ordered, so going through one
/// makes a rewrite that changes a single key change a single key — which is
/// what a hand-edited file deserves (plan §1.4).
pub fn render_config(config: &LlmRouterConfig) -> Result<String, LlmError> {
    let value = toml::Value::try_from(config)
        .map_err(|e| LlmError::Config(format!("Failed to serialize config: {}", e)))?;
    toml::to_string_pretty(&value)
        .map_err(|e| LlmError::Config(format!("Failed to serialize config: {}", e)))
}

/// Read a config together with the text it came from.
///
/// The text is what a comment-preserving write edits, and the parse is what the
/// caller mutates; taking both from one read keeps them describing the same
/// bytes (M3).
pub fn read_config_with_text(
    path: &std::path::Path,
) -> Result<(String, LlmRouterConfig), LlmError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| LlmError::Config(format!("Failed to read {}: {}", path.display(), e)))?;
    let config = toml::from_str(&content)
        .map_err(|e| LlmError::Config(format!("Failed to parse {}: {}", path.display(), e)))?;
    Ok((content, config))
}

/// Write a hierarchical LLM config to a TOML file.
///
/// An existing file is **edited**, not re-serialised: only the keys that differ
/// from what is on disk are rewritten, so comments, blank lines, key order and
/// keys these types do not model survive the write (M3). A file that does not
/// exist yet — or one this crate cannot parse, where there is no "before" to
/// diff against — is written from a full render as it always was.
pub fn write_config(path: &std::path::Path, config: &LlmRouterConfig) -> Result<(), LlmError> {
    let rendered = match read_config_with_text(path) {
        Ok((existing, before)) => render_config_preserving(&existing, &before, config)?,
        Err(_) => render_config(config)?,
    };
    std::fs::write(path, rendered)
        .map_err(|e| LlmError::Config(format!("Failed to write {}: {}", path.display(), e)))?;
    Ok(())
}

#[cfg(test)]
mod tests;
