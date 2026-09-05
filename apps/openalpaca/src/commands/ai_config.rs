//! Bridge between the `config` CLI subcommand and `config/llm.toml`.
//!
//! Reads/writes AI configuration directly via `openalpaca_llm` config helpers
//! and OS keychain (`SecretStore`), without requiring a running daemon.

use anyhow::{Context, Result};
use openalpaca_llm::config::{KeyConfig, read_config, write_config};
use std::collections::HashMap;

use super::ai_config_helpers::*;

/// Get a single AI config value by key. Decrypts api_keys.
pub fn get_ai_value(key: &str) -> Result<Option<String>> {
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let encryptor = encryptor()?;
    let store = secret_store();
    Ok(read_from_config(key, &config, &encryptor, &store))
}

/// Set a single AI config value. Stores api_keys in OS keychain. Creates llm.toml if missing.
pub fn set_ai_value(key: &str, value: &str) -> Result<()> {
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
        "ai.web_search.api_key" => {
            if let Some(ref mut ws) = config.web_search {
                ws.api_key = String::new();
            }
        }
        "ai.web_search.timeout_secs" => {
            if let Some(ref mut ws) = config.web_search {
                ws.timeout_secs = 15; // default
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
                    if k.id == cli_id
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
                } else if !keys.is_empty() {
                    // 2. No cli key → remove first primary, else first
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
pub fn list_ai_entries() -> Result<Vec<(String, String, String)>> {
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let encryptor = encryptor()?;
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
        "ai.web_search.api_key",
        "ai.web_search.timeout_secs",
    ];

    let mut entries = Vec::new();
    for key in &keys {
        if let Some(val) = read_from_config(key, &config, &encryptor, &store) {
            let kind = if key.ends_with(".enabled")
                || key.ends_with(".discovery")
                || key.ends_with(".cli_enabled")
            {
                "bool"
            } else if key.ends_with(".dimensions") || key.ends_with(".timeout_secs") {
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
    config.security = None;
    config.timeouts = None;
    config.endpoints = None;
    config.env_vars = None;
    config.web_search = None;

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

/// Upsert (add or update) a single provider key. Writes to disk immediately.
pub fn upsert_provider_key(
    provider: &str,
    key_id: &str,
    secret: &str,
    source: Option<&str>,
    priority: Option<&str>,
    tier: Option<&str>,
    notes: Option<&str>,
) -> Result<()> {
    let path = llm_config_path()?;
    let mut config = load_or_default(&path)?;
    let store = secret_store();
    let use_kc = is_keychain_enabled();

    let (new_secret_ref, new_secret_encrypted) = if use_kc {
        let sref = format!("llm/{}/{}", provider, uuid::Uuid::new_v4());
        store
            .set(&sref, secret)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        (Some(sref), None)
    } else {
        let encryptor = encryptor()?;
        let encrypted = encryptor
            .encrypt(secret)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        (None, Some(encrypted))
    };

    let providers = config.providers.get_or_insert_with(HashMap::new);
    let entry = providers
        .entry(provider.to_string())
        .or_insert_with(openalpaca_llm::config::ProviderConfig::default);
    let keys = entry.keys.get_or_insert_with(Vec::new);

    if let Some(existing) = keys.iter_mut().find(|k| k.id == key_id) {
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
            priority: Some(priority.unwrap_or("primary").to_string()),
            source: Some(source.unwrap_or("api_console").to_string()),
            notes: notes.map(|n| n.to_string()),
            rate_limit: None,
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
    let path = llm_config_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let config = read_config(&path).map_err(|e| anyhow::anyhow!("{}", e))?;
    let encryptor = encryptor()?;
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
