//! `/v1/workspaces` — §4.8's "Project moved", as a route.
//!
//! ```text
//! GET   /v1/workspaces?path=<abs>          -> what is recorded at that root
//! PATCH /v1/workspaces {old_path,new_path} -> re-base everything onto the new one
//! POST  /v1/workspaces/purge {path|all}    -> delete one project's history
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
//! **A root a project came *from* is resolved the way a turn's
//! `x-workspace-path` is** (R22, `request_project_root`): up to the nearest
//! `.git`/`.openalpaca`, falling back to the path itself when there is no
//! marker to walk to — which is the usual state of a root a project has already
//! been moved *out of*. A re-base **destination** is taken literally instead
//! (`resolve_destination`): walking it up would re-address a history onto a
//! directory nobody named. The resolved values are what comes back, so a caller
//! can see what was answered about rather than assume.
//!
//! The `GET` exists because the picker cannot honestly offer a re-base without
//! it: it is the only way to learn that the store at a chosen path records a
//! *different* root (`.layout`'s `project_root=`, written once when the store
//! was seeded), and what a re-base would actually move.
//!
//! Nothing here re-bases on its own. The `PATCH` is the only writer, and it
//! refuses rather than guesses: `404` when no row of the caller's names the old
//! root (or a row there belongs to somebody else), `409` when rows already name
//! the new one (two projects must not merge silently), `409` while a run under
//! the old root is in flight, `409` when either root is the home store, `409`
//! when the store directory itself cannot be moved, and `422` when the
//! destination sits inside another project's root.
//!
//! The `POST …/purge` is the other writer, and it answers a plan rather than a
//! number: one line per store entry, in the retention-class terms the seeded
//! store README already uses, saying `delete` or `keep` for each. `dry_run`
//! defaults to **true** on the route as well as on the CLI — the daemon fails
//! closed, so a caller that forgets the field gets the plan and not the
//! deletion. Its refusals are the re-base's, for the same reasons — with one
//! difference: `path` is never walked up to an ancestor the way a re-base's
//! *old* root is (`resolve_purge_root`, ruling R72), unless nothing of the
//! caller's is recorded under the literal path at all — rows are the proof of
//! a root (R75), so a path any of the four members still names exactly is
//! purgeable by that path outright, marker walk or no. A destructive verb is
//! the one place that walk must not run silently on a path rows do not back —
//! `purge /repo/src` is a `422` `WORKSPACE_NOT_A_ROOT` naming `/repo` rather
//! than a purge of the whole project for a path that named one file of it.

use std::path::Path;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use openalpaca_storage::store::{self, migrate};
use openalpaca_storage::{ArtifactStore, Database, PurgePlan, RebaseCounts, WorkspaceRows};
use serde::Deserialize;

use openalpaca_core::context::SharedContext;
use openalpaca_core::memory::scope_context::resolves_to_the_home_store;

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
/// as somewhere a project **is or was**.
///
/// Resolved through the same walk a turn's `x-workspace-path` takes (R22), and
/// when that finds no marker — a project directory that has already been moved
/// away, leaving nothing behind — the canonical path itself stands, because
/// that is still exactly what the rows recorded.
#[allow(clippy::result_large_err)]
fn resolve_root(input: &str) -> Result<String, Response> {
    let literal = canonical_path(input)?;
    Ok(request_project_root(Some(&literal)).unwrap_or(literal))
}

/// A canonicalized path, refused when the marker walk finds it sits *inside*
/// another root's project rather than naming a root of its own — the shape
/// [`resolve_destination`] and [`resolve_purge_root`] share, `describe`
/// supplying the message that is specific to which of the two callers this is.
///
/// A path that resolves to itself — its own `.git`, its own `.openalpaca` (the
/// P-12 shape, where the store is already there) — or to no marker anywhere,
/// is taken as given.
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

/// The re-base **destination**, taken literally.
///
/// The marker walk is right for a root a project came *from* and wrong for one
/// it is going to: a fresh directory inside another repository resolves to
/// *that* repository's root, so `rebase /old/proj /mono/sub/proj` with
/// `/mono/.git` present would silently re-address a whole history onto `/mono`
/// and then rename `/old/proj/.openalpaca` to `/mono/.openalpaca`. Echoing the
/// resolved value back is mitigation, not prevention.
///
/// So the destination is canonicalized and then checked: a path that resolves
/// to an **ancestor** is `422 WORKSPACE_NOT_A_ROOT`, naming the ancestor, and
/// the caller decides which of the two roots they meant.
#[allow(clippy::result_large_err)]
fn resolve_destination(input: &str) -> Result<String, Response> {
    let literal = canonical_path(input)?;
    refuse_if_inside_another_root(literal, |literal, ancestor| {
        format!(
            "{literal} is inside the project rooted at {ancestor}, so re-basing onto it \
             would move everything onto {ancestor} instead. Re-base onto {ancestor} if that \
             is what you meant, or give {literal} a project marker of its own first"
        )
    })
}

/// The purge **target**, taken almost literally — ruling R72, refined by R75.
///
/// A destructive verb is the one place the old-root marker walk
/// ([`resolve_root`]) must not run silently: `purge /repo/src` walking up to
/// `/repo` would delete the whole project for a path that named one file of
/// it, and the caller would only learn the resolved root from what the
/// response deleted. So the given path is canonicalized, and then — **rows
/// are the proof of a root (R75)** — taken as given outright when any of the
/// four members (`file_assets`, `session`, `task`, `memory`) still names that
/// exact literal path, before the marker walk is even consulted: a project
/// moved out from under a monorepo's `.git`, its own `.openalpaca` gone with
/// it, stays purgeable by the root its rows recorded even though the walk
/// would otherwise resolve it to the monorepo root. Only a path *nothing*
/// names is subjected to the ancestor check, exactly as a re-base destination
/// is: a path that resolves to an **ancestor** is `422 WORKSPACE_NOT_A_ROOT`,
/// naming that root, rather than purging it for a subdirectory the caller
/// gave; a path that resolves to itself, or to no marker at all, is taken as
/// given — and from there the ordinary `404` follows if nothing is recorded
/// under it after all.
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

/// The home store is not a project and never becomes one (R24, and the fold in
/// `memory::scope_context`): its artifacts directory is one identity space, and
/// a project re-based onto it would share that directory with the home scope —
/// where a later home-scope `put` sees those rows as belonging to no address of
/// its own and removes the files (§4.2).
///
/// Both roots are checked, because a re-base *out of* `$HOME` would record the
/// home store as a project just as surely.
#[allow(clippy::result_large_err)]
fn refuse_the_home_root(field: &str, root: &str) -> Result<(), Response> {
    if !resolves_to_the_home_store(Path::new(root)) {
        return Ok(());
    }
    Err(api_error(
        StatusCode::CONFLICT,
        "WORKSPACE_IS_HOME",
        format!(
            "{field} resolves to {root}, which is the home store — that is not a project and \
             cannot be one. Name the project directory itself"
        ),
    ))
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

    let rows = match ArtifactStore::new(db).workspace_rows(&root, None) {
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
///
/// Owner-scoped like every other injecting write: the rows this rewrites must
/// belong to `owner_id`, and a root holding somebody else's is a `404` rather
/// than a transaction over rows the caller cannot see. `session` and `task`
/// carry no owner column, which is precisely why the answer is a refusal and
/// not a half-scoped update.
pub(crate) fn rebase_workspace(db: &Database, owner_id: &str, request: RebaseRequest) -> Response {
    let old_root = match resolve_root(&request.old_path) {
        Ok(root) => root,
        Err(response) => return response,
    };
    let new_root = match resolve_destination(&request.new_path) {
        Ok(root) => root,
        Err(response) => return response,
    };
    for (field, root) in [("old_path", &old_root), ("new_path", &new_root)] {
        if let Err(response) = refuse_the_home_root(field, root) {
            return response;
        }
    }
    if old_root == new_root {
        return api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PATH",
            format!("{old_root} is already where it is"),
        );
    }

    let store = ArtifactStore::new(db);
    let old_rows = match store.workspace_rows(&old_root, Some(owner_id)) {
        Ok(rows) => rows,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    };
    if old_rows.other_owners > 0 {
        return api_error(
            StatusCode::NOT_FOUND,
            "WORKSPACE_NOT_FOUND",
            format!(
                "{} row(s) under {old_root} belong to another owner; re-basing would rewrite \
                 rows you cannot see",
                old_rows.other_owners
            ),
        );
    }
    if old_rows.counts.is_empty() {
        return api_error(
            StatusCode::NOT_FOUND,
            "WORKSPACE_NOT_FOUND",
            format!("nothing of yours is recorded under {old_root}"),
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

    // Unscoped on purpose: *anybody's* rows at the destination are the merge
    // this refusal exists to prevent, and one owner cannot re-base over
    // another's history by not being able to see it.
    let new_rows = match store.workspace_rows(&new_root, None) {
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

// ── POST /v1/workspaces/purge ────────────────────────────────────

/// What the purge needs from `AppState`, named so the status codes and the
/// retention-class plan can be tested without standing up a gateway.
pub(crate) struct PurgeDeps<'a> {
    pub db: &'a Database,
    /// The live lane registry — half of the in-flight guard.
    pub ctx: &'a SharedContext,
    /// The local user; every write is scoped to it (R40).
    pub owner: &'a str,
    /// The home store's `sessions/`, where every session directory lives.
    /// `None` when the store could not be resolved at boot: the rows still go,
    /// and the directories are reported as not removed rather than guessed at.
    pub sessions_root: Option<std::path::PathBuf>,
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

/// The deletion plan for one root, in the README's terms (`HOME_README` /
/// `PROJECT_README`, `store/mod.rs`).
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
            format!(
                "{} workspace memories, and the reserved memory/ directory",
                plan.kept.memories
            ),
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
    // `false`: a purge is only ever a project root (the home store is refused
    // long before this runs), so `state/` and `plugins/` are not the store's
    // own here — they are exactly the unknown names this list exists to
    // report (Minor #4). `config` stays off this list in either scope: the
    // project README reserves it too (the "reserved names" line above already
    // names it, "skills/, config/"), and `unknown_entries` knows so — an
    // existing `<project>/.openalpaca/config/` is named once, not twice.
    for name in store::unknown_entries(&Path::new(root).join(store::STORE_DIR_NAME), false) {
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
/// `403`). The in-flight guard is two checks, because either alone has a hole:
/// [`ArtifactStore::busy_tasks`] counts `queued`, `running` and `paused` rows
/// under `root` directly — `queued` included, because a run that has not
/// started yet already named this root and is about to resolve its store —
/// and the lane registry
/// ([`crate::routes::sessions::session_has_live_run`]) catches a live run
/// whose task was dispatched with **no** `workspace_id` at all (no active
/// session on its lane at spawn time, `dispatcher::lead_agent`) but whose
/// `session_id` still names a conversation under `root`.
#[allow(clippy::result_large_err)]
fn preflight(deps: &PurgeDeps<'_>, root: &str) -> Result<PurgePlan, Response> {
    refuse_the_home_root("path", root)?;

    let store = ArtifactStore::new(deps.db);
    let rows = store
        .workspace_rows(root, Some(deps.owner))
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    if rows.other_owners > 0 {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "WORKSPACE_NOT_FOUND",
            format!(
                "{} row(s) under {root} belong to another owner; purging would delete rows you \
                 cannot see",
                rows.other_owners
            ),
        ));
    }
    if rows.counts.is_empty() {
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
/// outside this project's own store is left alone and logged — the store never
/// deletes what it did not create, and a row whose `storage_path` points
/// somewhere else is exactly the ambiguity to fail closed on.
fn remove_purged_bytes(
    deps: &PurgeDeps<'_>,
    root: &str,
    outcome: &openalpaca_storage::PurgeOutcome,
) -> (usize, usize) {
    let mut dirs = 0;
    if let Some(sessions_root) = deps.sessions_root.as_deref() {
        for id in &outcome.session_ids {
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

    let store_root = Path::new(root).join(store::STORE_DIR_NAME);
    let mut files = 0;
    for path in &outcome.upload_paths {
        let path = Path::new(path);
        if !path.starts_with(&store_root) {
            tracing::warn!(
                "Purge left {} alone: it is outside {}",
                path.display(),
                store_root.display()
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

/// Delete one project's conversations, runs and uploads — or report what that
/// would delete, which is what happens unless the caller says `dry_run: false`.
///
/// The plan is the answer in both cases, with `applied` saying which call this
/// was: a dry run whose shape differs from the real one is a plan nobody can
/// check against the outcome.
pub(crate) fn purge_workspaces(deps: &PurgeDeps<'_>, request: PurgeRequest) -> Response {
    let store = ArtifactStore::new(deps.db);
    let roots = match (request.path.as_deref(), request.all) {
        (Some(path), false) => match resolve_purge_root(&store, path) {
            Ok(root) => vec![root],
            Err(response) => return response,
        },
        (None, true) => match store.project_roots() {
            Ok(roots) => roots,
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
        match preflight(deps, root) {
            Ok(plan) => plans.push(plan),
            Err(response) => return response,
        }
    }

    let mut projects = Vec::with_capacity(roots.len());
    for (root, plan) in roots.iter().zip(plans.iter()) {
        let entries = plan_entries(root, plan);
        let mut project = serde_json::json!({
            "path": root,
            "entries": entries,
            "counts": purge_counts_json(&plan.counts),
            "kept": {
                "artifacts": plan.kept.artifacts,
                "memories": plan.kept.memories,
            },
        });
        if !request.dry_run {
            let outcome = match store.purge_project(root) {
                Ok(outcome) => outcome,
                Err(e) => {
                    return api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "DB_ERROR",
                        format!("purging {root}: {e}"),
                    );
                }
            };
            let (dirs, files) = remove_purged_bytes(deps, root, &outcome);
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
                "never swept; deleting it is a factory reset",
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
    rebase_workspace(&state.db, &state.local_user_id, request)
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
    purge_workspaces(
        &PurgeDeps {
            db: &state.db,
            ctx: &state.gateway.shared_context,
            owner: &state.local_user_id,
            sessions_root: state
                .gateway
                .shared_context
                .session_log()
                .map(|service| service.root().to_path_buf()),
        },
        request,
    )
}

#[cfg(test)]
mod tests;
