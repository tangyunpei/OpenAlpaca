//! AES-256-GCM encryption for API key secrets at rest.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, Generate, KeyInit},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::path::Path;
use std::sync::OnceLock;

const PREFIX: &str = "aes256:";

/// The environment variable a process may carry its master key in — the
/// fallback [`KeyEncryptor::from_env`] reads when no key was installed with
/// [`KeyEncryptor::install_process_key`]. The CLI and the test harnesses use
/// it; the daemon does not (F3: it installs the key and removes the variable,
/// so no process it spawns receives the key).
pub const MASTER_KEY_ENV: &str = "OPENALPACA_MASTER_KEY";

/// The master key this process was handed, as the 64 hex characters
/// [`KeyEncryptor::ensure_at`] returns, trimmed and in lower case — validated
/// before it is stored, so a read of it cannot fail. Filled once, by
/// [`KeyEncryptor::install_process_key`]; never by a unit test, since every
/// test in a binary shares it.
static PROCESS_KEY: OnceLock<String> = OnceLock::new();

/// Encrypts and decrypts API key secrets using AES-256-GCM.
pub struct KeyEncryptor {
    key: Key<Aes256Gcm>,
}

impl KeyEncryptor {
    /// The encryptor for this process's master key: the one installed with
    /// [`Self::install_process_key`], else the one in [`MASTER_KEY_ENV`].
    ///
    /// This crate sits below `openalpaca_storage` in the dependency graph, so it
    /// never resolves a path of its own: a process that wants a key file says
    /// which directory holds it ([`Self::load_or_generate_at`], with
    /// `store::master_key_dir()`). The daemon does that once in its boot
    /// preamble and installs the key here — in process memory, never in its
    /// environment, so no program it spawns receives it (F3) — and everything
    /// downstream of it — router construction, keychain migration, the settings
    /// service — reads the same key without knowing where it lives. The
    /// environment read is the fallback the CLI and the test harnesses use.
    pub fn from_env() -> Result<Self, String> {
        Self::from_env_opt()?.ok_or_else(|| {
            format!(
                "no master key: the process must install one first \
                 (KeyEncryptor::install_process_key, as the daemon's boot preamble does) \
                 or carry it in {MASTER_KEY_ENV}"
            )
        })
    }

    /// Hands this process its master key: what [`Self::from_env`] and every
    /// reader built on it use from now on, before any environment variable.
    ///
    /// `hex_key` is validated first (64 hex characters, 32 bytes), so a later
    /// read cannot fail, and stored trimmed and in lower case, so the same 32
    /// bytes in either hex case are the same key. Installing the **same** key
    /// again is a no-op; a **different** one is refused — one process with two
    /// master keys would silently split what it encrypts from what it can
    /// decrypt.
    pub fn install_process_key(hex_key: &str) -> Result<(), String> {
        Self::from_hex(hex_key, "master key")?;
        match PROCESS_KEY.set(hex_key.trim().to_ascii_lowercase()) {
            Ok(()) => Ok(()),
            Err(rejected) if PROCESS_KEY.get() == Some(&rejected) => Ok(()),
            Err(_) => {
                Err("a different master key is already installed in this process".to_string())
            }
        }
    }

    /// [`Self::install_process_key`], then [`MASTER_KEY_ENV`] removed from the
    /// process environment — inherited from the launching shell or not — so no
    /// child this process spawns receives the key (F3). The daemon's boot
    /// preamble calls this once; so does the integration test that proves it
    /// (`tests/master_key_leaves_the_environment.rs`).
    ///
    /// # Safety
    ///
    /// Calls [`std::env::remove_var`]: the caller must be the only thread that
    /// can be reading or writing the environment — in the daemon, the
    /// single-threaded preamble before the tokio runtime starts.
    pub unsafe fn install_process_key_and_clear_env(hex_key: &str) -> Result<(), String> {
        Self::install_process_key(hex_key)?;
        // SAFETY: the caller guarantees no other thread touches the environment
        // (this function's contract, above).
        unsafe { std::env::remove_var(MASTER_KEY_ENV) };
        Ok(())
    }

    /// The encryptor for the master key kept in `dir`, generating one if `dir`
    /// has none. A key this process installed, or one in [`MASTER_KEY_ENV`],
    /// still wins when there is one (`Self::from_env_opt`'s order). The daemon
    /// and the CLI agree because both name the same file, `state/.master_key`
    /// — the daemon no longer exports it (F3).
    pub fn load_or_generate_at(dir: &Path) -> Result<Self, String> {
        if let Some(encryptor) = Self::from_env_opt()? {
            return Ok(encryptor);
        }
        Self::from_hex(&Self::ensure_at(dir)?, "generated master key")
    }

    /// The encryptor for a master key that **already exists** in `dir` — the
    /// read-only twin of [`Self::load_or_generate_at`], for paths that only
    /// *decrypt*. `Ok(None)` says there is no key yet.
    ///
    /// A decrypt path must not mint one: a freshly generated key cannot decrypt
    /// anything that was encrypted with the old one, so the caller's outcome is
    /// the same either way — and the key file is durable state in `state/`,
    /// which a read has no business creating. A key this process installed, or
    /// one in [`MASTER_KEY_ENV`], still wins when there is one, as in
    /// [`Self::load_or_generate_at`].
    pub fn load_at(dir: &Path) -> Result<Option<Self>, String> {
        if let Some(encryptor) = Self::from_env_opt()? {
            return Ok(Some(encryptor));
        }
        let key_path = dir.join(".master_key");
        match std::fs::read_to_string(&key_path) {
            Ok(contents) => Self::from_hex(contents.trim(), "master key").map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("Failed to read {}: {e}", key_path.display())),
        }
    }

    /// The installed key first, then the environment; `Ok(None)` when the
    /// process has neither.
    fn from_env_opt() -> Result<Option<Self>, String> {
        if let Some(hex_key) = PROCESS_KEY.get() {
            return Self::from_hex(hex_key, "the installed master key").map(Some);
        }
        match std::env::var(MASTER_KEY_ENV) {
            Ok(hex_key) => Self::from_hex(&hex_key, MASTER_KEY_ENV).map(Some),
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
        let key = Key::<Aes256Gcm>::try_from(bytes.as_slice())
            .map_err(|e| format!("Invalid {source}: {e}"))?;
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
        let key_bytes: [u8; 32] = Generate::generate();
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

        let nonce_bytes: [u8; 12] = Generate::generate();
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
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
        let nonce = Nonce::try_from(nonce_bytes).map_err(|e| format!("Invalid nonce: {e}"))?;

        let cipher = Aes256Gcm::new(&self.key);
        let plaintext = cipher
            .decrypt(&nonce, ciphertext)
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
