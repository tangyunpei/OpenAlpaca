use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    pub timeouts: Option<super::runtime::TimeoutsConfig>,
    pub endpoints: Option<super::runtime::EndpointsConfig>,
    pub env_vars: Option<super::runtime::EnvVarsConfig>,
    pub web_search: Option<WebSearchConfig>,
}

/// Brave Search API configuration for the `web_search` built-in tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSearchConfig {
    /// Brave Search API key. If empty, web_search tool returns a helpful error.
    pub api_key: String,
    /// Request timeout in seconds (default: 15, range: 1–60).
    pub timeout_secs: u64,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            timeout_secs: 15,
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    pub max_cost_per_task: Option<f64>,
    pub max_cost_per_agent: Option<f64>,
}
