//! OS-backed secret store abstraction.
//!
//! Provides a `SecretStore` trait with a `KeyringSecretStore` implementation
//! that delegates to the OS keychain (macOS login keychain, the Secret Service
//! on other unix, Windows Credential Manager) through `keyring-core` and the
//! native credential store crates.
//!
//! [`CachingSecretStore`] wraps any `SecretStore` with an in-memory cache so
//! that each unique `secret_ref` triggers at most one OS keychain access.

use keyring_core::{CredentialStore, Entry};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

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

/// A credential store made once, on first use, and shared by every
/// [`KeyringSecretStore`] that points at it.
type StoreCell = LazyLock<keyring_core::Result<Arc<CredentialStore>>>;

/// This process's native credential store, made on the first keychain call.
///
/// Nothing here sets `keyring_core`'s process-wide default store: every entry
/// is built against this one, so no caller has to install a store before the
/// first call, and a lookup can never answer `NoDefaultStore`.
static NATIVE_STORE: StoreCell = LazyLock::new(native_store);

/// The store keyring 4's `v1` facade chose for this platform, made with the
/// same constructor, so every item keeps the address it had: on macOS the
/// login keychain, on Windows the Credential Manager, on any other unix but
/// Android the Secret Service over zbus (`crypto-rust`).
fn native_store() -> keyring_core::Result<Arc<CredentialStore>> {
    #[cfg(target_os = "macos")]
    let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new()?;
    #[cfg(target_os = "windows")]
    let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new()?;
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    let store: Arc<CredentialStore> = zbus_secret_service_keyring_store::Store::new()?;
    #[cfg(all(any(unix, windows), not(any(target_os = "ios", target_os = "android"))))]
    {
        Ok(store)
    }
    #[cfg(not(all(any(unix, windows), not(any(target_os = "ios", target_os = "android")))))]
    {
        Err(keyring_core::Error::Invalid(
            "platform".to_string(),
            "must be macOS, Windows, or a non-iOS, non-Android *nix variant".to_string(),
        ))
    }
}

/// OS keychain-backed secret store.
///
/// Service = `"OpenAlpaca"`, account = `secret_ref` value
/// (e.g. `"llm/anthropic/<uuid>"`), built with no store modifiers.
pub struct KeyringSecretStore {
    store: &'static StoreCell,
}

impl KeyringSecretStore {
    /// The platform's native keychain. Constructing one touches nothing; the
    /// store itself is made on the first `get`, `set` or `delete` of the
    /// process, and a store that could not be made fails every call with the
    /// reason (headless Linux with no Secret Service, for one).
    pub fn native() -> Self {
        Self {
            store: &NATIVE_STORE,
        }
    }

    fn entry(&self, secret_ref: &str) -> Result<Entry, String> {
        let store = self
            .store
            .as_ref()
            .map_err(|e| format!("OS keychain unavailable for '{secret_ref}': {e}"))?;
        store
            .build(SERVICE, secret_ref, None)
            .map_err(|e| format!("Keyring entry error for '{secret_ref}': {e}"))
    }
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, secret_ref: &str) -> Result<Option<String>, String> {
        match self.entry(secret_ref)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Keyring get error for '{secret_ref}': {e}")),
        }
    }

    fn set(&self, secret_ref: &str, secret: &str) -> Result<(), String> {
        self.entry(secret_ref)?
            .set_password(secret)
            .map_err(|e| format!("Keyring set error for '{secret_ref}': {e}"))
    }

    fn delete(&self, secret_ref: &str) -> Result<(), String> {
        match self.entry(secret_ref)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring_core::Error::NoEntry) => Ok(()), // already gone
            Err(e) => Err(format!("Keyring delete error for '{secret_ref}': {e}")),
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
#[derive(Default)]
pub struct MemorySecretStore {
    store: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
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
