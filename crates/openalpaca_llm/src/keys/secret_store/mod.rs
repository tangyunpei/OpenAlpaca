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

/// Blanket impl so `Box<dyn SecretStore>` can be used as a `SecretStore`.
impl SecretStore for Box<dyn SecretStore> {
    fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        (**self).get(secret_ref)
    }
    fn set(&self, secret_ref: &str, secret: &str) -> Result<(), String> {
        (**self).set(secret_ref, secret)
    }
    fn delete(&self, secret_ref: &str) -> Result<(), String> {
        (**self).delete(secret_ref)
    }
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

    /// Pre-fetch a batch of `secret_ref` keys into the cache.
    ///
    /// Reads every key from the underlying store (e.g. OS keychain) and
    /// populates the cache so that later `get()` calls are pure cache hits.
    /// All keychain prompts happen here, back-to-back, in a single phase.
    ///
    /// Returns `true` if the underlying keychain is functional (even if
    /// some keys are missing), `false` if the keychain infrastructure
    /// itself is broken (headless / Docker / no D-Bus).
    pub fn prefetch(&self, keys: &[&str]) -> bool {
        for key in keys {
            match self.inner.get(key) {
                Ok(val) => {
                    let mut cache = self.cache.write().unwrap();
                    cache.insert(key.to_string(), val);
                }
                Err(_) => {
                    // Infrastructure error → keychain not functional
                    return false;
                }
            }
        }
        // If keys is empty we still need to verify keychain works,
        // so do a single probe read on a non-existent key.
        if keys.is_empty() {
            return self.inner.get("__openalpaca_probe__").is_ok();
        }
        true
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
mod tests;
