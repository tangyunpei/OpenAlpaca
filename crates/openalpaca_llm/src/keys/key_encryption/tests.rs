use super::*;

fn test_encryptor() -> KeyEncryptor {
    let mut key_bytes = [0u8; 32];
    // Use a fixed key for tests
    for (i, b) in key_bytes.iter_mut().enumerate() {
        *b = i as u8;
    }
    KeyEncryptor {
        key: Key::<Aes256Gcm>::from(key_bytes),
    }
}

#[test]
fn test_encrypt_decrypt_roundtrip() {
    let enc = test_encryptor();
    let plaintext = "sk-ant-api03-test-key-1234567890";
    let encrypted = enc.encrypt(plaintext).unwrap();

    assert!(KeyEncryptor::is_encrypted(&encrypted));
    assert!(encrypted.starts_with("aes256:"));

    let decrypted = enc.decrypt(&encrypted).unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn test_different_encryptions_differ() {
    let enc = test_encryptor();
    let plaintext = "sk-test-key";
    let e1 = enc.encrypt(plaintext).unwrap();
    let e2 = enc.encrypt(plaintext).unwrap();
    // Different nonces should produce different ciphertexts
    assert_ne!(e1, e2);
    // But both decrypt to the same plaintext
    assert_eq!(enc.decrypt(&e1).unwrap(), plaintext);
    assert_eq!(enc.decrypt(&e2).unwrap(), plaintext);
}

#[test]
fn test_is_encrypted() {
    assert!(KeyEncryptor::is_encrypted("aes256:abc123"));
    assert!(!KeyEncryptor::is_encrypted("sk-plain-key"));
    assert!(!KeyEncryptor::is_encrypted(""));
}

#[test]
fn test_decrypt_invalid_prefix() {
    let enc = test_encryptor();
    let result = enc.decrypt("invalid:data");
    assert!(result.is_err());
}

#[test]
fn test_decrypt_invalid_base64() {
    let enc = test_encryptor();
    let result = enc.decrypt("aes256:not-valid-base64!!!");
    assert!(result.is_err());
}

#[test]
fn test_decrypt_too_short() {
    let enc = test_encryptor();
    let short = format!("aes256:{}", BASE64.encode(&[0u8; 5]));
    let result = enc.decrypt(&short);
    assert!(result.is_err());
}

#[test]
fn test_hex_roundtrip() {
    let data = vec![0x00, 0x01, 0xff, 0xab, 0xcd];
    let encoded = hex::encode(&data);
    assert_eq!(encoded, "0001ffabcd");
    let decoded = hex::decode(&encoded).unwrap();
    assert_eq!(decoded, data);
}

#[test]
fn test_ensure_at_creates_key() {
    let tmp = tempfile::tempdir().unwrap();
    let hex_key = KeyEncryptor::ensure_at(tmp.path()).unwrap();
    assert_eq!(hex_key.len(), 64); // 32 bytes = 64 hex chars
    // Reading again returns the same key
    let hex_key2 = KeyEncryptor::ensure_at(tmp.path()).unwrap();
    assert_eq!(hex_key, hex_key2);
}

#[test]
fn test_ensure_at_validates_existing() {
    let tmp = tempfile::tempdir().unwrap();
    // Write a valid key manually
    let valid_hex = "00".repeat(32);
    std::fs::write(tmp.path().join(".master_key"), &valid_hex).unwrap();
    let result = KeyEncryptor::ensure_at(tmp.path()).unwrap();
    assert_eq!(result, valid_hex);
}

#[test]
fn test_ensure_at_rejects_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    // Write invalid key (too short)
    std::fs::write(tmp.path().join(".master_key"), "abcd").unwrap();
    let result = KeyEncryptor::ensure_at(tmp.path());
    assert!(result.is_err());
}

// ============================================================================
// This crate resolves no paths of its own (R11)
// ============================================================================

/// Points `HOME` at a temp dir for the duration of a test.
///
/// `openalpaca_llm` sits below `openalpaca_storage` in the dependency graph, so
/// it must never look a directory up — every path is an argument. These tests
/// prove that by sandboxing the only lookup anything here could make: a
/// home-relative "application data" directory. Nothing may appear under the
/// sandbox except what the test hands the code explicitly.
struct HomeSandbox {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_home: Option<std::ffi::OsString>,
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl HomeSandbox {
    fn enter(root: &std::path::Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var_os("HOME");
        // SAFETY: serialized by ENV_LOCK; no other test in this crate reads HOME.
        unsafe { std::env::set_var("HOME", root) };
        Self {
            _lock: lock,
            prev_home,
        }
    }
}

impl Drop for HomeSandbox {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        match self.prev_home.take() {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}

fn children(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn the_config_write_lock_sits_beside_the_file_it_guards() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeSandbox::enter(tmp.path());

    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let llm_toml = config_dir.join("llm.toml");
    std::fs::write(&llm_toml, "").unwrap();

    let lock = acquire_config_write_lock(&llm_toml).unwrap();
    drop(lock);

    assert_eq!(
        children(tmp.path()),
        vec!["config".to_string()],
        "locking a config file must not create an application data root"
    );
    assert!(
        config_dir.join("llm.toml.lock").exists(),
        "the lock belongs beside the file it guards"
    );
}

#[test]
fn load_or_generate_at_uses_the_directory_it_is_given() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeSandbox::enter(tmp.path());

    let state = tmp.path().join("home").join("state");
    std::fs::create_dir_all(&state).unwrap();

    let enc = KeyEncryptor::load_or_generate_at(&state).unwrap();
    let encrypted = enc.encrypt("sk-secret").unwrap();

    assert!(
        state.join(".master_key").exists(),
        "the key is generated in the directory it was given"
    );
    assert_eq!(
        children(tmp.path()),
        vec!["home".to_string()],
        "no second key directory was invented"
    );

    // A second call reads the same key back rather than generating another.
    let again = KeyEncryptor::load_or_generate_at(&state).unwrap();
    assert_eq!(again.decrypt(&encrypted).unwrap(), "sk-secret");
}

/// The **read-only** loader: a decrypt path asks whether a key exists and is
/// told `None`, without one appearing on disk because it asked.
#[test]
fn load_at_never_mints_a_master_key() {
    let tmp = tempfile::tempdir().unwrap();
    let _home = HomeSandbox::enter(tmp.path());

    let state = tmp.path().join("home").join("state");
    std::fs::create_dir_all(&state).unwrap();

    assert!(
        KeyEncryptor::load_at(&state).unwrap().is_none(),
        "a read reported a key that does not exist"
    );
    assert!(
        !state.join(".master_key").exists(),
        "reading for the master key created it"
    );

    // With a key in place it reads that one, rather than a fresh one.
    let encrypted = KeyEncryptor::load_or_generate_at(&state)
        .unwrap()
        .encrypt("sk-secret")
        .unwrap();
    let read = KeyEncryptor::load_at(&state)
        .unwrap()
        .expect("the key exists now");
    assert_eq!(read.decrypt(&encrypted).unwrap(), "sk-secret");
}

// ============================================================================
// The on-disk secret format is pinned by a literal, never by the code (P05)
// ============================================================================

/// A secret as `llm.toml` holds it — `aes256:` + base64(12-byte nonce ‖
/// AES-256-GCM ciphertext ‖ 16-byte tag) — written in September 2026 by the
/// aes-gcm 0.10 build of this construction under the key `00 01 … 1f`. The
/// literal, not the code, is the oracle: a build that cannot read it locks
/// every stored key out, so it stays as it is through any crypto bump
/// (aes-gcm 0.11, deps plan P11) — a failure here is a format change, never
/// a test to update.
#[test]
fn a_secret_written_by_the_aes_gcm_0_10_build_still_decrypts() {
    const KEY_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const WRITTEN_IN_SEPTEMBER_2026: &str =
        "aes256:HFTMjzaU0zEto0whNGd6YqMOlnKtBdUbBHMw8KubnY16SHhYQwgboFOmStXEzYAXfEtXNQavok+LGjf7";

    let enc = KeyEncryptor::from_hex(KEY_HEX, "the fixture's key").unwrap();
    assert!(KeyEncryptor::is_encrypted(WRITTEN_IN_SEPTEMBER_2026));
    assert_eq!(
        enc.decrypt(WRITTEN_IN_SEPTEMBER_2026).unwrap(),
        "sk-ant-api03-test-key-1234567890"
    );
}
