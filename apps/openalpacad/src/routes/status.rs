//! `GET /v1/status` — where the daemon keeps things (plan §4.7 item 4).
//!
//! The authenticated sibling of `/v1/health`: it names absolute paths on the
//! owner's disk, so it sits behind the token and `/v1/health` stays the public,
//! four-field liveness probe.
//!
//! This is the **first slice** of GAP-14. It answers one question — *where do
//! my files live?* — with the three store roots (`home_root`, `state_dir`,
//! `db_path`) read from `openalpaca_storage::store`, the single source of truth
//! for every path in the system, plus the project a request would resolve to.
//! Phase 8 item 1 widens the same route with `started_at`, `uptime_secs`,
//! `schema_version`, `log_path`, the size accounting and the session numbers;
//! nothing here presupposes their shape.
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

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use openalpaca_storage::store;
use serde::Serialize;

// The header reader and the resolver are shared with `POST /v1/files/upload`
// (`routes/mod.rs`): one header name, one resolver, two consumers.
use super::{api_error, request_project_root, workspace_header};

/// The `GET /v1/status` body. Phase 8 adds fields; it never renames these.
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
}

/// `GET /v1/status`
pub async fn status_handler(headers: HeaderMap) -> Response {
    let roots = match store_roots() {
        Ok(roots) => roots,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORE_UNAVAILABLE",
                format!("Could not resolve the store roots: {e}"),
            );
        }
    };
    let (home_root, state_dir, db_path) = roots;

    let body = StatusResponse {
        home_root,
        state_dir,
        db_path,
        project_root: request_project_root(workspace_header(&headers).as_deref()),
    };
    (StatusCode::OK, Json(body)).into_response()
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

fn display(path: std::path::PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
