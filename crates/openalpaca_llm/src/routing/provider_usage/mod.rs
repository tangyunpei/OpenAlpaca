//! External provider usage tracking.
//!
//! Fetches usage data from provider APIs (opt-in, cached with 5-min TTL).
//! All failures are silent (debug-level logs) and never block LLM calls.

use crate::config::LlmRuntimeConfig;
use crate::keys::credential_discovery::CredentialSource;
use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// External usage data from a provider API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalUsage {
    pub period: String,
    pub cost_usd: f64,
    pub token_count: u64,
    pub rate_limit_remaining: Option<u64>,
    pub fetched_at: String,
    pub approximate: bool,
}

/// Provider-level usage summary combining local and external data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsageSummary {
    pub provider: String,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub total_requests: u64,
    pub health: String,
    pub external_usage: Option<ExternalUsage>,
}

/// Cached usage entry.
struct CachedUsage {
    data: ExternalUsage,
    fetched_at: Instant,
}

impl CachedUsage {
    fn is_stale(&self, ttl: Duration) -> bool {
        self.fetched_at.elapsed() > ttl
    }
}

/// Tracks and caches external provider usage.
pub struct ProviderUsageTracker {
    cache: Arc<RwLock<HashMap<CredentialSource, CachedUsage>>>,
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    client: reqwest::Client,
    runtime_config: Arc<ArcSwap<LlmRuntimeConfig>>,
}

impl Default for ProviderUsageTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderUsageTracker {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
            client: reqwest::Client::new(),
            runtime_config: Arc::new(ArcSwap::from_pointee(LlmRuntimeConfig::default())),
        }
    }

    pub fn with_runtime_config(runtime_config: Arc<ArcSwap<LlmRuntimeConfig>>) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
            client: reqwest::Client::new(),
            runtime_config,
        }
    }

    /// Get usage for a credential source, fetching if stale.
    pub async fn get_usage(&self, source: CredentialSource, token: &str) -> Option<ExternalUsage> {
        let rt = self.runtime_config.load();
        let cache_ttl = Duration::from_secs(rt.timeouts.usage_cache_ttl_secs);
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&source)
                && !cached.is_stale(cache_ttl)
            {
                return Some(cached.data.clone());
            }
        }

        // Fetch fresh data
        let usage = self.fetch_usage(source, token).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(
                source,
                CachedUsage {
                    data: usage.clone(),
                    fetched_at: Instant::now(),
                },
            );
        }

        Some(usage)
    }

    /// Fetch usage from the external provider API.
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    async fn fetch_usage(&self, source: CredentialSource, token: &str) -> Option<ExternalUsage> {
        let result = match source {
            CredentialSource::ClaudeCode => self.fetch_anthropic_usage(token).await,
            CredentialSource::Codex => self.fetch_openai_usage(token).await,
        };

        match result {
            Ok(usage) => Some(usage),
            Err(e) => {
                tracing::debug!("Failed to fetch external usage for {:?}: {}", source, e);
                None
            }
        }
    }

    /// When no HTTP client features are enabled, always return None.
    #[cfg(not(any(feature = "anthropic", feature = "openai", feature = "ollama")))]
    async fn fetch_usage(&self, _source: CredentialSource, _token: &str) -> Option<ExternalUsage> {
        None
    }

    /// Fetch usage from Anthropic API.
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    async fn fetch_anthropic_usage(&self, token: &str) -> Result<ExternalUsage, String> {
        let rt = self.runtime_config.load();
        let fetch_timeout = Duration::from_secs(rt.timeouts.usage_fetch_timeout_secs);
        let url = &rt.endpoints.anthropic_usage;
        let result = tokio::time::timeout(
            fetch_timeout,
            self.client.get(url.as_str()).bearer_auth(token).send(),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                if !response.status().is_success() {
                    return Err(format!(
                        "Anthropic usage API returned {}",
                        response.status()
                    ));
                }

                let body: String = response
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read response: {}", e))?;

                parse_anthropic_usage_response(&body)
            }
            Ok(Err(e)) => Err(format!("HTTP error: {}", e)),
            Err(_) => Err("Timeout fetching Anthropic usage".to_string()),
        }
    }

    /// Fetch usage from OpenAI API.
    #[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
    async fn fetch_openai_usage(&self, token: &str) -> Result<ExternalUsage, String> {
        let rt = self.runtime_config.load();
        let fetch_timeout = Duration::from_secs(rt.timeouts.usage_fetch_timeout_secs);
        let url = &rt.endpoints.openai_usage;
        let result = tokio::time::timeout(
            fetch_timeout,
            self.client.get(url.as_str()).bearer_auth(token).send(),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                if !response.status().is_success() {
                    return Err(format!("OpenAI usage API returned {}", response.status()));
                }

                let body: String = response
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read response: {}", e))?;

                parse_openai_usage_response(&body)
            }
            Ok(Err(e)) => Err(format!("HTTP error: {}", e)),
            Err(_) => Err("Timeout fetching OpenAI usage".to_string()),
        }
    }
}

/// Parse Anthropic usage response.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama", test))]
fn parse_anthropic_usage_response(body: &str) -> Result<ExternalUsage, String> {
    #[derive(Deserialize)]
    struct AnthropicUsage {
        total_cost: Option<f64>,
        total_tokens: Option<u64>,
        period: Option<String>,
    }

    let parsed: AnthropicUsage = serde_json::from_str(body)
        .map_err(|e| format!("Failed to parse Anthropic usage: {}", e))?;

    let now = chrono_like_now();

    Ok(ExternalUsage {
        period: parsed.period.unwrap_or_else(current_period),
        cost_usd: parsed.total_cost.unwrap_or(0.0),
        token_count: parsed.total_tokens.unwrap_or(0),
        rate_limit_remaining: None,
        fetched_at: now,
        approximate: true,
    })
}

/// Parse OpenAI usage response.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama", test))]
fn parse_openai_usage_response(body: &str) -> Result<ExternalUsage, String> {
    #[derive(Deserialize)]
    struct OpenAIUsage {
        total_usage: Option<f64>, // in cents
        total_tokens: Option<u64>,
    }

    let parsed: OpenAIUsage =
        serde_json::from_str(body).map_err(|e| format!("Failed to parse OpenAI usage: {}", e))?;

    let now = chrono_like_now();

    Ok(ExternalUsage {
        period: current_period(),
        cost_usd: parsed.total_usage.map(|c| c / 100.0).unwrap_or(0.0),
        token_count: parsed.total_tokens.unwrap_or(0),
        rate_limit_remaining: None,
        fetched_at: now,
        approximate: true,
    })
}

/// Get current period string (YYYY-MM).
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama", test))]
fn current_period() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Simple date calculation without chrono
    let days_since_epoch = now / 86400;
    let year = 1970 + (days_since_epoch * 400 / 146097); // approximate
    let month = ((now % 31536000) / 2628000) + 1; // approximate month
    format!("{}-{:02}", year, month.min(12))
}

/// Get ISO-8601 timestamp string without chrono.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama", test))]
fn chrono_like_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}Z", now)
}

#[cfg(test)]
mod tests;
