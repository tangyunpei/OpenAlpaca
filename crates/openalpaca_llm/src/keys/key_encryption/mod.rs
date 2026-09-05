//! AES-256-GCM encryption for API key secrets at rest.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::path::Path;

const PREFIX: &str = "aes256:";

/// Encrypts and decrypts API key secrets using AES-256-GCM.
pub struct KeyEncryptor {
    key: Key<Aes256Gcm>,
}

impl KeyEncryptor {
    /// The encryptor for the master key in `OPENALPACA_MASTER_KEY`.
    ///
    /// This crate sits below `openalpaca_storage` in the dependency graph, so it
    /// never resolves a path of its own: a process that wants a key file says
    /// which directory holds it ([`Self::load_or_generate_at`], with
    /// `store::master_key_dir()`). The daemon does that once in its boot
    /// preamble and exports the key here, so everything downstream of it —
    /// router construction, keychain migration, the settings service — reads the
    /// same key without knowing where it lives.
    pub fn from_env() -> Result<Self, String> {
        Self::from_env_opt()?.ok_or_else(|| {
            "OPENALPACA_MASTER_KEY is not set: the process must load the master key first \
             (KeyEncryptor::load_or_generate_at(store::master_key_dir()?))"
                .to_string()
        })
    }

    /// The encryptor for the master key kept in `dir`, generating one if `dir`
    /// has none. `OPENALPACA_MASTER_KEY` still wins when it is set, so a daemon
    /// and a CLI in the same environment agree.
    pub fn load_or_generate_at(dir: &Path) -> Result<Self, String> {
        if let Some(encryptor) = Self::from_env_opt()? {
            return Ok(encryptor);
        }
        Self::from_hex(&Self::ensure_at(dir)?, "generated master key")
    }

    fn from_env_opt() -> Result<Option<Self>, String> {
        match std::env::var("OPENALPACA_MASTER_KEY") {
            Ok(hex_key) => Self::from_hex(&hex_key, "OPENALPACA_MASTER_KEY").map(Some),
            Err(_) => Ok(None),
        }
    }

    fn from_hex(hex_key: &str, source: &str) -> Result<Self, String> {
        let bytes =
            hex::decode(hex_key.trim()).map_err(|e| format!("Invalid {source} hex: {e}"))?;
        if bytes.len() != 32 {
            return Err(format!(
                "{source} must be 32 bytes (64 hex chars), got {}",
                bytes.len()
            ));
        }
        Ok(Self {
            key: *Key::<Aes256Gcm>::from_slice(&bytes),
        })
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
}

/// Acquire a file lock for writing `config_path`. Returns a guard that releases
/// on drop.
///
/// The lock sits beside the file it guards (`<config_path>.lock`), so every
/// writer of the same config agrees on it without this crate resolving a
/// directory of its own — and locking a config never creates a second root.
pub fn acquire_config_write_lock(config_path: &Path) -> Result<file_lock::FileLock, String> {
    let mut name = config_path
        .file_name()
        .ok_or_else(|| format!("Config path has no file name: {}", config_path.display()))?
        .to_os_string();
    name.push(".lock");
    let lock_path = config_path.with_file_name(name);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
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
mod tests;
