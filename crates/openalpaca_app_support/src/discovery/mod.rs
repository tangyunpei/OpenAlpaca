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
    /// When the token expires: [`TOKEN_LIFETIME`] after its issue. The daemon
    /// replaces the token [`ROTATE_AFTER`] after its issue, so a running
    /// daemon's file is never read expired (T21 (2)).
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
// Token Generation and Rotation (T21 (2))
// ============================================================================

/// How long a token lives from its issue — what `auth.expires_at` says.
pub const TOKEN_LIFETIME: Duration = Duration::hours(24);

/// How long after its issue the running daemon replaces a token with a new
/// one (T21 (2)): half its lifetime, so a token read from the file has at
/// least twelve hours left, and the daemon, which honours the token it
/// replaced until that one's own `expires_at`, holds at most two.
pub const ROTATE_AFTER: Duration = Duration::hours(12);

/// A new token issued at `now`, expiring [`TOKEN_LIFETIME`] later.
pub fn issue_auth(now: DateTime<Utc>) -> Auth {
    Auth {
        token: generate_token(),
        issued_at: now,
        expires_at: now + TOKEN_LIFETIME,
    }
}

impl Discovery {
    /// Whether the daemon should replace this file's token at `now`:
    /// [`ROTATE_AFTER`] after its issue, or at once when the wall clock is
    /// before the issue (it was set back). Past the expiry — a Mac that slept
    /// through the schedule — it is due at the first look after the wake.
    pub fn rotation_due(&self, now: DateTime<Utc>) -> bool {
        now >= self.auth.issued_at + ROTATE_AFTER || now < self.auth.issued_at
    }

    /// This file with a new token issued at `now` and everything else as it
    /// is: the instance, pid, start, address and build describe the run, and
    /// a changed `instance_id` would tell a client the daemon restarted.
    pub fn with_rotated_auth(&self, now: DateTime<Utc>) -> Discovery {
        Discovery {
            auth: issue_auth(now),
            ..self.clone()
        }
    }
}

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

/// Writes discovery.json atomically (write to tmp, then rename).
/// Sets file permissions to 600 on Unix (owner read/write only).
/// Cleans up tmp file on failure to prevent accumulation.
pub fn write_discovery_atomic(d: &Discovery) -> anyhow::Result<PathBuf> {
    // `state_dir()` creates the directory (0700 on Unix) if it is missing;
    // `discovery_path()` itself is a pure path query.
    paths::state_dir()?;
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

/// Checks if the discovery token has expired — the clients' check, kept by
/// T21 (2): a running daemon replaces its token before this fires, so it
/// refuses the file of a daemon that is gone, hung or from before T21.
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
// Singleton Lock (an advisory whole-file lock)
// ============================================================================

/// The daemon's singleton lock, held for as long as this guard lives.
///
/// It is the open lock file itself: `std`'s `File::try_lock` takes a `flock`
/// lock on Unix, which belongs to the open file description, so dropping the
/// guard closes the file and releases the lock — no explicit unlock. Another
/// open of the same file, in this process or another, is refused while it
/// lives.
#[must_use = "the singleton lock is released as soon as this guard is dropped"]
#[derive(Debug)]
pub struct SingleInstanceLock(
    #[expect(dead_code, reason = "held only so that dropping it releases the lock")] fs::File,
);

/// Takes the daemon's singleton lock (`state/openalpacad.lock`) without
/// waiting: a lock another holder already has is refused at once.
///
/// - Returns: A [`SingleInstanceLock`] guard. The lock is released when this
///   guard is dropped.
///
/// # Errors
/// When another holder has the lock (another daemon is running or starting,
/// or a local verb holds the store), the error says that the lock file is
/// held by another process. A lock file that cannot be opened (one owned by
/// another user) or any other failure is passed through.
pub fn acquire_single_instance_lock() -> anyhow::Result<SingleInstanceLock> {
    // `state_dir()` creates the directory (0700 on Unix) if it is missing;
    // `lock_path()` itself is a pure path query.
    paths::state_dir()?;
    let lock_path = paths::lock_path()?;

    // Opened for writing (append implies it), so a lock file this user cannot
    // write — one a daemon run under `sudo` left owned by root — is refused
    // here as it always was.
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lock_path)
        .map_err(|e| anyhow::anyhow!("Failed to acquire singleton lock: {e}"))?;

    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            bail!("{} is held by another process", lock_path.display());
        }
        Err(fs::TryLockError::Error(e)) => {
            bail!("Failed to acquire singleton lock: {e}");
        }
    }

    // Set restrictive permissions on lock file
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600));
    }

    Ok(SingleInstanceLock(file))
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
        auth: issue_auth(now),
        build: Build {
            version: version.to_string(),
            protocol: crate::protocol::reported(),
        },
    }
}

// ============================================================================
// Connection Info (for GUI/CLI)
// ============================================================================

/// Simplified connection info returned to GUI/CLI.
///
/// `version` and `protocol` (Q04) are the daemon's `build.version` and
/// `build.protocol`, additive: an older shell's webview never reads them, and
/// a window reads them as absent from an older shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub base_url: String,
    pub token: String,
    pub instance_id: String,
    pub version: String,
    pub protocol: u32,
}

impl From<&Discovery> for ConnectionInfo {
    fn from(d: &Discovery) -> Self {
        ConnectionInfo {
            base_url: format!("http://{}:{}", d.listen.host, d.listen.port),
            token: d.auth.token.clone(),
            instance_id: d.instance_id.clone(),
            version: d.build.version.clone(),
            protocol: d.build.protocol,
        }
    }
}

#[cfg(test)]
mod tests;
