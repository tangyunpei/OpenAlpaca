//! Settings service layer for LLM key management.
//!
//! Provides a high-level API for managing API keys with encryption,
//! config persistence, and hot-reload via ArcSwap.

use crate::config::{
    KeyConfig, LlmRouterConfig, ProviderConfig, read_config, render_config, write_config,
};
use crate::keys::key_encryption::KeyEncryptor;
use crate::keys::key_pool::{
    ApiKey, KeyHealthStatus, KeyPool, KeyStatus, ProviderType,
    SelectionStrategy, mask_secret,
};
use crate::keys::secret_store::SecretStore;
use crate::routing::router::LlmRouter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use super::settings_types::{
    AddKeyRequest, KeyInfo, KeyInput, KeyValidationResult, LlmSettingsResponse,
    OrchestratorConfigResponse, OrchestratorInfo, ProviderInfo, ReorderKeysRequest,
    SetKeyPriorityRequest, UpdateOrchestratorRequest, ValidateKeyRequest,
};

/// How this service puts `llm.toml` on disk.
///
/// The one atomic writer for hand-edited config (plan §1.4, P-11) lives in
/// `openalpaca_core`, which sits *above* this crate in the dependency graph, so
/// the host injects it here rather than this crate reaching up for it. The
/// daemon passes `openalpaca_core::config_io::atomic_write_with_backup`; the
/// default is the plain write this service always did, which is what a test or
/// a caller with no core dependency gets.
///
/// Called with `<config_path>.lock` already held — `file_lock` locks per
/// descriptor, so a writer that took the lock again would deadlock.
pub type ConfigWriter = Arc<dyn Fn(&Path, &str) -> Result<(), String> + Send + Sync>;

fn default_config_writer() -> ConfigWriter {
    Arc::new(|path: &Path, contents: &str| {
        std::fs::write(path, contents)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))
    })
}

/// High-level service for managing LLM settings.
/// Decouples route handlers from domain logic.
pub struct LlmSettingsService {
    router: Arc<LlmRouter>,
    config_path: PathBuf,
    encryptor: KeyEncryptor,
    secret_store: Arc<dyn SecretStore>,
    config_writer: ConfigWriter,
}

/// What a provider toggle did (GAP-15).
#[derive(Debug, Clone)]
pub struct ProviderEnabledOutcome {
    pub id: String,
    pub enabled: bool,
    /// Model ids `deregister_provider` stripped from the registry. Empty on an
    /// enable.
    pub removed_models: Vec<String>,
    /// Catalogue entries an enable put back after an earlier disable stripped
    /// them. Zero on a disable.
    pub restored_models: usize,
    /// Set when the file was written but the router could not load the
    /// provider — no usable key, or the provider is not compiled in. The
    /// disposition is still the owner's, and a restart would reach the same
    /// state, so this is reported rather than turned into a failure.
    pub warning: Option<String>,
}

/// Why a provider toggle did not happen. Every variant except `Reload` leaves
/// both the file and the router exactly as they were.
#[derive(Debug, thiserror::Error)]
pub enum SetProviderEnabledError {
    #[error("unknown provider '{0}'")]
    UnknownProvider(String),
    #[error(
        "'{provider}' serves the default model '{model}'; choose a different default model first"
    )]
    IsDefaultProvider { provider: String, model: String },
    #[error("{0}")]
    Persist(String),
}

impl LlmSettingsService {
    pub fn new(router: Arc<LlmRouter>, config_path: PathBuf) -> Result<Self, String> {
        let encryptor = KeyEncryptor::from_env()?;
        let secret_store: Arc<dyn SecretStore> =
            Arc::new(crate::keys::secret_store::MemorySecretStore::new());
        Ok(Self {
            router,
            config_path,
            encryptor,
            secret_store,
            config_writer: default_config_writer(),
        })
    }

    pub fn new_with_secret_store(
        router: Arc<LlmRouter>,
        config_path: PathBuf,
        secret_store: Arc<dyn SecretStore>,
    ) -> Result<Self, String> {
        let encryptor = KeyEncryptor::from_env()?;
        Ok(Self {
            router,
            config_path,
            encryptor,
            secret_store,
            config_writer: default_config_writer(),
        })
    }

    /// Route every `llm.toml` write through `writer` — the daemon's hook for
    /// the one atomic writer (see [`ConfigWriter`]).
    #[must_use]
    pub fn with_config_writer(mut self, writer: ConfigWriter) -> Self {
        self.config_writer = writer;
        self
    }

    #[cfg(test)]
    fn for_tests(router: Arc<LlmRouter>, config_path: PathBuf, encryptor: KeyEncryptor) -> Self {
        Self {
            router,
            config_path,
            encryptor,
            secret_store: Arc::new(crate::keys::secret_store::MemorySecretStore::new()),
            config_writer: default_config_writer(),
        }
    }

    /// Get current LLM settings with masked keys.
    pub async fn get_config(&self) -> Result<LlmSettingsResponse, String> {
        let config =
            read_config(&self.config_path).map_err(|e| format!("Failed to read config: {e}"))?;

        let orchestrator = config
            .orchestrator
            .as_ref()
            .map(|o| OrchestratorInfo {
                model: o.model.clone(),
                fallback_models: o.fallback_models.clone().unwrap_or_default(),
            })
            .unwrap_or_else(|| OrchestratorInfo {
                model: self.router.default_model(),
                fallback_models: vec![],
            });

        let mut providers = HashMap::new();
        let configured = self.router.configured_providers();

        for provider_type in ProviderType::all() {
            let provider_name = provider_type.to_string();
            let is_configured = configured.contains(&provider_type);

            // Get live key statuses (only available for configured providers)
            let statuses = if is_configured {
                self.router
                    .key_statuses(provider_type)
                    .await
                    .unwrap_or_default()
            } else {
                vec![]
            };
            let status_map: HashMap<String, &KeyStatus> =
                statuses.iter().map(|s| (s.id.clone(), s)).collect();

            // Get config-level info
            let provider_config = config
                .providers
                .as_ref()
                .and_then(|p| p.get(&provider_name));

            let strategy = provider_config
                .and_then(|p| {
                    p.key_selection_strategy
                        .as_deref()
                        .or(p.strategy.as_deref())
                })
                .unwrap_or("round_robin")
                .to_string();

            let keys: Vec<KeyInfo> = provider_config
                .and_then(|p| p.keys.as_ref())
                .map(|keys| {
                    keys.iter()
                        .map(|k| {
                            let status = status_map
                                .get(&k.id)
                                .map(|s| match s.health {
                                    KeyHealthStatus::Healthy => "healthy",
                                    KeyHealthStatus::RateLimited => "rate_limited",
                                    KeyHealthStatus::Error => "error",
                                    KeyHealthStatus::Unknown => "unknown",
                                })
                                .unwrap_or("unknown");

                            // Mask the secret: resolve via keychain > encrypted > env var
                            let masked = if let Some(ref sref) = k.secret_ref {
                                self.secret_store
                                    .get(sref)
                                    .ok()
                                    .flatten()
                                    .map(|s| mask_secret(&s))
                                    .unwrap_or_else(|| "*** (keychain)".to_string())
                            } else if let Some(ref enc) = k.secret_encrypted {
                                self.encryptor
                                    .decrypt(enc)
                                    .ok()
                                    .map(|s| mask_secret(&s))
                                    .unwrap_or_else(|| "*** (encrypted)".to_string())
                            } else if let Some(ref env) = k.secret_env {
                                std::env::var(env)
                                    .map(|s| mask_secret(&s))
                                    .unwrap_or_else(|_| format!("${}", env))
                            } else {
                                "***".to_string()
                            };

                            KeyInfo {
                                id: k.id.clone(),
                                masked_secret: masked,
                                tier: k.tier.clone(),
                                priority: k
                                    .priority
                                    .clone()
                                    .unwrap_or_else(|| "primary".to_string()),
                                source: k.source.clone().unwrap_or_else(|| "other".to_string()),
                                notes: k.notes.clone(),
                                status: status.to_string(),
                                monthly_usage_usd: None,
                                managed: None,
                                credential_status: None,
                                credential_expires_at: None,
                                external_usage: None,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let enabled = if is_configured {
                provider_config.and_then(|p| p.enabled).unwrap_or(true)
            } else {
                provider_config.and_then(|p| p.enabled).unwrap_or(false)
            };

            providers.insert(
                provider_name,
                ProviderInfo {
                    enabled,
                    key_selection_strategy: strategy,
                    keys,
                },
            );
        }

        Ok(LlmSettingsResponse {
            orchestrator,
            providers,
        })
    }

    /// Add or update a key for a provider.
    pub async fn upsert_key(&self, req: AddKeyRequest) -> Result<(), String> {
        let provider_type = parse_provider_type(&req.provider)
            .ok_or_else(|| format!("Unknown provider: {}", req.provider))?;

        let key_id = req.key.id.unwrap_or_else(|| {
            format!(
                "key_{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("0")
            )
        });

        // Check if keychain is enabled from config
        let use_keychain = read_config(&self.config_path)
            .ok()
            .and_then(|c| c.security.as_ref().map(|s| s.use_keychain))
            .unwrap_or(false);

        let (new_secret_ref, new_secret_encrypted) = if use_keychain {
            // Store in OS keychain via secret_ref
            let sref = format!("llm/{}/{}", req.provider, uuid::Uuid::new_v4());
            self.secret_store.set(&sref, &req.key.secret)?;
            (Some(sref), None)
        } else {
            // Store as local encrypted value
            let encrypted = self
                .encryptor
                .encrypt(&req.key.secret)
                .map_err(|e| format!("Failed to encrypt key: {e}"))?;
            (None, Some(encrypted))
        };

        let new_key_config = KeyConfig {
            id: key_id.clone(),
            secret_env: None,
            secret_ref: new_secret_ref,
            secret_encrypted: new_secret_encrypted,
            tier: req.key.tier.clone(),
            priority: req.key.priority.clone(),
            source: req.key.source.clone(),
            notes: req.key.notes.clone(),
            rate_limit: None,
        };

        self.persist_and_reload(provider_type, |config| {
            let providers = config.providers.get_or_insert_with(HashMap::new);
            let provider =
                providers
                    .entry(req.provider.clone())
                    .or_insert_with(|| ProviderConfig {
                        enabled: Some(true),
                        keys: Some(Vec::new()),
                        ..Default::default()
                    });
            let keys = provider.keys.get_or_insert_with(Vec::new);

            // If updating existing key, delete old secret_ref from keychain
            if let Some(existing) = keys.iter_mut().find(|k| k.id == key_id) {
                if let Some(ref old_ref) = existing.secret_ref {
                    let _ = self.secret_store.delete(old_ref);
                }
                *existing = new_key_config;
            } else {
                keys.push(new_key_config);
            }
        })
        .await
    }

    /// Remove a key from a provider.
    pub async fn remove_key(&self, provider: &str, key_id: &str) -> Result<(), String> {
        let provider_type = parse_provider_type(provider)
            .ok_or_else(|| format!("Unknown provider: {}", provider))?;

        let provider_name = provider.to_string();
        let kid = key_id.to_string();
        let mut found = false;

        self.persist_and_reload(provider_type, |config| {
            if let Some(ref mut providers) = config.providers
                && let Some(ref mut pc) = providers.get_mut(&provider_name)
                && let Some(ref mut keys) = pc.keys
            {
                // Delete secret_ref from keychain before removing
                for k in keys.iter() {
                    if k.id == kid
                        && let Some(ref sref) = k.secret_ref
                    {
                        let _ = self.secret_store.delete(sref);
                    }
                }
                let before = keys.len();
                keys.retain(|k| k.id != kid);
                found = keys.len() < before;
            }
        })
        .await?;

        if !found {
            return Err(format!(
                "Key '{}' not found in provider '{}'",
                key_id, provider
            ));
        }
        Ok(())
    }

    /// Reorder keys and optionally set a new primary.
    pub async fn reorder_keys(&self, req: ReorderKeysRequest) -> Result<(), String> {
        let provider_type = parse_provider_type(&req.provider)
            .ok_or_else(|| format!("Unknown provider: {}", req.provider))?;

        self.persist_and_reload(provider_type, |config| {
            if let Some(ref mut providers) = config.providers
                && let Some(ref mut pc) = providers.get_mut(&req.provider)
                && let Some(ref mut keys) = pc.keys
            {
                // Reorder keys according to key_order
                let mut ordered = Vec::with_capacity(keys.len());
                for id in &req.key_order {
                    if let Some(pos) = keys.iter().position(|k| &k.id == id) {
                        ordered.push(keys.remove(pos));
                    }
                }
                // Append any remaining keys not in the order list
                ordered.append(keys);
                *keys = ordered;

                // Set primary if specified
                if let Some(ref primary_id) = req.primary_key_id {
                    for k in keys.iter_mut() {
                        k.priority = Some(if k.id == *primary_id {
                            "primary".to_string()
                        } else {
                            "fallback".to_string()
                        });
                    }
                }
            }
        })
        .await
    }

    /// Set a single key's priority (primary/fallback) without changing other keys.
    pub async fn set_key_priority(&self, req: SetKeyPriorityRequest) -> Result<(), String> {
        let provider_type = parse_provider_type(&req.provider)
            .ok_or_else(|| format!("Unknown provider: {}", req.provider))?;

        if req.priority != "primary" && req.priority != "fallback" {
            return Err(format!("Invalid key priority: {}", req.priority));
        }

        let provider_name = req.provider.clone();
        let kid = req.key_id.clone();
        let new_priority = req.priority.clone();
        let mut found = false;

        self.persist_and_reload(provider_type, |config| {
            if let Some(ref mut providers) = config.providers
                && let Some(ref mut pc) = providers.get_mut(&provider_name)
                && let Some(ref mut keys) = pc.keys
                && let Some(k) = keys.iter_mut().find(|k| k.id == kid)
            {
                k.priority = Some(new_priority.clone());
                found = true;
            }
        })
        .await?;

        if !found {
            return Err(format!(
                "Key '{}' not found in provider '{}'",
                req.key_id, req.provider
            ));
        }

        Ok(())
    }

    /// Validate a key by detecting its format and querying available models.
    pub async fn validate_key(
        &self,
        req: ValidateKeyRequest,
    ) -> Result<KeyValidationResult, String> {
        let provider_type = parse_provider_type(&req.provider)
            .ok_or_else(|| format!("Unknown provider: {}", req.provider))?;

        // Detect source from key format
        let detected_source = if req.secret.starts_with("sk-ant-oat") {
            Some("claude_code".to_string())
        } else if req.secret.starts_with("sk-ant-") || req.secret.starts_with("sk-") {
            Some("api_console".to_string())
        } else {
            None
        };

        // Inline format validation (no cross-crate dep on openalpaca_storage)
        let (format_error, is_api_compatible) = match req.provider.as_str() {
            "anthropic" => {
                if req.secret.starts_with("sk-ant-oat") {
                    // Setup token — validate as setup-token
                    if req.secret.len() < 80 {
                        (
                            Some(format!(
                                "Setup token too short ({} chars, expected >= 80).",
                                req.secret.len()
                            )),
                            false,
                        )
                    } else {
                        (None, false) // valid setup token, NOT API-compatible
                    }
                } else if !req.secret.starts_with("sk-ant-") {
                    (
                        Some("Anthropic API keys start with 'sk-ant-'.".to_string()),
                        false,
                    )
                } else if req.secret.len() < 40 {
                    (
                        Some(format!(
                            "API key too short ({} chars, expected >= 40).",
                            req.secret.len()
                        )),
                        false,
                    )
                } else {
                    (None, true) // valid API key
                }
            }
            "openai" => {
                if req.secret.starts_with("sk-ant-") {
                    (
                        Some("This looks like an Anthropic key, not an OpenAI key.".to_string()),
                        false,
                    )
                } else if !req.secret.starts_with("sk-") {
                    (Some("OpenAI API keys start with 'sk-'.".to_string()), false)
                } else if req.secret.len() < 20 {
                    (
                        Some(format!(
                            "Key too short ({} chars, expected >= 20).",
                            req.secret.len()
                        )),
                        false,
                    )
                } else {
                    (None, true)
                }
            }
            _ => (None, true),
        };
        let valid = format_error.is_none();

        // Detect tier from key prefix
        let tier = if req.secret.contains("api03") {
            Some("tier3".to_string())
        } else if req.secret.contains("api02") {
            Some("tier2".to_string())
        } else {
            Some("tier1".to_string())
        };

        // Only call list_models_for_provider for API-compatible keys
        let models_available = if valid && is_api_compatible {
            self.router
                .list_models_for_provider(provider_type, &req.secret)
                .await
                .unwrap_or_default()
        } else {
            vec![]
        };

        Ok(KeyValidationResult {
            valid,
            tier,
            detected_source,
            models_available,
            rate_limits: None,
            format_error,
        })
    }

    /// List all available models from the registry.
    pub fn available_models(&self) -> Vec<crate::routing::model_registry::ModelEntry> {
        self.router.available_models()
    }

    /// List ALL registered models with pricing (not just API-discovered ones).
    /// Returns defaults + discovered models. Useful for pricing/billing UI.
    pub fn all_models_with_pricing(&self) -> Vec<crate::routing::model_registry::ModelEntry> {
        self.router.model_registry().list_models()
    }

    /// Estimate cost for a given model and token count.
    /// Returns estimated USD cost. Falls back to Sonnet-like pricing for unknown models.
    pub fn estimate_cost(&self, model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
        self.router
            .cost_tracker
            .calculate_cost(model, input_tokens, output_tokens)
    }

    /// Refresh models by querying each configured provider's API.
    pub async fn refresh_models(&self) {
        self.router.refresh_models().await;
    }

    /// Get live key health status for all providers.
    pub async fn key_health(&self) -> HashMap<String, Vec<KeyStatus>> {
        let mut result = HashMap::new();
        for provider_type in self.router.configured_providers() {
            if let Some(statuses) = self.router.key_statuses(&provider_type).await {
                result.insert(provider_type.to_string(), statuses);
            }
        }
        result
    }

    /// Get orchestrator config (model + fallback_models) from disk.
    /// Stats (agents/tasks/cost) are populated by the caller.
    pub fn get_orchestrator_config(&self) -> Result<(String, Vec<String>), String> {
        let config =
            read_config(&self.config_path).map_err(|e| format!("Failed to read config: {e}"))?;

        let model = config
            .orchestrator
            .as_ref()
            .map(|o| o.model.clone())
            .unwrap_or_else(|| self.router.default_model());

        let fallback_models = config
            .orchestrator
            .as_ref()
            .and_then(|o| o.fallback_models.clone())
            .unwrap_or_default();

        Ok((model, fallback_models))
    }

    /// Update orchestrator config (model + fallback_models).
    /// Takes effect on next restart (no hot-reload of orchestrator model).
    pub fn update_orchestrator_config(&self, req: UpdateOrchestratorRequest) -> Result<(), String> {
        let mut config =
            read_config(&self.config_path).map_err(|e| format!("Failed to read config: {e}"))?;

        let orch =
            config
                .orchestrator
                .get_or_insert_with(|| crate::config::OrchestratorLlmConfig {
                    model: "claude-sonnet-4-5-20250929".to_string(),
                    fallback_models: None,
                });
        orch.model = req.model;
        orch.fallback_models = if req.fallback_models.is_empty() {
            None
        } else {
            Some(req.fallback_models)
        };

        write_config(&self.config_path, &config)
            .map_err(|e| format!("Failed to write config: {e}"))?;

        Ok(())
    }

    /// Build a KeyPool from config file for a specific provider.
    /// Used by TokenManager to create merged pools (config keys + discovered keys).
    pub fn build_key_pool_for_provider(
        &self,
        provider_type: &ProviderType,
    ) -> Result<KeyPool, String> {
        let config =
            read_config(&self.config_path).map_err(|e| format!("Failed to read config: {e}"))?;
        self.build_key_pool_from_config(&config, provider_type)
    }

    /// Build a Vec<ApiKey> from config file for a specific provider.
    /// Used by TokenManager to merge config keys with discovered keys.
    pub fn build_key_pool_keys_for_provider(
        &self,
        provider_type: &ProviderType,
    ) -> Result<Vec<ApiKey>, String> {
        let config =
            read_config(&self.config_path).map_err(|e| format!("Failed to read config: {e}"))?;
        self.build_api_keys_from_config(&config, provider_type)
    }

    /// Get a reference to the router.
    pub fn router(&self) -> &Arc<LlmRouter> {
        &self.router
    }

    /// Internal: apply a mutation to the config, persist, and hot-reload.
    async fn persist_and_reload<F>(
        &self,
        provider_type: ProviderType,
        mutate: F,
    ) -> Result<(), String>
    where
        F: FnOnce(&mut LlmRouterConfig),
    {
        let config = self.persist_only(mutate)?;

        // Build a new KeyPool from the updated config and hot-reload it via
        // ArcSwap — registering the provider if it is not in the router yet.
        let new_pool = self.build_key_pool_from_config(&config, &provider_type)?;
        if !self.router.reload_keys(&provider_type, new_pool) {
            tracing::info!(
                "Provider {:?} not in router, registering now",
                provider_type
            );
            self.register_provider_from_config(&config, provider_type)?;
        }

        Ok(())
    }

    /// Internal: read → mutate → write, with no router reload.
    ///
    /// The write half of [`Self::persist_and_reload`], split out because a
    /// provider toggle's hot path is not a key-pool reload (GAP-15) — and
    /// because the two must stay ordered write-first: a refused write returns
    /// here with the router untouched.
    ///
    /// The bytes go out through the injected [`ConfigWriter`] — the daemon's is
    /// the one atomic writer (§1.4, P-11), so `llm.toml` gets the same
    /// tmp → fsync → rotate → rename and five-version backup as `mcp.toml`.
    /// The document is rendered through `toml::Value`, whose tables are
    /// ordered, so a rewrite that changes one key changes one key: the
    /// serialiser's own `HashMap` order is not stable between reads, and an
    /// owner's file must not reshuffle itself on every toggle.
    ///
    /// Returns the mutated config, which the caller usually needs anyway.
    fn persist_only<F>(&self, mutate: F) -> Result<LlmRouterConfig, String>
    where
        F: FnOnce(&mut LlmRouterConfig),
    {
        // D4: the whole read-modify-write is under the config write lock, and
        // the injected writer must not take it again.
        let _lock = crate::keys::key_encryption::acquire_config_write_lock(&self.config_path)?;

        let mut config =
            read_config(&self.config_path).map_err(|e| format!("Failed to read config: {e}"))?;
        mutate(&mut config);

        let rendered = render_config(&config).map_err(|e| e.to_string())?;
        (self.config_writer)(&self.config_path, &rendered)?;

        Ok(config)
    }

    /// Turn one provider on or off (GAP-15).
    ///
    /// Write-first: `llm.toml` is the disposition, so it lands before the
    /// router is touched and a refused write changes nothing. Then the hot
    /// path — a disable unloads the provider and strips its models, so
    /// in-flight calls finish and new ones fall through to the fallback chain
    /// (or fail as unconfigured); an enable re-registers it, puts back the
    /// catalogue the disable stripped, and refreshes from the provider's API.
    pub async fn set_provider_enabled(
        &self,
        provider: &str,
        enabled: bool,
    ) -> Result<ProviderEnabledOutcome, SetProviderEnabledError> {
        let provider_type = parse_provider_type(provider)
            .filter(|pt| ProviderType::all().contains(pt))
            .ok_or_else(|| SetProviderEnabledError::UnknownProvider(provider.to_string()))?;

        let current = read_config(&self.config_path)
            .map_err(|e| SetProviderEnabledError::Persist(format!("Failed to read config: {e}")))?;

        // Turning off the provider that serves the default model would leave
        // every request with nowhere to go, so it is refused rather than done
        // and reported.
        if !enabled
            && let Some((model, owner)) = self.default_model_provider(&current)
            && owner == provider_type
        {
            return Err(SetProviderEnabledError::IsDefaultProvider {
                provider: provider.to_string(),
                model,
            });
        }

        let name = provider.to_string();
        let config = self
            .persist_only(|config| {
                let providers = config.providers.get_or_insert_with(HashMap::new);
                providers.entry(name).or_default().enabled = Some(enabled);
            })
            .map_err(SetProviderEnabledError::Persist)?;

        let mut outcome = ProviderEnabledOutcome {
            id: provider.to_string(),
            enabled,
            removed_models: Vec::new(),
            restored_models: 0,
            warning: None,
        };

        if enabled {
            // `deregister_provider` took the catalogue with it, and
            // `refresh_models` can only mark entries that still exist — so the
            // defaults (and the config's own `[models]` rows) go back first,
            // or the model picker stays empty until the daemon restarts.
            outcome.restored_models = self
                .router
                .model_registry()
                .restore_defaults_for_provider(&provider_type);
            if let Some(models) = config.models.as_ref() {
                self.router.model_registry().reload_from_config(models);
            }
            if let Err(e) = self.register_provider_from_config(&config, provider_type) {
                tracing::warn!(provider = %provider, error = %e, "provider enabled in config but not loaded");
                outcome.warning = Some(e);
            } else {
                self.router.refresh_models().await;
            }
        } else {
            outcome.removed_models = self.router.deregister_provider(&provider_type);
            tracing::info!(
                provider = %provider,
                models = outcome.removed_models.len(),
                "provider disabled and unloaded"
            );
        }

        Ok(outcome)
    }

    /// The default model and the provider that serves it, as far as anything
    /// can say: the registry first, then the config's own `[models]` table for
    /// a model the registry has never seen.
    fn default_model_provider(&self, config: &LlmRouterConfig) -> Option<(String, ProviderType)> {
        let model = config
            .orchestrator
            .as_ref()
            .map(|o| o.model.clone())
            .unwrap_or_else(|| self.router.default_model());

        let provider = self
            .router
            .model_registry()
            .resolve_provider(&model)
            .or_else(|| {
                config
                    .models
                    .as_ref()
                    .and_then(|models| models.get(&model))
                    .and_then(|entry| parse_provider_type(&entry.provider))
            })?;
        Some((model, provider))
    }

    /// Build and register a provider that wasn't in the router at startup.
    #[allow(unused_variables)]
    fn register_provider_from_config(
        &self,
        config: &LlmRouterConfig,
        provider_type: ProviderType,
    ) -> Result<(), String> {
        let api_keys = self.build_api_keys_from_config(config, &provider_type)?;
        // Ollama is keyless — the boot builder registers it with no key at all
        // — so the demand for one belongs to the arms that need it, not here.
        let first_key = api_keys.first().map(|k| k.secret.clone());
        let require_key = || {
            first_key
                .clone()
                .ok_or_else(|| format!("No keys for {:?}, cannot register provider", provider_type))
        };

        let pool = self.build_key_pool_from_config(config, &provider_type)?;

        let provider_name = provider_type.to_string();
        let base_url = config
            .providers
            .as_ref()
            .and_then(|p| p.get(&provider_name))
            .and_then(|pc| pc.base_url.clone());

        let rt = self.router.runtime_config();
        let provider: Option<Arc<dyn crate::LlmProvider>> = match &provider_type {
            #[cfg(feature = "anthropic")]
            ProviderType::Anthropic => {
                let model = rt
                    .provider_defaults
                    .get("anthropic")
                    .map(|d| d.default_model.clone());
                let max_tokens = rt
                    .provider_defaults
                    .get("anthropic")
                    .map(|d| d.default_max_tokens);
                Some(Arc::new(
                    crate::providers::anthropic::AnthropicProvider::new(
                        require_key()?,
                        model,
                        max_tokens,
                    ),
                ))
            }
            #[cfg(feature = "openai")]
            ProviderType::OpenAI => {
                let model = rt
                    .provider_defaults
                    .get("openai")
                    .map(|d| d.default_model.clone());
                let max_tokens = rt
                    .provider_defaults
                    .get("openai")
                    .map(|d| d.default_max_tokens);
                Some(Arc::new(crate::providers::openai::OpenAiProvider::new(
                    require_key()?,
                    model,
                    base_url,
                    max_tokens,
                )))
            }
            #[cfg(feature = "ollama")]
            ProviderType::Ollama => {
                let model = rt
                    .provider_defaults
                    .get("ollama")
                    .map(|d| d.default_model.clone())
                    .unwrap_or_else(|| "llama3".to_string());
                Some(Arc::new(crate::providers::ollama::OllamaProvider::new(
                    model, base_url,
                )))
            }
            #[allow(unreachable_patterns)]
            _ => None,
        };

        match provider {
            Some(p) => {
                self.router.register_provider(provider_type, p, pool);
                tracing::info!("Registered provider in router");
                Ok(())
            }
            None => Err(format!(
                "Provider not available (feature not enabled)"
            )),
        }
    }

    /// Build a KeyPool from the current config for a specific provider.
    fn build_key_pool_from_config(
        &self,
        config: &LlmRouterConfig,
        provider_type: &ProviderType,
    ) -> Result<KeyPool, String> {
        let api_keys = self.build_api_keys_from_config(config, provider_type)?;

        let provider_name = provider_type.to_string();
        let provider_config = config
            .providers
            .as_ref()
            .and_then(|p| p.get(&provider_name));

        let strategy_str = provider_config.and_then(|p| {
            p.key_selection_strategy
                .as_deref()
                .or(p.strategy.as_deref())
        });
        let strategy = match strategy_str {
            Some("lru") | Some("least_recently_used") => SelectionStrategy::LeastRecentlyUsed,
            Some("primary_fallback") => SelectionStrategy::PrimaryFallback,
            _ => SelectionStrategy::RoundRobin,
        };

        Ok(KeyPool::new(api_keys, strategy))
    }

    /// Build a Vec<ApiKey> from config for a specific provider.
    fn build_api_keys_from_config(
        &self,
        config: &LlmRouterConfig,
        provider_type: &ProviderType,
    ) -> Result<Vec<ApiKey>, String> {
        let provider_name = provider_type.to_string();

        let provider_config = config
            .providers
            .as_ref()
            .and_then(|p| p.get(&provider_name));

        let mut api_keys = Vec::new();
        if let Some(pc) = provider_config
            && let Some(ref keys) = pc.keys
        {
            for key_config in keys {
                // Resolve secret: secret_env > secret_ref > secret_encrypted
                let secret = if let Some(ref env_var) = key_config.secret_env {
                    std::env::var(env_var).map_err(|_| {
                        format!("Missing env var '{}' for key '{}'", env_var, key_config.id)
                    })?
                } else if let Some(ref sref) = key_config.secret_ref {
                    self.secret_store.get(sref)?.ok_or_else(|| {
                        format!(
                            "Secret '{}' not found in keychain for key '{}'",
                            sref, key_config.id
                        )
                    })?
                } else if let Some(ref encrypted) = key_config.secret_encrypted {
                    if KeyEncryptor::is_encrypted(encrypted) {
                        self.encryptor.decrypt(encrypted).map_err(|e| {
                            format!("Failed to decrypt key '{}': {e}", key_config.id)
                        })?
                    } else {
                        encrypted.clone()
                    }
                } else {
                    return Err(format!("No secret for key '{}'", key_config.id));
                };

                let mut api_key = ApiKey::new(key_config.id.clone(), provider_type.clone(), secret);
                super::key_pool_builder::apply_key_config_metadata(&mut api_key, key_config);
                api_keys.push(api_key);
            }
        }

        Ok(api_keys)
    }

    /// Get a reference to the secret store.
    pub fn secret_store(&self) -> &Arc<dyn SecretStore> {
        &self.secret_store
    }
}

fn parse_provider_type(name: &str) -> Option<ProviderType> {
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

pub use super::key_pool_builder::build_key_pool_from_provider_config;

#[cfg(test)]
mod tests;
