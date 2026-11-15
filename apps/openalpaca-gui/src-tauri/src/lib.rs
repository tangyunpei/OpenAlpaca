//! OpenAlpaca GUI - Tauri Backend
//!
//! Provides Tauri commands for:
//! - Connecting to the daemon via discovery.json
//! - Ensuring the daemon is running (spawning if needed)
//! - Waiting until a daemon the webview asked to stop is really gone
//! - Reading the end of the daemon log, so a daemon that would not start can
//!   say why
//! - Showing an artifact in its folder (Finder, Explorer, the file manager)
//!
//! Every command is `async`, so none runs on the main thread, which draws the
//! window (Tauri runs a command without `async` there); every file read, the
//! spawn and every other blocking call runs on the blocking pool
//! (`tauri::async_runtime::spawn_blocking`), so it never holds a thread of
//! the async runtime either; and the liveness probe is tokio's connect under
//! tokio's timeout (Q12).
//!
//! A command refuses with a [`ShellError`] (Q13): `{ kind, message }` to the
//! webview, `kind` what refused and `message` the sentence it shows.
//!
//! The shell logs to `state/logs/gui.log` (Q14): its `tracing` lines and
//! Tauri's `log` records, its startup marks, a panic, and a web content
//! process that ended — after which the window is reloaded.
//!
//! The daemon it spawns runs on the `PATH` of the account's login shell,
//! captured once at the app's start, or on a fixed list when that capture
//! fails (Q21, [`login_path`]); `ensure_daemon_running` and
//! `get_connection_info` answer which, for a daemon this app spawned.

mod login_path;

use login_path::{DaemonPath, DaemonPathReport};
use openalpaca_storage::daemon_lifecycle::{self, StopOutcome};
use openalpaca_storage::discovery::{self, ConnectionInfo};
use openalpaca_storage::store;
use serde::Serialize;
use std::future::Future;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::webview::PageLoadEvent;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::util::SubscriberInitExt;

// ============================================================================
// The refusals
// ============================================================================

/// Why a command refused, as the webview receives it (Q13): an object whose
/// `kind` is the variant in snake_case and whose `message` is the sentence
/// the webview shows as it is — the sentence each command answered with
/// before the refusals were typed, byte for byte. `SpawnFailed` carries the
/// daemon log's tail beside it. The webview branches on `kind`, never on the
/// sentence's words.
///
/// The TypeScript side mirrors it by hand (`src/lib/shell-error.ts`), and
/// `src/lib/shell-errors.json` holds every variant's wire form, which a test
/// on each side reads (tauri-specta, the typed-IPC form, is not yet at a
/// stable 2.0). Tauri's own refusals — an argument missing, a command not
/// admitted — stay bare strings, and are not this.
#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ShellError {
    /// `get_connection_info`: there is no `discovery.json`, so no daemon is
    /// serving (or it removed the file on its way out).
    #[error("{message}")]
    NotRunning { message: String },
    /// `get_connection_info`: the discovery file's token has expired.
    #[error("{message}")]
    Expired { message: String },
    /// `get_connection_info`: `discovery.json` could not be found, read or
    /// parsed, or its read could not be run.
    #[error("{message}")]
    DiscoveryUnreadable { message: String },
    /// `ensure_daemon_running`: no daemon came up — it could not be spawned,
    /// or the one spawned did not answer within 5 seconds. `log_tail` is what
    /// this run wrote to `daemon.log` (also quoted in `message`), `None` when
    /// it wrote nothing or the log could not be read.
    #[error("{message}")]
    SpawnFailed {
        message: String,
        log_tail: Option<String>,
    },
    /// `read_daemon_log_tail`: the log could not be found or read, or its
    /// read could not be run.
    #[error("{message}")]
    LogUnreadable { message: String },
    /// `await_daemon_stopped`: the wait could not be run, so whether the
    /// daemon is gone is not known.
    #[error("{message}")]
    StopUnconfirmed { message: String },
    /// `reveal_path`: the path is not one the shell shows
    /// (`store::RevealRefusal`).
    #[error("{message}")]
    RevealRefused { message: String },
    /// `reveal_path`: the platform's program could not show an admitted
    /// path, or could not be run.
    #[error("{message}")]
    RevealFailed { message: String },
}

/// [`ShellError::SpawnFailed`] with no log tail.
fn spawn_failed(message: String) -> ShellError {
    ShellError::SpawnFailed {
        message,
        log_tail: None,
    }
}

/// [`ShellError::SpawnFailed`] for a daemon that did not answer within the
/// wait: `tail` is what this run wrote to `log_path`, empty when nothing.
fn did_not_start(log_path: &Path, tail: String) -> ShellError {
    if tail.is_empty() {
        spawn_failed(format!(
            "The daemon did not start within 5 seconds, and wrote nothing to its log \
             ({}). Check that the daemon binary is installed beside the app.",
            log_path.display()
        ))
    } else {
        ShellError::SpawnFailed {
            message: format!(
                "The daemon did not start within 5 seconds. Its log ({}) ends with:\n\n{tail}",
                log_path.display()
            ),
            log_tail: Some(tail),
        }
    }
}

// ============================================================================
// The connection the webview reads
// ============================================================================

/// What `ensure_daemon_running` and `get_connection_info` answer: the
/// daemon's [`ConnectionInfo`] (`{ baseUrl, token, instanceId }`) and
/// `daemonPath`, where the `PATH` of a daemon this app spawned came from
/// (Q21) — `null` for a daemon this run of the app did not spawn: one started
/// from a terminal, or by an earlier run of the app.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellConnection {
    #[serde(flatten)]
    info: ConnectionInfo,
    daemon_path: Option<DaemonPathReport>,
}

/// Every daemon this run of the app spawned, by its process id, with the
/// `PATH` it was given — every one, not the last: two starts that overlap (a
/// reload while the first waits for the capture) each spawn a daemon, and
/// the one that takes the store's lock and writes `discovery.json` may be
/// either. The capture is taken once, so each carries the same report.
static SPAWNED: Mutex<Vec<(u32, DaemonPathReport)>> = Mutex::new(Vec::new());

/// Remember the daemon just spawned as `pid`, with the `PATH` it was given;
/// nothing where the app captures none.
fn record_spawned(pid: u32, report: Option<DaemonPathReport>) {
    if let Some(report) = report {
        SPAWNED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((pid, report));
    }
}

/// Forget every daemon spawned so far, so no test inherits another's.
#[cfg(test)]
fn forget_spawned() {
    SPAWNED.lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// `d`'s connection, with `daemonPath` when `d` names a daemon this run
/// spawned (its `pid`).
fn shell_connection(d: &discovery::Discovery) -> ShellConnection {
    let daemon_path = SPAWNED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(pid, _)| *pid == d.pid)
        .map(|(_, report)| report.clone());
    ShellConnection {
        info: ConnectionInfo::from(d),
        daemon_path,
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Get connection info from discovery.json.
/// Returns error if daemon is not running or discovery is invalid.
///
/// The file is read on the blocking pool.
#[tauri::command]
async fn get_connection_info() -> Result<ShellConnection, ShellError> {
    tauri::async_runtime::spawn_blocking(read_connection_info)
        .await
        .map_err(|e| ShellError::DiscoveryUnreadable {
            message: format!("Could not read the daemon's discovery file: {e}"),
        })?
}

/// [`get_connection_info`]'s work: read discovery.json and check its token.
fn read_connection_info() -> Result<ShellConnection, ShellError> {
    let Some(d) = discovery::read_discovery().map_err(|e| ShellError::DiscoveryUnreadable {
        message: e.to_string(),
    })?
    else {
        return Err(ShellError::NotRunning {
            message: "Discovery not found: daemon may not be running".into(),
        });
    };

    discovery::ensure_not_expired(&d).map_err(|e| ShellError::Expired {
        message: e.to_string(),
    })?;

    Ok(shell_connection(&d))
}

/// Ensure the daemon is running, spawning it if necessary.
/// Returns connection info once daemon is ready.
///
/// Every read of discovery.json, the spawn and the read of the log run on
/// the blocking pool, and the probe is tokio's, so the command holds no
/// runtime thread while it waits on a disk, a connect or the new process.
#[tauri::command]
async fn ensure_daemon_running() -> Result<ShellConnection, ShellError> {
    // Step 1: Check if a daemon is already running AND actually alive. A stale
    // discovery.json (left by a crashed/killed daemon or a reboot) would
    // otherwise be trusted forever, so the GUI would never respawn. Liveness is
    // the authoritative signal — a live daemon is used even if the file's own
    // 24h expiry has lapsed (long uptime).
    if let Some(info) = live_daemon().await {
        return Ok(info);
    }

    // Step 2: Spawn the daemon process
    let spawned = tauri::async_runtime::spawn_blocking(spawn_daemon)
        .await
        .map_err(|e| spawn_failed(format!("Failed to spawn daemon: {e}")))?
        .map_err(|e| spawn_failed(format!("Failed to spawn daemon: {e:#}")))?;
    let log_start = spawned.log_start;
    record_spawned(spawned.pid, spawned.path);

    // Step 3: Wait for the new daemon to appear AND accept connections.
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;

        if let Some(info) = live_daemon().await {
            return Ok(info);
        }
    }

    // It did not come up. The daemon's own reason — a database this build
    // does not know, a store it cannot create, another daemon holding the
    // lock — is in its log, and nothing else can
    // say it: with no daemon serving `GET /v1/status` there is nobody to ask
    // where the log is. Only what *this* spawn wrote is read, so an earlier
    // run's last words are never presented as this one's.
    let (log_path, tail) = tauri::async_runtime::spawn_blocking(move || {
        let log_path = store::daemon_log_path().map_err(|e| format!("{e:#}"))?;
        let tail =
            store::read_log_tail(&log_path, log_start, TIMEOUT_TAIL_LINES).unwrap_or_default();
        Ok::<_, String>((log_path, tail))
    })
    .await
    .map_err(|e| format!("Could not read the daemon log: {e}"))
    .and_then(|read| read)
    .map_err(spawn_failed)?;
    Err(did_not_start(&log_path, tail))
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
/// the workspace's gate set (`default-members`) leaves this crate out, and
/// it is checked and tested only by hand (`-p openalpaca_gui`).
///
/// The read runs on the blocking pool.
#[tauri::command]
async fn read_daemon_log_tail(lines: usize) -> Result<String, ShellError> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = store::daemon_log_path().map_err(|e| format!("{e:#}"))?;
        store::read_log_tail(&path, 0, lines)
            .map_err(|e| format!("Could not read {}: {e}", path.display()))
    })
    .await
    .map_err(|e| format!("Could not read the daemon log: {e}"))
    .and_then(|read| read)
    .map_err(|message| ShellError::LogUnreadable { message })
}

/// What [`await_daemon_stopped`] concluded, serialized to the webview in
/// camelCase: `{ outcome, pid, waitedMs }`.
///
/// `outcome` is one of `not_running`, `stopped`, `lock_still_held` or
/// `still_alive` — [`StopOutcome`]'s four answers, a [`StopReportOutcome`]
/// serialized in snake_case. Only `not_running` and `stopped` mean a daemon
/// can be started now; `pid` is set only for `still_alive`, so the webview
/// can name the process it could not wait out.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonStopReport {
    outcome: StopReportOutcome,
    pid: Option<u32>,
    waited_ms: u64,
}

/// [`StopOutcome`]'s four answers without their data, as the webview reads
/// [`DaemonStopReport`]'s `outcome` (Q13: an enum, where it was a string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StopReportOutcome {
    NotRunning,
    Stopped,
    LockStillHeld,
    StillAlive,
}

impl From<StopOutcome> for DaemonStopReport {
    fn from(outcome: StopOutcome) -> Self {
        let millis = |waited: Duration| u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
        match outcome {
            StopOutcome::NotRunning => DaemonStopReport {
                outcome: StopReportOutcome::NotRunning,
                pid: None,
                waited_ms: 0,
            },
            StopOutcome::Stopped { waited } => DaemonStopReport {
                outcome: StopReportOutcome::Stopped,
                pid: None,
                waited_ms: millis(waited),
            },
            StopOutcome::LockStillHeld { waited } => DaemonStopReport {
                outcome: StopReportOutcome::LockStillHeld,
                pid: None,
                waited_ms: millis(waited),
            },
            StopOutcome::StillAlive { pid, waited } => DaemonStopReport {
                outcome: StopReportOutcome::StillAlive,
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
async fn await_daemon_stopped() -> Result<DaemonStopReport, ShellError> {
    tauri::async_runtime::spawn_blocking(|| {
        daemon_lifecycle::wait_for_daemon_exit(daemon_lifecycle::STOP_TIMEOUT)
    })
    .await
    .map(DaemonStopReport::from)
    .map_err(|e| ShellError::StopUnconfirmed {
        message: format!("Could not wait for the daemon to stop: {e}"),
    })
}

/// Show a file in its folder: Finder with the file selected on macOS
/// (`open -R`), Explorer with it selected on Windows (`explorer /select,`),
/// and the folder holding it in the file manager elsewhere (`xdg-open`).
///
/// The path comes from the webview — an artifact's `path` as the daemon served
/// it — so the shell does not take it on trust: `store::revealable_path`
/// resolves it, symlinks included, and admits only a regular file in an
/// OpenAlpaca store or under the root of a project `projects/` records, where
/// a file an agent wrote into the project lives (storage design §13, ST-18:
/// Reveal answers for such a file whatever its kind). The platform program
/// gets that resolved path as an argument of its own, never through a shell.
/// This shows the file; opening it stays the daemon's route
/// (`POST /v1/files/{id}/open`). A refusal's sentence is one the webview
/// shows as it is: [`ShellError::RevealRefused`] for a path the check does
/// not admit, [`ShellError::RevealFailed`] for one the platform could not
/// show.
///
/// The check (it reads `projects/`) and the program both run on the blocking
/// pool, so a slow disk or file manager never stalls another `invoke`.
#[tauri::command]
async fn reveal_path(path: String) -> Result<(), ShellError> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = store::revealable_path(Path::new(&path)).map_err(|refusal| {
            ShellError::RevealRefused {
                message: refusal.to_string(),
            }
        })?;
        reveal_in_folder(&path).map_err(|message| ShellError::RevealFailed { message })
    })
    .await
    .map_err(|e| ShellError::RevealFailed {
        message: format!("Could not show the file in its folder: {e}."),
    })?
}

/// Run the platform's reveal for an already admitted path and wait for it.
fn reveal_in_folder(path: &Path) -> Result<(), String> {
    let (program, args) = if cfg!(target_os = "macos") {
        ("open", vec!["-R".into(), path.as_os_str().to_owned()])
    } else if cfg!(windows) {
        // `/select,` and the path as two arguments, so a path with a space is
        // quoted on its own: `explorer /select, "C:\a b\c.md"`.
        (
            "explorer",
            vec![
                "/select,".into(),
                without_verbatim_prefix(path).into_os_string(),
            ],
        )
    } else {
        let folder = path.parent().unwrap_or(path);
        ("xdg-open", vec![folder.as_os_str().to_owned()])
    };
    let status = Command::new(program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Could not show {} in its folder: {e}.", path.display()))?;
    // Explorer exits 1 even when it opened the window, so on Windows only a
    // failure to start it counts.
    if status.success() || cfg!(windows) {
        Ok(())
    } else {
        Err(format!(
            "{program} could not show {} in its folder ({status}).",
            path.display()
        ))
    }
}

/// `C:\…` for a canonical `\\?\C:\…`, which Explorer does not understand;
/// any other path as it is.
fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let mut components = path.components();
    if let Some(Component::Prefix(prefix)) = components.next()
        && let Prefix::VerbatimDisk(drive) = prefix.kind()
    {
        let mut plain = PathBuf::from(format!("{}:\\", char::from(drive)));
        plain.extend(components.filter(|c| !matches!(c, Component::RootDir)));
        return plain;
    }
    path.to_path_buf()
}

/// The daemon discovery.json names, if there is one and it answers the
/// probe. The file is read on the blocking pool; a missing, unreadable or
/// unparsable file is no daemon, as is a read that could not be run.
async fn live_daemon() -> Option<ShellConnection> {
    let d = tauri::async_runtime::spawn_blocking(discovery::read_discovery)
        .await
        .ok()?
        .ok()??;
    daemon_is_alive(&d).await.then(|| shell_connection(&d))
}

/// How long the probe waits for one address's connect.
const LIVENESS_PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// Liveness probe: can we open a TCP connection to the daemon's listen address?
/// A stale discovery.json (dead daemon) fails this, so callers respawn instead
/// of returning a dead endpoint.
///
/// tokio's resolver and connect, each address under tokio's timeout: the
/// waits are the runtime's timers, so a slow or silent address holds no
/// thread.
async fn daemon_is_alive(d: &discovery::Discovery) -> bool {
    let Ok(addrs) = tokio::net::lookup_host((d.listen.host.as_str(), d.listen.port)).await else {
        return false;
    };
    any_address_answers(addrs, tokio::net::TcpStream::connect).await
}

/// Whether `connect` completes for any of `addrs`, tried in order, each
/// given [`LIVENESS_PROBE_TIMEOUT`] and no more. The connect is a parameter
/// so a test can give the probe an address that never answers.
async fn any_address_answers<S, F>(
    addrs: impl IntoIterator<Item = SocketAddr>,
    connect: impl Fn(SocketAddr) -> F,
) -> bool
where
    F: Future<Output = std::io::Result<S>>,
{
    for addr in addrs {
        if let Ok(Ok(_)) = tokio::time::timeout(LIVENESS_PROBE_TIMEOUT, connect(addr)).await {
            return true;
        }
    }
    false
}

/// What [`spawn_daemon`] started.
struct Spawned {
    /// The log's length just before the spawn — where this run's output
    /// begins — so a failed start quotes only what this run wrote.
    log_start: u64,
    /// The daemon's process id, as its `discovery.json` will name it.
    pid: u32,
    /// The `PATH` it was given, `None` where the app captures none.
    path: Option<DaemonPathReport>,
}

/// Spawn the daemon as a detached background process, its stdout and stderr
/// appended to `daemon.log`, on the `PATH` [`login_path::for_the_daemon`]
/// answers — the login shell's, or the fallback list — waiting for the
/// app's capture at most until its deadline.
fn spawn_daemon() -> anyhow::Result<Spawned> {
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
    // daemon that refused to boot — a database this build does not know, a
    // store it cannot create, a master key it cannot read, another daemon
    // already holding the lock — told the user nothing
    // beyond "did not become ready within timeout". It now writes the same
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

    let daemon_path = login_path::for_the_daemon();

    tracing::info!("Spawning daemon: {}", path_to_use.display());
    tracing::info!("Daemon runtime dir: {}", app_dir.display());
    tracing::info!("Daemon config dir: {}", config_dir.display());
    if let Some(path) = &daemon_path {
        announce_daemon_path(path);
    }

    let mut cmd = daemon_command(&path_to_use, &app_dir, &config_dir, daemon_path.as_ref());
    cmd.stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr);

    // Only the detachment differs by platform. A platform that is neither
    // builds the command and never spawns it, as it never did.
    #[cfg(unix)]
    let pid = {
        use std::os::unix::process::CommandExt;

        // On Unix, use setsid to detach from terminal: a new session,
        // detached from the parent.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        let pid = child.id();
        reap_when_it_exits(child);
        pid
    };

    #[cfg(windows)]
    let pid = {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        // The `Child` is dropped, not reaped: that closes its handle and
        // leaves the detached daemon running.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()?
            .id()
    };

    #[cfg(not(any(unix, windows)))]
    let pid = {
        let _ = &mut cmd;
        0
    };

    Ok(Spawned {
        log_start,
        pid,
        path: daemon_path.as_ref().map(DaemonPath::report),
    })
}

/// The spawn's line in `gui.log`: the daemon's `PATH` source and the
/// capture's duration, never the `PATH`.
fn announce_daemon_path(path: &DaemonPath) {
    tracing::info!(
        source = path.source.as_str(),
        capture_ms = path.capture_ms(),
        "Daemon PATH source"
    );
}

/// The daemon's command but its stdio: its working directory the store's
/// root, `OPENALPACA_CONFIG_DIR`, [`store::MANAGED_LOG_ENV`], and — with a
/// [`DaemonPath`] — `PATH` set to it and [`store::PATH_SOURCE_ENV`] to its
/// source and capture time (`login-shell 212`), never the `PATH` itself.
/// Nothing else is set or removed: every other variable is the app's own,
/// inherited as before.
///
/// On Unix a `PATH` set on the command is also the one a bare program name
/// is looked up on, so a debug build's `openalpacad` fallback (no daemon
/// beside the app) is looked up on the daemon's `PATH`, not the app's.
fn daemon_command(
    program: &Path,
    app_dir: &Path,
    config_dir: &Path,
    path: Option<&DaemonPath>,
) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(app_dir)
        .env("OPENALPACA_CONFIG_DIR", config_dir)
        .env(store::MANAGED_LOG_ENV, "1");
    if let Some(path) = path {
        cmd.env("PATH", path.value())
            .env(store::PATH_SOURCE_ENV, path.announcement());
    }
    cmd
}

/// Wait on the spawned daemon from a thread of its own, so that when it exits
/// it is reaped instead of lingering as a zombie child of this app.
///
/// `setsid` gives the daemon its own session, but this process stays its
/// parent until one of them exits. A child nobody waits on is kept in the
/// process table after it exits — on Linux under its old name — and every
/// "is it gone yet?" check read that as a daemon still running: each Stop
/// waited out its whole budget and reported `still_alive`.
///
/// Waiting changes nothing else. `wait` neither signals nor holds the child,
/// the thread is never joined, and when the app quits the thread dies with
/// it while the daemon — in its own session — keeps running and is adopted by
/// init, which reaps it from then on. The daemon still outlives the window.
#[cfg(unix)]
fn reap_when_it_exits(mut child: std::process::Child) {
    let pid = child.id();
    let spawned = std::thread::Builder::new()
        .name("openalpacad-reaper".into())
        .spawn(move || match child.wait() {
            Ok(status) => tracing::info!("Daemon (pid {pid}) exited: {status}"),
            Err(e) => tracing::warn!("Could not wait on the daemon (pid {pid}): {e}"),
        });
    if let Err(e) = spawned {
        // The daemon is up either way; only its reaping is lost.
        tracing::warn!("Could not start the daemon reaper thread: {e}");
    }
}

// ============================================================================
// The shell's log (Q14)
// ============================================================================

/// The target every startup mark is logged under, so a reader of `gui.log`
/// (plan-rev3 §7.5's scenario (1)) finds them with one match: `startup mark
/// mark=<name> elapsed_ms=<ms since T0>`, the page-load marks with the
/// webview's label and the page's origin.
const STARTUP_TARGET: &str = "openalpaca_gui::startup";

/// The target of the line [`install_panic_hook`]'s hook writes.
const PANIC_TARGET: &str = "openalpaca_gui::panic";

/// Open `state/logs/gui.log` to append, after rotating it under the store's
/// caps (16 MB, three older generations, as `daemon.log`). A rotation that
/// failed comes back beside the file, for the caller to log once the log is
/// up: it is never a reason not to open it.
fn open_gui_log() -> anyhow::Result<(PathBuf, std::fs::File, Option<anyhow::Error>)> {
    store::logs_dir()?;
    let path = store::gui_log_path()?;
    let rotation = store::rotate_gui_log().err();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| anyhow::anyhow!("Failed to open {}: {e}", path.display()))?;
    Ok((path, file, rotation))
}

/// The subscriber that writes the shell's log: `tracing`'s plain format,
/// without colour escapes, appended to `file`, under `filter`.
fn gui_log_subscriber(
    file: std::fs::File,
    filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync + 'static {
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .finish()
}

/// `RUST_LOG` when it is set and valid, `info` otherwise — the daemon's rule.
fn gui_log_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Start the shell's log: [`open_gui_log`], then [`gui_log_subscriber`] as
/// the process's default, which (tracing-subscriber's `tracing-log`
/// feature) also takes Tauri's `log` records. Returns the log's path, for
/// the panic hook; `None` when it could not be opened. Never fatal: an app
/// that will not open because its log could not be is the worse bug, so a
/// failure goes to stderr and the app runs without a log.
fn init_gui_log() -> Option<PathBuf> {
    let (path, file, rotation) = match open_gui_log() {
        Ok(opened) => opened,
        Err(e) => {
            eprintln!("openalpaca_gui: the shell runs without its log: {e:#}");
            return None;
        }
    };
    if let Err(e) = gui_log_subscriber(file, gui_log_filter()).try_init() {
        eprintln!("openalpaca_gui: the shell log was not installed: {e}");
    }
    if let Some(e) = rotation {
        tracing::warn!("Could not rotate the shell log ({e:#}); appending to it as it is.");
    }
    Some(path)
}

/// Write each panic of the shell to `log_path` as one line, then run the
/// hook that was there before (Rust's own prints it to stderr).
///
/// The line is written straight to the file, not through `tracing`, so a
/// panic raised inside a log call cannot re-enter the subscriber that was
/// writing, and a panic is kept even when the subscriber is not installed.
fn install_panic_hook(log_path: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // One append of the whole line, so it lands whole beside the
        // subscriber's lines. Nothing to do if the file will not open.
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = file.write_all(panic_line(info).as_bytes());
        }
        previous(info);
    }));
}

/// A panic as one line of `gui.log`, in the subscriber's form: the time, the
/// level, the target, then where and what. A line break in the message (an
/// `assert_eq!`'s always has two), the thread's name or the place is written
/// `\n` or `\r`, so the whole panic stays on the one line a reader matching
/// [`PANIC_TARGET`] finds.
fn panic_line(info: &std::panic::PanicHookInfo<'_>) -> String {
    let mut time = String::new();
    let _ = SystemTime.format_time(&mut Writer::new(&mut time));
    let thread = std::thread::current();
    let thread = one_line(thread.name().unwrap_or("<unnamed>"));
    let location = info
        .location()
        .map_or_else(|| "an unknown place".to_owned(), ToString::to_string);
    let location = one_line(&location);
    let payload = one_line(info.payload_as_str().unwrap_or("Box<dyn Any>"));
    format!(
        "{time} ERROR {PANIC_TARGET}: the shell panicked on thread '{thread}' at {location}: \
         {payload}\n"
    )
}

/// `text` with each line feed written `\n` and each carriage return `\r`,
/// and nothing else changed.
fn one_line(text: &str) -> String {
    text.replace('\n', "\\n").replace('\r', "\\r")
}

/// Milliseconds since `t0`, as a mark logs them.
fn millis_since(t0: Instant) -> u64 {
    u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// T0, the shell's own start, as a startup mark: the first line `run` logs.
fn t0_mark(t0: Instant) {
    tracing::info!(
        target: STARTUP_TARGET,
        mark = "t0",
        elapsed_ms = millis_since(t0),
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        "startup mark"
    );
}

/// A page load of `label`'s webview as a startup mark (`Builder::on_page_load`):
/// its start and its end, each with the milliseconds since T0. A reload logs
/// them again, so the marks also show a window reloaded after its web
/// content process ended. The page's origin is logged, never its path or
/// query.
fn page_load_mark(t0: Instant, label: &str, event: PageLoadEvent, url: &tauri::Url) {
    let mark = match event {
        PageLoadEvent::Started => "page_load_started",
        PageLoadEvent::Finished => "page_load_finished",
    };
    tracing::info!(
        target: STARTUP_TARGET,
        mark,
        elapsed_ms = millis_since(t0),
        webview = label,
        origin = %page_origin(url),
        "startup mark"
    );
}

/// `scheme://host[:port]` of a page — `tauri://localhost` packaged, the dev
/// server's in a dev launch. Built by hand, since `Url::origin` is opaque
/// (`null`) for a custom scheme such as `tauri:`.
fn page_origin(url: &tauri::Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

/// The window's web content process ended (WebKit's `WebContent` crashed or
/// was killed), which leaves a blank window: log it and reload the webview.
/// Safe, because the state lives in the daemon: the reloaded page bootstraps
/// against the same daemon and `instance_id`. macOS and iOS only, where
/// Tauri reports it (`Builder::on_web_content_process_terminate`).
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
fn reload_after_web_content_ended<R: tauri::Runtime>(webview: &tauri::Webview<R>) {
    log_and_reload(webview.label(), || webview.reload());
}

/// The line for `label`'s webview whose web content process ended, then
/// `reload`, once; a reload that failed is a line too. The reload is a
/// parameter so a test sees it called, which the mock runtime cannot show.
fn log_and_reload(label: &str, reload: impl FnOnce() -> tauri::Result<()>) {
    tracing::warn!(
        webview = label,
        "The window's web content process ended; reloading the window."
    );
    if let Err(e) = reload() {
        tracing::error!(
            webview = label,
            "Could not reload the window after its web content process ended: {e}"
        );
    }
}

// ============================================================================
// Tauri App Entry Point
// ============================================================================

pub fn run() {
    // T0: the shell's own start, before anything else it does.
    let t0 = Instant::now();
    if let Some(log_path) = init_gui_log() {
        install_panic_hook(log_path);
    }
    t0_mark(t0);
    // Q21: the login shell's PATH for the daemon, captured once, on a thread
    // of its own — the window's first paint never waits on it.
    login_path::start();
    with_app_hooks(with_commands(tauri::Builder::default()), t0)
        .run(tauri::generate_context!())
        .expect("Error while running Tauri application");
}

/// The window's hooks (Q14): the page-load marks, and on macOS the reload
/// after the web content process ended.
fn with_app_hooks<R: tauri::Runtime>(builder: tauri::Builder<R>, t0: Instant) -> tauri::Builder<R> {
    let builder = builder.on_page_load(move |webview, payload| {
        page_load_mark(t0, webview.label(), payload.event(), payload.url());
    });
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let builder = builder.on_web_content_process_terminate(reload_after_web_content_ended);
    builder
}

/// The dialog plugin and the five commands the webview invokes: the one list,
/// which [`run`] builds the app from and the tests drive over a mock runtime.
fn with_commands<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_connection_info,
            ensure_daemon_running,
            await_daemon_stopped,
            read_daemon_log_tail,
            reveal_path
        ])
}

#[cfg(test)]
mod tests;
