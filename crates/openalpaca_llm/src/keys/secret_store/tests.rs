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
