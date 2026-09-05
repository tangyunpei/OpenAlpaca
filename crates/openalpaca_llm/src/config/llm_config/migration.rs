use tracing;

use super::router_config::{KeyConfig, LlmRouterConfig};
use super::{read_config, write_config};

/// Resolve a secret from a `KeyConfig` using the given secret store.
///
/// Resolution order: `secret_env` > `secret_ref` (keychain) > `secret_encrypted`
/// (AES-256-GCM local encryption — the fallback when no OS keychain is usable).
pub fn resolve_key_from_config(
    key_config: &KeyConfig,
    secret_store: Option<&dyn crate::keys::secret_store::SecretStore>,
) -> Option<String> {
    if let Some(ref env_var) = key_config.secret_env {
        return std::env::var(env_var).ok();
    }
    if let Some(ref sref) = key_config.secret_ref
        && let Some(store) = secret_store
        && let Ok(Some(s)) = store.get(sref)
    {
        return Some(s);
    }
    if let Some(ref encrypted) = key_config.secret_encrypted {
        if crate::keys::key_encryption::KeyEncryptor::is_encrypted(encrypted) {
            if let Ok(enc) = crate::keys::key_encryption::KeyEncryptor::from_env() {
                return enc.decrypt(encrypted).ok();
            }
        } else {
            return Some(encrypted.clone());
        }
    }
    None
}

/// Auto-migrate `secret_encrypted` → OS keychain (`secret_ref`).
///
/// For each key with `secret_encrypted`, decrypts the secret and stores it
/// in the OS keychain via `SecretStore`, then replaces `secret_encrypted`
/// with `secret_ref` in the config. Writes the updated config to disk.
///
/// Returns the number of secrets migrated.
pub fn migrate_llm_secrets(
    config_path: &std::path::Path,
    secret_store: &dyn crate::keys::secret_store::SecretStore,
) -> Result<u32, String> {
    // Check path is writable
    if config_path.exists() {
        let metadata = std::fs::metadata(config_path)
            .map_err(|e| format!("Cannot stat {}: {e}", config_path.display()))?;
        if metadata.permissions().readonly() {
            tracing::warn!(
                "Config at {} is read-only; skipping secret migration",
                config_path.display()
            );
            return Ok(0);
        }
    } else {
        return Ok(0);
    }

    let mut config = read_config(config_path)
        .map_err(|e| format!("Failed to read config for migration: {e}"))?;

    let encryptor = match crate::keys::key_encryption::KeyEncryptor::from_env() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Cannot load master key for migration: {e}");
            return Ok(0);
        }
    };

    let mut migrated = 0u32;

    if let Some(ref mut providers) = config.providers {
        for (provider_name, provider) in providers.iter_mut() {
            if let Some(ref mut keys) = provider.keys {
                for key in keys.iter_mut() {
                    // Skip if already has secret_ref or no secret_encrypted
                    if key.secret_ref.is_some() {
                        continue;
                    }
                    let Some(ref encrypted) = key.secret_encrypted else {
                        continue;
                    };
                    if !crate::keys::key_encryption::KeyEncryptor::is_encrypted(encrypted) {
                        continue;
                    }

                    let plaintext = match encryptor.decrypt(encrypted) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(
                                "Migration: cannot decrypt key '{}' for {}: {e}",
                                key.id,
                                provider_name
                            );
                            continue;
                        }
                    };

                    let sref = format!("llm/{}/{}", provider_name, uuid::Uuid::new_v4());

                    if let Err(e) = secret_store.set(&sref, &plaintext) {
                        tracing::warn!("Migration: cannot store key '{}' in keychain: {e}", key.id);
                        continue;
                    }

                    key.secret_ref = Some(sref);
                    key.secret_encrypted = None;
                    migrated += 1;
                }
            }
        }
    }

    if migrated > 0 {
        let _lock = crate::keys::key_encryption::acquire_config_write_lock(config_path)
            .map_err(|e| format!("Failed to acquire config lock for migration: {e}"))?;
        write_config(config_path, &config)
            .map_err(|e| format!("Failed to write migrated config: {e}"))?;
        tracing::info!("Migrated {migrated} secret(s) from llm.toml to OS keychain");
    }

    Ok(migrated)
}

/// Reverse-migrate `secret_ref` keys back to `secret_encrypted`.
///
/// For each key that has `secret_ref` (but no `secret_encrypted`), reads
/// the plaintext from the keychain, encrypts it locally, writes
/// `secret_encrypted`, and clears `secret_ref`.
///
/// Used when switching from keychain storage to local encrypted storage
/// (`[security] use_keychain = false`).
pub fn reverse_migrate_llm_secrets(
    config_path: &std::path::Path,
    secret_store: &dyn crate::keys::secret_store::SecretStore,
) -> Result<u32, String> {
    if !config_path.exists() {
        return Ok(0);
    }
    if let Ok(metadata) = std::fs::metadata(config_path)
        && metadata.permissions().readonly()
    {
        return Ok(0);
    }

    let mut config = read_config(config_path)
        .map_err(|e| format!("Failed to read config for reverse migration: {e}"))?;

    let encryptor = match crate::keys::key_encryption::KeyEncryptor::from_env() {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Cannot load master key for reverse migration: {e}");
            return Ok(0);
        }
    };

    let mut migrated = 0u32;

    if let Some(ref mut providers) = config.providers {
        for (provider_name, provider) in providers.iter_mut() {
            if let Some(ref mut keys) = provider.keys {
                for key in keys.iter_mut() {
                    // Skip if no secret_ref or already has secret_encrypted
                    let Some(ref sref) = key.secret_ref else {
                        continue;
                    };
                    if key.secret_encrypted.is_some() {
                        continue;
                    }

                    // Read plaintext from keychain
                    let plaintext = match secret_store.get(sref) {
                        Ok(Some(p)) => p,
                        Ok(None) => {
                            tracing::warn!(
                                "Reverse migration: secret_ref '{}' for key '{}' ({}) not found in keychain, skipping",
                                sref,
                                key.id,
                                provider_name
                            );
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Reverse migration: cannot read '{}' from keychain for key '{}': {e}",
                                sref,
                                key.id
                            );
                            continue;
                        }
                    };

                    // Encrypt locally
                    let encrypted = match encryptor.encrypt(&plaintext) {
                        Ok(enc) => enc,
                        Err(e) => {
                            tracing::warn!("Failed to encrypt key '{}': {e}", key.id);
                            continue;
                        }
                    };
                    key.secret_encrypted = Some(encrypted);
                    key.secret_ref = None;
                    migrated += 1;
                }
            }
        }
    }

    if migrated > 0 {
        let _lock = crate::keys::key_encryption::acquire_config_write_lock(config_path)
            .map_err(|e| format!("Failed to acquire config lock for reverse migration: {e}"))?;
        write_config(config_path, &config)
            .map_err(|e| format!("Failed to write reverse-migrated config: {e}"))?;
        tracing::info!(
            "Reverse-migrated {migrated} secret(s) from OS keychain to local encrypted storage"
        );
    }

    Ok(migrated)
}

/// Collect all unique `secret_ref` values from the config.
///
/// Used at startup to pre-fetch every keychain key in a single batch,
/// so that all macOS Keychain password prompts happen at once.
pub fn collect_secret_refs(config: &LlmRouterConfig) -> Vec<String> {
    let mut refs = Vec::new();
    if let Some(ref providers) = config.providers {
        for provider in providers.values() {
            if let Some(ref keys) = provider.keys {
                for key in keys {
                    if let Some(ref sref) = key.secret_ref
                        && !refs.contains(sref)
                    {
                        refs.push(sref.clone());
                    }
                }
            }
        }
    }
    refs
}
