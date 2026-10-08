//! `/v1/workspaces` — a project's life (design §6, §9).
//!
//! ```text
//! GET   /v1/workspaces                      -> every project: [{id, root, name, missing}]
//! GET   /v1/workspaces?path=<abs>           -> what the path resolves to, and the rows there
//! POST  /v1/workspaces {path}               -> choose a folder: register its project (R10)
//! PATCH /v1/workspaces {old_path,new_path}  -> re-point a project onto its new path
//! POST  /v1/workspaces/purge {path,dry_run} -> delete everything OpenAlpaca holds for it (ST-19)
//! ```
//!
//! A project is a folder under `~/.openalpaca/projects/` whose `project.json`
//! records its id and root (`store::projects`); rows name it by that root in
//! five columns. These routes resolve a client's path the one way a turn's is
//! resolved (`projects::resolve`), answer the resolved root, and refuse rather
//! than guess: `404` when no project is registered there or a row there is
//! another owner's (R40), `409` while a run is in flight, the project's
//! memory slot stays held or its re-point has not finished, `409`/`422` for a
//! target that is the home store, already a project, or not a directory.
//!
//! The delete answers a plan, one line per entry in the seeded README's
//! retention terms, `delete` or `keep`; `dry_run` defaults to **true**, so a
//! caller that forgets the field gets the plan and not the deletion. Under the
//! project's memory slot it deletes the rows — keeping, in the root store,
//! every file another conversation still links — the conversations' logs, the
//! memory partitions of every owner and `projects/<name>/`. It never touches
//! the project's own folder and never iterates `storage_path`. It takes the
//! project exactly, by its root as registered: a path inside a project is
//! `422 WORKSPACE_NOT_A_ROOT`, naming the root, rather than a deletion of
//! the whole project for a path that named one folder of it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use openalpaca_storage::project_life::{self, DeleteOutcome, DeletePlan, KeptFiles};
use openalpaca_storage::store::projects::{
    self, Project, ProjectError, ProjectId, ResolvedProject,
};
use openalpaca_storage::{ArtifactStore, Database, PurgeCounts, RebaseCounts};
use serde::{Deserialize, Serialize};

use openalpaca_core::context::SharedContext;
use openalpaca_core::memory::service::{MemoryError, MemoryService, QuiesceGuard};

use super::api_error;
use crate::AppState;

// ── Request types ────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct WorkspaceQuery {
    /// `?path=` names one path to describe; absent, the route lists every
    /// project (§9). A path is never read against the daemon's directory.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RebaseRequest {
    pub old_path: String,
    pub new_path: String,
}

/// `POST /v1/workspaces`: the folder a person chose (the app's `Use`,
/// `openalpaca chat --project`).
#[derive(Debug, Deserialize)]
pub struct ChooseWorkspaceRequest {
    /// Absolute. Inside a git repository it chooses the repository; any other
    /// folder becomes a project of its own (R10).
    pub path: String,
}

/// `POST /v1/workspaces/purge`: one project, by its root as registered — a
/// path inside it is refused, naming the root.
#[derive(Debug, Deserialize)]
pub struct PurgeRequest {
    pub path: String,
    /// Absent means a dry run: the destructive reading of a missing field is
    /// the one this route will not take.
    #[serde(default = "yes")]
    pub dry_run: bool,
}

fn yes() -> bool {
    true
}

// ── Answers ──────────────────────────────────────────────────────

/// One project as its folder under `projects/` records it.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub root: String,
    pub name: String,
    /// Its root is not a directory (ST-7): its data is kept.
    pub missing: bool,
}

impl From<&Project> for ProjectView {
    fn from(project: &Project) -> Self {
        Self {
            id: project.id.to_string(),
            root: project.root_string(),
            name: project.name.clone(),
            missing: project.missing,
        }
    }
}

/// The five path columns' rows under one root (`ArtifactStore::workspace_rows`).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct RowCountsView {
    /// `file_assets`, every origin.
    pub artifacts: usize,
    pub sessions: usize,
    pub tasks: usize,
    /// `conversation_messages.workspace_root`.
    pub messages: usize,
    /// `lane_followups.workspace_path`.
    pub followups: usize,
}

impl From<&RebaseCounts> for RowCountsView {
    fn from(counts: &RebaseCounts) -> Self {
        Self {
            artifacts: counts.file_assets,
            sessions: counts.sessions,
            tasks: counts.tasks,
            messages: counts.messages,
            followups: counts.followups,
        }
    }
}

/// `GET /v1/workspaces?path=`: what the path resolves to, and what is recorded there.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceView {
    /// The path asked about, canonicalized (as written when it does not
    /// exist) — or, when it is a project's recorded root as written, that
    /// root.
    pub path: String,
    /// The project root it resolves to; `null` = no project.
    pub root: Option<String>,
    /// `null` = a root no folder under `projects/` records yet (R12), or no project.
    pub id: Option<String>,
    pub name: Option<String>,
    pub missing: bool,
    /// Under `root`; zeros when there is none.
    pub rows: RowCountsView,
    /// `running` or `paused`.
    pub active_tasks: usize,
    /// `queued`. The re-point and the delete refuse on these and on
    /// `active_tasks` alike.
    pub queued_tasks: usize,
}

/// `PATCH /v1/workspaces`: what moved, and the project at its new root.
#[derive(Debug, Clone, Serialize)]
pub struct RepointView {
    pub old_path: String,
    pub new_path: String,
    pub moved: RowCountsView,
    pub project: ProjectView,
}

/// One line of the delete's plan: an entry, what it holds for this project,
/// the retention class the seeded README gives it, and the verdict.
#[derive(Debug, Clone, Serialize)]
pub struct DeletePlanEntry {
    pub entry: String,
    pub holds: String,
    pub retention: String,
    /// `"delete"` or `"keep"`.
    pub action: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DeleteCountsView {
    pub sessions: usize,
    pub messages: usize,
    pub tool_calls: usize,
    pub followups: usize,
    pub tasks: usize,
    pub spans: usize,
    pub run_events: usize,
    pub uploads: usize,
    /// By-products the delete removes: one `file_assets` row per artifact,
    /// however many versions it has.
    pub artifacts: usize,
    /// Note files of every owner under `projects/<name>/memory/`. On a real
    /// delete, those that went: they go with the folder, so a delete that
    /// kept it (`removed.project_folder: false`) counts only what a fresh
    /// count no longer finds there.
    pub memory_notes: usize,
}

/// Files another conversation still links, moved to the root store.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct KeptView {
    pub uploads: usize,
    pub artifacts: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RemovedView {
    pub session_dirs: usize,
    pub memory_partitions: usize,
    pub project_folder: bool,
}

/// The delete's answer: the plan, or what was deleted — the same shape, with
/// `applied` saying which call this was.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteDataView {
    pub dry_run: bool,
    pub applied: bool,
    pub project: ProjectView,
    pub entries: Vec<DeletePlanEntry>,
    pub counts: DeleteCountsView,
    pub kept: KeptView,
    /// `null` on a dry run.
    pub removed: Option<RemovedView>,
}

// ── Path resolution ──────────────────────────────────────────────

/// A client's path as written: trimmed, absolute, without a trailing `/` —
/// nothing resolved. What the Projects list and `store list` print is a
/// recorded root in exactly this form, so a caller naming one is matched
/// against the recorded roots before anything canonicalizes it
/// ([`projects::by_recorded_root`]).
///
/// A relative path is refused rather than resolved: it would be read against
/// the *daemon's* working directory, which is the bug ruling R22 fixed.
#[allow(clippy::result_large_err)]
fn written_path(input: &str) -> Result<String, Response> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !Path::new(trimmed).is_absolute() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PATH",
            format!("a workspace path must be absolute, got '{input}'"),
        ));
    }
    Ok(without_trailing_slash(trimmed))
}

/// `/repo/` and `/repo` are one project; a bare root keeps its separator.
fn without_trailing_slash(text: &str) -> String {
    let without_slash = text.trim_end_matches('/');
    match without_slash.is_empty() {
        true => text.to_string(),
        false => without_slash.to_string(),
    }
}

/// A client's path, canonicalized and nothing more — not resolved to a
/// project's root here. As [`written_path`] refuses it.
#[allow(clippy::result_large_err)]
fn canonical_path(input: &str) -> Result<String, Response> {
    Ok(canonical_of(&written_path(input)?))
}

/// [`canonical_path`] of a path [`written_path`] already accepted: as written
/// when it does not exist.
fn canonical_of(written: &str) -> String {
    let path = Path::new(written);
    let canonical = path.canonicalize();
    let resolved = canonical.as_deref().unwrap_or(path);
    without_trailing_slash(&resolved.to_string_lossy())
}

/// The `404` for a root holding somebody else's rows — the same words a root
/// holding *nothing* of yours gets.
///
/// R40 answers a foreign row with `404` rather than `403` precisely so a caller
/// learns nothing about it; a message that said "3 row(s) under /repo belong to
/// another owner" handed back both the existence and the size of that history,
/// which a `404` exists not to confirm. The count is still worth having — it is
/// the one thing that distinguishes this refusal from an empty root when an
/// operator reads the log — so it goes there, at `debug`.
fn owner_mismatch(root: &str, other_owners: usize) -> Response {
    tracing::debug!(
        root,
        other_owners,
        "refusing a workspace write: rows under this root belong to another owner"
    );
    api_error(
        StatusCode::NOT_FOUND,
        "WORKSPACE_NOT_FOUND",
        format!("nothing of yours is recorded under {root}"),
    )
}

/// The `500` for a listing of `projects/` that failed — the list's own
/// answer, and the only one a recorded root gets while its project cannot be
/// matched as written (residual X1).
fn projects_unreadable(e: &anyhow::Error) -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "IO_ERROR",
        format!("the projects folder could not be read: {e:#}"),
    )
}

/// The registered project a client path names, or the answer that refuses
/// it. `written` is the path as the client sent it ([`written_path`]) and
/// `path` its canonical form ([`canonical_of`]). A recorded root sent as
/// written is that project ([`projects::by_recorded_root`]), even when it no
/// longer canonicalizes to itself — a folder renamed by case only, or moved
/// with a link left at the old path, whose canonical form may be another
/// project's root. Anything else is resolved the way a turn's path is:
/// `404 WORKSPACE_NOT_FOUND` for a root no folder under `projects/` records
/// (R12's words) or a path in no project. A listing of `projects/` that
/// fails is `500 IO_ERROR` naming the projects folder
/// ([`projects_unreadable`]) — never a fall-through to the resolution,
/// which for a recorded root that now leads elsewhere would name that other
/// project.
///
/// First, a project whose re-point has not finished — listed at its new
/// root, its folder under the old name, found by `lookup` under neither path
/// until the next start finishes it — answers `409 WORKSPACE_BUSY` at either
/// path, naming that restart, as `register` does there: re-pointed or
/// deleted now, it would be split from the rows already moved.
#[allow(clippy::result_large_err)]
fn registered_project(written: &str, path: &str) -> Result<Project, Response> {
    let named = if written == path {
        vec![written]
    } else {
        vec![written, path]
    };
    for named in named {
        let moving = match projects::unfinished_repoint_at(Path::new(named)) {
            Ok(Some(moving)) => moving,
            Ok(None) => continue,
            Err(e) => return Err(projects_unreadable(&e)),
        };
        let from = moving
            .previous_root
            .as_deref()
            .map_or_else(String::new, |p| p.display().to_string());
        return Err(api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{named} names the project in {}, whose re-point from {from} to {} has not \
                 finished; the next start of the daemon finishes it, and until then the \
                 project can be neither re-pointed nor have its data deleted",
                moving.dir.display(),
                moving.root.display()
            ),
        ));
    }
    match projects::by_recorded_root(Path::new(written)) {
        Ok(Some(project)) => return Ok(project),
        Ok(None) => {}
        Err(e) => return Err(projects_unreadable(&e)),
    }
    match projects::resolve(Path::new(path)) {
        Ok(Some(ResolvedProject {
            project: Some(project),
            ..
        })) => Ok(project),
        Ok(Some(resolved)) => Err(api_error(
            StatusCode::NOT_FOUND,
            "WORKSPACE_NOT_FOUND",
            ProjectError::NotRegistered {
                root: resolved.root_string(),
            }
            .to_string(),
        )),
        Ok(None) => Err(api_error(
            StatusCode::NOT_FOUND,
            "WORKSPACE_NOT_FOUND",
            format!(
                "{path} is in no project: no folder under projects/ records it and no git \
                 repository holds it"
            ),
        )),
        Err(e) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "IO_ERROR",
            format!("{path} could not be resolved: {e:#}"),
        )),
    }
}

/// How often, and for how long, the re-point and the delete try for the
/// project's memory slot. `quiesce` refuses rather than waits, and the 30 s
/// reconciliation and an index rebuild hold the same slot for moments, so
/// three seconds outlasts them in practice and is short enough for a dialog.
pub(crate) const QUIESCE_RETRY_EVERY: Duration = Duration::from_millis(100);
pub(crate) const QUIESCE_RETRY_FOR: Duration = Duration::from_secs(3);

/// `MemoryService::quiesce`, tried every [`QUIESCE_RETRY_EVERY`] until
/// [`QUIESCE_RETRY_FOR`] has passed; the last refusal is returned.
async fn quiesce_project(
    memory: &MemoryService,
    id: &ProjectId,
) -> Result<QuiesceGuard, MemoryError> {
    let deadline = tokio::time::Instant::now() + QUIESCE_RETRY_FOR;
    loop {
        match memory.quiesce(id) {
            Ok(guard) => return Ok(guard),
            Err(e) if tokio::time::Instant::now() >= deadline => return Err(e),
            Err(_) => tokio::time::sleep(QUIESCE_RETRY_EVERY).await,
        }
    }
}

/// `project` as its folder records it now, read again once the project's
/// memory slot is held: the re-point and the delete each resolve their
/// project before they wait for that slot, and another re-point or delete of
/// the same project may hold it meanwhile. Read by id — never by root, which
/// for a recorded root that now leads elsewhere would name another project —
/// and taken only when its root and its folder are still the ones it was
/// read with; once the slot is held no other re-point or delete of this id
/// can start, so it stays so until the slot is let go. Anything else — moved,
/// deleted, re-registered under a new id, or marked mid re-point — is
/// `409 WORKSPACE_BUSY`, saying `nothing was <done>`.
#[allow(clippy::result_large_err)]
fn still_the_project(project: &Project, done: &str) -> Result<Project, Response> {
    match projects::by_id(&project.id) {
        Ok(Some(now))
            if now.root == project.root
                && now.dir == project.dir
                && now.previous_root.is_none() =>
        {
            Ok(now)
        }
        Ok(_) => Err(api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{} was re-pointed or its data deleted while this waited; nothing was {done}. \
                 List the projects again",
                project.root.display()
            ),
        )),
        Err(e) => Err(projects_unreadable(&e)),
    }
}

// ── GET /v1/workspaces ───────────────────────────────────────────

/// With no `path`: every project OpenAlpaca knows, as its folder under
/// `projects/` records it, sorted by folder name — a bare array (§9).
/// Creates nothing.
pub(crate) fn list_workspaces() -> Response {
    match projects::list() {
        Ok(list) => Json(list.iter().map(ProjectView::from).collect::<Vec<_>>()).into_response(),
        Err(e) => projects_unreadable(&e),
    }
}

/// What a path resolves to — the way a turn's `x-workspace-path` is
/// resolved, registering nothing (§3) — and the rows recorded under that
/// root, so a caller sees what a re-point would move and what the delete
/// would refuse on.
pub(crate) fn get_workspace(db: &Database, query: WorkspaceQuery) -> Response {
    let Some(raw) = query.path else {
        return list_workspaces();
    };
    let written = match written_path(&raw) {
        Ok(written) => written,
        Err(response) => return response,
    };
    // A recorded root, as the list printed it, names its own project — the
    // re-point and delete dialogs read their counts here (see
    // `registered_project`). A listing that fails says so rather than
    // resolving: the canonical form may be another project's root.
    let (path, resolved) = match projects::by_recorded_root(Path::new(&written)) {
        Ok(Some(project)) => (
            written,
            Some(ResolvedProject {
                root: project.root.clone(),
                project: Some(project),
            }),
        ),
        Err(e) => return projects_unreadable(&e),
        Ok(None) => {
            let path = canonical_of(&written);
            match projects::resolve(Path::new(&path)) {
                Ok(resolved) => (path, resolved),
                Err(e) => {
                    return api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "IO_ERROR",
                        format!("{path} could not be resolved: {e:#}"),
                    );
                }
            }
        }
    };
    let Some(resolved) = resolved else {
        return Json(WorkspaceView {
            path,
            root: None,
            id: None,
            name: None,
            missing: false,
            rows: RowCountsView::default(),
            active_tasks: 0,
            queued_tasks: 0,
        })
        .into_response();
    };
    let root = resolved.root_string();
    let rows = match ArtifactStore::new(db).workspace_rows(&root, None) {
        Ok(rows) => rows,
        Err(e) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string());
        }
    };
    let project = resolved.project.as_ref();
    Json(WorkspaceView {
        path,
        root: Some(root),
        id: project.map(|p| p.id.to_string()),
        name: project.map(|p| p.name.clone()),
        missing: project.is_some_and(|p| p.missing),
        rows: RowCountsView::from(&rows.counts),
        active_tasks: rows.active_tasks,
        queued_tasks: rows.queued_tasks,
    })
    .into_response()
}

// ── POST /v1/workspaces ──────────────────────────────────────────

/// Choose a folder: register the project it resolves to — the git repository
/// it lies in, else the folder itself (R10, ST-6) — and answer it. A choice
/// already registered answers the same project. The fifth registration site
/// of §2.2: the daemon registers, a read never does.
pub(crate) fn choose_workspace(request: ChooseWorkspaceRequest) -> Response {
    let path = match canonical_path(&request.path) {
        Ok(path) => path,
        Err(response) => return response,
    };
    match projects::choose(Path::new(&path)) {
        Ok(project) => Json(ProjectView::from(&project)).into_response(),
        Err(e) => project_error_response(&e).unwrap_or_else(|| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_REGISTER_FAILED",
                format!("could not record the project for {path}: {e:#}"),
            )
        }),
    }
}

// ── PATCH /v1/workspaces ─────────────────────────────────────────

/// A [`ProjectError`] anywhere in `e`'s chain, as the route answers it, or
/// `None` for any other failure (interfaces §6.1): not registered `404`, the
/// home store `409`, not a directory `422`, already a project — or a path
/// of another project's unfinished re-point — `409`, the same path `400` —
/// each with the error's own sentence.
pub(crate) fn project_error_response(e: &anyhow::Error) -> Option<Response> {
    let error = e
        .chain()
        .find_map(|cause| cause.downcast_ref::<ProjectError>())?;
    let status = match error {
        ProjectError::NotRegistered { .. } => StatusCode::NOT_FOUND,
        ProjectError::IsHome { .. }
        | ProjectError::Exists { .. }
        | ProjectError::RepointUnfinished { .. } => StatusCode::CONFLICT,
        ProjectError::NotADirectory { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        ProjectError::SamePath { .. } => StatusCode::BAD_REQUEST,
    };
    Some(api_error(status, error.code(), error.to_string()))
}

/// Re-point a project onto its new path (§6): the rows of the five path
/// columns and both `storage_path` prefixes in one transaction, and the
/// project's folder under `projects/` renamed — `project_life::repoint`.
///
/// Owner-scoped (R40): a project holding another owner's rows is the same
/// `404` an unknown one is. A project re-pointed or deleted while this waited
/// for its memory slot is a `409` ([`still_the_project`]). Refused while a
/// run is `queued`, `running` or
/// `paused` there — counted here and again inside `repoint`'s transaction
/// (`project_life::RunsInFlight`, the same `409`), since a run dispatched
/// while this waits has already resolved the project by its old root — and
/// while the project's memory slot stays held for
/// [`QUIESCE_RETRY_FOR`]. The target's refusals are `repoint`'s: not a
/// directory (422), the home store (409), already a project or named by the
/// rows (409, ST-20), the same path (400).
pub(crate) async fn rebase_workspace(
    db: &Database,
    memory: &MemoryService,
    owner_id: &str,
    request: RebaseRequest,
) -> Response {
    let old_written = match written_path(&request.old_path) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let new = match canonical_path(&request.new_path) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let project = match registered_project(&old_written, &canonical_of(&old_written)) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let root = project.root_string();
    let rows = match ArtifactStore::new(db).workspace_rows(&root, Some(owner_id)) {
        Ok(rows) => rows,
        Err(e) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string());
        }
    };
    if rows.other_owners > 0 {
        return owner_mismatch(&root, rows.other_owners);
    }
    // `queued` too: its dispatch has already resolved the project by this
    // root (`WorkspaceRows::queued_tasks`).
    let in_flight = rows.active_tasks + rows.queued_tasks;
    if in_flight > 0 {
        return api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{in_flight} run(s) under {root} are still in flight; re-point it once they \
                 finish"
            ),
        );
    }
    let _quiesced = match quiesce_project(memory, &project.id).await {
        Ok(guard) => guard,
        Err(e) => {
            return api_error(
                StatusCode::CONFLICT,
                "WORKSPACE_BUSY",
                format!(
                    "a memory write under {root} is still in flight ({e}); re-pointing now \
                     would rename its folder out from under it. Try again in a moment"
                ),
            );
        }
    };
    let project = match still_the_project(&project, "re-pointed") {
        Ok(project) => project,
        Err(response) => return response,
    };
    match project_life::repoint(db, &project.root, Path::new(&new)) {
        Ok(done) => Json(RepointView {
            old_path: done.before.root_string(),
            new_path: done.after.root_string(),
            moved: RowCountsView::from(&done.rows),
            project: ProjectView::from(&done.after),
        })
        .into_response(),
        Err(e) => repoint_failure(&e, &root, &new),
    }
}

/// How the PATCH answers `project_life::repoint`'s error `e` for the
/// re-point of `root` onto `new`.
fn repoint_failure(e: &anyhow::Error, root: &str, new: &str) -> Response {
    // A run dispatched while this waited for the memory slot: step 2's
    // transaction counted it, moved nothing and cleared the marker. A marker
    // that could not be cleared leaves the re-point for the next start to
    // finish, which is no "nothing was re-pointed": that is the 500 below,
    // with the whole chain (residual X3).
    let marker_kept = e
        .downcast_ref::<project_life::RepointLeftMarked>()
        .is_some();
    if let Some(busy) = e
        .chain()
        .find_map(|cause| cause.downcast_ref::<project_life::RunsInFlight>())
        .filter(|_| !marker_kept)
    {
        return api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{} run(s) of {} are queued, running or paused; nothing was re-pointed — \
                 re-point it once they finish",
                busy.runs, busy.root
            ),
        );
    }
    project_error_response(e).unwrap_or_else(|| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "WORKSPACE_MOVE_FAILED",
            format!("{root} could not be re-pointed onto {new}: {e:#}"),
        )
    })
}

// ── POST /v1/workspaces/purge ────────────────────────────────────

/// What the delete needs from `AppState`, named so its refusals and its order
/// can be tested without a gateway.
pub(crate) struct PurgeDeps<'a> {
    pub db: &'a Database,
    /// The live lane registry — half of the in-flight guard.
    pub ctx: &'a SharedContext,
    /// Where `session_changed{status:"deleted"}` goes, one per conversation.
    pub bus: &'a openalpaca_core::bus::EventBus,
    /// The local user; the delete is owner-scoped (R40).
    pub owner: &'a str,
    pub memory: &'a Arc<MemoryService>,
    /// The home store's `sessions/`. `None` when it could not be resolved at
    /// boot: the rows still go, the directories are left and logged.
    pub sessions_root: Option<std::path::PathBuf>,
    /// The live writers: each is stood down before its directory goes, or its
    /// next record re-creates it.
    pub session_log: Option<Arc<openalpaca_core::session_log::SessionLogService>>,
    /// The learning queue: a deleted conversation's pending turns are
    /// forgotten, as `DELETE /v1/sessions/{id}` does.
    pub learning: Option<Arc<openalpaca_core::memory::learning::LearningQueue>>,
}

/// The plan, in the README's retention terms; every entry named, zeroes
/// included. A dry run's figures are the plan's; a real delete's are what it
/// did — the same figures as its `counts`, `kept` and `removed`.
fn plan_entries(
    project: &Project,
    c: &PurgeCounts,
    kept: KeptFiles,
    memory_notes: usize,
    memory_partitions: usize,
) -> Vec<DeletePlanEntry> {
    let folder = format!("{}/{}/", projects::PROJECTS_DIR, project.name);
    let line =
        |entry: String, holds: String, retention: &str, action: &'static str| DeletePlanEntry {
            entry,
            holds,
            retention: retention.to_string(),
            action,
        };
    vec![
        line(
            "sessions/".to_string(),
            format!(
                "{} conversations, {} messages, {} tool calls, {} follow-ups — their logs live \
                 in the home store's sessions/",
                c.sessions, c.messages, c.tool_calls, c.followups
            ),
            "size-capped, optional age sweep",
            "delete",
        ),
        line(
            "runs (database)".to_string(),
            format!(
                "{} runs, {} subagent spans, {} run events",
                c.tasks, c.spans, c.run_events
            ),
            "never swept — removed only when you ask",
            "delete",
        ),
        line(
            format!("{folder}uploads/"),
            format!(
                "{} uploads handed to this project's conversations",
                c.uploads
            ),
            "swept: an upload attached to no message is deleted once past the grace period",
            "delete",
        ),
        line(
            format!("{folder}artifacts/"),
            format!(
                "{} by-products of this project's runs, with every version",
                c.artifacts
            ),
            "never garbage-collected — removed only when you ask",
            "delete",
        ),
        line(
            format!("{folder}memory/"),
            format!(
                "{memory_notes} memory notes of every owner, in {memory_partitions} partitions, \
                 with their history"
            ),
            "yours — never swept",
            "delete",
        ),
        line(
            folder.clone(),
            "project.json — the project's id and path; OpenAlpaca forgets the project".to_string(),
            "store metadata",
            "delete",
        ),
        line(
            "uploads/ (root store)".to_string(),
            format!(
                "{} uploads another conversation still shows, moved here under {}/",
                kept.uploads,
                project_life::KEPT_DIR
            ),
            "swept: an upload attached to no message is deleted once past the grace period",
            "keep",
        ),
        line(
            "artifacts/ (root store)".to_string(),
            format!(
                "{} by-products another conversation still shows, moved here under {}/ as \
                 their newest version",
                kept.artifacts,
                project_life::KEPT_DIR
            ),
            "never garbage-collected",
            "keep",
        ),
        line(
            project.root_string(),
            "the project's own folder — OpenAlpaca never touches it".to_string(),
            "yours",
            "keep",
        ),
    ]
}

fn delete_view(
    plan: &DeletePlan,
    counts: &PurgeCounts,
    kept: KeptFiles,
    memory_notes: usize,
    removed: Option<RemovedView>,
) -> DeleteDataView {
    let dry_run = removed.is_none();
    let memory_partitions = removed.map_or(plan.memory_partitions, |r| r.memory_partitions);
    DeleteDataView {
        dry_run,
        applied: !dry_run,
        project: ProjectView::from(&plan.project),
        entries: plan_entries(&plan.project, counts, kept, memory_notes, memory_partitions),
        counts: DeleteCountsView {
            sessions: counts.sessions,
            messages: counts.messages,
            tool_calls: counts.tool_calls,
            followups: counts.followups,
            tasks: counts.tasks,
            spans: counts.spans,
            run_events: counts.run_events,
            uploads: counts.uploads,
            artifacts: counts.artifacts,
            memory_notes,
        },
        kept: KeptView {
            uploads: kept.uploads,
            artifacts: kept.artifacts,
        },
        removed,
    }
}

/// The deleted conversations' log directories, each writer stood down first.
async fn remove_session_dirs(deps: &PurgeDeps<'_>, session_ids: &[String]) -> usize {
    let Some(sessions_root) = deps.sessions_root.as_deref() else {
        if !session_ids.is_empty() {
            tracing::warn!(
                "Deleted {} conversation(s) with no sessions root resolved; their logs stay on disk",
                session_ids.len()
            );
        }
        return 0;
    };
    let mut removed = 0;
    for id in session_ids {
        if let Some(service) = deps.session_log.as_deref() {
            service.forget(id).await;
        }
        let dir = sessions_root.join(openalpaca_core::session_log::session_dir_name(id));
        if !dir.exists() {
            continue;
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!("The delete could not remove {}: {e}", dir.display()),
        }
    }
    removed
}

/// `session_changed{status:"deleted"}` for every conversation the
/// transaction deleted — what `DELETE /v1/sessions/{id}` publishes.
fn announce_deleted_sessions(deps: &PurgeDeps<'_>, plan: &DeletePlan, outcome: &DeleteOutcome) {
    for id in &outcome.session_ids {
        let lane_key = plan
            .sessions
            .iter()
            .find(|session| &session.id == id)
            .map(|session| session.lane_key.clone())
            .unwrap_or_default();
        let _ = deps
            .bus
            .publish(openalpaca_core::events::SystemEvent::SessionChanged {
                session_id: id.clone(),
                lane_key,
                status: "deleted".to_string(),
                task_id: None,
                timestamp: chrono::Utc::now(),
            });
    }
}

/// Delete everything OpenAlpaca holds for one project (ST-19), or answer the
/// plan — which is what happens unless the caller says `dry_run: false`.
///
/// The project is taken exactly: a path that resolves to a registered
/// project but is not its root is `422 WORKSPACE_NOT_A_ROOT`, naming the
/// root, before anything is counted — the one verb that deletes
/// conversations and notes never walks a subfolder up to its project.
///
/// Refused, before anything moves, while a run is in flight: a `queued`,
/// `running` or `paused` run whose `workspace_id` is the root
/// (`busy_tasks`), or a live run of one of its conversations that was
/// dispatched with no workspace of its own (`session_has_live_run`). These
/// checks run before the wait for the memory slot, so the delete's
/// transaction counts the project's runs in flight again — under the root or
/// from one of its conversations — and refuses on any
/// (`project_life::RunsInFlight`, the same `409`). That count also sees a run
/// dispatched with no workspace in the few statements before its lane is
/// registered (`dispatcher::lead_agent`), which the lane registry does not;
/// one dispatched after the commit is accepted, as the old purge accepted
/// it: nothing has written a transcript yet. A project re-pointed or deleted
/// while this waited for its memory slot — the plan was made for the project
/// as it was — is a `409` and nothing is deleted ([`still_the_project`]).
/// Then, holding the project's memory slot: the rows (`delete_project_data`,
/// files another conversation links kept in the root store), every owner's
/// partitions (`MemoryService::remove_project`) and `projects/<name>/` —
/// directly after the rows, so a put that resolved the project before the
/// transaction finds its folder gone as soon as possible — then the
/// conversations' log directories and their summaries in the owner's home
/// partition. A failure
/// after the rows went is logged and reported in `removed`, never a `500`:
/// the deletion the caller asked for has happened. A partition that could
/// not be dropped keeps `projects/<name>/` (`project_folder: false`), so the
/// project stays registered and deleting its data again — which finds no row
/// left — drops the rest and the folder.
pub(crate) async fn delete_workspace_data(deps: &PurgeDeps<'_>, request: PurgeRequest) -> Response {
    let written = match written_path(&request.path) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let path = canonical_of(&written);
    let project = match registered_project(&written, &path) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let root = project.root_string();
    // The root as `register` recorded it, sent as written — what the list
    // prints, even when it no longer canonicalizes to itself — or a path
    // whose canonical form is it (a missing tail as written, ST-7, so a
    // missing project's root also compares equal).
    let exact = project.root.as_path() == Path::new(&written)
        || projects::canonical_root(Path::new(&path)).is_ok_and(|p| p == project.root);
    if !exact {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "WORKSPACE_NOT_A_ROOT",
            format!(
                "{path} is inside the project rooted at {root}; deleting a project's data takes \
                 its root itself. Delete the data of {root} if that is what you meant"
            ),
        );
    }
    let store = ArtifactStore::new(deps.db);
    let rows = match store.workspace_rows(&root, Some(deps.owner)) {
        Ok(rows) => rows,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    };
    if rows.other_owners > 0 {
        return owner_mismatch(&root, rows.other_owners);
    }
    let busy = match store.busy_tasks(&root) {
        Ok(busy) => busy,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    };
    if busy > 0 {
        return api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{busy} run(s) under {root} are queued, running or paused; delete its data once \
                 they finish"
            ),
        );
    }
    let plan = match project_life::delete_plan(deps.db, &project) {
        Ok(plan) => plan,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                format!("{e:#}"),
            );
        }
    };
    for session in &plan.sessions {
        if crate::routes::sessions::session_has_live_run(
            deps.db,
            deps.ctx,
            &session.id,
            &session.lane_key,
        ) {
            return api_error(
                StatusCode::CONFLICT,
                "WORKSPACE_BUSY",
                format!(
                    "conversation {} of {root} has a run in flight; cancel it before deleting \
                     the project's data",
                    session.id
                ),
            );
        }
    }
    if request.dry_run {
        return Json(delete_view(
            &plan,
            &plan.counts,
            plan.kept,
            plan.memory_notes,
            None,
        ))
        .into_response();
    }

    let _quiesced = match quiesce_project(deps.memory, &project.id).await {
        Ok(guard) => guard,
        Err(e) => {
            return api_error(
                StatusCode::CONFLICT,
                "WORKSPACE_BUSY",
                format!(
                    "a memory write under {root} is still in flight ({e}); nothing was \
                     deleted. Try again in a moment"
                ),
            );
        }
    };
    let project = match still_the_project(&project, "deleted") {
        Ok(project) => project,
        Err(response) => return response,
    };
    let outcome = match project_life::delete_project_data(deps.db, &project) {
        Ok(outcome) => outcome,
        Err(e) => {
            // A run that started while this waited for the memory slot: the
            // transaction refused it, and nothing was deleted.
            if let Some(busy) = e
                .chain()
                .find_map(|cause| cause.downcast_ref::<project_life::RunsInFlight>())
            {
                return api_error(StatusCode::CONFLICT, "WORKSPACE_BUSY", busy.to_string());
            }
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                format!("deleting the data of {root} failed, and nothing was deleted: {e:#}"),
            );
        }
    };
    // The partitions and the folder first, straight after the rows: a put
    // that resolved the project before the transaction can land its bytes
    // and a row in `projects/<name>/` until the folder goes, and none can
    // once it has (`store::create_below`, R-F3); the window left between
    // the commit and the removal is design §18's residue.
    //
    // `remove_project` stops at the first partition it cannot drop. The
    // folder then stays, so the project stays registered and the delete run
    // again — finding no row left — drops the rest and the folder. Removed
    // now, it would strand the snapshots left: nothing lists one whose
    // project no folder records (R-S11), and a delete of that root answers
    // `404`.
    let mut fresh_plan = None;
    let (memory_partitions, partitions_left) = match deps.memory.remove_project(&project.id) {
        Ok(dropped) => (dropped, false),
        Err(e) => {
            tracing::warn!(
                "Deleted the data of {root}, but not all of its memory partitions could be \
                 dropped: {e}. Its folder {}/{}/ was kept, so the project stays registered: \
                 delete its data again to drop the rest and the folder",
                projects::PROJECTS_DIR,
                project.name
            );
            // What went: the partitions the plan counted, less those a fresh
            // count still finds — none claimed when it cannot be read, and one
            // short if an owner's first note there landed while this waited
            // for the slot.
            fresh_plan = project_life::delete_plan(deps.db, &project).ok();
            let left = fresh_plan
                .as_ref()
                .map_or(plan.memory_partitions, |after| after.memory_partitions);
            (plan.memory_partitions.saturating_sub(left), true)
        }
    };
    let project_folder = if partitions_left {
        false
    } else {
        match project_life::remove_project_folder(&project) {
            Ok(removed) => removed,
            Err(e) => {
                tracing::warn!(
                    "Deleted the data of {root}, but its folder was left in place: {e:#}"
                );
                false
            }
        }
    };
    // The notes went with the folder. When it stayed — a partition left, or
    // a removal that stopped part-way — what went is the plan's count less
    // what a fresh count still finds under it: none claimed when it cannot
    // be read.
    let memory_notes = if project_folder {
        plan.memory_notes
    } else {
        fresh_plan
            .or_else(|| project_life::delete_plan(deps.db, &project).ok())
            .map_or(0, |after| {
                plan.memory_notes.saturating_sub(after.memory_notes)
            })
    };
    let session_dirs = remove_session_dirs(deps, &outcome.session_ids).await;
    if let Some(queue) = deps.learning.as_deref() {
        for id in &outcome.session_ids {
            queue.forget_session(id);
        }
    }
    // Design 09 §13.3 (LQ4), as today: the deleted conversations' summaries
    // go with them — from the owner's home partition only. The project's
    // own partitions went whole above, and a forget into one of them would
    // wait for ever on the slot this call holds (`begin_write` waits while
    // `quiesce` holds the slot).
    crate::routes::sessions::forget_conversation_summaries(
        Some(deps.memory.clone()),
        deps.owner,
        None,
        outcome.message_ids.clone(),
    )
    .await;
    announce_deleted_sessions(deps, &plan, &outcome);
    Json(delete_view(
        &plan,
        &outcome.counts,
        outcome.kept,
        memory_notes,
        Some(RemovedView {
            session_dirs,
            memory_partitions,
            project_folder,
        }),
    ))
    .into_response()
}

// ── Handlers ─────────────────────────────────────────────────────

pub async fn get_workspace_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WorkspaceQuery>,
) -> Response {
    get_workspace(&state.db, query)
}

pub async fn choose_workspace_handler(Json(request): Json<ChooseWorkspaceRequest>) -> Response {
    choose_workspace(request)
}

pub async fn rebase_workspace_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RebaseRequest>,
) -> Response {
    rebase_workspace(
        &state.db,
        &state.memory_service,
        &state.local_user_id,
        request,
    )
    .await
}

/// Delete a project's data, or answer the plan. A real run flushes every live
/// writer first — the barrier `DELETE /v1/sessions/{id}` awaits — so a writer
/// holding records for a transcript about to go cannot re-create it after.
pub async fn purge_workspace_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PurgeRequest>,
) -> Response {
    if !request.dry_run {
        crate::routes::sessions::flush_session_logs(&state).await;
    }
    let session_log = state.gateway.shared_context.session_log().cloned();
    delete_workspace_data(
        &PurgeDeps {
            db: &state.db,
            ctx: &state.gateway.shared_context,
            bus: &state.gateway.bus,
            owner: &state.local_user_id,
            memory: &state.memory_service,
            sessions_root: session_log
                .as_ref()
                .map(|service| service.root().to_path_buf()),
            session_log,
            learning: state.gateway.learning().cloned(),
        },
        request,
    )
    .await
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod life_tests;
