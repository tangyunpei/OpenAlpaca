use crate::config::{KeyConfig, ProviderConfig};
use crate::keys::key_encryption::KeyEncryptor;
use crate::keys::key_pool::{
    ApiKey, KeyPool, KeyPriority, KeySource, ProviderType, SelectionStrategy,
};
use crate::keys::secret_store::SecretStore;

/// Copy every metadata field from a `KeyConfig` onto an `ApiKey`.
///
/// This is the single source of truth for key-config → ApiKey field mapping,
/// shared by the boot path (`llm_config::router_builder`) and both runtime
/// rebuild paths (this module's hot-reload builder and
/// `LlmSettingsService::build_api_keys_from_config`). Keeping one copy
/// guarantees a hot-reload can never silently drop per-key settings such as
/// `rate_limit`.
pub(crate) fn apply_key_config_metadata(api_key: &mut ApiKey, key_config: &KeyConfig) {
    api_key.tier = key_config.tier.clone();
    api_key.rate_limit = key_config.rate_limit;
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
}

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
                            encryptor = Some(KeyEncryptor::from_env()?);
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
            apply_key_config_metadata(&mut api_key, key_config);
            api_keys.push(api_key);
        }
    }

    Ok(api_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_key_config(secret_env: &str) -> KeyConfig {
        KeyConfig {
            id: "key-1".to_string(),
            secret_env: Some(secret_env.to_string()),
            secret_ref: None,
            secret_encrypted: None,
            tier: Some("tier-4".to_string()),
            priority: Some("fallback".to_string()),
            source: Some("api_console".to_string()),
            notes: Some("note".to_string()),
            rate_limit: Some(42),
        }
    }

    #[test]
    fn apply_key_config_metadata_copies_all_fields() {
        let key_config = full_key_config("UNUSED");
        let mut api_key = ApiKey::new(
            "key-1".to_string(),
            ProviderType::Anthropic,
            "sk-test".to_string(),
        );
        apply_key_config_metadata(&mut api_key, &key_config);

        assert_eq!(api_key.tier, Some("tier-4".to_string()));
        assert_eq!(api_key.rate_limit, Some(42));
        assert_eq!(api_key.priority, KeyPriority::Fallback);
        assert_eq!(api_key.source, KeySource::ApiConsole);
        assert_eq!(api_key.notes, Some("note".to_string()));
    }

    /// Parity regression test: the hot-reload rebuild path must produce
    /// `ApiKey`s field-identical to the boot path for the same config.
    /// Both paths now share `apply_key_config_metadata`; this asserts the
    /// reload builder's output against a boot-style key built from the same
    /// `KeyConfig` — in particular the previously dropped `rate_limit`.
    #[test]
    fn hot_reload_builder_matches_boot_path_fields() {
        const ENV_VAR: &str = "OPENALPACA_TEST_KEY_POOL_PARITY_SECRET";
        // SAFETY: test-only env mutation with a test-unique variable name.
        unsafe { std::env::set_var(ENV_VAR, "sk-parity-secret") };

        let key_config = full_key_config(ENV_VAR);
        let provider_config = ProviderConfig {
            enabled: Some(true),
            base_url: None,
            strategy: None,
            key_selection_strategy: None,
            keys: Some(vec![key_config.clone()]),
            default_model: None,
            default_max_tokens: None,
        };

        let reload_keys = build_api_keys_from_provider_config(
            &provider_config,
            ProviderType::Anthropic,
            None,
        )
        .expect("reload path should build keys");
        assert_eq!(reload_keys.len(), 1);
        let reloaded = &reload_keys[0];

        // Boot-path equivalent (router_builder uses the same secret + helper).
        let mut boot_key = ApiKey::new(
            key_config.id.clone(),
            ProviderType::Anthropic,
            "sk-parity-secret".to_string(),
        );
        apply_key_config_metadata(&mut boot_key, &key_config);

        assert_eq!(reloaded.id, boot_key.id);
        assert_eq!(reloaded.provider, boot_key.provider);
        assert_eq!(reloaded.secret, boot_key.secret);
        assert_eq!(reloaded.tier, boot_key.tier);
        assert_eq!(reloaded.rate_limit, boot_key.rate_limit);
        assert_eq!(reloaded.priority, boot_key.priority);
        assert_eq!(reloaded.source, boot_key.source);
        assert_eq!(reloaded.notes, boot_key.notes);

        unsafe { std::env::remove_var(ENV_VAR) };
    }
}
