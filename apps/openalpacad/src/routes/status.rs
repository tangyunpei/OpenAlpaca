//! `GET /v1/status` — where the daemon keeps things, and how it is doing
//! (plan §4.7 item 4, §4.8; GAP-14).
//!
//! The authenticated sibling of `/v1/health`: it names absolute paths on the
//! owner's disk, so it sits behind the token and `/v1/health` stays the public,
//! four-field liveness probe.
//!
//! Three questions, one answer. **Where do my files live?** — the store roots
//! (`home_root`, `state_dir`, `db_path`, `log_path`) read from
//! `openalpaca_storage::store`, the single source of truth for every path in
//! the system, plus the project a request would resolve to. **How long has
//! this daemon been up, and against which schema?** — `started_at` (stamped at
//! the top of the daemon's `async_main`, so it is the run's own clock, not the
//! process table's) with `uptime_secs`, and `schema_version` read from the
//! database rather than counted from `MIGRATIONS`: the DB is what the daemon
//! actually opened, and a binary that failed to migrate must not report the
//! version it wished for. **How much disk is this costing, and against what
//! limits?** — §4.8's two numbers, never one: `upload_bytes` (quota-bearing)
//! and `produced_bytes` (informational), from one grouped scan; what the boot
//! session-log sweep did and how many log records the writers dropped; and
//! `retention` (§1, P-27) — the three `orchestrator.sessions` limits those
//! numbers are measured against, read from `AppState.daemon_config` rather
//! than copied, so a hand-edited `daemon.toml` is what the client sees.
//!
//! **`project_root` is a question about the caller's own turn.** The daemon
//! holds no per-lane record of a workspace — the request root is threaded
//! through a turn, never stored globally (ruling R22) — so the route answers
//! the same way `/v1/chat` does: it reads `x-workspace-path` and reports what
//! the marker walk makes of it. That is the useful answer, because the client
//! sends a raw directory and the daemon decides which root owns it: the reply
//! tells the GUI which project its next chat will write artifacts into. No
//! header (every connector lane, any client that chose no project) → `null`,
//! and so does a path that resolves to no project at all or to the home store,
//! which is not a project (`MemoryScopeContext::for_request`).
//!
//! **`log_path` is the CLI-managed log, or nothing** (N2, resolved: serve it).
//! `openalpaca daemon start` points the child's stdout and stderr at
//! `state/logs/daemon.log`, rotates it at 16 MB, and marks the child with
//! `store::MANAGED_LOG_ENV` — this run's own claim to the file, not just
//! *a* file that happens to be there. A daemon started any other way
//! (`cargo run`, the GUI sidecar), or one that inherited someone else's
//! leftover `daemon.log` without the marker, reports `null`: the honest
//! answer for a path this run never opened, never a path to something it did
//! not write (Important #3, T44 fix round 1). Phase B (a real in-daemon
//! appender, and un-discarding the sidecar's stdout) is a separate task; this
//! route reports what exists today.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use openalpaca_core::daemon_config::{RoutingConfig, SessionsConfig};
use openalpaca_core::session_log::{SessionLogService, sweep::SweepReport};
use openalpaca_storage::{Database, FileAssetRepository, store};
use serde::Serialize;

use crate::state::AppState;

// The header reader and the resolver are shared with `POST /v1/files/upload`
// (`routes/mod.rs`): one header name, one resolver, two consumers.
use super::{api_error, request_project_root, workspace_header};

/// The `GET /v1/status` body. Later phases add fields; they never rename these.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// `~/.openalpaca` (or `$OPENALPACA_HOME_STORE`) — the one root for app
    /// state and no-project content.
    pub home_root: String,
    /// `<home_root>/state` — machine state, never user-edited.
    pub state_dir: String,
    /// `<state_dir>/openalpaca.db` — the database this daemon opened.
    pub db_path: String,
    /// The project root the request's `x-workspace-path` resolves to, or
    /// `null` when the caller sent none (or sent one that is not a project).
    pub project_root: Option<String>,
    /// When this run began, RFC 3339.
    pub started_at: String,
    /// Seconds since `started_at`, never negative.
    pub uptime_secs: u64,
    /// The migration version the open database is actually at.
    pub schema_version: i32,
    /// `<state_dir>/logs/daemon.log`, when the CLI wrote one; `null` otherwise.
    pub log_path: Option<String>,
    /// Bytes the user uploaded — the total the 500 MB cap is read against.
    pub upload_bytes: i64,
    /// Bytes agents produced. Informational: never charged against the cap.
    pub produced_bytes: i64,
    /// The limits `upload_bytes`/`produced_bytes`/the boot sweep are measured
    /// against — the brief's `retention` block (§1, P-27).
    pub retention: RetentionStatus,
    /// What the session event log has to say for itself.
    pub sessions: SessionsStatus,
    /// The routing switches a client has to know about to decide what to
    /// offer. Today that is one: the experimental replay resume of §5.6c.
    pub routing: RoutingStatus,
}

/// The `[orchestrator.routing]` flags a client renders against.
///
/// `resume_enabled` is served because the alternative is a GUI that offers a
/// `Resume` control the daemon will answer `409 RESUME_DISABLED` to, or hides
/// one that works. The flag belongs to the daemon, so the daemon is what says.
#[derive(Debug, Serialize)]
pub struct RoutingStatus {
    /// Replay resume, §5.6c (S2). Off unless a `daemon.toml` turns it on.
    pub resume_enabled: bool,
}

impl From<&RoutingConfig> for RoutingStatus {
    fn from(config: &RoutingConfig) -> Self {
        Self {
            resume_enabled: config.resume_enabled,
        }
    }
}

/// The three `orchestrator.sessions` limits this daemon is actually
/// enforcing, read from `AppState.daemon_config` rather than hardcoded — a
/// hand-edited `daemon.toml` changes these with no code change, and the
/// Storage card's "raise the cap" advice needs the number it is asking the
/// owner to raise (Important #1, T44 fix round 1).
#[derive(Debug, Serialize)]
pub struct RetentionStatus {
    /// Per session, counting `log.jsonl` segments plus `results/`.
    pub log_max_session_bytes: u64,
    /// Across all sessions — the denominator `sessions.last_sweep`'s
    /// `over_cap_after` is measured against.
    pub log_max_total_bytes: u64,
    /// The configured age limit for archived session logs. No age sweep
    /// exists yet (owner decision T12, ruling R85): the value is parsed,
    /// clamped and served, and nothing consumes it; `0` is the default.
    pub log_retention_days: u32,
}

impl From<&SessionsConfig> for RetentionStatus {
    fn from(config: &SessionsConfig) -> Self {
        Self {
            log_max_session_bytes: config.log_max_session_bytes,
            log_max_total_bytes: config.log_max_total_bytes,
            log_retention_days: config.log_retention_days,
        }
    }
}

/// The session-log numbers, from the service the runner already holds.
#[derive(Debug, Serialize)]
pub struct SessionsStatus {
    /// The boot sweep's account, or `null` when no pass ran — the active set
    /// was unreadable, or no sessions root resolved. The pass runs once, at
    /// boot, so its clock is `started_at`.
    pub last_sweep: Option<SweepStatus>,
    /// Records the writers dropped this boot: a full channel, or a directory
    /// that would not open (§5.5 chose drops over stalls). Non-zero means a
    /// transcript has a hole in it.
    pub dropped_records: u64,
}

/// One boot sweep, as `sweep::SweepReport` recorded it.
#[derive(Debug, Serialize)]
pub struct SweepStatus {
    pub sessions_visited: usize,
    pub sessions_evicted: usize,
    pub files_removed: usize,
    pub bytes_freed: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Still over `log_max_total_bytes` with only protected bytes left — the
    /// one field worth a warning in the UI.
    pub over_cap_after: bool,
    pub index_rows_cleared: usize,
}

impl From<&SweepReport> for SweepStatus {
    fn from(report: &SweepReport) -> Self {
        Self {
            sessions_visited: report.sessions_visited,
            sessions_evicted: report.sessions_evicted,
            files_removed: report.files_removed,
            bytes_freed: report.bytes_freed,
            bytes_before: report.bytes_before,
            bytes_after: report.bytes_after,
            over_cap_after: report.over_cap_after,
            index_rows_cleared: report.index_rows_cleared,
        }
    }
}

/// What the body needs from the running daemon.
///
/// Taken as a struct rather than read off `AppState` inside the builder so the
/// route's own logic is exercisable without standing up a daemon: the tests
/// hand it a real `Database` and a real `SessionLogService`, which is the
/// point — every number here has to come from the thing that owns it.
pub(crate) struct StatusInputs<'a> {
    pub started_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
    pub db: &'a Database,
    pub session_log: Option<&'a SessionLogService>,
    /// Whether `openalpaca daemon start` marked *this* run as the owner of
    /// `store::daemon_log_path()` (Important #3). Gates `log_path` alongside
    /// the file existing — the file alone is not proof this run wrote it.
    pub managed_log: bool,
    /// `orchestrator.sessions`, as this daemon is actually enforcing it.
    pub sessions_config: SessionsConfig,
    /// `orchestrator.routing`, likewise.
    pub routing_config: RoutingConfig,
}

/// `GET /v1/status`
pub async fn status_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    status_response(
        &StatusInputs {
            started_at: state.started_at,
            now: Utc::now(),
            db: &state.db,
            session_log: state.gateway.shared_context.session_log().map(Arc::as_ref),
            managed_log: state.managed_log,
            sessions_config: state.daemon_config.load().orchestrator.sessions.clone(),
            routing_config: state.daemon_config.load().orchestrator.routing.clone(),
        },
        &headers,
    )
}

fn status_response(inputs: &StatusInputs<'_>, headers: &HeaderMap) -> Response {
    let (home_root, state_dir, db_path) = match store_roots() {
        Ok(roots) => roots,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORE_UNAVAILABLE",
                format!("Could not resolve the store roots: {e}"),
            );
        }
    };

    // The database answers for itself. A daemon that cannot read its own
    // schema version or size accounting is not in a state to report "ok" with
    // the numbers left out.
    let schema_version = match inputs.db.schema_version() {
        Ok(version) => version,
        Err(e) => return database_unavailable(e),
    };
    let bytes = match FileAssetRepository::new(inputs.db).storage_bytes_by_origin() {
        Ok(bytes) => bytes,
        Err(e) => return database_unavailable(e),
    };

    let body = StatusResponse {
        home_root,
        state_dir,
        db_path,
        project_root: request_project_root(workspace_header(headers).as_deref()),
        started_at: inputs.started_at.to_rfc3339(),
        uptime_secs: (inputs.now - inputs.started_at).num_seconds().max(0) as u64,
        schema_version,
        log_path: daemon_log_path(inputs.managed_log),
        upload_bytes: bytes.upload_bytes,
        produced_bytes: bytes.produced_bytes,
        retention: RetentionStatus::from(&inputs.sessions_config),
        sessions: SessionsStatus {
            last_sweep: inputs
                .session_log
                .and_then(|service| service.last_sweep())
                .map(SweepStatus::from),
            dropped_records: inputs
                .session_log
                .map(SessionLogService::dropped_total)
                .unwrap_or(0),
        },
        routing: RoutingStatus::from(&inputs.routing_config),
    };
    (StatusCode::OK, Json(body)).into_response()
}

fn database_unavailable(e: anyhow::Error) -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "DATABASE_UNAVAILABLE",
        format!("Could not read the database: {e}"),
    )
}

/// The three roots, as the store reports them.
///
/// `state_dir()` creates the directory when it is missing; the daemon made it
/// at boot, so in practice this is a read. Deriving the path any other way
/// would mean joining a literal directory name onto a root, which is exactly
/// what the store module exists to prevent.
fn store_roots() -> anyhow::Result<(String, String, String)> {
    Ok((
        display(store::home_root()?),
        display(store::state_dir()?),
        display(store::database_path()?),
    ))
}

/// The CLI-managed daemon log, reported only when it is really there *and*
/// this run is the one that opened it.
///
/// `store::daemon_log_path()` creates nothing, so asking the question does not
/// answer it: a daemon the CLI never started has no `state/logs/daemon.log`
/// and says so with `null` rather than handing the GUI a path whose Copy
/// button would put a non-existent file on the clipboard. The file existing
/// is not enough on its own, though — a GUI- or `cargo run`-launched daemon
/// can find a previous CLI daemon's log sitting at the exact same path, and
/// reporting that would hand the owner a file whose newest line predates the
/// running daemon (Important #3, T44 fix round 1). `managed` is that second
/// check: only a run `openalpaca daemon start` itself opened the file for.
fn daemon_log_path(managed: bool) -> Option<String> {
    if !managed {
        return None;
    }
    let path = store::daemon_log_path().ok()?;
    path.is_file().then(|| display(path))
}

fn display(path: std::path::PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
