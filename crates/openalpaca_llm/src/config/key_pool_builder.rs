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
    let api_keys = KeyResolver::new(secret_store, None)
        .resolve_all(Some(provider_config), &provider_type)?;

    Ok(KeyPool::new(
        api_keys,
        selection_strategy(Some(provider_config)),
    ))
}

pub(crate) fn selection_strategy(config: Option<&ProviderConfig>) -> SelectionStrategy {
    match config.and_then(|p| {
        p.key_selection_strategy
            .as_deref()
            .or(p.strategy.as_deref())
    }) {
        Some("lru") | Some("least_recently_used") => SelectionStrategy::LeastRecentlyUsed,
        Some("primary_fallback") => SelectionStrategy::PrimaryFallback,
        _ => SelectionStrategy::RoundRobin,
    }
}

/// Resolves one configured key; callers choose whether a failure skips a key
/// (startup) or rejects the entire update (settings and hot reload).
pub(crate) struct KeyResolver<'a> {
    secret_store: Option<&'a dyn SecretStore>,
    encryptor: Option<&'a KeyEncryptor>,
    loaded_encryptor: Option<KeyEncryptor>,
}

impl<'a> KeyResolver<'a> {
    pub(crate) fn new(
        secret_store: Option<&'a dyn SecretStore>,
        encryptor: Option<&'a KeyEncryptor>,
    ) -> Self {
        Self {
            secret_store,
            encryptor,
            loaded_encryptor: None,
        }
    }

    pub(crate) fn resolve(
        &mut self,
        key: &KeyConfig,
        provider: &ProviderType,
    ) -> Result<ApiKey, String> {
        let secret = if let Some(env_var) = &key.secret_env {
            std::env::var(env_var)
                .map_err(|_| format!("Missing env var '{}' for key '{}'", env_var, key.id))?
        } else if let Some(secret_ref) = &key.secret_ref {
            let store = self.secret_store.ok_or_else(|| {
                format!(
                    "No secret store available to resolve '{}' for key '{}'",
                    secret_ref, key.id
                )
            })?;
            store.get(secret_ref)?.ok_or_else(|| {
                format!(
                    "Secret '{}' not found in keychain for key '{}'",
                    secret_ref, key.id
                )
            })?
        } else if let Some(encrypted) = &key.secret_encrypted {
            if KeyEncryptor::is_encrypted(encrypted) {
                let encryptor = match self.encryptor {
                    Some(encryptor) => encryptor,
                    None => &*self.loaded_encryptor.insert(KeyEncryptor::from_env()?),
                };
                encryptor
                    .decrypt(encrypted)
                    .map_err(|e| format!("Failed to decrypt key '{}': {e}", key.id))?
            } else {
                encrypted.clone()
            }
        } else {
            return Err(format!("No secret for key '{}'", key.id));
        };
        let mut api_key = ApiKey::new(key.id.clone(), provider.clone(), secret);
        apply_key_config_metadata(&mut api_key, key);
        Ok(api_key)
    }

    pub(crate) fn resolve_all(
        &mut self,
        config: Option<&ProviderConfig>,
        provider: &ProviderType,
    ) -> Result<Vec<ApiKey>, String> {
        config
            .into_iter()
            .flat_map(|p| p.keys.iter().flatten())
            .map(|key| self.resolve(key, provider))
            .collect()
    }
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
            request_timeout_secs: None,
        };

        let reload_keys = KeyResolver::new(None, None)
            .resolve_all(Some(&provider_config), &ProviderType::Anthropic)
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

    #[test]
    fn secret_reference_takes_priority_and_does_not_fall_back_when_missing() {
        let store = crate::MemorySecretStore::new();
        store.set("fixture/key", "from-store").unwrap();
        let mut key = full_key_config("unused");
        key.secret_env = None;
        key.secret_ref = Some("fixture/key".into());
        key.secret_encrypted = Some("fallback-secret".into());
        let mut resolver = KeyResolver::new(Some(&store), None);
        assert_eq!(
            resolver
                .resolve(&key, &ProviderType::OpenAI)
                .unwrap()
                .secret,
            "from-store"
        );
        store.delete("fixture/key").unwrap();
        assert!(
            resolver
                .resolve(&key, &ProviderType::OpenAI)
                .unwrap_err()
                .contains("not found in keychain")
        );
        key.secret_ref = None;
        assert_eq!(
            resolver
                .resolve(&key, &ProviderType::OpenAI)
                .unwrap()
                .secret,
            "fallback-secret"
        );
    }

    #[test]
    fn supplied_encryptor_decrypts_without_loading_another_master_key() {
        let dir = tempfile::tempdir().unwrap();
        let encryptor = KeyEncryptor::load_or_generate_at(dir.path()).unwrap();
        let mut key = full_key_config("unused");
        key.secret_env = None;
        key.secret_encrypted = Some(encryptor.encrypt("encrypted-fixture").unwrap());
        let mut resolver = KeyResolver::new(None, Some(&encryptor));
        assert_eq!(
            resolver
                .resolve(&key, &ProviderType::Anthropic)
                .unwrap()
                .secret,
            "encrypted-fixture"
        );
        assert!(resolver.loaded_encryptor.is_none());
    }

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn boot_skips_a_missing_key_while_runtime_rebuild_rejects_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("llm.toml");
        let text = r#"
            [providers.openai]
            [[providers.openai.keys]]
            id = "missing"
            secret_ref = "fixture/missing"
            [[providers.openai.keys]]
            id = "valid"
            secret_ref = "fixture/valid"
            rate_limit = 42
        "#;
        std::fs::write(&path, text).unwrap();
        let store = crate::MemorySecretStore::new();
        store.set("fixture/valid", "sk-fixture").unwrap();
        let router = crate::build_router_with_secret_store(&path, Some(&store)).unwrap();
        let keys = router.key_statuses(&ProviderType::OpenAI).await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, "valid");
        let config: crate::LlmRouterConfig = toml::from_str(text).unwrap();
        let result = build_key_pool_from_provider_config(
            &config.providers.unwrap()["openai"],
            ProviderType::OpenAI,
            Some(&store),
        );
        assert!(matches!(result, Err(error) if error.contains("fixture/missing")));
    }
}
