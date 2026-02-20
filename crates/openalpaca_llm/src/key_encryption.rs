//! AES-256-GCM encryption for API key secrets at rest.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::path::{Path, PathBuf};

const PREFIX: &str = "aes256:";

/// Encrypts and decrypts API key secrets using AES-256-GCM.
pub struct KeyEncryptor {
    key: Key<Aes256Gcm>,
}

impl KeyEncryptor {
    /// Load master key from env var or config file, generating if neither exists.
    pub fn load_or_generate() -> Result<Self, String> {
        // 1. Try env var
        if let Ok(hex_key) = std::env::var("OPENALPACA_MASTER_KEY") {
            let bytes = hex::decode(&hex_key)
                .map_err(|e| format!("Invalid OPENALPACA_MASTER_KEY hex: {e}"))?;
            if bytes.len() != 32 {
                return Err(format!(
                    "OPENALPACA_MASTER_KEY must be 32 bytes (64 hex chars), got {}",
                    bytes.len()
                ));
            }
            let key = *Key::<Aes256Gcm>::from_slice(&bytes);
            return Ok(Self { key });
        }

        // 2. Try key file at canonical location (app_dir)
        let key_path = Self::key_file_path()?;
        if key_path.exists() {
            let hex_key = std::fs::read_to_string(&key_path)
                .map_err(|e| format!("Failed to read master key file: {e}"))?;
            let hex_key = hex_key.trim();
            let bytes =
                hex::decode(hex_key).map_err(|e| format!("Invalid master key file hex: {e}"))?;
            if bytes.len() != 32 {
                return Err(format!(
                    "Master key file must contain 32 bytes, got {}",
                    bytes.len()
                ));
            }
            let key = *Key::<Aes256Gcm>::from_slice(&bytes);
            return Ok(Self { key });
        }

        // 3. Auto-generate via ensure_at
        let dir = key_path
            .parent()
            .ok_or_else(|| "Cannot determine parent directory for master key".to_string())?;
        let hex_key = Self::ensure_at(dir)?;
        let bytes = hex::decode(&hex_key).map_err(|e| format!("Invalid generated key hex: {e}"))?;
        let key = *Key::<Aes256Gcm>::from_slice(&bytes);
        Ok(Self { key })
    }

    /// Race-safe master key generation at a specific directory.
    ///
    /// 1. Try read first (common path — key already exists)
    /// 2. On NotFound: generate + atomic `create_new(true)` write
    /// 3. On AlreadyExists (another process won the race): re-read and validate
    ///
    /// Returns the 64-char hex-encoded master key.
    pub fn ensure_at(dir: &Path) -> Result<String, String> {
        let key_path = dir.join(".master_key");

        // 1. Try read first (common path)
        match std::fs::read_to_string(&key_path) {
            Ok(contents) => {
                let hex_key = contents.trim().to_string();
                let bytes = hex::decode(&hex_key).map_err(|e| {
                    format!("Invalid master key hex at {}: {e}", key_path.display())
                })?;
                if bytes.len() != 32 {
                    return Err(format!(
                        "Master key at {} must be 32 bytes, got {}",
                        key_path.display(),
                        bytes.len()
                    ));
                }
                return Ok(hex_key);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Fall through to generate
            }
            Err(e) => {
                return Err(format!("Failed to read {}: {e}", key_path.display()));
            }
        }

        // 2. Generate + atomic write
        let mut key_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut key_bytes);
        let hex_key = hex::encode(&key_bytes);

        if let Some(parent) = key_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create dir {}: {e}", parent.display()))?;
        }

        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&key_path)
        {
            Ok(mut f) => {
                f.write_all(hex_key.as_bytes())
                    .map_err(|e| format!("Failed to write {}: {e}", key_path.display()))?;
                f.flush()
                    .map_err(|e| format!("Failed to flush {}: {e}", key_path.display()))?;
                f.sync_all()
                    .map_err(|e| format!("Failed to sync {}: {e}", key_path.display()))?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o600);
                    let _ = std::fs::set_permissions(&key_path, perms);
                }

                Ok(hex_key)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another process won the race — re-read and validate
                let contents = std::fs::read_to_string(&key_path)
                    .map_err(|e| format!("Failed to re-read {}: {e}", key_path.display()))?;
                let hex_key = contents.trim().to_string();
                let bytes = hex::decode(&hex_key).map_err(|e| {
                    format!("Invalid master key hex at {}: {e}", key_path.display())
                })?;
                if bytes.len() != 32 {
                    return Err(format!(
                        "Master key at {} must be 32 bytes, got {}",
                        key_path.display(),
                        bytes.len()
                    ));
                }
                Ok(hex_key)
            }
            Err(e) => Err(format!("Failed to create {}: {e}", key_path.display())),
        }
    }

    /// Encrypt a plaintext secret. Returns `"aes256:<base64(nonce+ciphertext)>"`.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, String> {
        let cipher = Aes256Gcm::new(&self.key);

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| format!("Encryption failed: {e}"))?;

        // Concatenate nonce + ciphertext
        let mut combined = Vec::with_capacity(12 + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);

        Ok(format!("{}{}", PREFIX, BASE64.encode(&combined)))
    }

    /// Decrypt an encrypted secret. Expects `"aes256:<base64(nonce+ciphertext)>"`.
    pub fn decrypt(&self, encrypted: &str) -> Result<String, String> {
        let payload = encrypted
            .strip_prefix(PREFIX)
            .ok_or_else(|| format!("Invalid encrypted format: missing '{}' prefix", PREFIX))?;

        let combined = BASE64
            .decode(payload)
            .map_err(|e| format!("Invalid base64: {e}"))?;

        if combined.len() < 12 {
            return Err("Invalid encrypted data: too short".to_string());
        }

        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let cipher = Aes256Gcm::new(&self.key);
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| format!("Decryption failed: {e}"))?;

        String::from_utf8(plaintext).map_err(|e| format!("Invalid UTF-8: {e}"))
    }

    /// Check if a value is encrypted (has the `"aes256:"` prefix).
    pub fn is_encrypted(value: &str) -> bool {
        value.starts_with(PREFIX)
    }

    /// Canonical master key file path.
    ///
    /// Defaults to `app_dir()/.master_key` (always writable, CWD-independent).
    /// Only `OPENALPACA_MASTER_KEY` env var overrides (checked in `load_or_generate()`).
    fn key_file_path() -> Result<PathBuf, String> {
        use directories::ProjectDirs;
        if let Some(proj) = ProjectDirs::from("", "", "OpenAlpaca") {
            return Ok(proj.data_dir().join(".master_key"));
        }
        // Fallback to CWD (should not happen on supported platforms)
        let cwd = std::env::current_dir().map_err(|e| format!("Failed to get current dir: {e}"))?;
        Ok(cwd.join("config").join(".master_key"))
    }
}

/// Acquire a file lock for writing llm.toml. Returns guard that releases on drop.
pub fn acquire_config_write_lock() -> Result<file_lock::FileLock, String> {
    use directories::ProjectDirs;
    let lock_dir = if let Some(proj) = ProjectDirs::from("", "", "OpenAlpaca") {
        proj.data_dir().to_path_buf()
    } else {
        std::env::current_dir().map_err(|e| format!("Failed to get current dir: {e}"))?
    };
    std::fs::create_dir_all(&lock_dir).map_err(|e| format!("Failed to create lock dir: {e}"))?;
    let lock_path = lock_dir.join("llm.toml.lock");
    let opts = file_lock::FileOptions::new().write(true).create(true);
    file_lock::FileLock::lock(&lock_path, true, opts).map_err(|e| {
        format!(
            "Failed to acquire config write lock at {}: {e}",
            lock_path.display()
        )
    })
}

/// Simple hex encoding/decoding (avoids pulling in the `hex` crate).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        if !s.len().is_multiple_of(2) {
            return Err("Odd hex string length".to_string());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(&s[i..i + 2], 16)
                    .map_err(|e| format!("Invalid hex at position {}: {}", i, e))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
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
}
