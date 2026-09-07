//! `/v1/workspaces` — §4.8's "Project moved", as a route.
//!
//! ```text
//! GET   /v1/workspaces?path=<abs>          -> what is recorded at that root
//! PATCH /v1/workspaces {old_path,new_path} -> re-base everything onto the new one
//! ```
//!
//! A project's path is its identity in four places — `file_assets.project_root`,
//! `session.workspace_id`, `task.workspace_id` and the memory scope key — so
//! moving the directory strands all four at once. Claude Code's encoded-cwd
//! directories are the cautionary example the plan cites: a rename there leaves
//! transcripts, memory and history under the old name for good. A path-derived
//! identity is cheap only if re-basing is **one transaction**, which is what
//! `ArtifactStore::rebase_project` is.
//!
//! **Both paths are resolved the way a turn's `x-workspace-path` is** (R22,
//! `request_project_root`): up to the nearest `.git`/`.openalpaca`, falling
//! back to the path itself when there is no marker to walk to — which is the
//! usual state of a root a project has already been moved *out of*. The
//! resolved values are what comes back, so a caller can see what was answered
//! about rather than assume.
//!
//! The `GET` exists because the picker cannot honestly offer a re-base without
//! it: it is the only way to learn that the store at a chosen path records a
//! *different* root (`.layout`'s `project_root=`, written once when the store
//! was seeded), and what a re-base would actually move.
//!
//! Nothing here re-bases on its own. The `PATCH` is the only writer, and it
//! refuses rather than guesses: `404` when no row names the old root, `409`
//! when rows already name the new one (two projects must not merge silently),
//! `409` while a run under the old root is in flight, and `409` when the store
//! directory itself cannot be moved.

use std::path::Path;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use openalpaca_storage::store::{self, migrate};
use openalpaca_storage::{ArtifactStore, Database, RebaseCounts, WorkspaceRows};
use serde::Deserialize;

use super::{api_error, request_project_root};
use crate::AppState;

// ── Request types ────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct WorkspaceQuery {
    /// The project root to describe. Required: this route answers about one
    /// root, and a missing path is a question, not a filter.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RebaseRequest {
    pub old_path: String,
    pub new_path: String,
}

// ── Path resolution ──────────────────────────────────────────────

/// The canonical root string the four members hold, for a path a client named.
///
/// A relative path is refused rather than resolved: it would be read against
/// the *daemon's* working directory, which is the bug ruling R22 fixed. An
/// absolute path is resolved through the same walk a turn's `x-workspace-path`
/// takes, and when that finds no marker — a project directory that has already
/// been moved away, leaving nothing behind — the canonical path itself stands,
/// because that is still exactly what the rows recorded.
#[allow(clippy::result_large_err)]
fn resolve_root(input: &str) -> Result<String, Response> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !Path::new(trimmed).is_absolute() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PATH",
            format!("a workspace path must be absolute, got '{input}'"),
        ));
    }
    if let Some(root) = request_project_root(Some(trimmed)) {
        return Ok(root);
    }
    let path = Path::new(trimmed);
    let canonical = path.canonicalize();
    let resolved = canonical.as_deref().unwrap_or(path);
    let text = resolved.to_string_lossy();
    // `/repo/` and `/repo` are one project; a bare root keeps its separator.
    let trimmed_text = text.trim_end_matches('/');
    Ok(match trimmed_text.is_empty() {
        true => text.to_string(),
        false => trimmed_text.to_string(),
    })
}

// ── Serialisation ────────────────────────────────────────────────

fn counts_json(counts: &RebaseCounts) -> serde_json::Value {
    serde_json::json!({
        "artifacts": counts.file_assets,
        "sessions": counts.sessions,
        "tasks": counts.tasks,
        "memories": counts.memories,
    })
}

// ── GET /v1/workspaces ───────────────────────────────────────────

/// What is recorded at one root, and whether its store thinks it lives
/// somewhere else.
///
/// `recorded_root` is the store's own `.layout` marker; `moved` is the whole
/// point — a store present at this path whose marker names a *different* one is
/// a project that was moved on disk, and the re-base is what re-attaches its
/// history. A store seeded before the marker existed reports `null` and
/// `moved: false`, which is "nothing to say", not "not moved".
pub(crate) fn get_workspace(db: &Database, query: WorkspaceQuery) -> Response {
    let Some(path) = query.path else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "MISSING_PATH",
            "?path= is required: this route describes one workspace root",
        );
    };
    let root = match resolve_root(&path) {
        Ok(root) => root,
        Err(response) => return response,
    };

    let store_root = Path::new(&root).join(store::STORE_DIR_NAME);
    let store_present = store_root.is_dir();
    let recorded_root = match store::recorded_project_root(&store_root) {
        Ok(recorded) => recorded,
        Err(e) => {
            // An unreadable marker is not a reason to refuse the whole answer:
            // the counts below are the half that decides a re-base.
            tracing::warn!(
                "Cannot read the store marker in {}: {e:#}",
                store_root.display()
            );
            None
        }
    };
    let moved = matches!(&recorded_root, Some(recorded) if recorded != &root);

    let rows = match ArtifactStore::new(db).workspace_rows(&root) {
        Ok(rows) => rows,
        Err(e) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string());
        }
    };

    Json(workspace_json(
        &root,
        store_present,
        recorded_root,
        moved,
        &rows,
    ))
    .into_response()
}

fn workspace_json(
    root: &str,
    store_present: bool,
    recorded_root: Option<String>,
    moved: bool,
    rows: &WorkspaceRows,
) -> serde_json::Value {
    serde_json::json!({
        "path": root,
        "store_present": store_present,
        "recorded_root": recorded_root,
        "moved": moved,
        "rows": counts_json(&rows.counts),
        "active_tasks": rows.active_tasks,
    })
}

// ── PATCH /v1/workspaces ─────────────────────────────────────────

/// Re-base one project root onto another: the four members in one transaction,
/// then the store directory itself when it is still at the old root.
///
/// The order is the plan's: rows first, bytes second. The move is *planned*
/// before the transaction opens, so the one failure that would leave rows
/// pointing at a directory nobody moved is refused up front instead.
pub(crate) fn rebase_workspace(db: &Database, request: RebaseRequest) -> Response {
    let old_root = match resolve_root(&request.old_path) {
        Ok(root) => root,
        Err(response) => return response,
    };
    let new_root = match resolve_root(&request.new_path) {
        Ok(root) => root,
        Err(response) => return response,
    };
    if old_root == new_root {
        return api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PATH",
            format!("{old_root} is already where it is"),
        );
    }

    let store = ArtifactStore::new(db);
    let old_rows = match store.workspace_rows(&old_root) {
        Ok(rows) => rows,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    };
    if old_rows.counts.is_empty() {
        return api_error(
            StatusCode::NOT_FOUND,
            "WORKSPACE_NOT_FOUND",
            format!("nothing is recorded under {old_root}"),
        );
    }
    if old_rows.active_tasks > 0 {
        return api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{} run(s) under {old_root} are still in flight; a running or paused run \
                 resolved its store when it started, and re-basing the row would not move it",
                old_rows.active_tasks
            ),
        );
    }

    let new_rows = match store.workspace_rows(&new_root) {
        Ok(rows) => rows,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    };
    if !new_rows.counts.is_empty() {
        return api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_EXISTS",
            format!(
                "{new_root} already has a store recorded against it; re-basing onto it would \
                 merge two projects' histories"
            ),
        );
    }

    // Decide the on-disk move before a row changes, so the one outcome that
    // cannot be undone by re-running this call is refused rather than half done.
    // Both an error (a cross-volume rename) and `Ambiguous` (a store at each
    // root) are refusals; only the other two outcomes may proceed.
    match migrate::plan_project_store_move(Path::new(&old_root), Path::new(&new_root)) {
        Ok(migrate::StoreMove::Rename | migrate::StoreMove::NothingToMove) => {}
        Ok(migrate::StoreMove::Ambiguous) => {
            return api_error(
                StatusCode::CONFLICT,
                "WORKSPACE_MOVE_BLOCKED",
                format!(
                    "two stores: {old_root}/{dir} and {new_root}/{dir} both exist. Refusing to \
                     choose between them — keep the one you want, move the other aside, and \
                     try again",
                    dir = store::STORE_DIR_NAME
                ),
            );
        }
        Err(e) => {
            return api_error(
                StatusCode::CONFLICT,
                "WORKSPACE_MOVE_BLOCKED",
                format!("{e:#}"),
            );
        }
    }

    let counts = match store.rebase_project(&old_root, &new_root) {
        Ok(counts) => counts,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    };

    let store_moved = match migrate::move_project_store(Path::new(&old_root), Path::new(&new_root))
    {
        Ok(moved) => moved,
        // The rows are already re-based. Saying so — with both paths — is the
        // only useful answer: the directory needs one `mv` and nothing else.
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_MOVE_FAILED",
                format!(
                    "the rows were re-based onto {new_root}, but the store directory could not \
                     be moved: {e:#}"
                ),
            );
        }
    };

    // The store now lives at the new root and must say so, or the next look
    // would report it as moved all over again.
    let new_store_root = Path::new(&new_root).join(store::STORE_DIR_NAME);
    if new_store_root.is_dir()
        && let Err(e) = store::set_recorded_project_root(&new_store_root, &new_root)
    {
        tracing::warn!(
            "Re-based onto {new_root} but could not update {}: {e:#}",
            new_store_root.display()
        );
    }

    Json(serde_json::json!({
        "old_path": old_root,
        "new_path": new_root,
        "moved": counts_json(&counts),
        "store_moved": store_moved,
    }))
    .into_response()
}

// ── Handlers ─────────────────────────────────────────────────────

pub async fn get_workspace_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WorkspaceQuery>,
) -> Response {
    get_workspace(&state.db, query)
}

pub async fn rebase_workspace_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RebaseRequest>,
) -> Response {
    rebase_workspace(&state.db, request)
}

#[cfg(test)]
mod tests;
