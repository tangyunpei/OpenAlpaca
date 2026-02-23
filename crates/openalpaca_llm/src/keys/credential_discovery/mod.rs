//! OAuth credential discovery and token management.
//!
//! Reads OAuth credentials from well-known paths for Claude Code and Codex CLI,
//! manages token refresh, and injects merged key pools into the router.

use crate::keys::key_pool::{ApiKey, KeyPool, KeyPriority, KeySource, ProviderType, SelectionStrategy};
use crate::routing::router::LlmRouter;
use crate::config::settings_service::LlmSettingsService;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing;

/// An OAuth token read from a credential file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    #[serde(alias = "accessToken")]
    pub access_token: String,
    #[serde(alias = "refreshToken")]
    pub refresh_token: Option<String>,
    #[serde(alias = "expiresAt")]
    pub expires_at: Option<i64>,
}

impl OAuthToken {
    /// Returns true if the token is expired (less than 60s remaining).
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                exp - now < 60
            }
            None => false, // No expiry info — assume valid
        }
    }
}

/// Source of a discovered credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    ClaudeCode,
    Codex,
}

/// A credential discovered from a local CLI tool.
#[derive(Debug, Clone)]
pub struct DiscoveredCredential {
    pub source: CredentialSource,
    pub token: OAuthToken,
    pub provider_type: ProviderType,
}

/// Info about a discovered credential (for API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredCredentialInfo {
    pub source: CredentialSource,
    pub provider: String,
    pub status: String,
    pub expires_at: Option<i64>,
    pub auto_refresh: bool,
}

/// Configuration for credential discovery.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CredentialDiscoveryConfig {
    pub claude_code: Option<bool>,
    pub codex: Option<bool>,
    pub refresh_interval_secs: Option<u64>,
    pub fetch_external_usage: Option<bool>,
}

/// Manages discovered OAuth tokens and background refresh.
pub struct TokenManager {
    tokens: Arc<RwLock<HashMap<CredentialSource, OAuthToken>>>,
    config: CredentialDiscoveryConfig,
}

impl TokenManager {
    pub async fn new(config: CredentialDiscoveryConfig) -> Self {
        let mut tokens = HashMap::new();

        // Initial discovery
        let discovered = discover_all(&config).await;
        for cred in discovered {
            tokens.insert(cred.source, cred.token);
        }

        Self {
            tokens: Arc::new(RwLock::new(tokens)),
            config,
        }
    }

    /// Get the current access token for a credential source.
    pub async fn get_token(&self, source: CredentialSource) -> Option<String> {
        let tokens = self.tokens.read().await;
        tokens.get(&source).map(|t| t.access_token.clone())
    }

    /// Start the background refresh loop.
    pub fn start_refresh_loop(
        self: &Arc<Self>,
        settings_service: Arc<LlmSettingsService>,
        router: Arc<LlmRouter>,
        cancel: CancellationToken,
    ) -> JoinHandle<()> {
        let tm = Arc::clone(self);
        let interval_secs = tm.config.refresh_interval_secs.unwrap_or(30);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
            // Skip the initial tick (we already did initial discovery)
            interval.tick().await;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("TokenManager refresh loop shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        tm.rescan(&settings_service, &router).await;
                    }
                }
            }
        })
    }

    /// Get info about all discovered credential sources.
    pub async fn discovered_sources(&self) -> Vec<DiscoveredCredentialInfo> {
        let tokens = self.tokens.read().await;
        let mut result = Vec::new();

        for (&source, token) in tokens.iter() {
            let (provider, provider_str) = match source {
                CredentialSource::ClaudeCode => (ProviderType::Anthropic, "anthropic"),
                CredentialSource::Codex => (ProviderType::OpenAI, "openai"),
            };
            let _ = provider; // used for type consistency

            let status = if token.is_expired() {
                "expired"
            } else {
                "active"
            };

            result.push(DiscoveredCredentialInfo {
                source,
                provider: provider_str.to_string(),
                status: status.to_string(),
                expires_at: token.expires_at,
                auto_refresh: token.refresh_token.is_some(),
            });
        }

        result
    }

    /// Discover credentials and inject merged pools into router.
    pub async fn rescan(
        &self,
        settings_service: &LlmSettingsService,
        router: &LlmRouter,
    ) -> Vec<DiscoveredCredentialInfo> {
        let discovered = discover_all(&self.config).await;

        // Update stored tokens
        {
            let mut tokens = self.tokens.write().await;
            for cred in &discovered {
                tokens.insert(cred.source, cred.token.clone());
            }
        }

        // For each discovered credential, build a merged pool and inject
        for cred in &discovered {
            if cred.token.is_expired() {
                // Try re-reading the credential file (CLI may have refreshed)
                let refreshed = match cred.source {
                    CredentialSource::ClaudeCode => discover_claude_code().await,
                    CredentialSource::Codex => discover_codex().await,
                };

                if let Some(refreshed_cred) = refreshed
                    && !refreshed_cred.token.is_expired()
                {
                    let mut tokens = self.tokens.write().await;
                    tokens.insert(cred.source, refreshed_cred.token.clone());
                    self.inject_credential(settings_service, router, &refreshed_cred)
                        .await;
                    continue;
                }

                tracing::debug!(
                    "Credential {:?} is expired, skipping injection",
                    cred.source
                );
                continue;
            }

            self.inject_credential(settings_service, router, cred).await;
        }

        self.discovered_sources().await
    }

    /// Inject a single credential into the router as a merged pool.
    async fn inject_credential(
        &self,
        settings_service: &LlmSettingsService,
        router: &LlmRouter,
        cred: &DiscoveredCredential,
    ) {
        let provider_type = cred.provider_type;

        // Build a discovered key
        let key_id = format!("auto_{:?}", cred.source).to_lowercase();
        let mut api_key = ApiKey::new(key_id, provider_type, cred.token.access_token.clone());
        api_key.priority = KeyPriority::Fallback;
        api_key.source = match cred.source {
            CredentialSource::ClaudeCode => KeySource::ClaudeCode,
            CredentialSource::Codex => KeySource::Codex,
        };
        api_key.notes = Some(format!("Auto-discovered from {:?}", cred.source));

        // Get base config keys from settings service
        let base_pool_keys = settings_service.build_key_pool_for_provider(provider_type);

        match base_pool_keys {
            Ok(_base_pool) => {
                // Create a merged pool: base config keys + discovered key
                let merged_keys = vec![api_key];
                let merged_pool = self
                    .build_merged_pool(settings_service, provider_type, merged_keys)
                    .await;

                if let Some(pool) = merged_pool
                    && !router.reload_keys(provider_type, pool)
                {
                    // Provider not configured — register it
                    tracing::info!("Registering auto-discovered provider {:?}", provider_type);
                    self.register_auto_provider(router, provider_type, cred)
                        .await;
                }
            }
            Err(e) => {
                tracing::debug!(
                    "Could not build base pool for {:?}: {}, registering standalone",
                    provider_type,
                    e
                );
                // No existing config — register a new provider with just the discovered key
                self.register_auto_provider(router, provider_type, cred)
                    .await;
            }
        }
    }

    /// Build a merged KeyPool from config keys + extra discovered keys.
    async fn build_merged_pool(
        &self,
        settings_service: &LlmSettingsService,
        provider_type: ProviderType,
        extra_keys: Vec<ApiKey>,
    ) -> Option<KeyPool> {
        // Get the base pool from config
        match settings_service.build_key_pool_for_provider(provider_type) {
            Ok(base_pool) => {
                // Since we can't extract keys from KeyPool, we rebuild from config
                // and add extra keys by creating a new pool that includes them.
                // The simplest approach: use config keys + extra keys together.
                // We'll get the config keys via the settings service's internal method.
                let config_keys = settings_service
                    .build_key_pool_keys_for_provider(provider_type)
                    .unwrap_or_default();

                let mut all_keys = config_keys;
                all_keys.extend(extra_keys);

                let strategy = if all_keys.iter().any(|k| k.priority == KeyPriority::Fallback) {
                    SelectionStrategy::PrimaryFallback
                } else {
                    base_pool.strategy()
                };

                Some(KeyPool::new(all_keys, strategy))
            }
            Err(_) => {
                // No config keys — just use extra keys
                if extra_keys.is_empty() {
                    return None;
                }
                Some(KeyPool::new(extra_keys, SelectionStrategy::PrimaryFallback))
            }
        }
    }

    /// Register a new auto-discovered provider when no config exists.
    async fn register_auto_provider(
        &self,
        router: &LlmRouter,
        provider_type: ProviderType,
        cred: &DiscoveredCredential,
    ) {
        let key_id = format!("auto_{:?}", cred.source).to_lowercase();
        let mut api_key = ApiKey::new(key_id, provider_type, cred.token.access_token.clone());
        api_key.priority = KeyPriority::Fallback;
        api_key.source = match cred.source {
            CredentialSource::ClaudeCode => KeySource::ClaudeCode,
            CredentialSource::Codex => KeySource::Codex,
        };

        let pool = KeyPool::new(vec![api_key], SelectionStrategy::PrimaryFallback);

        // Build the provider implementation
        let provider: Option<Arc<dyn crate::LlmProvider>> = match provider_type {
            #[cfg(feature = "anthropic")]
            ProviderType::Anthropic => Some(Arc::new(
                crate::providers::anthropic::AnthropicProvider::new(
                    cred.token.access_token.clone(),
                    None,
                    None,
                ),
            )),
            #[cfg(feature = "openai")]
            ProviderType::OpenAI => Some(Arc::new(crate::providers::openai::OpenAiProvider::new(
                cred.token.access_token.clone(),
                None,
                None,
                None,
            ))),
            _ => None,
        };

        if let Some(provider) = provider {
            router.register_provider(provider_type, provider, pool);
            tracing::info!("Registered auto-discovered provider {:?}", provider_type);
        }
    }
}

// ── Discovery Functions ──────────────────────────────────────────────

/// Discover all available credentials based on config.
pub async fn discover_all(config: &CredentialDiscoveryConfig) -> Vec<DiscoveredCredential> {
    let mut results = Vec::new();

    if config.claude_code.unwrap_or(true)
        && let Some(cred) = discover_claude_code().await
    {
        results.push(cred);
    }

    if config.codex.unwrap_or(true)
        && let Some(cred) = discover_codex().await
    {
        results.push(cred);
    }

    results
}

/// Discover Claude Code credentials from `~/.claude/.credentials.json`.
pub async fn discover_claude_code() -> Option<DiscoveredCredential> {
    let home = dirs_path()?;
    let cred_path = home.join(".claude").join(".credentials.json");

    match tokio::fs::read(&cred_path).await {
        Ok(data) => match serde_json::from_slice::<OAuthToken>(&data) {
            Ok(token) => {
                tracing::debug!(
                    "Discovered Claude Code credentials at {}",
                    cred_path.display()
                );
                Some(DiscoveredCredential {
                    source: CredentialSource::ClaudeCode,
                    token,
                    provider_type: ProviderType::Anthropic,
                })
            }
            Err(e) => {
                tracing::debug!(
                    "Failed to parse Claude Code credentials at {}: {}",
                    cred_path.display(),
                    e
                );
                None
            }
        },
        Err(e) => {
            tracing::debug!(
                "Claude Code credentials not found at {}: {}",
                cred_path.display(),
                e
            );

            // Fallback: try macOS Keychain
            #[cfg(target_os = "macos")]
            {
                return discover_from_keychain(
                    "Claude Code-credentials",
                    ProviderType::Anthropic,
                    CredentialSource::ClaudeCode,
                )
                .await;
            }

            #[cfg(not(target_os = "macos"))]
            None
        }
    }
}

/// Discover Codex credentials from `~/.codex/auth.json`.
pub async fn discover_codex() -> Option<DiscoveredCredential> {
    let home = dirs_path()?;
    let cred_path = home.join(".codex").join("auth.json");

    match tokio::fs::read(&cred_path).await {
        Ok(data) => match serde_json::from_slice::<OAuthToken>(&data) {
            Ok(token) => {
                tracing::debug!("Discovered Codex credentials at {}", cred_path.display());
                Some(DiscoveredCredential {
                    source: CredentialSource::Codex,
                    token,
                    provider_type: ProviderType::OpenAI,
                })
            }
            Err(e) => {
                tracing::debug!(
                    "Failed to parse Codex credentials at {}: {}",
                    cred_path.display(),
                    e
                );
                None
            }
        },
        Err(e) => {
            tracing::debug!(
                "Codex credentials not found at {}: {}",
                cred_path.display(),
                e
            );
            None
        }
    }
}

/// Attempt to read credentials from macOS Keychain.
#[cfg(target_os = "macos")]
async fn discover_from_keychain(
    service_name: &str,
    provider_type: ProviderType,
    source: CredentialSource,
) -> Option<DiscoveredCredential> {
    use tokio::time::{Duration, timeout};

    let output = timeout(
        Duration::from_secs(1),
        tokio::process::Command::new("security")
            .args(["find-generic-password", "-s", service_name, "-w"])
            .output(),
    )
    .await;

    match output {
        Ok(Ok(out)) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
            match serde_json::from_str::<OAuthToken>(&raw) {
                Ok(token) => {
                    tracing::debug!(
                        "Discovered credentials from macOS Keychain: {}",
                        service_name
                    );
                    Some(DiscoveredCredential {
                        source,
                        token,
                        provider_type,
                    })
                }
                Err(e) => {
                    tracing::debug!(
                        "Failed to parse Keychain credential for {}: {}",
                        service_name,
                        e
                    );
                    None
                }
            }
        }
        Ok(Ok(_)) => {
            tracing::debug!("Keychain entry '{}' not found", service_name);
            None
        }
        Ok(Err(e)) => {
            tracing::debug!(
                "Failed to run security command for '{}': {}",
                service_name,
                e
            );
            None
        }
        Err(_) => {
            tracing::debug!("Keychain lookup for '{}' timed out", service_name);
            None
        }
    }
}

/// Get the user's home directory.
fn dirs_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests;
