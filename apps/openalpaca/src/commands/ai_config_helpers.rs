//! Internal helpers for AI config read/write operations.

use anyhow::{Context, Result};
use openalpaca_llm::cli_backend::{CliBackendConfig, CliBackendsConfig};
use openalpaca_llm::config::{
    EmbeddingsConfig, KeyConfig, LlmRouterConfig, OrchestratorLlmConfig, read_config,
};
use openalpaca_llm::keys::credential_discovery::CredentialDiscoveryConfig;
use openalpaca_llm::keys::key_encryption::KeyEncryptor;
use openalpaca_llm::keys::secret_store::{KeyringSecretStore, MemorySecretStore, SecretStore};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Ensure master key exists at its canonical location before any crypto operations.
pub(super) fn ensure_master_key() {
    if let Ok(dir) = openalpaca_storage::store::master_key_dir() {
        let _ = KeyEncryptor::ensure_at(&dir);
    }
}

/// Check whether OS keychain is enabled via `[security] use_keychain` in llm.toml.
pub(super) fn is_keychain_enabled() -> bool {
    llm_config_path()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| read_config(&p).ok())
        .and_then(|c| c.security.as_ref().map(|s| s.use_keychain))
        .unwrap_or(false)
}

/// Get the secret store matching the current config.
pub(super) fn secret_store() -> Box<dyn SecretStore> {
    if is_keychain_enabled() {
        Box::new(KeyringSecretStore)
    } else {
        Box::new(MemorySecretStore::new())
    }
}

/// Returns the path to `config/llm.toml`.
pub fn llm_config_path() -> Result<PathBuf> {
    // 1. OPENALPACA_CONFIG_DIR override
    if let Ok(dir) = std::env::var("OPENALPACA_CONFIG_DIR") {
        let p = PathBuf::from(&dir);
        if p.is_dir() {
            return Ok(p.join("llm.toml"));
        }
    }
    // 2. Writable home_root()/config/
    if let Ok(dir) = openalpaca_storage::store::runtime_config_dir() {
        let p = dir.join("llm.toml");
        if p.exists() {
            return Ok(p);
        }
    }
    // 3. Walk up from CWD (dev/repo)
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    Ok(cwd.join("config").join("llm.toml"))
}

pub(super) fn load_or_default(path: &Path) -> Result<LlmRouterConfig> {
    if path.exists() {
        read_config(path).map_err(|e| anyhow::anyhow!("{}", e))
    } else {
        Ok(default_config())
    }
}

pub(super) fn default_config() -> LlmRouterConfig {
    LlmRouterConfig {
        orchestrator: Some(OrchestratorLlmConfig {
            model: "claude-sonnet-4-5-20250929".to_string(),
            fallback_models: None,
        }),
        providers: Some(HashMap::new()),
        ..Default::default()
    }
}

/// Mask a key for display — shows `"sk-...abcd"` (last 4 chars).
pub(super) fn mask_key_value(secret: &str) -> String {
    if secret.len() <= 4 {
        return "*".repeat(secret.len());
    }
    let suffix = &secret[secret.len() - 4..];
    format!("sk-...{}", suffix)
}

pub(super) fn extract_provider(key: &str) -> Result<String> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() >= 3 {
        Ok(parts[1].to_string())
    } else {
        Err(anyhow::anyhow!("Invalid AI key format: {}", key))
    }
}

pub(super) fn apply_to_config(
    key: &str,
    value: &str,
    config: &mut LlmRouterConfig,
    store: &dyn SecretStore,
    source_hint: Option<&str>,
) -> Result<()> {
    match key {
        "ai.default_model" => {
            config
                .orchestrator
                .get_or_insert_with(|| OrchestratorLlmConfig {
                    model: value.to_string(),
                    fallback_models: None,
                })
                .model = value.to_string();
        }
        "ai.fallback_models" => {
            let models: Vec<String> = value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            config
                .orchestrator
                .get_or_insert_with(|| OrchestratorLlmConfig {
                    model: "claude-sonnet-4-5-20250929".to_string(),
                    fallback_models: None,
                })
                .fallback_models = Some(models);
        }
        "ai.claude_code.discovery" => {
            config
                .credential_discovery
                .get_or_insert_with(CredentialDiscoveryConfig::default)
                .claude_code = Some(value == "true");
        }
        "ai.codex.discovery" => {
            config
                .credential_discovery
                .get_or_insert_with(CredentialDiscoveryConfig::default)
                .codex = Some(value == "true");
        }
        "ai.claude_code.cli_enabled" => {
            config
                .cli_backends
                .get_or_insert_with(CliBackendsConfig::default)
                .claude_code
                .get_or_insert(CliBackendConfig {
                    path: None,
                    enabled: None,
                    timeout_secs: None,
                })
                .enabled = Some(value == "true");
        }
        "ai.claude_code.cli_path" => {
            config
                .cli_backends
                .get_or_insert_with(CliBackendsConfig::default)
                .claude_code
                .get_or_insert(CliBackendConfig {
                    path: None,
                    enabled: None,
                    timeout_secs: None,
                })
                .path = Some(value.to_string());
        }
        "ai.codex.cli_enabled" => {
            config
                .cli_backends
                .get_or_insert_with(CliBackendsConfig::default)
                .codex
                .get_or_insert(CliBackendConfig {
                    path: None,
                    enabled: None,
                    timeout_secs: None,
                })
                .enabled = Some(value == "true");
        }
        "ai.codex.cli_path" => {
            config
                .cli_backends
                .get_or_insert_with(CliBackendsConfig::default)
                .codex
                .get_or_insert(CliBackendConfig {
                    path: None,
                    enabled: None,
                    timeout_secs: None,
                })
                .path = Some(value.to_string());
        }
        "ai.embeddings.enabled" => {
            config
                .embeddings
                .get_or_insert_with(|| EmbeddingsConfig {
                    enabled: false,
                    provider: String::new(),
                    model: None,
                    dimensions: None,
                })
                .enabled = value == "true";
        }
        "ai.embeddings.provider" => {
            config
                .embeddings
                .get_or_insert_with(|| EmbeddingsConfig {
                    enabled: false,
                    provider: String::new(),
                    model: None,
                    dimensions: None,
                })
                .provider = value.to_string();
        }
        "ai.embeddings.model" => {
            config
                .embeddings
                .get_or_insert_with(|| EmbeddingsConfig {
                    enabled: false,
                    provider: String::new(),
                    model: None,
                    dimensions: None,
                })
                .model = Some(value.to_string());
        }
        "ai.embeddings.dimensions" => {
            let dim: u32 = value
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid dimensions value: {}", value))?;
            config
                .embeddings
                .get_or_insert_with(|| EmbeddingsConfig {
                    enabled: false,
                    provider: String::new(),
                    model: None,
                    dimensions: None,
                })
                .dimensions = Some(dim);
        }
        "ai.web_search.api_key" => {
            config
                .web_search
                .get_or_insert_with(Default::default)
                .api_key = value.to_string();
        }
        "ai.web_search.timeout_secs" => {
            let secs: u64 = value
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid timeout_secs value: {}", value))?;
            config
                .web_search
                .get_or_insert_with(Default::default)
                .timeout_secs = secs;
        }
        k if k.starts_with("ai.") && k.ends_with(".enabled") => {
            let provider = extract_provider(k)?;
            let providers = config.providers.get_or_insert_with(HashMap::new);
            let entry = providers.entry(provider).or_default();
            entry.enabled = Some(value == "true");
        }
        k if k.starts_with("ai.") && k.ends_with(".api_key") => {
            let provider = extract_provider(k)?;
            let cli_id = format!("{}_cli", provider);
            let source = source_hint.unwrap_or("api_console").to_string();
            let use_kc = is_keychain_enabled();

            let (new_secret_ref, new_secret_encrypted) = if use_kc {
                let sref = format!("llm/{}/{}", provider, uuid::Uuid::new_v4());
                store
                    .set(&sref, value)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                (Some(sref), None)
            } else {
                let encryptor =
                    KeyEncryptor::load_or_generate().map_err(|e| anyhow::anyhow!("{}", e))?;
                let encrypted = encryptor
                    .encrypt(value)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                (None, Some(encrypted))
            };

            let providers = config.providers.get_or_insert_with(HashMap::new);
            let entry = providers.entry(provider).or_default();
            let keys = entry.keys.get_or_insert_with(Vec::new);
            if let Some(existing) = keys.iter_mut().find(|k| k.id == cli_id) {
                if let Some(ref old_ref) = existing.secret_ref {
                    let _ = store.delete(old_ref);
                }
                existing.secret_ref = new_secret_ref;
                existing.secret_encrypted = new_secret_encrypted;
                existing.source = Some(source);
                existing.priority = Some("primary".to_string());
            } else {
                let already_stored = keys.iter().any(|k| {
                    if use_kc {
                        k.secret_ref
                            .as_ref()
                            .and_then(|sr| store.get(sr).ok().flatten())
                            .map(|decrypted| decrypted == value)
                            .unwrap_or(false)
                    } else {
                        k.secret_encrypted
                            .as_ref()
                            .and_then(|enc| {
                                KeyEncryptor::load_or_generate()
                                    .ok()
                                    .and_then(|e| e.decrypt(enc).ok())
                            })
                            .map(|decrypted| decrypted == value)
                            .unwrap_or(false)
                    }
                });
                if !already_stored {
                    keys.push(KeyConfig {
                        id: cli_id,
                        secret_env: None,
                        secret_ref: new_secret_ref,
                        secret_encrypted: new_secret_encrypted,
                        tier: None,
                        priority: Some("primary".to_string()),
                        source: Some(source),
                        notes: None,
                        rate_limit: None,
                    });
                } else if let Some(ref sref) = new_secret_ref {
                    let _ = store.delete(sref);
                }
            }
        }
        k if k.starts_with("ai.") && k.ends_with(".base_url") => {
            let provider = extract_provider(k)?;
            let providers = config.providers.get_or_insert_with(HashMap::new);
            let entry = providers.entry(provider).or_default();
            entry.base_url = Some(value.to_string());
        }
        _ => return Err(anyhow::anyhow!("Unknown AI config key: {}", key)),
    }
    Ok(())
}

pub(super) fn read_from_config(
    key: &str,
    config: &LlmRouterConfig,
    encryptor: &KeyEncryptor,
    store: &dyn SecretStore,
) -> Option<String> {
    match key {
        "ai.default_model" => config.orchestrator.as_ref().map(|o| o.model.clone()),
        "ai.fallback_models" => config
            .orchestrator
            .as_ref()
            .and_then(|o| o.fallback_models.as_ref())
            .map(|m| m.join(", ")),
        "ai.claude_code.discovery" => config
            .credential_discovery
            .as_ref()
            .and_then(|cd| cd.claude_code)
            .map(|b| b.to_string()),
        "ai.codex.discovery" => config
            .credential_discovery
            .as_ref()
            .and_then(|cd| cd.codex)
            .map(|b| b.to_string()),
        "ai.claude_code.cli_enabled" => config
            .cli_backends
            .as_ref()
            .and_then(|cb| cb.claude_code.as_ref())
            .and_then(|cc| cc.enabled)
            .map(|b| b.to_string()),
        "ai.claude_code.cli_path" => config
            .cli_backends
            .as_ref()
            .and_then(|cb| cb.claude_code.as_ref())
            .and_then(|cc| cc.path.clone()),
        "ai.codex.cli_enabled" => config
            .cli_backends
            .as_ref()
            .and_then(|cb| cb.codex.as_ref())
            .and_then(|cx| cx.enabled)
            .map(|b| b.to_string()),
        "ai.codex.cli_path" => config
            .cli_backends
            .as_ref()
            .and_then(|cb| cb.codex.as_ref())
            .and_then(|cx| cx.path.clone()),
        "ai.embeddings.enabled" => config.embeddings.as_ref().map(|e| e.enabled.to_string()),
        "ai.embeddings.provider" => config
            .embeddings
            .as_ref()
            .filter(|e| !e.provider.is_empty())
            .map(|e| e.provider.clone()),
        "ai.embeddings.model" => config.embeddings.as_ref().and_then(|e| e.model.clone()),
        "ai.embeddings.dimensions" => config
            .embeddings
            .as_ref()
            .and_then(|e| e.dimensions)
            .map(|d| d.to_string()),
        "ai.web_search.api_key" => config
            .web_search
            .as_ref()
            .filter(|ws| !ws.api_key.is_empty())
            .map(|ws| ws.api_key.clone()),
        "ai.web_search.timeout_secs" => config
            .web_search
            .as_ref()
            .map(|ws| ws.timeout_secs.to_string()),
        k if k.ends_with(".enabled") => {
            let provider = extract_provider(k).ok()?;
            config
                .providers
                .as_ref()?
                .get(&provider)?
                .enabled
                .map(|b| b.to_string())
        }
        k if k.ends_with(".api_key") => {
            let provider = extract_provider(k).ok()?;
            let keys = config.providers.as_ref()?.get(&provider)?.keys.as_ref()?;
            let cli_id = format!("{}_cli", provider);
            let key = keys
                .iter()
                .find(|k| k.id == cli_id)
                .or_else(|| if keys.len() == 1 { keys.first() } else { None })
                .or_else(|| {
                    keys.iter()
                        .find(|k| k.priority.as_deref() == Some("primary"))
                })
                .or_else(|| keys.first())?;
            if let Some(ref sref) = key.secret_ref {
                return store.get(sref).ok().flatten();
            }
            if let Some(ref encrypted) = key.secret_encrypted {
                return encryptor.decrypt(encrypted).ok();
            }
            if let Some(ref env_var) = key.secret_env {
                return std::env::var(env_var).ok();
            }
            None
        }
        k if k.ends_with(".base_url") => {
            let provider = extract_provider(k).ok()?;
            config.providers.as_ref()?.get(&provider)?.base_url.clone()
        }
        _ => None,
    }
}
