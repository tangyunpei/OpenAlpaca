//! OpenAlpaca GUI - Tauri Backend
//!
//! Provides Tauri commands for:
//! - Connecting to the daemon via discovery.json
//! - Ensuring the daemon is running (spawning if needed)
//! - Waiting until a daemon the webview asked to stop is really gone
//! - Reading the end of the daemon log, so a daemon that would not start can
//!   say why

use openalpaca_storage::daemon_lifecycle::{self, StopOutcome};
use openalpaca_storage::discovery::{self, ConnectionInfo};
use openalpaca_storage::store;
use serde::Serialize;
use std::process::Command;
use std::time::Duration;

// ============================================================================
// Tauri Commands
// ============================================================================

/// Get connection info from discovery.json.
/// Returns error if daemon is not running or discovery is invalid.
#[tauri::command]
fn get_connection_info() -> Result<ConnectionInfo, String> {
    let Some(d) = discovery::read_discovery().map_err(|e| e.to_string())? else {
        return Err("Discovery not found: daemon may not be running".into());
    };

    discovery::ensure_not_expired(&d).map_err(|e| e.to_string())?;

    Ok(ConnectionInfo::from(&d))
}

/// Ensure the daemon is running, spawning it if necessary.
/// Returns connection info once daemon is ready.
#[tauri::command]
async fn ensure_daemon_running() -> Result<ConnectionInfo, String> {
    // Step 1: Check if a daemon is already running AND actually alive. A stale
    // discovery.json (left by a crashed/killed daemon or a reboot) would
    // otherwise be trusted forever, so the GUI would never respawn. Liveness is
    // the authoritative signal — a live daemon is used even if the file's own
    // 24h expiry has lapsed (long uptime).
    if let Ok(Some(d)) = discovery::read_discovery()
        && daemon_is_alive(&d)
    {
        return Ok(ConnectionInfo::from(&d));
    }

    // Step 2: Spawn the daemon process
    let log_start = spawn_daemon().map_err(|e| format!("Failed to spawn daemon: {e:#}"))?;

    // Step 3: Wait for the new daemon to appear AND accept connections.
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;

        if let Ok(Some(d)) = discovery::read_discovery()
            && daemon_is_alive(&d)
        {
            return Ok(ConnectionInfo::from(&d));
        }
    }

    // It did not come up. The daemon's own reason — a legacy-schema database,
    // an older install's data, a store it cannot create, another daemon
    // holding the lock — is in its log, and nothing else can say it: with no
    // daemon serving `GET /v1/status` there is nobody to ask where the log
    // is. Only what *this* spawn wrote is read, so an earlier run's last
    // words are never presented as this one's.
    let log_path = store::daemon_log_path().map_err(|e| format!("{e:#}"))?;
    let tail = store::read_log_tail(&log_path, log_start, TIMEOUT_TAIL_LINES).unwrap_or_default();
    if tail.is_empty() {
        Err(format!(
            "The daemon did not start within 5 seconds, and wrote nothing to its log \
             ({}). Check that the daemon binary is installed beside the app.",
            log_path.display()
        ))
    } else {
        Err(format!(
            "The daemon did not start within 5 seconds. Its log ({}) ends with:\n\n{tail}",
            log_path.display()
        ))
    }
}

/// How many log lines a failed start quotes. Enough for the longest refusal
/// the daemon writes at boot (the older-install one runs to about thirty
/// lines) with its log line in front.
const TIMEOUT_TAIL_LINES: usize = 40;

/// The last `lines` lines of the daemon log, newest last — `""` when there is
/// no log yet.
///
/// Backs Settings → Connection's `Show daemon log`, for a daemon that did
/// start but is misbehaving, or one that would not start at all: this reads
/// the file directly, so it answers even when nothing is serving `/v1/*`.
/// Reads at most `store::LOG_TAIL_READ_BYTES` from the end of the file; the
/// line handling is `store::read_log_tail`'s, which is where it is tested —
/// CI compiles none of this crate.
#[tauri::command]
fn read_daemon_log_tail(lines: usize) -> Result<String, String> {
    let path = store::daemon_log_path().map_err(|e| format!("{e:#}"))?;
    store::read_log_tail(&path, 0, lines)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))
}

/// What [`await_daemon_stopped`] concluded, serialized to the webview in
/// camelCase: `{ outcome, pid, waitedMs }`.
///
/// `outcome` is one of `not_running`, `stopped`, `lock_still_held` or
/// `still_alive` — [`StopOutcome`]'s four answers, spelled as data. Only
/// `not_running` and `stopped` mean a daemon can be started now; `pid` is set
/// only for `still_alive`, so the webview can name the process it could not
/// wait out.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonStopReport {
    outcome: &'static str,
    pid: Option<u32>,
    waited_ms: u64,
}

impl From<StopOutcome> for DaemonStopReport {
    fn from(outcome: StopOutcome) -> Self {
        let millis = |waited: Duration| u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
        match outcome {
            StopOutcome::NotRunning => DaemonStopReport {
                outcome: "not_running",
                pid: None,
                waited_ms: 0,
            },
            StopOutcome::Stopped { waited } => DaemonStopReport {
                outcome: "stopped",
                pid: None,
                waited_ms: millis(waited),
            },
            StopOutcome::LockStillHeld { waited } => DaemonStopReport {
                outcome: "lock_still_held",
                pid: None,
                waited_ms: millis(waited),
            },
            StopOutcome::StillAlive { pid, waited } => DaemonStopReport {
                outcome: "still_alive",
                pid: Some(pid),
                waited_ms: millis(waited),
            },
        }
    }
}

/// Wait until the daemon is really gone: first its process, then the
/// singleton lock, for at most `daemon_lifecycle::STOP_TIMEOUT` (15 s).
///
/// The webview stops a daemon over HTTP (`POST /v1/command {"command":
/// "shutdown"}`), and that route's `200 shutting_down` is an acceptance, not a
/// completion: the daemon still has up to 10 s of shutdown tail to run, and a
/// replacement started inside it loses the non-blocking lock race and exits.
/// So the GUI never reports "stopped" on the strength of the 200 — it asks
/// this command, which is a thin wrapper over the same
/// `daemon_lifecycle::wait_for_daemon_exit` the CLI's `daemon stop|restart`
/// wait through. It signals nothing: this shell never kills a process.
///
/// The wait polls with a blocking sleep, so it runs on the blocking pool —
/// on the async runtime's own threads it would stall every other `invoke`
/// for up to 15 s.
#[tauri::command]
async fn await_daemon_stopped() -> Result<DaemonStopReport, String> {
    tauri::async_runtime::spawn_blocking(|| {
        daemon_lifecycle::wait_for_daemon_exit(daemon_lifecycle::STOP_TIMEOUT)
    })
    .await
    .map(DaemonStopReport::from)
    .map_err(|e| format!("Could not wait for the daemon to stop: {e}"))
}

/// Liveness probe: can we open a TCP connection to the daemon's listen address?
/// A stale discovery.json (dead daemon) fails this, so callers respawn instead
/// of returning a dead endpoint.
fn daemon_is_alive(d: &discovery::Discovery) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(addrs) = (d.listen.host.as_str(), d.listen.port).to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
            return true;
        }
    }
    false
}

/// Spawn the daemon as a detached background process, its stdout and stderr
/// appended to `daemon.log`.
///
/// Returns the log's length just before the spawn — where this run's output
/// begins — so a failed start quotes only what this run wrote.
fn spawn_daemon() -> anyhow::Result<u64> {
    // Find the daemon executable
    // In development: should be in the same target directory
    // In production: should be bundled with the app

    let daemon_name = if cfg!(windows) {
        "openalpacad.exe"
    } else {
        "openalpacad"
    };

    // Try to find daemon in common locations
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().unwrap_or(std::path::Path::new("."));

    let daemon_path = exe_dir.join(daemon_name);

    let path_to_use = if daemon_path.exists() {
        daemon_path
    } else {
        #[cfg(debug_assertions)]
        {
            // Dev convenience: fall back to PATH
            std::path::PathBuf::from(daemon_name)
        }
        #[cfg(not(debug_assertions))]
        {
            anyhow::bail!(
                "Daemon binary not found at {}. The application may be incorrectly installed.",
                daemon_path.display()
            );
        }
    };

    // D1: one root. `ensure_store` seeds README/.layout; the config dir must
    // exist before the spawn — `resolve_config_base_dir` ignores an
    // `OPENALPACA_CONFIG_DIR` that does not.
    let app_dir = store::ensure_store(&store::StoreScope::Home)?;
    let config_dir = store::ensure_runtime_config_dir()?;

    // The sidecar's output used to go to /dev/null on both platforms, so a
    // daemon that refused to boot — a legacy-schema database, an older
    // install's data, a store it cannot create, a master key it cannot read,
    // another daemon already holding the lock — told the user nothing beyond
    // "did not become ready within timeout". It now writes the same
    // `daemon.log` the CLI launcher writes, under the same rotation, and marks
    // itself as that file's owner so `GET /v1/status` hands the path back
    // (T30: `MANAGED_LOG_ENV` means "the launcher pointed stdio at the file
    // and rotated it", which is now true of both launchers).
    store::logs_dir()?;
    let log_path = store::daemon_log_path()?;
    if let Err(e) = store::rotate_daemon_log() {
        // Never fatal: a daemon that will not start because its log could not
        // be renamed is the worse bug.
        tracing::warn!("Could not rotate the daemon log ({e:#}); appending to it as it is.");
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| anyhow::anyhow!("Failed to open {}: {e}", log_path.display()))?;
    let log_start = log_file.metadata().map(|m| m.len()).unwrap_or(0);
    let stdout = std::process::Stdio::from(log_file.try_clone()?);
    let stderr = std::process::Stdio::from(log_file);

    tracing::info!("Spawning daemon: {}", path_to_use.display());
    tracing::info!("Daemon runtime dir: {}", app_dir.display());
    tracing::info!("Daemon config dir: {}", config_dir.display());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // On Unix, use setsid to detach from terminal
        let mut cmd = Command::new(&path_to_use);
        cmd.stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .current_dir(&app_dir)
            .env("OPENALPACA_CONFIG_DIR", &config_dir)
            .env(store::MANAGED_LOG_ENV, "1");

        // Create new session (detach from parent)
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        cmd.spawn()?;
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        Command::new(&path_to_use)
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .current_dir(&app_dir)
            .env("OPENALPACA_CONFIG_DIR", &config_dir)
            .env(store::MANAGED_LOG_ENV, "1")
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()?;
    }

    Ok(log_start)
}

// ============================================================================
// Tauri App Entry Point
// ============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_connection_info,
            ensure_daemon_running,
            await_daemon_stopped,
            read_daemon_log_tail
        ])
        .run(tauri::generate_context!())
        .expect("Error while running Tauri application");
}
