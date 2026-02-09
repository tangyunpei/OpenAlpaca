//! OS-backed secret store abstraction.
//!
//! Provides a `SecretStore` trait with a `KeyringSecretStore` implementation
//! that delegates to the OS keychain (macOS Keychain, Linux Secret Service,
//! Windows Credential Manager) via the `keyring` crate.

use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "OpenAlpaca";

/// Abstraction over a secure secret storage backend.
pub trait SecretStore: Send + Sync {
    /// Retrieve a secret by reference key. Returns `Ok(None)` if not found.
    fn get(&self, secret_ref: &str) -> Result<Option<String>, String>;
    /// Store a secret under the given reference key.
    fn set(&self, secret_ref: &str, secret: &str) -> Result<(), String>;
    /// Delete a secret by reference key. No-op if not found.
    fn delete(&self, secret_ref: &str) -> Result<(), String>;
}

/// OS keychain-backed secret store.
///
/// Service = `"OpenAlpaca"`, account = `secret_ref` value
/// (e.g. `"llm/anthropic/<uuid>"`).
pub struct KeyringSecretStore;

impl SecretStore for KeyringSecretStore {
    fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(SERVICE, secret_ref)
            .map_err(|e| format!("Keyring entry error for '{}': {e}", secret_ref))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Keyring get error for '{}': {e}", secret_ref)),
        }
    }

    fn set(&self, secret_ref: &str, secret: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, secret_ref)
            .map_err(|e| format!("Keyring entry error for '{}': {e}", secret_ref))?;
        entry
            .set_password(secret)
            .map_err(|e| format!("Keyring set error for '{}': {e}", secret_ref))
    }

    fn delete(&self, secret_ref: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, secret_ref)
            .map_err(|e| format!("Keyring entry error for '{}': {e}", secret_ref))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // already gone
            Err(e) => Err(format!("Keyring delete error for '{}': {e}", secret_ref)),
        }
    }
}

/// In-memory secret store for testing.
#[cfg(test)]
pub struct MemorySecretStore {
    store: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl MemorySecretStore {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
impl SecretStore for MemorySecretStore {
    fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        Ok(self.store.lock().unwrap().get(secret_ref).cloned())
    }

    fn set(&self, secret_ref: &str, secret: &str) -> Result<(), String> {
        self.store
            .lock()
            .unwrap()
            .insert(secret_ref.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), String> {
        self.store.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

// Also provide a non-test MemorySecretStore for use as a fallback
// when the OS keychain is unavailable.
#[cfg(not(test))]
pub struct MemorySecretStore {
    store: Mutex<HashMap<String, String>>,
}

#[cfg(not(test))]
impl Default for MemorySecretStore {
    fn default() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }
}

#[cfg(not(test))]
impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(not(test))]
impl SecretStore for MemorySecretStore {
    fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        Ok(self.store.lock().unwrap().get(secret_ref).cloned())
    }

    fn set(&self, secret_ref: &str, secret: &str) -> Result<(), String> {
        self.store
            .lock()
            .unwrap()
            .insert(secret_ref.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), String> {
        self.store.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_store_roundtrip() {
        let store = MemorySecretStore::new();
        assert_eq!(store.get("key1").unwrap(), None);

        store.set("key1", "secret-value").unwrap();
        assert_eq!(store.get("key1").unwrap(), Some("secret-value".to_string()));

        store.set("key1", "updated-value").unwrap();
        assert_eq!(
            store.get("key1").unwrap(),
            Some("updated-value".to_string())
        );
    }

    #[test]
    fn test_memory_store_delete() {
        let store = MemorySecretStore::new();
        store.set("key1", "value").unwrap();
        store.delete("key1").unwrap();
        assert_eq!(store.get("key1").unwrap(), None);
    }

    #[test]
    fn test_memory_store_delete_missing_is_ok() {
        let store = MemorySecretStore::new();
        // Deleting a non-existent key should succeed
        store.delete("nonexistent").unwrap();
    }

    #[test]
    fn test_memory_store_multiple_keys() {
        let store = MemorySecretStore::new();
        store.set("llm/anthropic/aaa", "secret-a").unwrap();
        store.set("llm/openai/bbb", "secret-b").unwrap();

        assert_eq!(
            store.get("llm/anthropic/aaa").unwrap(),
            Some("secret-a".to_string())
        );
        assert_eq!(
            store.get("llm/openai/bbb").unwrap(),
            Some("secret-b".to_string())
        );
    }
}
