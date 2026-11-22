//! OpenAlpaca GUI - Tauri Backend
//!
//! Provides Tauri commands for:
//! - Connecting to the daemon via discovery.json
//! - Ensuring a daemon this app can use is running: reusing the one
//!   discovery.json names when its `GET /v1/health` answers as that daemon
//!   in a protocol this shell speaks, waiting for one that is still starting,
//!   refusing one it cannot use, and spawning the sidecar beside the app only
//!   when there is none (Q04, [`reuse`])
//! - Waiting until a daemon the webview asked to stop is really gone
//! - Reading the end of the daemon log, so a daemon that would not start can
//!   say why
//!
//! Showing an artifact in its folder is the daemon's `POST
//! /v1/files/{id}/reveal` since Q06; the shell's `reveal_path` went.
//!
//! Every command is `async`, so none runs on the main thread, which draws the
//! window (Tauri runs a command without `async` there); every file read, the
//! spawn and every other blocking call runs on the blocking pool
//! (`tauri::async_runtime::spawn_blocking`), so it never holds a thread of
//! the async runtime either; and the liveness probe is tokio's connect under
//! tokio's timeout (Q12) — since Q04 a hand-written `GET /v1/health`.
//!
//! A command refuses with a [`ShellError`] (Q13): `{ kind, message }` to the
//! webview, `kind` what refused and `message` the sentence it shows.
//!
//! The shell logs to `state/logs/gui.log` (Q14): its `tracing` lines and
//! Tauri's `log` records, its startup marks, a panic, and a web content
//! process that ended — after which the window is reloaded, or, past the
//! reload bound, a dialog offers Reload or Quit.
//!
//! The daemon it spawns runs on the `PATH` of the account's login shell,
//! captured once at the app's start, or on a fixed list when that capture
//! fails (Q21, [`login_path`]); `ensure_daemon_running` and
//! `get_connection_info` answer which, for a daemon this app spawned. Beside
//! `PATH` it gets only the variables the owner lists in `daemon.toml`'s
//! `[daemon.env] passthrough_keys` that the login shell exports, named in the
//! same announcement, never by value (N-DAEMON-2).

mod login_path;
mod reuse;

use login_path::{DaemonPath, DaemonPathReport};
use openalpaca_app_support::daemon_lifecycle::{self, StopOutcome};
use openalpaca_app_support::discovery::{self, ConnectionInfo};
use openalpaca_app_support::{paths, protocol};
use reuse::MismatchWords;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
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
    /// or within 5 seconds of the spawn `discovery.json` named no live
    /// daemon, or the daemon it named died while the spawn's wait went on
    /// (Q04-VERDICT: an ask after the `starting` answer that finds it gone).
    /// `log_tail` is what this run wrote to `daemon.log` (also quoted in
    /// `message`), `None` when it wrote nothing or the log could not be
    /// read.
    #[error("{message}")]
    SpawnFailed {
        message: String,
        log_tail: Option<String>,
    },
    /// `ensure_daemon_running` (Q04): the daemon running speaks a protocol
    /// this shell does not — newer than `shell_protocol`, older than the
    /// oldest it supports, or in a discovery file it cannot read. Also
    /// `get_connection_info`, judged from the file's `build.protocol` alone
    /// when its pid is a live `openalpacad`, so the reconnect ladder parks on
    /// it too. Nothing was stopped or started; `remedy` is the way out, also
    /// the end of `message`.
    #[error("{message}")]
    Mismatch {
        message: String,
        daemon_version: Option<String>,
        daemon_protocol: Option<u32>,
        shell_protocol: u32,
        remedy: String,
    },
    /// `ensure_daemon_running` (Q04, C-09 with A-18): the discovery pid is a
    /// live `openalpacad` that does not answer yet — booting, or in its
    /// shutdown's tail. Nothing was spawned beside it: it is one found
    /// running, or the one this app spawned (or the CLI's that took the
    /// lock from it), answered so at once after the spawn (Q04-VERDICT);
    /// the window waits and asks again.
    #[error("{message}")]
    Starting { message: String },
    /// `ensure_daemon_running` (Q04): the daemon has not answered
    /// `protocol::STARTING_BUDGET` after the shell first saw it
    /// silent. Nothing was spawned over it, stopped or rotated; `log_tail` is
    /// the log's tail (also quoted in `message`), `None` when it holds
    /// nothing or could not be read (the reason then in `message`).
    #[error("{message}")]
    StillStarting {
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
/// daemon's [`ConnectionInfo`] (`{ baseUrl, token, instanceId, version,
/// protocol }`), `daemonPath`, where the `PATH` of a daemon this app spawned
/// came from (Q21) — `null` for a daemon this run of the app did not spawn:
/// one started from a terminal, or by an earlier run of the app — and `note`
/// (Q04), the sentence the window shows beside a daemon of an older
/// protocol or another version (`reuse::compat_note`), `null` for one of
/// this shell's.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShellConnection {
    #[serde(flatten)]
    info: ConnectionInfo,
    daemon_path: Option<DaemonPathReport>,
    note: Option<String>,
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
/// spawned (its `pid`), and the note its file's build earns.
fn shell_connection(d: &discovery::Discovery) -> ShellConnection {
    let note = reuse::compat_note(&reuse::THIS_SHELL, d.build.protocol, &d.build.version);
    connection_with(ConnectionInfo::from(d), d.pid, note)
}

/// The daemon `ensure_daemon_running` reuses, with the protocol and version
/// its health answered and the note its `protocol::decide_reuse` verdict
/// earns.
fn reused_connection(reused: protocol::Reused) -> ShellConnection {
    let note = reuse::reuse_note(&reuse::THIS_SHELL, &reused);
    let mut info = ConnectionInfo::from(&reused.discovery);
    info.protocol = reused.protocol;
    info.version = reused.version;
    connection_with(info, reused.discovery.pid, note)
}

/// `info` with the `daemonPath` of the daemon `pid` when this run spawned
/// it, and `note`.
fn connection_with(info: ConnectionInfo, pid: u32, note: Option<String>) -> ShellConnection {
    let daemon_path = SPAWNED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .find(|(spawned, _)| *spawned == pid)
        .map(|(_, report)| report.clone());
    ShellConnection {
        info,
        daemon_path,
        note,
    }
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Get connection info from discovery.json.
/// Returns error if daemon is not running or discovery is invalid.
///
/// The file is read on the blocking pool. Its mismatch and its answer are
/// the asking window's verdict, by the webview's label, as
/// `ensure_daemon_running`'s is (LADDER-LINE, 119-1).
#[tauri::command]
async fn get_connection_info<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
) -> Result<ShellConnection, ShellError> {
    connection_info_for_window(webview.label().to_owned()).await
}

/// [`get_connection_info`]'s work for the window labelled `window`, on the
/// blocking pool.
async fn connection_info_for_window(window: String) -> Result<ShellConnection, ShellError> {
    tauri::async_runtime::spawn_blocking(move || read_connection_info(&window))
        .await
        .map_err(|e| ShellError::DiscoveryUnreadable {
            message: format!("Could not read the daemon's discovery file: {e}"),
        })?
}

/// [`get_connection_info`]'s work: read discovery.json, refuse a daemon whose
/// file states a protocol this shell does not work with while its pid is a
/// live `openalpacad` (Q04: the reconnect ladder's rung asks no health, and
/// would otherwise connect to a daemon `ensure_daemon_running` refuses), and
/// check its token. A file over a pid that is gone is answered as before:
/// the socket fails, and the ladder goes on. The refusal is `window`'s
/// verdict, recorded where `ensure_daemon_running`'s are and, when it is a
/// change, one `INFO` line of `gui.log` ([`reuse::ladder_mismatch`]); its
/// answer is too, a reuse naming the ladder unless the window's last
/// verdict is already a reuse of that pid ([`reuse::ladder_answer`],
/// 119-1 (b)); another refusal records nothing.
fn read_connection_info(window: &str) -> Result<ShellConnection, ShellError> {
    let Some(d) = discovery::read_discovery().map_err(|e| ShellError::DiscoveryUnreadable {
        message: e.to_string(),
    })?
    else {
        return Err(ShellError::NotRunning {
            message: "Discovery not found: daemon may not be running".into(),
        });
    };

    if let Some(mismatch) = protocol::file_mismatch(&d, &reuse::THIS_SHELL)
        && daemon_lifecycle::pid_is_daemon(d.pid)
    {
        reuse::ladder_mismatch(&reuse::STARTING, window, &mismatch, |line| {
            tracing::info!("{line}");
        });
        return Err(mismatch.into_error());
    }

    discovery::ensure_not_expired(&d).map_err(|e| ShellError::Expired {
        message: e.to_string(),
    })?;

    reuse::ladder_answer(&reuse::STARTING, window, &d, |line| {
        tracing::info!("{line}");
    });
    Ok(shell_connection(&d))
}

/// Ensure a daemon this app can use is running, and answer its connection
/// (Q04, [`reuse::ensure_with`] over the machine's world): the daemon
/// `discovery.json` names is reused when its `GET /v1/health` answers as
/// that daemon in a protocol this shell speaks (with a note for an older
/// one, or another version); refused as a mismatch when it speaks another,
/// stopping and starting nothing; answered `starting` while its pid is a
/// live `openalpacad` that does not answer yet, at once and at every ask
/// until `protocol::STARTING_BUDGET` from the first silent look,
/// then reported still starting — never spawned over, its log never
/// rotated, its lock never probed. Only with no live daemon named is the
/// sidecar spawned, once, and the daemon the file then names judged the
/// same way: the CLI's, when it won the store's lock first; one that is
/// up but silent answers `starting` at once there too (Q04-VERDICT). Each
/// change of the asking window's verdict is one line of `gui.log`, keyed
/// by the webview's label.
///
/// Every read of discovery.json, the process-table read, the spawn and the
/// read of the log run on the blocking pool, and the probe is tokio's, so
/// the command holds no runtime thread while it waits on a disk, a connect
/// or the new process.
#[tauri::command]
async fn ensure_daemon_running<R: tauri::Runtime>(
    webview: tauri::Webview<R>,
) -> Result<ShellConnection, ShellError> {
    ensure_for_window(webview.label().to_owned()).await
}

/// [`ensure_daemon_running`]'s work for the window labelled `window`.
async fn ensure_for_window(window: String) -> Result<ShellConnection, ShellError> {
    reuse::ensure_with(
        &mut reuse::RealWorld,
        &reuse::STARTING,
        &reuse::THIS_SHELL,
        &window,
    )
    .await
    .map(reused_connection)
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
/// Reads at most `paths::LOG_TAIL_READ_BYTES` from the end of the file; the
/// line handling is `paths::read_log_tail`'s, which is where it is tested —
/// the workspace's gate set (`default-members`) leaves this crate out, and
/// it is checked and tested only by hand (`-p openalpaca_gui`).
///
/// The read runs on the blocking pool.
#[tauri::command]
async fn read_daemon_log_tail(lines: usize) -> Result<String, ShellError> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = paths::daemon_log_path().map_err(|e| format!("{e:#}"))?;
        paths::read_log_tail(&path, 0, lines)
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
/// completion: the daemon still has up to 10 s of shutdown tail to run, then
/// at most 1 s for the blocking work still running, and a replacement
/// started inside that loses the non-blocking lock race and exits.
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

/// The sidecar's file name.
const SIDECAR_NAME: &str = if cfg!(windows) {
    "openalpacad.exe"
} else {
    "openalpacad"
};

/// The sidecar beside the app whose executable is `exe` (D7 (a), Q04): `exe`
/// canonicalized (`std::fs::canonicalize`, so a symlinked app or a
/// symlinked folder above it leads to the real folder), and
/// [`SIDECAR_NAME`] in that folder. Tauri's `current_exe()`, which refuses
/// any symlinked ancestor on macOS, is not used. The one refusal, in every
/// profile, is an absent sidecar, its sentence naming the path looked at;
/// nothing on `PATH` is tried (a debug build's fallback to it is gone), so
/// the daemon started is always the one staged beside the app.
fn sidecar_beside(exe: &Path) -> anyhow::Result<PathBuf> {
    let exe = std::fs::canonicalize(exe)
        .map_err(|e| anyhow::anyhow!("Could not resolve the app's path {}: {e}", exe.display()))?;
    let daemon = exe.parent().unwrap_or(Path::new(".")).join(SIDECAR_NAME);
    if daemon.exists() {
        Ok(daemon)
    } else {
        Err(SidecarMissing { looked_at: daemon }.into())
    }
}

/// [`sidecar_beside`]'s refusal, typed so the start can tell it from every
/// other failure to spawn (`reuse::spawn_refusal`): `gui.log` logs it as a
/// verdict of its own (Q04-VERDICT). Its sentence is the one it always was.
#[derive(Debug, thiserror::Error)]
#[error(
    "Daemon binary not found at {}. The application may be incorrectly installed.",
    looked_at.display()
)]
struct SidecarMissing {
    looked_at: PathBuf,
}

/// Spawn the daemon as a detached background process, its stdout and stderr
/// appended to `daemon.log`, on the `PATH` [`login_path::for_the_daemon`]
/// answers — the login shell's, or the fallback list — waiting for the
/// app's capture at most until its deadline. The sidecar is located first
/// ([`sidecar_beside`]), so a refusal touches neither the store nor the log.
fn spawn_daemon() -> anyhow::Result<Spawned> {
    let path_to_use = sidecar_beside(&std::env::current_exe()?)?;

    // D1: one root. `ensure_home_store` seeds README/.layout; the config dir
    // must exist before the spawn — `resolve_config_base_dir` ignores an
    // `OPENALPACA_CONFIG_DIR` that does not.
    let app_dir = paths::ensure_home_store()?;
    let config_dir = paths::ensure_runtime_config_dir()?;

    // The sidecar's output used to go to /dev/null on both platforms, so a
    // daemon that refused to boot — a database this build does not know, a
    // store it cannot create, a master key it cannot read, another daemon
    // already holding the lock — told the user nothing
    // beyond "did not become ready within timeout". It now writes the same
    // `daemon.log` the CLI launcher writes, under the same rotation, and marks
    // itself as that file's owner so `GET /v1/status` hands the path back
    // (T30: `MANAGED_LOG_ENV` means "the launcher pointed stdio at the file
    // and rotated it", which is now true of both launchers).
    paths::logs_dir()?;
    let log_path = paths::daemon_log_path()?;
    if let Err(e) = paths::rotate_daemon_log() {
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

/// The spawn's line in `gui.log`: the daemon's `PATH` source, the
/// capture's duration and the keys passed beside it by name (N-DAEMON-2),
/// never the `PATH` or a value.
fn announce_daemon_path(path: &DaemonPath) {
    tracing::info!(
        source = path.source.as_str(),
        capture_ms = path.capture_ms(),
        passed_keys = path.passed_keys().join(",").as_str(),
        "Daemon PATH source"
    );
}

/// The daemon's command but its stdio: its working directory the store's
/// root, `OPENALPACA_CONFIG_DIR`, [`paths::MANAGED_LOG_ENV`], and — with a
/// [`DaemonPath`] — `PATH` set to it, each listed key the login shell
/// exported set to its value (N-DAEMON-2), and [`paths::PATH_SOURCE_ENV`] to
/// the source, the capture time and the keys' names (`login-shell 212
/// AWS_PROFILE`), never the `PATH` or a value. The listed keys are set
/// first, so a variable the app sets itself always keeps the app's value
/// (the list never holds one, nor any other `OPENALPACA_` name:
/// `login_path::passthrough_keys`). Nothing else is set or removed: every
/// other variable is the app's own, inherited as before — launchd's minimal
/// environment for an app opened from the Finder or the Dock, so there a
/// login shell's variable reaches the daemon only through the list; the
/// terminal's whole environment for an app started from one (*Rev 91* (4),
/// *Rev 92* (2): no change).
fn daemon_command(
    program: &Path,
    app_dir: &Path,
    config_dir: &Path,
    path: Option<&DaemonPath>,
) -> Command {
    let mut cmd = Command::new(program);
    if let Some(path) = path {
        cmd.envs(path.passed().iter().map(|(key, value)| (key, value)));
    }
    cmd.current_dir(app_dir)
        .env("OPENALPACA_CONFIG_DIR", config_dir)
        .env(paths::MANAGED_LOG_ENV, "1");
    if let Some(path) = path {
        cmd.env("PATH", path.value())
            .env(paths::PATH_SOURCE_ENV, path.announcement());
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
    paths::logs_dir()?;
    let path = paths::gui_log_path()?;
    let rotation = paths::rotate_gui_log().err();
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

/// At most this many reloads of one window within [`RELOAD_WINDOW`] after
/// its web content process ended (RELOAD-BOUND, `tasks/pending-decisions.md`
/// §1's *Rev 90* (b)): a page that crashes WebKit as it loads would
/// otherwise reload, crash and log in a loop.
const RELOAD_LIMIT: usize = 3;

/// The span [`RELOAD_LIMIT`] counts over: a reload this old or older no
/// longer counts.
const RELOAD_WINDOW: Duration = Duration::from_mins(1);

/// The dialog's first button (RELOAD-DIALOG), its default button: reload
/// the window once.
const RELOAD_BUTTON: &str = "Reload";

/// The dialog's second button: quit the app.
const QUIT_BUTTON: &str = "Quit";

/// The dialog's title and text, shown once a window's web content process
/// has ended past [`RELOAD_LIMIT`] reloads within [`RELOAD_WINDOW`].
const RELOAD_DIALOG_TITLE: &str = "The window keeps failing to load";
const RELOAD_DIALOG_MESSAGE: &str = "OpenAlpaca's window stopped working again after three \
     reloads within a minute, so it was not reloaded by itself. Reload it to try once more, \
     or quit the app and open it again; state/logs/gui.log has the details.";

/// The person's answer to the dialog a window past the reload bound shows
/// (RELOAD-DIALOG, `tasks/pending-decisions.md` §1's *Rev 100* (1) (c)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadAnswer {
    /// [`RELOAD_BUTTON`]: the window is reloaded once, and the reload counts
    /// against the bound like any other, from the click.
    Reload,
    /// [`QUIT_BUTTON`] — or, through the dialog plugin, any end of the
    /// dialog that is not a click: the app quits.
    Quit,
    /// A result that names neither button: the window is left as it is,
    /// and its next end asks again. The dialog plugin does not deliver one
    /// for these buttons: tauri-plugin-dialog 2.8.3 rewrites rfd's `Cancel`
    /// — an end that is not a click on a button, or an alert that failed
    /// to show — into the second button's label, so such an end reads as
    /// [`ReloadAnswer::Quit`] (`tasks/pending-decisions.md` §1's *Rev 109*).
    Neither,
}

impl ReloadAnswer {
    /// The answer the dialog plugin reports, by the button's label; a result
    /// with neither label answers neither. For these buttons the plugin
    /// reports only labels: an end that is not a click arrives as
    /// `Custom(QUIT_BUTTON)`, so it answers [`ReloadAnswer::Quit`].
    fn from_dialog(result: &tauri_plugin_dialog::MessageDialogResult) -> Self {
        use tauri_plugin_dialog::MessageDialogResult;
        match result {
            MessageDialogResult::Custom(label) if label == RELOAD_BUTTON => Self::Reload,
            MessageDialogResult::Custom(label) if label == QUIT_BUTTON => Self::Quit,
            _ => Self::Neither,
        }
    }
}

/// The dialog's buttons: [`RELOAD_BUTTON`] first, then [`QUIT_BUTTON`].
fn reload_dialog_buttons() -> tauri_plugin_dialog::MessageDialogButtons {
    tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
        RELOAD_BUTTON.to_owned(),
        QUIT_BUTTON.to_owned(),
    )
}

/// What the shell does for one end of a window's web content process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndAction {
    /// Within the bound: reload the window by itself.
    Reload,
    /// Past the bound: ask, in a dialog, whether to reload it or quit.
    Ask,
    /// The window's dialog is open: nothing more, no second dialog.
    StillAsking,
}

/// The reloads each window was given after its web content process ended,
/// by the webview's label, so one window's crashes never spend another's,
/// and the windows whose dialog is open. Only a reload performed counts —
/// a failed one too, and one the person chose in the dialog, from the click;
/// an end refused never — so the span slides over the reloads done, and a
/// window gets a reload by itself again once its oldest counted one is
/// [`RELOAD_WINDOW`] old. The clock is [`Instant`], monotonic: a clock set
/// back moves nothing. On macOS it is `CLOCK_UPTIME_RAW`, which stops while
/// the Mac sleeps, so the span is 60 s of awake time (`tasks/
/// pending-decisions.md` §1's *Rev 100* (2), taken as (a): awake time errs
/// only toward fewer reloads, and the dialog is the way out).
#[derive(Default)]
struct ReloadBound {
    reloads: HashMap<String, Vec<Instant>>,
    asking: HashSet<String>,
}

impl ReloadBound {
    /// Whether `label`'s window may be reloaded at `now`; when it may, the
    /// reload counts from `now`.
    fn admit(&mut self, label: &str, now: Instant) -> bool {
        let recent = self.reloads.entry(label.to_owned()).or_default();
        recent.retain(|at| now.saturating_duration_since(*at) < RELOAD_WINDOW);
        if recent.len() >= RELOAD_LIMIT {
            return false;
        }
        recent.push(now);
        true
    }

    /// What `label`'s window gets for its process ending at `now`: while its
    /// dialog is open, nothing more (the end is not counted); else a reload
    /// when [`ReloadBound::admit`] admits one; else the dialog, now open.
    fn on_end(&mut self, label: &str, now: Instant) -> EndAction {
        if self.asking.contains(label) {
            return EndAction::StillAsking;
        }
        if self.admit(label, now) {
            return EndAction::Reload;
        }
        self.asking.insert(label.to_owned());
        EndAction::Ask
    }

    /// `label`'s dialog answered at `at`: it is closed, and a reload chosen
    /// counts from `at`, past the limit too, so the bound is never reset.
    fn answered(&mut self, label: &str, answer: ReloadAnswer, at: Instant) {
        self.asking.remove(label);
        if answer == ReloadAnswer::Reload {
            self.reloads.entry(label.to_owned()).or_default().push(at);
        }
    }
}

/// A window whose web content process ended, as the reload path acts on it:
/// the webview and its app in the app ([`DialogWebview`]), a stand-in in the
/// tests, which sees each reload, quit and dialog, and answers the dialog
/// when it chooses.
trait EndedWindow: Clone + Send + 'static {
    /// The webview's label, which the bound counts by.
    fn label(&self) -> &str;
    /// Reloads the window's page.
    fn reload(&self) -> tauri::Result<()>;
    /// Quits the app.
    fn quit(&self);
    /// Shows the dialog without waiting for it; `on_answer` is called once,
    /// with the person's answer and the instant it came.
    fn ask(&self, on_answer: Box<dyn FnOnce(ReloadAnswer, Instant) + Send>);
}

/// The window in the app: its webview, the app it quits, and the dialog
/// plugin's message dialog, shown without blocking (the hook runs on the
/// main thread, where the plugin's `blocking_show` must not), whose answer
/// arrives on a thread of the plugin's. The dialog has no parent window, so
/// on macOS rfd shows it as a system `CFUserNotification` alert, not an
/// app-modal `NSAlert` nor a sheet on the window; an end of it that is not
/// a click reads as `Quit` ([`ReloadAnswer::from_dialog`]). Reached by no
/// test: its reload is `Webview::reload`, its quit `AppHandle::exit(0)`,
/// and its dialog the alert, the click on which is step 19's check at the
/// app.
struct DialogWebview<R: tauri::Runtime>(tauri::Webview<R>);

impl<R: tauri::Runtime> Clone for DialogWebview<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<R: tauri::Runtime> EndedWindow for DialogWebview<R> {
    fn label(&self) -> &str {
        self.0.label()
    }

    fn reload(&self) -> tauri::Result<()> {
        self.0.reload()
    }

    fn quit(&self) {
        use tauri::Manager as _;
        self.0.app_handle().exit(0);
    }

    fn ask(&self, on_answer: Box<dyn FnOnce(ReloadAnswer, Instant) + Send>) {
        use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
        self.0
            .dialog()
            .message(RELOAD_DIALOG_MESSAGE)
            .title(RELOAD_DIALOG_TITLE)
            .kind(MessageDialogKind::Error)
            .buttons(reload_dialog_buttons())
            .show_with_result(move |result| {
                on_answer(ReloadAnswer::from_dialog(&result), Instant::now());
            });
    }
}

/// The hook [`with_app_hooks`] registers for a web content process that
/// ended: [`reload_hook_over`] the window in the app, [`DialogWebview`].
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
fn bounded_reload_hook<R: tauri::Runtime>() -> impl Fn(&tauri::Webview<R>) + Send + Sync + 'static {
    reload_hook_over(|webview: &tauri::Webview<R>| DialogWebview(webview.clone()))
}

/// The hook over the window `window` makes of each webview:
/// [`web_content_ended`] on the real clock, over one [`ReloadBound`] the hook
/// owns for the app's life, so every end it is called for counts against
/// the same bound and one dialog per window is open at a time.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
fn reload_hook_over<R, W, F>(window: F) -> impl Fn(&tauri::Webview<R>) + Send + Sync + 'static
where
    R: tauri::Runtime,
    W: EndedWindow,
    F: Fn(&tauri::Webview<R>) -> W + Send + Sync + 'static,
{
    let bound = Arc::new(Mutex::new(ReloadBound::default()));
    move |webview| web_content_ended(&window(webview), &bound, Instant::now())
}

/// `bound`, its lock recovered when poisoned.
fn lock_bound(bound: &Mutex<ReloadBound>) -> std::sync::MutexGuard<'_, ReloadBound> {
    bound.lock().unwrap_or_else(|p| {
        tracing::warn!("The window reload bound's lock was poisoned; recovering.");
        p.into_inner()
    })
}

/// The window's web content process ended (WebKit's `WebContent` crashed or
/// was killed) at `now`, which leaves a blank window: one line, then what
/// [`ReloadBound::on_end`] decides. Within the bound the window is reloaded
/// by itself — safe, because the state lives in the daemon: the reloaded
/// page bootstraps against the same daemon and `instance_id`. Past it,
/// RELOAD-BOUND's `ERROR` line, and a dialog asks whether to reload or quit
/// ([`dialog_answered`]), never on a timer — though an end of the dialog
/// that is not a click reads, through the dialog plugin, as `Quit`; while
/// it is open, a further end is a line and nothing more. The
/// bound's lock is never held across a reload, the dialog or its answer.
/// macOS and iOS only, where Tauri reports it
/// (`Builder::on_web_content_process_terminate`). The window and the clock
/// are parameters, so a test sees each effect, at the instants it chooses.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
fn web_content_ended<W: EndedWindow>(window: &W, bound: &Arc<Mutex<ReloadBound>>, now: Instant) {
    let label = window.label();
    let action = lock_bound(bound).on_end(label, now);
    match action {
        EndAction::Reload => {
            tracing::warn!(
                webview = label,
                "The window's web content process ended; reloading the window."
            );
            reload_logged(window);
        }
        EndAction::Ask => {
            tracing::error!(
                webview = label,
                "The window's web content process ended again after {RELOAD_LIMIT} reloads \
                 within {} s; the window is not reloaded by itself: a dialog asks whether \
                 to reload it or quit the app.",
                RELOAD_WINDOW.as_secs()
            );
            let (answering, bound) = (window.clone(), Arc::clone(bound));
            window.ask(Box::new(move |answer, at| {
                dialog_answered(&answering, &bound, answer, at);
            }));
        }
        EndAction::StillAsking => {
            tracing::warn!(
                webview = label,
                "The window's web content process ended again while its dialog is open; \
                 the window is not reloaded by itself, and no second dialog is shown."
            );
        }
    }
}

/// `window`'s dialog answered with `answer` at `at`: the bound told, then
/// a line, then what the answer says — `Reload` reloads the window once,
/// `Quit` (its button, or an end of the dialog that was not a click) quits
/// the app, neither leaves the window as it is.
fn dialog_answered<W: EndedWindow>(
    window: &W,
    bound: &Mutex<ReloadBound>,
    answer: ReloadAnswer,
    at: Instant,
) {
    let label = window.label();
    lock_bound(bound).answered(label, answer, at);
    match answer {
        ReloadAnswer::Reload => {
            tracing::info!(
                webview = label,
                "Reload was chosen in the dialog; reloading the window."
            );
            reload_logged(window);
        }
        ReloadAnswer::Quit => {
            tracing::info!(
                webview = label,
                "The dialog answered Quit (its button, or an end of the dialog that was \
                 not a click); quitting the app."
            );
            window.quit();
        }
        ReloadAnswer::Neither => {
            tracing::warn!(
                webview = label,
                "The dialog answered neither Reload nor Quit; the window is left as it \
                 is, and its next end asks again."
            );
        }
    }
}

/// Reloads `window`; a reload that failed is a line.
fn reload_logged<W: EndedWindow>(window: &W) {
    if let Err(e) = window.reload() {
        tracing::error!(
            webview = window.label(),
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
/// after the web content process ended, bounded by the [`ReloadBound`] the
/// hook owns, one per app, and past it the dialog ([`bounded_reload_hook`]).
fn with_app_hooks<R: tauri::Runtime>(builder: tauri::Builder<R>, t0: Instant) -> tauri::Builder<R> {
    let builder = builder.on_page_load(move |webview, payload| {
        page_load_mark(t0, webview.label(), payload.event(), payload.url());
    });
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let builder = builder.on_web_content_process_terminate(bounded_reload_hook());
    builder
}

/// The dialog plugin and the four commands the webview invokes: the one list,
/// which [`run`] builds the app from and the tests drive over a mock runtime.
fn with_commands<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_connection_info,
            ensure_daemon_running,
            await_daemon_stopped,
            read_daemon_log_tail
        ])
}

#[cfg(test)]
mod tests;
