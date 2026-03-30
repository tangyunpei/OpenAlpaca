use crate::config::ProviderConfig;
use crate::keys::key_encryption::KeyEncryptor;
use crate::keys::key_pool::{
    ApiKey, KeyPool, KeyPriority, KeySource, ProviderType, SelectionStrategy,
};
use crate::keys::secret_store::SecretStore;

/// Build a `KeyPool` from a `ProviderConfig` without needing an `LlmSettingsService` instance.
///
/// Used by the hot-reload handler in main.rs to rebuild key pools when llm.toml changes.
/// Resolves secrets via env vars and secret_store (keychain). Encrypted keys require
/// a `KeyEncryptor` which is lazily loaded.
pub fn build_key_pool_from_provider_config(
    provider_config: &ProviderConfig,
    provider_type: ProviderType,
    secret_store: Option<&dyn SecretStore>,
) -> Result<KeyPool, String> {
    let api_keys =
        build_api_keys_from_provider_config(provider_config, provider_type, secret_store)?;

    let strategy_str = provider_config
        .key_selection_strategy
        .as_deref()
        .or(provider_config.strategy.as_deref());
    let strategy = match strategy_str {
        Some("lru") | Some("least_recently_used") => SelectionStrategy::LeastRecentlyUsed,
        Some("primary_fallback") => SelectionStrategy::PrimaryFallback,
        _ => SelectionStrategy::RoundRobin,
    };

    Ok(KeyPool::new(api_keys, strategy))
}

/// Build a `Vec<ApiKey>` from a `ProviderConfig` without needing an `LlmSettingsService` instance.
fn build_api_keys_from_provider_config(
    provider_config: &ProviderConfig,
    provider_type: ProviderType,
    secret_store: Option<&dyn SecretStore>,
) -> Result<Vec<ApiKey>, String> {
    let mut api_keys = Vec::new();

    if let Some(ref keys) = provider_config.keys {
        // Lazily load encryptor only if needed
        let mut encryptor: Option<KeyEncryptor> = None;

        for key_config in keys {
            let secret = if let Some(ref env_var) = key_config.secret_env {
                std::env::var(env_var).map_err(|_| {
                    format!("Missing env var '{}' for key '{}'", env_var, key_config.id)
                })?
            } else if let Some(ref sref) = key_config.secret_ref {
                match secret_store {
                    Some(store) => store.get(sref)?.ok_or_else(|| {
                        format!(
                            "Secret '{}' not found in keychain for key '{}'",
                            sref, key_config.id
                        )
                    })?,
                    None => {
                        return Err(format!(
                            "No secret store available to resolve '{}' for key '{}'",
                            sref, key_config.id
                        ));
                    }
                }
            } else if let Some(ref encrypted) = key_config.secret_encrypted {
                if KeyEncryptor::is_encrypted(encrypted) {
                    let enc = match encryptor {
                        Some(ref e) => e,
                        None => {
                            encryptor = Some(KeyEncryptor::load_or_generate()?);
                            encryptor.as_ref().unwrap()
                        }
                    };
                    enc.decrypt(encrypted)
                        .map_err(|e| format!("Failed to decrypt key '{}': {e}", key_config.id))?
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
