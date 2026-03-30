//! Settings service layer for LLM key management.
//!
//! Provides a high-level API for managing API keys with encryption,
//! config persistence, and hot-reload via ArcSwap.

use crate::config::{KeyConfig, LlmRouterConfig, ProviderConfig, read_config, write_config};
use crate::keys::key_encryption::KeyEncryptor;
use crate::keys::key_pool::{
    ApiKey, KeyHealthStatus, KeyPool, KeyPriority, KeySource, KeyStatus, ProviderType,
    SelectionStrategy, mask_secret,
};
use crate::keys::secret_store::SecretStore;
use crate::routing::router::LlmRouter;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use super::settings_types::{
    AddKeyRequest, KeyInfo, KeyInput, KeyValidationResult, LlmSettingsResponse,
    OrchestratorConfigResponse, OrchestratorInfo, ProviderInfo, ReorderKeysRequest,
    SetKeyPriorityRequest, UpdateOrchestratorRequest, ValidateKeyRequest,
};

/// High-level service for managing LLM settings.
/// Decouples route handlers from domain logic.
pub struct LlmSettingsService {
    router: Arc<LlmRouter>,
    config_path: PathBuf,
    encryptor: KeyEncryptor,
    secret_store: Arc<dyn SecretStore>,
}

impl LlmSettingsService {
    pub fn new(router: Arc<LlmRouter>, config_path: PathBuf) -> Result<Self, String> {
        let encryptor = KeyEncryptor::load_or_generate()?;
        let secret_store: Arc<dyn SecretStore> =
            Arc::new(crate::keys::secret_store::MemorySecretStore::new());
        Ok(Self {
            router,
            config_path,
            encryptor,
            secret_store,
        })
    }

    pub fn new_with_secret_store(
        router: Arc<LlmRouter>,
        config_path: PathBuf,
        secret_store: Arc<dyn SecretStore>,
    ) -> Result<Self, String> {
        let encryptor = KeyEncryptor::load_or_generate()?;
        Ok(Self {
            router,
            config_path,
            encryptor,
            secret_store,
        })
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
            monthly_budget: None,
            priority: req.key.priority.clone(),
            source: req.key.source.clone(),
            notes: req.key.notes.clone(),
            rate_limit: None,
            allowed_models: None,
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
        // D4: Acquire config write lock
        let _lock = crate::keys::key_encryption::acquire_config_write_lock()?;

        // 1. Read current config
        let mut config =
            read_config(&self.config_path).map_err(|e| format!("Failed to read config: {e}"))?;

        // 2. Apply mutation
        mutate(&mut config);

        // 3. Write to disk
        write_config(&self.config_path, &config)
            .map_err(|e| format!("Failed to write config: {e}"))?;

        // 4. Build new KeyPool from updated config
        let new_pool = self.build_key_pool_from_config(&config, &provider_type)?;

        // 5. Hot-reload via ArcSwap — register provider if not yet in router
        if !self.router.reload_keys(&provider_type, new_pool) {
            tracing::info!(
                "Provider {:?} not in router, registering now",
                provider_type
            );
            self.register_provider_from_config(&config, provider_type)?;
        }

        Ok(())
    }

    /// Build and register a provider that wasn't in the router at startup.
    #[allow(unused_variables)]
    fn register_provider_from_config(
        &self,
        config: &LlmRouterConfig,
        provider_type: ProviderType,
    ) -> Result<(), String> {
        let api_keys = self.build_api_keys_from_config(config, &provider_type)?;
        let first_key = api_keys
            .first()
            .ok_or_else(|| format!("No keys for {:?}, cannot register provider", provider_type))?
            .secret
            .clone();

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
                        first_key, model, max_tokens,
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
                    first_key, model, base_url, max_tokens,
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
                api_key.tier = key_config.tier.clone();
                api_key.monthly_budget = key_config.monthly_budget;
                api_key.priority = match key_config.priority.as_deref() {
                    Some("fallback") => KeyPriority::Fallback,
                    _ => KeyPriority::Primary,
                };
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
