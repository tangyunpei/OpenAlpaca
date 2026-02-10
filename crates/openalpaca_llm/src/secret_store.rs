//! OS-backed secret store abstraction.
//!
//! Provides a `SecretStore` trait with a `KeyringSecretStore` implementation
//! that delegates to the OS keychain (macOS Keychain, Linux Secret Service,
//! Windows Credential Manager) via the `keyring` crate.
//!
//! [`CachingSecretStore`] wraps any `SecretStore` with an in-memory cache so
//! that each unique `secret_ref` triggers at most one OS keychain access.

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

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

/// Caching wrapper around any [`SecretStore`] implementation.
///
/// Caches `get()` results in an in-memory `HashMap` so each unique
/// `secret_ref` hits the underlying store (e.g. OS keychain) at most once.
/// `set()` and `delete()` are write-through: they update the underlying
/// store first, then synchronize the cache.
///
/// This avoids repeated macOS Keychain password prompts during daemon
/// startup when multiple subsystems resolve the same API keys.
pub struct CachingSecretStore {
    inner: Box<dyn SecretStore>,
    cache: RwLock<HashMap<String, Option<String>>>,
}

impl CachingSecretStore {
    /// Create a new caching wrapper around the given secret store.
    pub fn new(inner: Box<dyn SecretStore>) -> Self {
        Self {
            inner,
            cache: RwLock::new(HashMap::new()),
        }
    }
}

impl SecretStore for CachingSecretStore {
    fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        // Fast path: check cache with read lock
        {
            let cache = self.cache.read().unwrap();
            if let Some(cached) = cache.get(secret_ref) {
                return Ok(cached.clone());
            }
        }
        // Cache miss: fetch from underlying store
        let result = self.inner.get(secret_ref)?;
        // Populate cache (also caches None to avoid repeated misses)
        {
            let mut cache = self.cache.write().unwrap();
            cache.insert(secret_ref.to_string(), result.clone());
        }
        Ok(result)
    }

    fn set(&self, secret_ref: &str, secret: &str) -> Result<(), String> {
        // Write-through: update underlying store first
        self.inner.set(secret_ref, secret)?;
        // Then update cache
        let mut cache = self.cache.write().unwrap();
        cache.insert(secret_ref.to_string(), Some(secret.to_string()));
        Ok(())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), String> {
        // Delete-through: remove from underlying store first
        self.inner.delete(secret_ref)?;
        // Remove from cache so next get() re-checks the store
        let mut cache = self.cache.write().unwrap();
        cache.remove(secret_ref);
        Ok(())
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

    // ── CachingSecretStore tests ──────────────────────────────────────

    #[test]
    fn test_caching_store_caches_gets() {
        let inner = MemorySecretStore::new();
        inner.set("key1", "value1").unwrap();
        let caching = CachingSecretStore::new(Box::new(inner));

        assert_eq!(caching.get("key1").unwrap(), Some("value1".to_string()));
        // Second get returns cached value
        assert_eq!(caching.get("key1").unwrap(), Some("value1".to_string()));
    }

    #[test]
    fn test_caching_store_caches_none() {
        let caching = CachingSecretStore::new(Box::new(MemorySecretStore::new()));
        // First get: miss → returns None and caches it
        assert_eq!(caching.get("nonexistent").unwrap(), None);
        // Second get: cache hit → still None without hitting inner store
        assert_eq!(caching.get("nonexistent").unwrap(), None);
    }

    #[test]
    fn test_caching_store_write_through() {
        let caching = CachingSecretStore::new(Box::new(MemorySecretStore::new()));
        caching.set("key1", "value1").unwrap();
        assert_eq!(caching.get("key1").unwrap(), Some("value1".to_string()));
        // Overwrite updates cache
        caching.set("key1", "value2").unwrap();
        assert_eq!(caching.get("key1").unwrap(), Some("value2".to_string()));
    }

    #[test]
    fn test_caching_store_delete_through() {
        let caching = CachingSecretStore::new(Box::new(MemorySecretStore::new()));
        caching.set("key1", "value1").unwrap();
        assert_eq!(caching.get("key1").unwrap(), Some("value1".to_string()));
        caching.delete("key1").unwrap();
        // After delete, get() re-checks inner (which returns None)
        assert_eq!(caching.get("key1").unwrap(), None);
    }

    #[test]
    fn test_caching_store_delete_missing_is_ok() {
        let caching = CachingSecretStore::new(Box::new(MemorySecretStore::new()));
        caching.delete("nonexistent").unwrap();
    }

    #[test]
    fn test_caching_store_set_after_cached_none() {
        let caching = CachingSecretStore::new(Box::new(MemorySecretStore::new()));
        // Cache a None result
        assert_eq!(caching.get("key1").unwrap(), None);
        // set() should update the cache from None → Some
        caching.set("key1", "now-exists").unwrap();
        assert_eq!(
            caching.get("key1").unwrap(),
            Some("now-exists".to_string())
        );
    }

    #[test]
    fn test_caching_store_multiple_keys_independent() {
        let caching = CachingSecretStore::new(Box::new(MemorySecretStore::new()));
        caching.set("llm/anthropic/aaa", "secret-a").unwrap();
        caching.set("llm/openai/bbb", "secret-b").unwrap();

        assert_eq!(
            caching.get("llm/anthropic/aaa").unwrap(),
            Some("secret-a".to_string())
        );
        assert_eq!(
            caching.get("llm/openai/bbb").unwrap(),
            Some("secret-b".to_string())
        );

        // Delete one, other unaffected
        caching.delete("llm/anthropic/aaa").unwrap();
        assert_eq!(caching.get("llm/anthropic/aaa").unwrap(), None);
        assert_eq!(
            caching.get("llm/openai/bbb").unwrap(),
            Some("secret-b".to_string())
        );
    }
}
