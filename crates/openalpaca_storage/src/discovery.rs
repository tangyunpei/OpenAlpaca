//! Discovery mechanism for daemon-GUI/CLI communication.
//!
//! The daemon writes a `discovery.json` file containing:
//! - Listening address (host:port)
//! - Authentication token (32 bytes, base64url encoded)
//! - Instance metadata (PID, version, etc.)
//!
//! GUI/CLI read this file to connect to the running daemon.

use anyhow::{Context, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::paths;

// ============================================================================
// Data Structures
// ============================================================================

/// Root structure of discovery.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    /// Schema version for forward compatibility
    pub schema: u32,
    /// Unique instance identifier (UUID v4)
    pub instance_id: String,
    /// Process ID of the daemon
    pub pid: u32,
    /// When daemon started (UTC) - for stale detection
    pub started_at: DateTime<Utc>,
    /// Listening address info
    pub listen: Listen,
    /// Authentication credentials
    pub auth: Auth,
    /// Build metadata
    pub build: Build,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listen {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    /// Bearer token for API authentication (base64url, 32 bytes)
    pub token: String,
    /// When the token was issued
    pub issued_at: DateTime<Utc>,
    /// When the token expires (daemon will regenerate)
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    /// Semantic version of the daemon
    pub version: String,
    /// API protocol version
    pub protocol: u32,
}

// ============================================================================
// Token Generation
// ============================================================================

/// Generates a cryptographically secure token using OS-level CSPRNG.
/// Returns 32 bytes encoded as base64url (no padding) = 43 characters.
/// Uses rand::rng() which provides a secure, OS-seeded random generator.
pub fn generate_token() -> String {
    use rand::RngExt;
    let bytes: [u8; 32] = rand::rng().random();
    URL_SAFE_NO_PAD.encode(bytes)
}

// ============================================================================
// Directory & File Management
// ============================================================================

/// Ensures the app directory exists with proper permissions (700 on Unix).
pub fn ensure_app_dir() -> anyhow::Result<PathBuf> {
    let dir = paths::app_dir()?;
    fs::create_dir_all(&dir).context("Failed to create app directory")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best effort: don't fail if permissions can't be set
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }

    Ok(dir)
}

/// Writes discovery.json atomically (write to tmp, then rename).
/// Sets file permissions to 600 on Unix (owner read/write only).
/// Cleans up tmp file on failure to prevent accumulation.
pub fn write_discovery_atomic(d: &Discovery) -> anyhow::Result<PathBuf> {
    ensure_app_dir()?;
    let path = paths::discovery_path()?;
    let tmp = path.with_extension("json.tmp");

    let json = serde_json::to_vec_pretty(d).context("Failed to serialize discovery")?;

    // Write to temporary file (with cleanup on failure)
    let write_result = (|| -> anyhow::Result<()> {
        let mut f = fs::File::create(&tmp).context("Failed to create temporary discovery file")?;
        f.write_all(&json)
            .context("Failed to write discovery content")?;
        let _ = f.sync_all(); // Best effort fsync
        Ok(())
    })();

    if let Err(e) = write_result {
        // Clean up tmp file on failure (best effort)
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // Atomic rename
    if let Err(e) = fs::rename(&tmp, &path) {
        // Clean up tmp file on rename failure
        let _ = fs::remove_file(&tmp);
        return Err(e).context("Failed to atomically replace discovery.json");
    }

    // Set restrictive permissions (600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    Ok(path)
}

/// Reads discovery.json if it exists.
/// Returns None if the file doesn't exist, error if it exists but is invalid.
pub fn read_discovery() -> anyhow::Result<Option<Discovery>> {
    let path = paths::discovery_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).context("Failed to read discovery.json")?;
    let d: Discovery = serde_json::from_slice(&bytes).context("Failed to parse discovery.json")?;
    Ok(Some(d))
}

/// Removes discovery.json (called during daemon shutdown).
pub fn remove_discovery() -> anyhow::Result<()> {
    let path = paths::discovery_path()?;
    if path.exists() {
        fs::remove_file(&path).context("Failed to remove discovery.json")?;
    }
    Ok(())
}

// ============================================================================
// Validation
// ============================================================================

/// Checks if the discovery token has expired.
pub fn ensure_not_expired(d: &Discovery) -> anyhow::Result<()> {
    if Utc::now() >= d.auth.expires_at {
        bail!("Discovery token has expired");
    }
    Ok(())
}

/// Checks if the daemon process is still alive (by PID).
/// Note: This is a heuristic - PIDs can be reused.
#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks if process exists without sending a signal
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
pub fn is_process_alive(_pid: u32) -> bool {
    // On non-Unix, we can't easily check - assume alive and let health check verify
    true
}

// ============================================================================
// Singleton Lock (POSIX Advisory Lock)
// ============================================================================

/// Acquires an exclusive lock on the daemon lock file.
///
/// - `block`: If true, wait for the lock. If false, fail immediately if locked.
/// - Returns: A `FileLock` guard. The lock is released when this guard is dropped.
///
/// # Errors
/// Returns error if the lock cannot be acquired (another daemon is running).
pub fn acquire_single_instance_lock(block: bool) -> anyhow::Result<file_lock::FileLock> {
    ensure_app_dir()?;
    let lock_path = paths::lock_path()?;
    let lock_path_str = lock_path.to_str().context("Lock path is not valid UTF-8")?;

    let options = file_lock::FileOptions::new()
        .write(true)
        .create(true)
        .append(true);

    let guard = file_lock::FileLock::lock(lock_path_str, block, options)
        .map_err(|e| anyhow::anyhow!("Failed to acquire singleton lock: {e}"))?;

    // Set restrictive permissions on lock file
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600));
    }

    Ok(guard)
}

// ============================================================================
// Factory Functions
// ============================================================================

/// Creates a new Discovery struct with generated token.
///
/// # Arguments
/// - `host`: Listening host (usually "127.0.0.1")
/// - `port`: Listening port (from OS dynamic allocation)
/// - `instance_id`: Unique instance ID (UUID v4)
/// - `version`: Daemon version string
pub fn make_discovery(host: &str, port: u16, instance_id: String, version: &str) -> Discovery {
    let now = Utc::now();
    Discovery {
        schema: 1,
        instance_id,
        pid: std::process::id(),
        started_at: now,
        listen: Listen {
            host: host.to_string(),
            port,
        },
        auth: Auth {
            token: generate_token(),
            issued_at: now,
            expires_at: now + Duration::hours(24),
        },
        build: Build {
            version: version.to_string(),
            protocol: 1,
        },
    }
}

// ============================================================================
// Connection Info (for GUI/CLI)
// ============================================================================

/// Simplified connection info returned to GUI/CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub base_url: String,
    pub token: String,
    pub instance_id: String,
}

impl From<&Discovery> for ConnectionInfo {
    fn from(d: &Discovery) -> Self {
        ConnectionInfo {
            base_url: format!("http://{}:{}", d.listen.host, d.listen.port),
            token: d.auth.token.clone(),
            instance_id: d.instance_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_generation() {
        let token = generate_token();
        // 32 bytes -> 43 chars in base64url (no padding)
        assert_eq!(token.len(), 43);
        // Should be valid base64url
        assert!(URL_SAFE_NO_PAD.decode(&token).is_ok());
    }

    #[test]
    fn test_make_discovery() {
        let d = make_discovery("127.0.0.1", 8080, "test-id".to_string(), "0.1.0");
        assert_eq!(d.schema, 1);
        assert_eq!(d.listen.host, "127.0.0.1");
        assert_eq!(d.listen.port, 8080);
        assert_eq!(d.instance_id, "test-id");
        assert_eq!(d.build.version, "0.1.0");
        assert!(d.auth.expires_at > Utc::now());
    }

    #[test]
    fn test_connection_info_from_discovery() {
        let d = make_discovery("127.0.0.1", 9999, "my-instance".to_string(), "1.0.0");
        let info = ConnectionInfo::from(&d);
        assert_eq!(info.base_url, "http://127.0.0.1:9999");
        assert_eq!(info.instance_id, "my-instance");
        assert!(!info.token.is_empty());
    }
}
