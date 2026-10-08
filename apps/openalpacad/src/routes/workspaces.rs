//! `/v1/workspaces` — a project's life (design §6), as routes.
//!
//! ```text
//! GET   /v1/workspaces?path=<abs>          -> what is recorded at that root
//! PATCH /v1/workspaces {old_path,new_path} -> re-point a project onto its new folder
//! POST  /v1/workspaces/purge {path|all}    -> delete one project's history
//! ```
//!
//! A project's canonical path is its identity in five row columns
//! (`task.workspace_id`, `session.workspace_id`,
//! `conversation_messages.workspace_root`, `lane_followups.workspace_path`,
//! `file_assets.project_root`) and in `file_assets.storage_path`'s prefix, so a
//! folder moved by hand strands them until it is re-pointed. The re-point is
//! `project_life::repoint`: the project's `project.json` marked, one
//! transaction, the folder under `projects/` renamed, the marker cleared — and
//! a crash between two steps is finished at the next boot (R24). The route
//! owns the checks storage cannot see: the owner (R40), the runs in flight and
//! the memory service's quiesce.
//!
//! The `GET` and the purge keep their pre-design shapes until Stage 1 step
//! (5) (interfaces §6, C8): the `GET` still reads an earlier build's in-project
//! `.layout` for `recorded_root` and `moved`, and the purge still resolves its
//! path by rows and the marker walk (R72, R75). `dry_run` defaults to **true**
//! on the purge — the daemon fails closed.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use openalpaca_storage::project_life;
use openalpaca_storage::store::projects::{ProjectError, ResolvedProject};
use openalpaca_storage::store::{self, projects};
use openalpaca_storage::{ArtifactStore, Database, PurgePlan, RebaseCounts, WorkspaceRows};
use serde::Deserialize;

use openalpaca_core::context::SharedContext;
use openalpaca_core::memory::service::MemoryService;

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

/// `{"path": "<root>"}` **or** `{"all": true}`, never both and never neither,
/// plus a `dry_run` that defaults to `true`.
#[derive(Debug, Deserialize)]
pub struct PurgeRequest {
    pub path: Option<String>,
    #[serde(default)]
    pub all: bool,
    /// Absent means a dry run. The destructive reading of a missing field is
    /// the one this route will not take.
    #[serde(default = "yes")]
    pub dry_run: bool,
}

fn yes() -> bool {
    true
}

// ── Path resolution ──────────────────────────────────────────────

/// A client's path, canonicalized and nothing more — no marker walk.
///
/// A relative path is refused rather than resolved: it would be read against
/// the *daemon's* working directory, which is the bug ruling R22 fixed.
#[allow(clippy::result_large_err)]
fn canonical_path(input: &str) -> Result<String, Response> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !Path::new(trimmed).is_absolute() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PATH",
            format!("a workspace path must be absolute, got '{input}'"),
        ));
    }
    let path = Path::new(trimmed);
    let canonical = path.canonicalize();
    let resolved = canonical.as_deref().unwrap_or(path);
    let text = resolved.to_string_lossy();
    // `/repo/` and `/repo` are one project; a bare root keeps its separator.
    let without_slash = text.trim_end_matches('/');
    Ok(match without_slash.is_empty() {
        true => text.to_string(),
        false => without_slash.to_string(),
    })
}

/// The canonical root string the four members hold, for a path a client named
/// as somewhere a project **is or was** — the `GET`'s resolution.
///
/// **Rows are the proof of a root (R75), here as well as on the purge.** A path
/// any of the four members still names *exactly* is taken as given, before the
/// marker walk is consulted: a project moved out from under a monorepo's `.git`
/// would otherwise resolve to the monorepo root, and the `GET` would describe
/// `/mono` rather than the path the rows recorded. The count is unscoped for
/// the same reason [`resolve_purge_root`]'s is: what is being established is
/// that this path was a root, not whose rows they are.
///
/// Only a path nothing names goes through the walk a turn's `x-workspace-path`
/// takes (R22), and when *that* finds no marker the canonical path itself
/// stands, because that is still exactly what the rows recorded.
#[allow(clippy::result_large_err)]
fn resolve_root(store: &ArtifactStore<'_>, input: &str) -> Result<String, Response> {
    let literal = canonical_path(input)?;
    let rows = store
        .workspace_rows(&literal, None)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    if !rows.counts.is_empty() {
        return Ok(literal);
    }
    Ok(request_project_root(Some(&literal)).unwrap_or(literal))
}

/// A canonicalized path, refused when it resolves to *another* root above it
/// rather than naming a root of its own — the purge's guard against deleting
/// a whole project for a path that named one folder of it (R72). A path that
/// resolves to itself, or to nothing, is taken as given. Removed with the
/// purge's path rules in Stage 1 step (5).
#[allow(clippy::result_large_err)]
fn refuse_if_inside_another_root(
    literal: String,
    describe: impl FnOnce(&str, &str) -> String,
) -> Result<String, Response> {
    match request_project_root(Some(&literal)) {
        Some(ancestor) if ancestor != literal => Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "WORKSPACE_NOT_A_ROOT",
            describe(&literal, &ancestor),
        )),
        _ => Ok(literal),
    }
}

/// The purge **target**, taken almost literally — ruling R72, refined by R75.
///
/// A destructive verb is the one place the old-root marker walk
/// ([`resolve_root`]) must not run silently: `purge /repo/src` walking up to
/// `/repo` would delete the whole project for a path that named one file of
/// it, and the caller would only learn the resolved root from what the
/// response deleted. So the given path is canonicalized, and then — **rows
/// are the proof of a root (R75)** — taken as given outright when any of the
/// three members (`file_assets`, `session`, `task`) still names that
/// exact literal path, before the marker walk is even consulted: a project
/// moved out from under a monorepo's `.git`, its own `.openalpaca` gone with
/// it, stays purgeable by the root its rows recorded even though the walk
/// would otherwise resolve it to the monorepo root. Only a path *nothing*
/// names is subjected to the ancestor check: a path that resolves to an
/// **ancestor** is `422 WORKSPACE_NOT_A_ROOT`, naming that root, rather than
/// purging it for a subdirectory the caller gave; a path that resolves to
/// itself, or to no marker at all, is taken as given — and from there the
/// ordinary `404` follows if nothing is recorded under it after all.
#[allow(clippy::result_large_err)]
fn resolve_purge_root(store: &ArtifactStore<'_>, input: &str) -> Result<String, Response> {
    let literal = canonical_path(input)?;
    let rows = store
        .workspace_rows(&literal, None)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    if !rows.counts.is_empty() {
        return Ok(literal);
    }
    refuse_if_inside_another_root(literal, |literal, ancestor| {
        format!(
            "purge names the project root itself; {literal} is inside the project rooted at \
             {ancestor}. Purge {ancestor} if that is what you meant, or name {literal} directly \
             once it has a project marker of its own"
        )
    })
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

/// The home store is not a project and never becomes one (R24, and R13's
/// `store::projects::overlaps_home_store`, which also refuses a folder inside
/// it or around it): the purge's refusal. The re-point's is
/// [`ProjectError::IsHome`], from `project_life`.
#[allow(clippy::result_large_err)]
fn refuse_the_home_root(field: &str, root: &str) -> Result<(), Response> {
    if !store::projects::overlaps_home_store(Path::new(root)) {
        return Ok(());
    }
    Err(api_error(
        StatusCode::CONFLICT,
        "WORKSPACE_IS_HOME",
        format!(
            "{field} resolves to {root}, which is the home store, lies inside it or contains \
             it — that is not a project and cannot be one. Name the project directory itself"
        ),
    ))
}

// ── Serialisation ────────────────────────────────────────────────

fn counts_json(counts: &RebaseCounts) -> serde_json::Value {
    serde_json::json!({
        "artifacts": counts.file_assets,
        "sessions": counts.sessions,
        "tasks": counts.tasks,
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
    let store = ArtifactStore::new(db);
    let root = match resolve_root(&store, &path) {
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

    let rows = match store.workspace_rows(&root, None) {
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
        // Both numbers, because the two writers refuse on different sets: a
        // re-base waits for `running`/`paused` (live in-process state a row
        // rewrite cannot reach), a purge refuses on `queued` as well (the run
        // has already named this root and is about to resolve its store). A
        // caller shown only `active_tasks` could not tell why its purge was
        // refused with nothing running.
        "queued_tasks": rows.queued_tasks,
    })
}

// ── PATCH /v1/workspaces ─────────────────────────────────────────

/// A [`ProjectError`] anywhere in `e`'s chain, as the route answers it, or
/// `None` for any other failure (interfaces §6.1): not registered `404`, the
/// home store `409`, not a directory `422`, already a project `409`, the
/// same path `400` — each with the error's own sentence.
pub(crate) fn project_error_response(e: &anyhow::Error) -> Option<Response> {
    let error = e
        .chain()
        .find_map(|cause| cause.downcast_ref::<ProjectError>())?;
    let status = match error {
        ProjectError::NotRegistered { .. } => StatusCode::NOT_FOUND,
        ProjectError::IsHome { .. } | ProjectError::Exists { .. } => StatusCode::CONFLICT,
        ProjectError::NotADirectory { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        ProjectError::SamePath { .. } => StatusCode::BAD_REQUEST,
    };
    Some(api_error(status, error.code(), error.to_string()))
}

/// Re-point a project onto the folder it is in now (design §6): the route's
/// half — the owner check, the in-flight check, the memory quiesce — then
/// [`project_life::repoint`], which refuses a new path that is not a
/// directory, overlaps the home store, is the same path, or is already a
/// project or named by rows (ST-20), and otherwise rewrites the five path
/// columns and both `storage_path` prefixes in one transaction and renames
/// the project's folder under `projects/`.
///
/// Owner-scoped like every injecting write (R40): a path no project owns and
/// a project holding another owner's rows answer the same `404`, in the same
/// words. Until Stage 1 step (4) keys memory by the project's id, a
/// re-pointed project's notes stay keyed by its old folder (unreleased
/// commits only).
pub(crate) fn rebase_workspace(
    db: &Database,
    memory: &MemoryService,
    owner_id: &str,
    request: RebaseRequest,
) -> Response {
    let old_path = match canonical_path(&request.old_path) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let new_path = match canonical_path(&request.new_path) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let project = match projects::resolve(Path::new(&old_path)) {
        Ok(Some(ResolvedProject {
            project: Some(project),
            ..
        })) => project,
        Ok(_) => {
            return api_error(
                StatusCode::NOT_FOUND,
                "WORKSPACE_NOT_FOUND",
                format!("nothing of yours is recorded under {old_path}"),
            );
        }
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_MOVE_FAILED",
                format!("{e:#}"),
            );
        }
    };
    let old_root = project.root_string();

    let store = ArtifactStore::new(db);
    let rows = match store.workspace_rows(&old_root, Some(owner_id)) {
        Ok(rows) => rows,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    };
    if rows.other_owners > 0 {
        return owner_mismatch(&old_root, rows.other_owners);
    }
    if rows.active_tasks > 0 {
        return api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{} run(s) under {old_root} are still in flight; a running or paused run \
                 resolved its store when it started, and re-pointing the row would not move it",
                rows.active_tasks
            ),
        );
    }

    // Held until this function returns: every memory write for the project
    // waits, and one already in flight refuses the re-point.
    let _quiesced = match memory.quiesce(Path::new(&old_root)) {
        Ok(guard) => guard,
        Err(e) => {
            return api_error(
                StatusCode::CONFLICT,
                "WORKSPACE_BUSY",
                format!(
                    "a memory write under {old_root} is still in flight ({e}); re-pointing now \
                     would move its folder out from under it. Try again in a moment"
                ),
            );
        }
    };

    match project_life::repoint(db, Path::new(&old_root), Path::new(&new_path)) {
        Ok(done) => Json(serde_json::json!({
            "old_path": done.before.root_string(),
            "new_path": done.after.root_string(),
            "moved": counts_json(&done.rows),
            "store_moved": done.before.name != done.after.name,
        }))
        .into_response(),
        Err(e) => project_error_response(&e).unwrap_or_else(|| {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "WORKSPACE_MOVE_FAILED",
                format!("{old_root} was not re-pointed onto {new_path}: {e:#}"),
            )
        }),
    }
}

// ── POST /v1/workspaces/purge ────────────────────────────────────

/// What the purge needs from `AppState`, named so the status codes and the
/// retention-class plan can be tested without standing up a gateway.
pub(crate) struct PurgeDeps<'a> {
    pub db: &'a Database,
    /// The live lane registry — half of the in-flight guard.
    pub ctx: &'a SharedContext,
    /// Where the `session_changed{status:"deleted"}` frames go: a purge removes
    /// conversations, and every client that renders a sidebar learns about a
    /// deleted one from that event (`DELETE /v1/sessions/{id}` publishes the
    /// same frame). Without it the purge left stale rows on screen until the
    /// next unrelated refetch.
    pub bus: &'a openalpaca_core::bus::EventBus,
    /// The local user; every write is scoped to it (R40).
    pub owner: &'a str,
    /// The memory service: a project known by its memory partition alone is
    /// still a project the `--all` listing names, and one whose memory a
    /// purge keeps.
    pub memory: &'a Arc<MemoryService>,
    /// The home store's `sessions/`, where every session directory lives.
    /// `None` when the store could not be resolved at boot: the rows still go,
    /// and the directories are reported as not removed rather than guessed at.
    pub sessions_root: Option<std::path::PathBuf>,
    /// The live writers, for the same reason `DELETE /v1/sessions/{id}` holds
    /// them: a writer still tracked for a purged session re-creates its
    /// directory on the next record, so each one is stood down before its
    /// directory goes.
    pub session_log: Option<Arc<openalpaca_core::session_log::SessionLogService>>,
}

/// One line of the plan: an entry of the store, what it holds here, the
/// retention class the seeded README gives it, and the verdict.
fn plan_entry(
    entry: &str,
    holds: impl Into<String>,
    retention: &str,
    action: &str,
) -> serde_json::Value {
    serde_json::json!({
        "entry": entry,
        "holds": holds.into(),
        "retention": retention,
        "action": action,
    })
}

/// The deletion plan for one root, in the retention terms of the home README
/// (`HOME_README`, `store/mod.rs`).
///
/// Every member is named including the zeroes: "nothing else went" is the half
/// a reader is checking for, and a plan that silently omits an empty entry
/// cannot be read as a promise about it.
fn plan_entries(root: &str, plan: &PurgePlan) -> Vec<serde_json::Value> {
    let c = &plan.counts;
    let mut entries = vec![
        plan_entry(
            "sessions/",
            format!(
                "{} conversations, {} messages, {} tool calls, {} follow-ups — their logs live \
                 in the home store's sessions/",
                c.sessions, c.messages, c.tool_calls, c.followups
            ),
            "size-capped, optional age sweep",
            "delete",
        ),
        plan_entry(
            "runs (database)",
            format!(
                "{} runs, {} subagent spans, {} run events",
                c.tasks, c.spans, c.run_events
            ),
            "never swept — removed only when you ask",
            "delete",
        ),
        plan_entry(
            "uploads/",
            format!("{} uploads copied into this project", c.uploads),
            "swept: an upload attached to no message is deleted once past the grace period",
            "delete",
        ),
        plan_entry(
            "artifacts/",
            format!(
                "{} files produced by runs in this project",
                plan.kept.artifacts
            ),
            "never garbage-collected",
            "keep",
        ),
        plan_entry(
            "memory/",
            "this project's memory notes — files, not rows, so a purge of the database \
             cannot reach them; only the episode notes summarising the conversations it \
             deletes go with those conversations"
                .to_string(),
            "yours — never swept",
            "keep",
        ),
        plan_entry(
            "skills/, config/",
            "reserved; not created until used",
            "yours — never swept",
            "keep",
        ),
        plan_entry(
            ".layout, README.md, .gitignore",
            "this store's own markers",
            "store metadata — never swept",
            "keep",
        ),
    ];
    // `false`: a purge is only ever a project (the home store is refused long
    // before this runs). The listing reads the project's folder under
    // `projects/` — never the project's own folder (ST-9) — and a root with
    // no folder there has nothing to list.
    let folder = store::store_root(&store::StoreScope::Project(std::path::PathBuf::from(root)));
    for name in folder
        .map(|dir| store::unknown_entries(&dir, false))
        .unwrap_or_default()
    {
        entries.push(plan_entry(
            &name,
            "not created by OpenAlpaca",
            "the store never deletes what it did not create",
            "keep",
        ));
    }
    entries
}

fn purge_counts_json(counts: &openalpaca_storage::PurgeCounts) -> serde_json::Value {
    serde_json::json!({
        "sessions": counts.sessions,
        "messages": counts.messages,
        "tool_calls": counts.tool_calls,
        "followups": counts.followups,
        "tasks": counts.tasks,
        "spans": counts.spans,
        "run_events": counts.run_events,
        "uploads": counts.uploads,
    })
}

/// Everything that must be true before a single row of `root` is deleted.
///
/// Owner-scoped like the re-base and for the same reason: `session` and `task`
/// carry no owner column, so a root holding somebody else's rows is a `404`
/// rather than a transaction over rows the caller cannot see (R40 — never a
/// `403`). The in-flight guard is exactly two checks, each covering what the
/// other cannot: [`ArtifactStore::busy_tasks`] counts `queued`, `running` and
/// `paused` **rows** whose own `workspace_id` column equals `root` —
/// `queued` included, because a run that has not started yet already named
/// this root and is about to resolve its store — and the lane registry
/// ([`crate::routes::sessions::session_has_live_run`]) separately catches a
/// **lane** with a live workflow whose task was dispatched with no
/// `workspace_id` of its own (no active session on its lane at spawn time,
/// `dispatcher::lead_agent::dispatch`) but whose `session_id` still names a
/// conversation under `root`.
///
/// This is what is checked, not everything that could be. A task row is
/// inserted with `status = queued` (`dispatcher::lead_agent::dispatch`,
/// ~`:297`) several statements before that same call registers its lane in
/// the registry (`register_workflow_for_lane`, `:426`). A task dispatched
/// with no `workspace_id` therefore exists, briefly, in a state neither check
/// sees: not counted by `busy_tasks` (its `workspace_id` is not `root`, or
/// anything) and not yet a live lane the registry knows about. A purge that
/// lands in that window is not refused by either half of this guard. The
/// window is one dispatch call — the row reaches `register_workflow_for_lane`
/// within milliseconds — and is accepted rather than closed: nothing has
/// written a transcript yet, and the very next purge attempt against `root`,
/// once registration has caught up, refuses like any other in-flight run.
///
/// `memory_roots` are the roots the memory service knows for this owner
/// ([`MemoryService::known_project_roots`]): a project with the caller's
/// notes and no row in any member is still a project, so it gets its plan —
/// `memory/` kept, nothing counted — instead of the `404` a root nothing
/// names. A root only another owner's partition knows is that `404`.
#[allow(clippy::result_large_err)]
fn preflight(
    deps: &PurgeDeps<'_>,
    root: &str,
    memory_roots: &HashSet<String>,
) -> Result<PurgePlan, Response> {
    refuse_the_home_root("path", root)?;

    let store = ArtifactStore::new(deps.db);
    let rows = store
        .workspace_rows(root, Some(deps.owner))
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    if rows.other_owners > 0 {
        // The same fixed message the re-base answers with: a `404` that named
        // the foreign rows and counted them would disclose exactly what it
        // exists not to confirm.
        return Err(owner_mismatch(root, rows.other_owners));
    }
    if rows.counts.is_empty() && !memory_roots.contains(root) {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "WORKSPACE_NOT_FOUND",
            format!("nothing of yours is recorded under {root}"),
        ));
    }
    let busy = store
        .busy_tasks(root)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    if busy > 0 {
        return Err(api_error(
            StatusCode::CONFLICT,
            "WORKSPACE_BUSY",
            format!(
                "{busy} run(s) under {root} are queued, running or paused; purging would delete \
                 the transcript a run is about to write into, or is about to resolve this store \
                 to start writing into"
            ),
        ));
    }

    let plan = store
        .purge_plan(root)
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    for session in &plan.sessions {
        if crate::routes::sessions::session_has_live_run(
            deps.db,
            deps.ctx,
            &session.id,
            &session.lane_key,
        ) {
            return Err(api_error(
                StatusCode::CONFLICT,
                "WORKSPACE_BUSY",
                format!(
                    "conversation {} under {root} has a run in flight; cancel it before purging",
                    session.id
                ),
            ));
        }
    }
    Ok(plan)
}

/// The bytes, after the rows: the session directories the transaction orphaned
/// and the upload blobs its rows addressed.
///
/// Rows first is the plan's order, so a crash here leaves files nothing
/// addresses rather than rows addressing files that are gone. An upload path
/// outside the project's folder under `projects/` is left alone and logged —
/// the store never deletes what it did not create, and a row whose
/// `storage_path` points somewhere else is exactly the ambiguity to fail
/// closed on.
async fn remove_purged_bytes(
    deps: &PurgeDeps<'_>,
    root: &str,
    outcome: &openalpaca_storage::PurgeOutcome,
) -> (usize, usize) {
    let mut dirs = 0;
    if let Some(sessions_root) = deps.sessions_root.as_deref() {
        for id in &outcome.session_ids {
            // Stand the writer down first: it creates the directory on its next
            // record (`SessionLogWriter::open` does `create_dir_all`), so a
            // removal with a live handle still out leaves an empty directory
            // nothing owns.
            if let Some(service) = deps.session_log.as_deref() {
                service.forget(id).await;
            }
            let dir = sessions_root.join(openalpaca_core::session_log::session_dir_name(id));
            if !dir.exists() {
                continue;
            }
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => dirs += 1,
                Err(e) => tracing::warn!("Purge could not remove {}: {e}", dir.display()),
            }
        }
    } else if !outcome.session_ids.is_empty() {
        tracing::warn!(
            "Purged {} session row(s) with no sessions root resolved; their logs stay on disk",
            outcome.session_ids.len()
        );
    }

    // The purge guard: an upload's bytes are removed only from inside this
    // project's folder under `projects/` (ST-1). The store never deletes what
    // it did not create, and a row whose `storage_path` points anywhere else
    // is exactly the ambiguity to fail closed on. A root with no folder there
    // (R12) has no bytes of its own to remove.
    let folder =
        match store::store_root(&store::StoreScope::Project(std::path::PathBuf::from(root))) {
            Ok(dir) => Some(dir),
            Err(e) => {
                if !outcome.upload_paths.is_empty() {
                    tracing::warn!("Purge left every upload of {root} alone: {e:#}");
                }
                None
            }
        };
    let mut files = 0;
    for path in &outcome.upload_paths {
        let path = Path::new(path);
        if !folder.as_deref().is_some_and(|dir| path.starts_with(dir)) {
            tracing::warn!(
                "Purge left {} alone: it is outside the project's folder",
                path.display()
            );
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => files += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("Purge could not remove {}: {e}", path.display()),
        }
    }
    (dirs, files)
}

/// Announce every conversation the purge deleted, one frame each — the same
/// `session_changed{status:"deleted"}` `DELETE /v1/sessions/{id}` publishes.
///
/// A client's sidebar learns that a conversation is gone from this event and
/// nothing else: the purge used to delete rows and directories in silence, so
/// every open window kept rendering conversations that no longer existed until
/// some unrelated refetch. Driven by `outcome.session_ids` — what the
/// transaction actually deleted — with the lane read from the plan that named
/// the same sessions a moment earlier; a session the plan somehow did not name
/// is announced with an empty lane rather than dropped, because the deletion is
/// the fact clients need.
fn announce_purged_sessions(
    deps: &PurgeDeps<'_>,
    plan: &PurgePlan,
    outcome: &openalpaca_storage::PurgeOutcome,
) {
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

/// Delete one project's conversations, runs and uploads — or report what that
/// would delete, which is what happens unless the caller says `dry_run: false`.
///
/// The plan is the answer in both cases, with `applied` saying which call this
/// was: a dry run whose shape differs from the real one is a plan nobody can
/// check against the outcome.
pub(crate) async fn purge_workspaces(deps: &PurgeDeps<'_>, request: PurgeRequest) -> Response {
    let store = ArtifactStore::new(deps.db);
    // The roots the caller's control snapshots know: where the `memory`
    // table's UNION used to be in the listing, now the memory service's
    // knowledge — scoped to the owner, as the row path is.
    let memory_roots: HashSet<String> = deps
        .memory
        .known_project_roots(deps.owner)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let roots = match (request.path.as_deref(), request.all) {
        (Some(path), false) => match resolve_purge_root(&store, path) {
            Ok(root) => vec![root],
            Err(response) => return response,
        },
        (None, true) => match store.project_roots() {
            Ok(mut roots) => {
                roots.extend(memory_roots.iter().cloned());
                roots.sort();
                roots.dedup();
                roots
            }
            Err(e) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string());
            }
        },
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "name exactly one of \"path\" (one project root) or \"all\": true (every \
                 recorded root)",
            );
        }
    };

    // Every refusal is decided before the first deletion, so `--all` is never
    // half applied: one busy root refuses the whole call rather than purging
    // the others and reporting the failure afterwards.
    let mut plans = Vec::with_capacity(roots.len());
    for root in &roots {
        match preflight(deps, root, &memory_roots) {
            Ok(plan) => plans.push(plan),
            Err(response) => return response,
        }
    }

    // Every root's rows in **one** transaction, before any bytes move: a DB
    // failure on the third root must not leave the first two purged, which is
    // what a transaction per root did — and the `500` named none of them.
    let outcomes = if request.dry_run {
        Vec::new()
    } else {
        match store.purge_projects(&roots) {
            Ok(outcomes) => outcomes,
            Err(e) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "DB_ERROR",
                    format!("purging {}: {e}", roots.join(", ")),
                );
            }
        }
    };

    let mut projects = Vec::with_capacity(roots.len());
    for (index, (root, plan)) in roots.iter().zip(plans.iter()).enumerate() {
        let entries = plan_entries(root, plan);
        let mut project = serde_json::json!({
            "path": root,
            "entries": entries,
            "counts": purge_counts_json(&plan.counts),
            "kept": {
                "artifacts": plan.kept.artifacts,
            },
        });
        if let Some(outcome) = outcomes.get(index) {
            let (dirs, files) = remove_purged_bytes(deps, root, outcome).await;
            announce_purged_sessions(deps, plan, outcome);
            // Design 09 §13.3 (LQ4): the purged conversations' summaries go
            // with them — by the message ids the transaction deleted.
            crate::routes::sessions::forget_conversation_summaries(
                Some(deps.memory.clone()),
                deps.owner,
                Some(root),
                outcome.message_ids.clone(),
            )
            .await;
            project["counts"] = purge_counts_json(&outcome.counts);
            project["removed"] = serde_json::json!({
                "session_dirs": dirs,
                "upload_files": files,
            });
        }
        projects.push(project);
    }

    let mut body = serde_json::json!({
        "dry_run": request.dry_run,
        "applied": !request.dry_run,
        "projects": projects,
    });
    // Only `--all` claims to have looked at everything, so only `--all` owes
    // the reader the lines about what it deliberately did not look at: the
    // home scope's own conversations and uploads, and the home store's
    // `state/` — never a purge target at all, named here rather than left for
    // the reader to assume (Minor #3).
    if request.all {
        let home = match store.home_scope_rows() {
            Ok(home) => home,
            Err(e) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string());
            }
        };
        body["home_scope"] = serde_json::Value::Array(vec![
            plan_entry(
                "no project",
                format!(
                    "{} conversations and {} uploads with no project — openalpaca sessions \
                     delete handles those",
                    home.sessions, home.uploads
                ),
                "the home store is not a project",
                "keep",
            ),
            plan_entry(
                "state/",
                "the machine's — the database, keys and logs",
                "never swept; a factory reset deletes only the database inside it",
                "keep",
            ),
        ]);
    }
    Json(body).into_response()
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
    rebase_workspace(
        &state.db,
        &state.memory_service,
        &state.local_user_id,
        request,
    )
}

/// Delete a project's conversations, runs and uploads, or report the plan.
///
/// The flush happens only when the call is going to delete something: a dry
/// run changes nothing, so awaiting every live writer's fsync would be an
/// fsync-per-writer's worth of latency for a call that has no rows to protect.
/// A real run flushes first, unconditionally — a writer holding records for a
/// transcript this call is about to delete would otherwise re-create the
/// directory after it went. The same barrier `DELETE /v1/sessions/{id}` awaits.
pub async fn purge_workspace_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PurgeRequest>,
) -> Response {
    if !request.dry_run {
        crate::routes::sessions::flush_session_logs(&state).await;
    }
    let session_log = state.gateway.shared_context.session_log().cloned();
    purge_workspaces(
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
        },
        request,
    )
    .await
}

#[cfg(test)]
mod tests;
