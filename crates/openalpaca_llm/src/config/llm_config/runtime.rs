use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::router_config::LlmRouterConfig;

fn default_usage_cache_ttl() -> u64 {
    300
}
fn default_usage_fetch_timeout() -> u64 {
    10
}
fn default_cli_timeout() -> u64 {
    120
}
fn default_llm_request_timeout() -> u64 {
    120
}

/// The range [`LlmRuntimeConfig::request_timeout_for`] will accept: below a
/// second no call can finish, and a day is already past any wall clock a caller
/// has. A value outside it is clamped and said out loud, never taken as meant.
const MIN_REQUEST_TIMEOUT_SECS: u64 = 1;
const MAX_REQUEST_TIMEOUT_SECS: u64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutsConfig {
    #[serde(default = "default_usage_cache_ttl")]
    pub usage_cache_ttl_secs: u64,
    #[serde(default = "default_usage_fetch_timeout")]
    pub usage_fetch_timeout_secs: u64,
    #[serde(default = "default_cli_timeout")]
    pub cli_backend_timeout_secs: u64,
    /// Wall-clock budget for one **non-streaming** LLM HTTP call, in seconds
    /// (default 120 — what the client was hard-coded to). A streamed call is
    /// not bounded by it: the HTTP layer bounds the connect and the idle gap
    /// between chunks, and the wall clock belongs to the loop's
    /// `max_stream_duration` (L7). `[providers.<name>] request_timeout_secs`
    /// overrides it for one provider — a local model generating at 20 tok/s
    /// needs minutes where a cloud call needs seconds.
    #[serde(default = "default_llm_request_timeout")]
    pub llm_request_timeout_secs: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            usage_cache_ttl_secs: default_usage_cache_ttl(),
            usage_fetch_timeout_secs: default_usage_fetch_timeout(),
            cli_backend_timeout_secs: default_cli_timeout(),
            llm_request_timeout_secs: default_llm_request_timeout(),
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
    /// `[providers.<name>] request_timeout_secs` — this provider's own
    /// non-streaming budget. `None` means the `[timeouts]` default applies.
    pub request_timeout_secs: Option<u64>,
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
                request_timeout_secs: None,
            },
        );
        provider_defaults.insert(
            "openai".to_string(),
            ProviderDefaults {
                default_model: "gpt-4o".to_string(),
                default_max_tokens: 4096,
                base_url: Some("https://api.openai.com/v1".to_string()),
                request_timeout_secs: None,
            },
        );
        provider_defaults.insert(
            "ollama".to_string(),
            ProviderDefaults {
                default_model: "llama3".to_string(),
                default_max_tokens: 4096,
                base_url: Some("http://localhost:11434/v1".to_string()),
                request_timeout_secs: None,
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

impl LlmRuntimeConfig {
    /// How long one **non-streaming** call to `provider` may take in total.
    ///
    /// `[providers.<name>] request_timeout_secs` when the file names one, else
    /// `[timeouts] llm_request_timeout_secs`, else 120 s — the value the HTTP
    /// client used to be built with, so a cloud provider that configures
    /// nothing behaves exactly as before (L7). Both provider-construction
    /// paths — the boot builder and the runtime registration a toggle performs
    /// — resolve it here, so they cannot disagree.
    pub fn request_timeout_for(&self, provider: &str) -> std::time::Duration {
        let configured = self
            .provider_defaults
            .get(provider)
            .and_then(|d| d.request_timeout_secs)
            .unwrap_or(self.timeouts.llm_request_timeout_secs);
        let clamped = configured.clamp(MIN_REQUEST_TIMEOUT_SECS, MAX_REQUEST_TIMEOUT_SECS);
        if clamped != configured {
            tracing::warn!(
                provider,
                configured,
                used = clamped,
                "request_timeout_secs is outside {MIN_REQUEST_TIMEOUT_SECS}..={MAX_REQUEST_TIMEOUT_SECS}s — clamped"
            );
        }
        std::time::Duration::from_secs(clamped)
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
                    request_timeout_secs: pc
                        .request_timeout_secs
                        .or_else(|| existing.and_then(|e| e.request_timeout_secs)),
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
