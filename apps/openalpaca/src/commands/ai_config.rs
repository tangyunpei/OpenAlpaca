//! Bridge between the `config` CLI subcommand and `config/llm.toml`.
//!
//! Reads/writes AI configuration directly via `openalpaca_llm` config helpers
//! and OS keychain (`SecretStore`), without requiring a running daemon.

use anyhow::{Context, Result};
use openalpaca_llm::cli_backend::{CliBackendConfig, CliBackendsConfig};
use openalpaca_llm::config::{
    EmbeddingsConfig, KeyConfig, LlmRouterConfig, OrchestratorLlmConfig, ProviderConfig,
    read_config, write_config,
};
use openalpaca_llm::keys::credential_discovery::CredentialDiscoveryConfig;
use openalpaca_llm::keys::key_encryption::KeyEncryptor;
use openalpaca_llm::keys::secret_store::{KeyringSecretStore, MemorySecretStore, SecretStore};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Ensure master key exists at canonical app_dir before any crypto operations.
fn ensure_master_key() {
    if let Ok(app_dir) = openalpaca_storage::paths::app_dir() {
        let _ = KeyEncryptor::ensure_at(&app_dir);
    }
}

/// Check whether OS keychain is enabled via `[security] use_keychain` in llm.toml.
fn is_keychain_enabled() -> bool {
    llm_config_path()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| read_config(&p).ok())
        .and_then(|c| c.security.as_ref().map(|s| s.use_keychain))
        .unwrap_or(false)
}

/// Get the secret store matching the current config.
/// - `use_keychain = true`: OS keychain (KeyringSecretStore)
/// - `use_keychain = false` (default): in-memory store (keys resolved via secret_encrypted)
fn secret_store() -> Box<dyn SecretStore> {
    if is_keychain_enabled() {
        Box::new(KeyringSecretStore)
    } else {
        Box::new(MemorySecretStore::new())
    }
}

/// Returns the path to `config/llm.toml`.
///
/// Resolution order:
/// 1. `OPENALPACA_CONFIG_DIR` env var override
/// 2. `app_dir()/config/llm.toml` (writable, created if needed)
/// 3. Walk up from CWD looking for `config/llm.toml` (dev/repo fallback)
pub fn llm_config_path() -> Result<PathBuf> {
    // 1. OPENALPACA_CONFIG_DIR override
    if let Ok(dir) = std::env::var("OPENALPACA_CONFIG_DIR") {
        let p = PathBuf::from(&dir);
        if p.is_dir() {
            return Ok(p.join("llm.toml"));
        }
    }
    // 2. Writable app_dir/config/
    if let Ok(app) = openalpaca_storage::paths::app_dir() {
        let p = app.join("config").join("llm.toml");
        if p.exists() {
            return Ok(p);
        }
    }
    // 3. Walk up from CWD (dev/repo)
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    Ok(cwd.join("config").join("llm.toml"))
}

/// Get a single AI config value by key. Decrypts api_keys.
pub fn get_ai_value(key: &str) -> Result<Option<String>> {
    ensure_master_key();
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let encryptor = KeyEncryptor::load_or_generate().map_err(|e| anyhow::anyhow!("{}", e))?;
    let store = secret_store();
    Ok(read_from_config(key, &config, &encryptor, &store))
}

/// Set a single AI config value. Stores api_keys in OS keychain. Creates llm.toml if missing.
pub fn set_ai_value(key: &str, value: &str) -> Result<()> {
    ensure_master_key();
    let path = llm_config_path()?;
    let mut config = load_or_default(&path)?;
    let store = secret_store();

    apply_to_config(key, value, &mut config, &store, None)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    write_config(&path, &config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

/// Set multiple AI config values in a single read+write (for TUI batch save).
///
/// Entries with key `"__source:<real_key>"` are treated as source-hint metadata
/// for the corresponding `<real_key>`, not as config values themselves.
pub fn set_ai_values_batch(entries: &[(&str, &str)]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    ensure_master_key();
    let path = llm_config_path()?;
    let mut config = load_or_default(&path)?;
    let store = secret_store();

    // Build a source-hint lookup from companion entries
    let source_hints: HashMap<&str, &str> = entries
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("__source:").map(|real_key| (real_key, *v)))
        .collect();

    for (key, value) in entries {
        // Skip metadata companion entries
        if key.starts_with("__source:") {
            continue;
        }
        let source: Option<&str> = source_hints.get(*key).copied();
        apply_to_config(key, value, &mut config, &store, source)?;
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    write_config(&path, &config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

/// Delete/reset a single AI config value.
///
/// - `api_key` → removes `{provider}_cli` key (with migration fallback)
/// - `enabled` → sets to `None`
/// - `model`/`fallback`/`base_url` → sets to `None`
pub fn delete_ai_value(key: &str) -> Result<()> {
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let store = secret_store();

    match key {
        "ai.default_model" => {
            if let Some(ref mut orch) = config.orchestrator {
                orch.model = "claude-sonnet-4-5-20250929".to_string();
            }
        }
        "ai.fallback_models" => {
            if let Some(ref mut orch) = config.orchestrator {
                orch.fallback_models = None;
            }
        }
        "ai.claude_code.discovery" => {
            if let Some(ref mut cd) = config.credential_discovery {
                cd.claude_code = None;
            }
        }
        "ai.codex.discovery" => {
            if let Some(ref mut cd) = config.credential_discovery {
                cd.codex = None;
            }
        }
        "ai.claude_code.cli_enabled" => {
            if let Some(ref mut cb) = config.cli_backends
                && let Some(ref mut cc) = cb.claude_code
            {
                cc.enabled = None;
            }
        }
        "ai.claude_code.cli_path" => {
            if let Some(ref mut cb) = config.cli_backends
                && let Some(ref mut cc) = cb.claude_code
            {
                cc.path = None;
            }
        }
        "ai.codex.cli_enabled" => {
            if let Some(ref mut cb) = config.cli_backends
                && let Some(ref mut cx) = cb.codex
            {
                cx.enabled = None;
            }
        }
        "ai.codex.cli_path" => {
            if let Some(ref mut cb) = config.cli_backends
                && let Some(ref mut cx) = cb.codex
            {
                cx.path = None;
            }
        }
        "ai.embeddings.enabled" => {
            if let Some(ref mut emb) = config.embeddings {
                emb.enabled = false;
            }
        }
        "ai.embeddings.provider" => {
            if let Some(ref mut emb) = config.embeddings {
                emb.provider = String::new();
            }
        }
        "ai.embeddings.model" => {
            if let Some(ref mut emb) = config.embeddings {
                emb.model = None;
            }
        }
        "ai.embeddings.dimensions" => {
            if let Some(ref mut emb) = config.embeddings {
                emb.dimensions = None;
            }
        }
        k if k.ends_with(".enabled") => {
            let provider = extract_provider(k)?;
            if let Some(ref mut providers) = config.providers
                && let Some(entry) = providers.get_mut(&provider)
            {
                entry.enabled = None;
            }
        }
        k if k.ends_with(".api_key") => {
            let provider = extract_provider(k)?;
            if let Some(ref mut providers) = config.providers
                && let Some(entry) = providers.get_mut(&provider)
                && let Some(ref mut keys) = entry.keys
            {
                let cli_id = format!("{}_cli", provider);
                // Delete keychain secrets for removed keys
                for k in keys.iter() {
                    if (k.id == cli_id || keys.len() == 1)
                        && let Some(ref sref) = k.secret_ref
                    {
                        let _ = store.delete(sref);
                    }
                }
                // 1. Try to remove {provider}_cli key
                let before = keys.len();
                keys.retain(|k| k.id != cli_id);
                if keys.len() < before {
                    // Removed cli key — done
                } else if keys.len() == 1 {
                    // 2. Migration fallback: exactly one key (legacy) → remove it
                    if let Some(ref sref) = keys[0].secret_ref {
                        let _ = store.delete(sref);
                    }
                    keys.remove(0);
                } else if !keys.is_empty() {
                    // 3. Multiple keys, no cli key → remove first primary, else first
                    let pos = keys
                        .iter()
                        .position(|k| k.priority.as_deref() == Some("primary"))
                        .unwrap_or(0);
                    if let Some(ref sref) = keys[pos].secret_ref {
                        let _ = store.delete(sref);
                    }
                    keys.remove(pos);
                }
                if !keys.is_empty() {
                    eprintln!(
                        "Note: {} key(s) remain for '{}'. Use `config` → API-Keys → {} to manage individual keys.",
                        keys.len(),
                        provider,
                        provider
                    );
                }
            }
        }
        k if k.ends_with(".base_url") => {
            let provider = extract_provider(k)?;
            if let Some(ref mut providers) = config.providers
                && let Some(entry) = providers.get_mut(&provider)
            {
                entry.base_url = None;
            }
        }
        _ => return Err(anyhow::anyhow!("Unknown AI config key: {}", key)),
    }

    write_config(&path, &config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

/// List all currently-set AI values. Returns `(key, value, kind)`.
/// API keys are returned decrypted.
pub fn list_ai_entries() -> Result<Vec<(String, String, String)>> {
    ensure_master_key();
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let encryptor = KeyEncryptor::load_or_generate().map_err(|e| anyhow::anyhow!("{}", e))?;
    let store = secret_store();

    let keys = [
        "ai.default_model",
        "ai.fallback_models",
        "ai.embeddings.enabled",
        "ai.embeddings.provider",
        "ai.embeddings.model",
        "ai.embeddings.dimensions",
        "ai.anthropic.enabled",
        "ai.anthropic.api_key",
        "ai.openai.enabled",
        "ai.openai.api_key",
        "ai.ollama.enabled",
        "ai.ollama.base_url",
        "ai.claude_code.discovery",
        "ai.claude_code.cli_enabled",
        "ai.claude_code.cli_path",
        "ai.codex.discovery",
        "ai.codex.cli_enabled",
        "ai.codex.cli_path",
    ];

    let mut entries = Vec::new();
    for key in &keys {
        if let Some(val) = read_from_config(key, &config, &encryptor, &store) {
            let kind = if key.ends_with(".enabled")
                || key.ends_with(".discovery")
                || key.ends_with(".cli_enabled")
            {
                "bool"
            } else if key.ends_with(".dimensions") {
                "int"
            } else {
                "string"
            };
            entries.push((key.to_string(), val, kind.to_string()));
        }
    }
    Ok(entries)
}

/// Clear all AI/LLM configuration in `llm.toml`, resetting to defaults.
///
/// Deletes keychain secrets for provider keys, then sets every section to `None`.
/// The runtime will fall back to built-in defaults on next load.
pub fn clear_ai_config() -> Result<()> {
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let mut config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;

    // Delete keychain secrets before clearing providers
    if let Some(ref mut providers) = config.providers {
        let store = secret_store();
        for (_name, prov) in providers.iter_mut() {
            if let Some(ref keys) = prov.keys {
                for k in keys {
                    if let Some(ref sref) = k.secret_ref {
                        let _ = store.delete(sref);
                    }
                }
            }
        }
    }

    // Clear all sections
    config.orchestrator = None;
    config.providers = None;
    config.credential_discovery = None;
    config.cli_backends = None;
    config.embeddings = None;
    config.models = None;
    config.fallback_chains = None;
    config.limits = None;
    config.security = None;
    config.timeouts = None;
    config.endpoints = None;
    config.env_vars = None;

    write_config(&path, &config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

// ── Multi-key helpers (disk-immediate) ──────────────────────────────────

/// Key display info returned by `list_provider_keys()`.
pub struct ProviderKeyInfo {
    pub id: String,
    pub display_secret: String,
    pub source: String,
    pub priority: String,
    pub tier: String,
}

/// Mask a key for display — shows `"sk-...abcd"` (last 4 chars).
fn mask_key_value(secret: &str) -> String {
    if secret.len() <= 4 {
        return "*".repeat(secret.len());
    }
    let suffix = &secret[secret.len() - 4..];
    format!("sk-...{}", suffix)
}

/// Upsert (add or update) a single provider key. Writes to disk immediately.
/// Stores secret via OS keychain (when enabled) or local encrypted storage.
pub fn upsert_provider_key(
    provider: &str,
    key_id: &str,
    secret: &str,
    source: Option<&str>,
    priority: Option<&str>,
    tier: Option<&str>,
    notes: Option<&str>,
) -> Result<()> {
    ensure_master_key();
    let path = llm_config_path()?;
    let mut config = load_or_default(&path)?;
    let store = secret_store();
    let use_kc = is_keychain_enabled();

    // Store secret via keychain or local encryption
    let (new_secret_ref, new_secret_encrypted) = if use_kc {
        let sref = format!("llm/{}/{}", provider, uuid::Uuid::new_v4());
        store
            .set(&sref, secret)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        (Some(sref), None)
    } else {
        let encryptor = KeyEncryptor::load_or_generate().map_err(|e| anyhow::anyhow!("{}", e))?;
        let encrypted = encryptor
            .encrypt(secret)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        (None, Some(encrypted))
    };

    let providers = config.providers.get_or_insert_with(HashMap::new);
    let entry = providers
        .entry(provider.to_string())
        .or_insert_with(ProviderConfig::default);
    let keys = entry.keys.get_or_insert_with(Vec::new);

    if let Some(existing) = keys.iter_mut().find(|k| k.id == key_id) {
        // Delete old keychain secret if present
        if let Some(ref old_ref) = existing.secret_ref {
            let _ = store.delete(old_ref);
        }
        existing.secret_ref = new_secret_ref;
        existing.secret_encrypted = new_secret_encrypted;
        if let Some(s) = source {
            existing.source = Some(s.to_string());
        }
        if let Some(p) = priority {
            existing.priority = Some(p.to_string());
        }
        if let Some(t) = tier {
            existing.tier = Some(t.to_string());
        }
        if let Some(n) = notes {
            existing.notes = Some(n.to_string());
        }
    } else {
        keys.push(KeyConfig {
            id: key_id.to_string(),
            secret_env: None,
            secret_ref: new_secret_ref,
            secret_encrypted: new_secret_encrypted,
            tier: tier.map(|t| t.to_string()),
            monthly_budget: None,
            priority: Some(priority.unwrap_or("primary").to_string()),
            source: Some(source.unwrap_or("api_console").to_string()),
            notes: notes.map(|n| n.to_string()),
            rate_limit: None,
            allowed_models: None,
        });
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    write_config(&path, &config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

/// List all keys for a provider from disk.
pub fn list_provider_keys(provider: &str) -> Result<Vec<ProviderKeyInfo>> {
    ensure_master_key();
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let encryptor = KeyEncryptor::load_or_generate().map_err(|e| anyhow::anyhow!("{}", e))?;
    let store = secret_store();

    let keys = match config
        .providers
        .as_ref()
        .and_then(|p| p.get(provider))
        .and_then(|pc| pc.keys.as_ref())
    {
        Some(keys) => keys,
        None => return Ok(Vec::new()),
    };

    let mut result = Vec::new();
    for k in keys {
        let display_secret = if let Some(ref sref) = k.secret_ref {
            match store.get(sref) {
                Ok(Some(plain)) => mask_key_value(&plain),
                Ok(None) => "(not in keychain)".to_string(),
                Err(_) => "(keychain error)".to_string(),
            }
        } else if let Some(ref enc) = k.secret_encrypted {
            match encryptor.decrypt(enc) {
                Ok(plain) => mask_key_value(&plain),
                Err(_) => "(decrypt error)".to_string(),
            }
        } else if let Some(ref env_var) = k.secret_env {
            format!("env:${}", env_var)
        } else {
            "(no secret)".to_string()
        };

        result.push(ProviderKeyInfo {
            id: k.id.clone(),
            display_secret,
            source: k.source.clone().unwrap_or_else(|| "other".to_string()),
            priority: k.priority.clone().unwrap_or_else(|| "primary".to_string()),
            tier: k.tier.clone().unwrap_or_else(|| "-".to_string()),
        });
    }
    Ok(result)
}

/// Remove a specific key by ID from a provider. Writes to disk immediately.
pub fn remove_provider_key(provider: &str, key_id: &str) -> Result<()> {
    let path = llm_config_path()?;
    if !path.exists() {
        return Err(anyhow::anyhow!("Config file not found"));
    }
    let mut config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let store = secret_store();

    if let Some(ref mut providers) = config.providers
        && let Some(entry) = providers.get_mut(provider)
        && let Some(ref mut keys) = entry.keys
    {
        // Delete keychain secret before removing
        for k in keys.iter() {
            if k.id == key_id
                && let Some(ref sref) = k.secret_ref
            {
                let _ = store.delete(sref);
            }
        }
        let before = keys.len();
        keys.retain(|k| k.id != key_id);
        if keys.len() == before {
            return Err(anyhow::anyhow!(
                "Key '{}' not found in provider '{}'",
                key_id,
                provider
            ));
        }
    }

    write_config(&path, &config).map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

// ── Internal helpers ────────────────────────────────────────────────────

fn load_or_default(path: &Path) -> Result<LlmRouterConfig> {
    if path.exists() {
        read_config(path).map_err(|e| anyhow::anyhow!("{}", e))
    } else {
        Ok(default_config())
    }
}

fn default_config() -> LlmRouterConfig {
    LlmRouterConfig {
        orchestrator: Some(OrchestratorLlmConfig {
            model: "claude-sonnet-4-5-20250929".to_string(),
            fallback_models: None,
        }),
        providers: Some(HashMap::new()),
        ..Default::default()
    }
}

fn apply_to_config(
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
                    batch_size: None,
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
                    batch_size: None,
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
                    batch_size: None,
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
                    batch_size: None,
                })
                .dimensions = Some(dim);
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

            // Determine how to store the secret
            let (new_secret_ref, new_secret_encrypted) = if use_kc {
                // Store in OS keychain via secret_ref
                let sref = format!("llm/{}/{}", provider, uuid::Uuid::new_v4());
                store
                    .set(&sref, value)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                (Some(sref), None)
            } else {
                // Store as local encrypted value
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
                // Delete old keychain secret if present
                if let Some(ref old_ref) = existing.secret_ref {
                    let _ = store.delete(old_ref);
                }
                existing.secret_ref = new_secret_ref;
                existing.secret_encrypted = new_secret_encrypted;
                existing.source = Some(source);
                existing.priority = Some("primary".to_string());
            } else {
                // Before creating a new entry, check if this exact secret is
                // already stored under a different key ID (avoids duplicates
                // when save_and_exit round-trips a value read from disk).
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
                        monthly_budget: None,
                        priority: Some("primary".to_string()),
                        source: Some(source),
                        notes: None,
                        rate_limit: None,
                        allowed_models: None,
                    });
                } else if let Some(ref sref) = new_secret_ref {
                    // Clean up the unused keychain entry
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

fn extract_provider(key: &str) -> Result<String> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() >= 3 {
        Ok(parts[1].to_string())
    } else {
        Err(anyhow::anyhow!("Invalid AI key format: {}", key))
    }
}

fn read_from_config(
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
            // 1. Find key with id == "{provider}_cli"
            let key = keys
                .iter()
                .find(|k| k.id == cli_id)
                // 2. Fallback: if exactly one key, use it
                .or_else(|| if keys.len() == 1 { keys.first() } else { None })
                // 3. Fallback: first key with priority == "primary"
                .or_else(|| {
                    keys.iter()
                        .find(|k| k.priority.as_deref() == Some("primary"))
                })
                // 4. Fallback: first key
                .or_else(|| keys.first())?;
            // Resolve: secret_ref > secret_encrypted > secret_env
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
