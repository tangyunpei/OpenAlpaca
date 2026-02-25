use super::*;

fn test_encryptor() -> KeyEncryptor {
    let mut key_bytes = [0u8; 32];
    // Use a fixed key for tests
    for (i, b) in key_bytes.iter_mut().enumerate() {
        *b = i as u8;
    }
    KeyEncryptor {
        key: Key::<Aes256Gcm>::from_slice(&key_bytes).clone(),
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
