use super::*;
use keyring_core::mock;

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
    assert_eq!(caching.get("key1").unwrap(), Some("now-exists".to_string()));
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

// ── KeyringSecretStore tests ──────────────────────────────────────
//
// None of these reaches the OS keychain. The native store cannot be exercised
// without reading the developer's login keychain, so each test points a
// `KeyringSecretStore` at a cell of its own holding keyring-core's in-memory
// mock store: the same `SecretStore` code and the same first-call
// initialisation as `KeyringSecretStore::native()`, with only the store
// swapped. Nothing in this crate sets keyring-core's default store.

/// A fresh mock store. Each test's cell makes one on its first use, as
/// `NATIVE_STORE` makes the native store.
fn fresh_mock() -> keyring_core::Result<Arc<CredentialStore>> {
    let store: Arc<CredentialStore> = mock::Store::new()?;
    Ok(store)
}

#[test]
fn the_first_lookup_in_a_fresh_process_never_answers_no_default_store() {
    static FRESH: StoreCell = LazyLock::new(fresh_mock);
    assert!(
        keyring_core::get_default_store().is_none(),
        "the process starts with no default store, as the daemon and the CLI do"
    );

    let keyring = KeyringSecretStore { store: &FRESH };
    // The first call of the cell's life: it makes the store, builds the entry
    // against it and finds nothing — an absent secret, not an error.
    assert_eq!(keyring.get("llm/anthropic/first-lookup"), Ok(None));
    keyring
        .set("llm/anthropic/first-lookup", "sk-first")
        .unwrap();
    assert_eq!(
        keyring.get("llm/anthropic/first-lookup"),
        Ok(Some("sk-first".to_string()))
    );

    assert!(
        keyring_core::get_default_store().is_none(),
        "the store is owned here; the process-wide default was never set or read"
    );
}

#[test]
fn keyring_items_are_addressed_by_the_openalpaca_service_and_the_ref_as_account() {
    static STORE: StoreCell = LazyLock::new(fresh_mock);
    let keyring = KeyringSecretStore { store: &STORE };
    keyring.set("llm/openai/abc", "sk-one").unwrap();
    keyring.set("llm/openai/abc", "sk-two").unwrap();

    // The item keyring 4's `v1` facade addressed: service "OpenAlpaca",
    // account the ref, no modifiers (the mock refuses any modifier).
    let item = STORE
        .as_ref()
        .unwrap()
        .build("OpenAlpaca", "llm/openai/abc", None)
        .unwrap();
    assert_eq!(item.get_password().unwrap(), "sk-two");

    keyring.delete("llm/openai/abc").unwrap();
    assert!(matches!(
        item.get_password(),
        Err(keyring_core::Error::NoEntry)
    ));
    assert_eq!(keyring.get("llm/openai/abc"), Ok(None));
    // Deleting what is already gone is not an error.
    keyring.delete("llm/openai/abc").unwrap();
}

#[test]
fn a_keychain_failure_other_than_no_entry_is_an_error() {
    static STORE: StoreCell = LazyLock::new(fresh_mock);
    let keyring = KeyringSecretStore { store: &STORE };
    keyring.set("llm/anthropic/locked", "sk-locked").unwrap();
    let item = STORE
        .as_ref()
        .unwrap()
        .build("OpenAlpaca", "llm/anthropic/locked", None)
        .unwrap();
    let cred: &mock::Cred = item.as_any().downcast_ref().unwrap();

    cred.set_error(keyring_core::Error::NoStorageAccess("locked".into()));
    let err = keyring.get("llm/anthropic/locked").unwrap_err();
    assert!(
        err.starts_with("Keyring get error for 'llm/anthropic/locked'"),
        "{err}"
    );

    cred.set_error(keyring_core::Error::NoStorageAccess("locked".into()));
    let err = keyring.delete("llm/anthropic/locked").unwrap_err();
    assert!(err.starts_with("Keyring delete error"), "{err}");

    cred.set_error(keyring_core::Error::NoStorageAccess("locked".into()));
    let err = keyring.set("llm/anthropic/locked", "sk-new").unwrap_err();
    assert!(err.starts_with("Keyring set error"), "{err}");

    // Each error is returned once; the secret is untouched by them.
    assert_eq!(
        keyring.get("llm/anthropic/locked"),
        Ok(Some("sk-locked".to_string()))
    );
}

#[test]
fn a_store_that_could_not_be_made_fails_every_call_and_the_daemon_falls_back() {
    static BROKEN: StoreCell = LazyLock::new(|| {
        Err(keyring_core::Error::NoStorageAccess(
            "no Secret Service on the session bus".into(),
        ))
    });
    let keyring = KeyringSecretStore { store: &BROKEN };
    let err = keyring.get("llm/openai/xyz").unwrap_err();
    assert_eq!(
        err,
        "OS keychain unavailable for 'llm/openai/xyz': \
         Couldn't access platform storage: no Secret Service on the session bus"
    );
    assert!(keyring.set("llm/openai/xyz", "sk").is_err());
    assert!(keyring.delete("llm/openai/xyz").is_err());

    // `initialize_secret_store` reads `false` here as "the keychain is not
    // functional" and serves an in-memory store instead.
    let caching = CachingSecretStore::new(Box::new(KeyringSecretStore { store: &BROKEN }));
    assert!(!caching.prefetch(&[]));
    assert!(!caching.prefetch(&["llm/openai/xyz"]));
}
