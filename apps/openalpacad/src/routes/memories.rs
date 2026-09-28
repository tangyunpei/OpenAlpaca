//! `/v1/memories*` — the user-directed memory API (memory redesign, design
//! §7 *Surfaces*, §7a *`/v1/memories` and `/v1/persona/user`*; owner
//! decisions G2 and G30; plan 3.8).
//!
//! Fifteen routes over one [`MemoryService`], every one under the bearer
//! middleware and in the shared `api_error` envelope. The owner is the
//! daemon's `local_user_id`; the project is the `x-workspace-path` header
//! resolved by the same marker walk a chat turn uses (absent → the global
//! partition only); the surface is `UserSurface`, so a write here records
//! origin `user` — the only surface that does (design §4).
//!
//! **A revision travels in `If-Match` and nowhere else.** A mutation that
//! addresses one note — `POST /v1/memories/{id}`, `POST …/undo`, `DELETE
//! …/{id}` — takes `If-Match: "<revision>"`: absent is `428
//! MEMORY_IF_MATCH_REQUIRED`, stale is `409 MEMORY_REVISION_CONFLICT` with
//! the current revision in the message and as `ETag`, and a body
//! `expected_revision` on any single-note mutation or on a create is `400
//! MEMORY_INVALID_INPUT` — the header is the one place, so a client cannot
//! send two revisions and have one silently win. (`POST /v1/memories/forget`
//! takes its targets' revisions in the body by contract, one per id; a
//! create's `supersedes.expected_revision` is the *target's* revision.) A
//! successful read or write answers `ETag: "<revision>"`.
//!
//! **Layered retrieval** (design 09 §13.1, MD-14): `GET /v1/memories/search`
//! takes the tool's `detail`, `date_from`, `date_to` and `order_by` — its
//! `detail` defaults to `snippets`, the answer the app's Memory view reads —
//! and `GET /v1/memories/timeline` is `memory_timeline`: around exactly one
//! of `anchor` (a note id), `q` (its first hit) or `date`, in index rows.
//!
//! **Suggestions** (Learning v2, design 09 §3.5): `GET
//! /v1/memories/suggestions` lists the pending ones, each with what else it
//! changes of its note besides the title and the text (`changes`, the L8
//! close-out's A4); `confirm` of a
//! suggestion takes the replaced note's revision as `target_revision` in
//! the body — like a create's `supersedes.expected_revision`, it is the
//! *other* note's — and answers `superseded` and `dismissed`; `undo`
//! withdraws a learned note or dismisses any candidate (*Keep mine*),
//! answering `status: "withdrawn"` and what came back as `restored`.
//!
//! Error codes (design §7a): `MEMORY_NOT_FOUND` 404, `MEMORY_REVISION_CONFLICT`
//! 409, `MEMORY_QUARANTINED` 409, `MEMORY_NOT_UNDOABLE` 409,
//! `MEMORY_INVALID_INPUT` 400, `MEMORY_IF_MATCH_REQUIRED` 428, and — for the
//! timeline while the search index is rebuilt — `MEMORY_INDEX_REBUILDING` 503.
//! A cursor from another filter set is `MEMORY_INVALID_INPUT`.
//!
//! Every write publishes `memory_changed` from inside the service (design
//! §7a), so nothing here publishes anything. The service does synchronous
//! file work; each call runs on the blocking pool.

use axum::{
    Json,
    extract::{
        Path as AxumPath, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use openalpaca_core::memory::service::{
    Attribution, Capabilities, DayEdge, ForgetInput, ForgetItemStatus, ListInput,
    MemoryAccessContext, MemoryError, MemoryService, Patch, ReadInput, SearchDetail, SearchInput,
    SearchOrder, StoreInput, Surface, TimelineAnchor, TimelineInput, UpdateAction, UpdateInput,
    WorkspaceHint, parse_date_bound, wire,
};
use openalpaca_storage::memory_files::{
    Dedup, NoteId, NoteKind, NoteScope, NoteSource, NoteStatus, ReplacePolicy, SourceType,
};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

use super::{api_error, persona::if_match, workspace_header};
use crate::AppState;

// ── Query and body types ────────────────────────────────────────────────
//
// Every object is closed (design §7a): an unknown key is a `400`, so the
// tool-side refusal of `owner_id`, `workspace_id`, a path or a trust field
// (design §3 rule 6) holds on the wire too.

/// `GET /v1/memories`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListMemoriesQuery {
    pub kind: Option<String>,
    pub status: Option<String>,
    pub scope: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_history: bool,
    #[serde(default)]
    pub include_candidates: bool,
}

/// `GET /v1/memories/search`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchMemoriesQuery {
    pub q: String,
    pub kind: Option<String>,
    pub scope: Option<String>,
    /// 1–50, default 10.
    pub limit: Option<usize>,
    #[serde(default)]
    pub historical: bool,
    pub as_of: Option<String>,
    /// `snippets` (default here) or `index` (design 09 §13.1).
    pub detail: Option<String>,
    /// RFC 3339, or a date `YYYY-MM-DD` from its start.
    pub date_from: Option<String>,
    /// RFC 3339, or a date `YYYY-MM-DD` to its end.
    pub date_to: Option<String>,
    /// `relevance` (default), `newest` or `oldest`.
    pub order_by: Option<String>,
}

/// `GET /v1/memories/timeline` (design 09 §13.1): exactly one of `anchor`,
/// `q` and `date`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineMemoriesQuery {
    /// A note id.
    pub anchor: Option<String>,
    /// A search whose first hit is the anchor.
    pub q: Option<String>,
    /// An instant: RFC 3339, or a date `YYYY-MM-DD` from its start.
    pub date: Option<String>,
    /// 0–20, default 5.
    pub before: Option<usize>,
    /// 0–20, default 5.
    pub after: Option<usize>,
}

/// `GET /v1/memories/recently-learned`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentlyLearnedQuery {
    /// RFC 3339, or a window such as `7d`, `24h`, `30m`; default 7 days.
    pub since: Option<String>,
    pub limit: Option<usize>,
}

/// `GET /v1/memories/suggestions` (design 09 §3.5).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionsQuery {
    /// Default 20, at most 100.
    pub limit: Option<usize>,
}

/// `GET /v1/memories/recently-used`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentlyUsedQuery {
    /// Default 50, at most 200 (the ring's capacity).
    pub limit: Option<usize>,
}

/// `GET /v1/memories/{id}`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadMemoryQuery {
    /// A heading: that section only.
    pub section: Option<String>,
    /// A kept `.history` revision instead of the current file.
    pub revision: Option<String>,
}

/// One `sources` entry of a create: a DB handle, verified by the service.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBody {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

/// The note a create replaces (design §6, *supersede*): its id and the
/// revision the caller saw, so a moved target is a `409`, never a silent
/// overwrite.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupersedesBody {
    pub id: String,
    pub expected_revision: String,
}

/// `POST /v1/memories` — the `memory_store` body (design §7a).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMemoryBody {
    pub title: String,
    pub body: String,
    pub kind: String,
    pub scope: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub fact_key: Option<String>,
    #[serde(default)]
    pub sources: Vec<SourceBody>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub supersedes: Option<SupersedesBody>,
    /// Refused: a revision travels in `If-Match` only, and a create has
    /// nothing to match — the field exists so the refusal names it.
    #[serde(default)]
    pub expected_revision: Option<serde_json::Value>,
}

/// `memory_update.patch` (design §7a): an absent field is untouched, an
/// explicit `null` clears a nullable one.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    pub title: Option<String>,
    pub body: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub fact_key: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub valid_from: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub valid_to: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub review_after: Option<Option<String>>,
}

/// `POST /v1/memories/{id}` — `memory_update` minus the revision, which is
/// the `If-Match` header.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateMemoryBody {
    /// `edit` | `archive` | `confirm`.
    pub action: String,
    pub patch: Option<PatchBody>,
    pub reason: Option<String>,
    /// With `confirm` only: the revision of the note a suggestion replaces,
    /// as the caller last saw it (design 09 §3.2, S5) — required for a
    /// suggestion, so a note edited since is a `409`, never retired unseen.
    pub target_revision: Option<String>,
    /// Refused: the revision is `If-Match`, never a body field.
    #[serde(default)]
    pub expected_revision: Option<serde_json::Value>,
}

/// One target of `POST /v1/memories/forget`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgetTargetBody {
    pub id: String,
    pub expected_revision: String,
}

/// `POST /v1/memories/forget` — the `memory_forget` body (design §7a).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgetMemoriesBody {
    pub targets: Vec<ForgetTargetBody>,
    pub reason: Option<String>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

// ── Handlers ────────────────────────────────────────────────────────────

fn api(state: &Arc<AppState>) -> MemoryApi {
    MemoryApi {
        service: state.memory_service.clone(),
        owner_id: state.local_user_id.clone(),
    }
}

fn bad_query(e: QueryRejection) -> Response {
    invalid(format!("query: {}", e.body_text())).into_response()
}

fn bad_body(e: JsonRejection) -> Response {
    invalid(format!("body: {}", e.body_text())).into_response()
}

/// `GET /v1/memories`.
pub async fn list_memories_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    query: Result<Query<ListMemoriesQuery>, QueryRejection>,
) -> Response {
    match query {
        Ok(Query(q)) => api(&state).list(&headers, q).await,
        Err(e) => bad_query(e),
    }
}

/// `GET /v1/memories/search`.
pub async fn search_memories_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    query: Result<Query<SearchMemoriesQuery>, QueryRejection>,
) -> Response {
    match query {
        Ok(Query(q)) => api(&state).search(&headers, q).await,
        Err(e) => bad_query(e),
    }
}

/// `GET /v1/memories/timeline`.
pub async fn timeline_memories_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    query: Result<Query<TimelineMemoriesQuery>, QueryRejection>,
) -> Response {
    match query {
        Ok(Query(q)) => api(&state).timeline(&headers, q).await,
        Err(e) => bad_query(e),
    }
}

/// `GET /v1/memories/recently-learned`.
pub async fn recently_learned_memories_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    query: Result<Query<RecentlyLearnedQuery>, QueryRejection>,
) -> Response {
    match query {
        Ok(Query(q)) => api(&state).recently_learned(&headers, q).await,
        Err(e) => bad_query(e),
    }
}

/// `GET /v1/memories/recently-used`.
pub async fn recently_used_memories_handler(
    State(state): State<Arc<AppState>>,
    query: Result<Query<RecentlyUsedQuery>, QueryRejection>,
) -> Response {
    match query {
        Ok(Query(q)) => api(&state).recently_used(q),
        Err(e) => bad_query(e),
    }
}

/// `GET /v1/memories/suggestions`.
pub async fn suggestions_memories_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    query: Result<Query<SuggestionsQuery>, QueryRejection>,
) -> Response {
    match query {
        Ok(Query(q)) => api(&state).suggestions(&headers, q).await,
        Err(e) => bad_query(e),
    }
}

/// `GET /v1/memories/doctor`.
pub async fn memory_doctor_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    api(&state).doctor(&headers).await
}

/// `POST /v1/memories`.
pub async fn create_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<CreateMemoryBody>, JsonRejection>,
) -> Response {
    match body {
        Ok(Json(body)) => api(&state).create(&headers, body).await,
        Err(e) => bad_body(e),
    }
}

/// `POST /v1/memories/forget`: each target its own forget — the pending
/// suggestions it names first, then the other targets in the order given
/// (the L8 close-out's repair round 2), the results in the order given; a
/// forgotten note's pending suggestions are withdrawn once every target has
/// had its turn, so a suggestion named with the note it would replace is
/// forgotten as asked, in either order (the close-out's A3). A target that
/// fails ends the request with its refusal, after the withdrawals the
/// targets before it owe (its repair round 1). `openalpaca memory forget`
/// sends its ids here in requests of up to 20, the pending suggestions
/// first.
pub async fn forget_memories_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Json<ForgetMemoriesBody>, JsonRejection>,
) -> Response {
    match body {
        Ok(Json(body)) => api(&state).forget(&headers, body).await,
        Err(e) => bad_body(e),
    }
}

/// `POST /v1/memories/sync`.
pub async fn sync_memories_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    api(&state).sync(&headers).await
}

/// `GET /v1/memories/{id}`.
pub async fn get_memory_handler(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    query: Result<Query<ReadMemoryQuery>, QueryRejection>,
) -> Response {
    match query {
        Ok(Query(q)) => api(&state).read(&headers, &id, q).await,
        Err(e) => bad_query(e),
    }
}

/// `GET /v1/memories/{id}/history`.
pub async fn get_memory_history_handler(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    api(&state).history(&headers, &id).await
}

/// `POST /v1/memories/{id}`.
pub async fn update_memory_handler(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    body: Result<Json<UpdateMemoryBody>, JsonRejection>,
) -> Response {
    match body {
        Ok(Json(body)) => api(&state).update(&headers, &id, body).await,
        Err(e) => bad_body(e),
    }
}

/// `POST /v1/memories/{id}/undo`.
pub async fn undo_memory_handler(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    api(&state).undo(&headers, &id).await
}

/// `DELETE /v1/memories/{id}`.
pub async fn delete_memory_handler(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    api(&state).delete(&headers, &id).await
}

// ── The API over the service ────────────────────────────────────────────

/// Everything a handler reads of the `AppState`: the service and the owner.
/// Driven directly by the tests (an `AppState` is not constructible there).
pub(crate) struct MemoryApi {
    pub(crate) service: Arc<MemoryService>,
    pub(crate) owner_id: String,
}

impl MemoryApi {
    /// The user-directed context for this request (design §7a, the
    /// per-surface table's `/v1/memories*` row): the daemon's owner, the
    /// header's project, `UserSurface`, every capability.
    fn context(
        service: &MemoryService,
        owner_id: &str,
        workspace: Option<&str>,
    ) -> Result<MemoryAccessContext, MemoryError> {
        let hint = match workspace {
            Some(path) => WorkspaceHint::Path(Path::new(path)),
            None => WorkspaceHint::None,
        };
        service.context(
            owner_id,
            hint,
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution {
                source: Some("api".to_string()),
                ..Attribution::default()
            },
        )
    }

    /// Runs `f` under this request's context on the blocking pool.
    async fn run<T, F>(&self, headers: &HeaderMap, f: F) -> Result<T, MemoryError>
    where
        T: Send + 'static,
        F: FnOnce(&MemoryService, &MemoryAccessContext) -> Result<T, MemoryError> + Send + 'static,
    {
        let service = self.service.clone();
        let owner_id = self.owner_id.clone();
        let workspace = workspace_header(headers);
        tokio::task::spawn_blocking(move || {
            let ctx = Self::context(&service, &owner_id, workspace.as_deref())?;
            f(&service, &ctx)
        })
        .await
        .map_err(|e| MemoryError::Internal(anyhow::anyhow!("memory task failed: {e}")))?
    }

    async fn list(&self, headers: &HeaderMap, q: ListMemoriesQuery) -> Response {
        let input = match list_input(q) {
            Ok(i) => i,
            Err(r) => return r.into_response(),
        };
        match self.run(headers, move |s, ctx| s.list(ctx, input)).await {
            Ok(out) => ok(wire::list_output_json(&out)),
            Err(e) => refusal(e),
        }
    }

    async fn search(&self, headers: &HeaderMap, q: SearchMemoriesQuery) -> Response {
        let (input, detail) = match search_input(q) {
            Ok(i) => i,
            Err(r) => return r.into_response(),
        };
        match self.run(headers, move |s, ctx| s.search(ctx, input)).await {
            Ok(out) => ok(wire::search_json(&out, detail)),
            Err(e) => refusal(e),
        }
    }

    async fn timeline(&self, headers: &HeaderMap, q: TimelineMemoriesQuery) -> Response {
        let input = match timeline_input(q) {
            Ok(i) => i,
            Err(r) => return r.into_response(),
        };
        match self
            .run(headers, move |s, ctx| s.timeline(ctx, input))
            .await
        {
            Ok(out) => ok(wire::timeline_json(&out)),
            Err(e) => refusal(e),
        }
    }

    async fn recently_learned(&self, headers: &HeaderMap, q: RecentlyLearnedQuery) -> Response {
        let since = match q.since.as_deref().map(parse_since).transpose() {
            Ok(s) => s,
            Err(r) => return r.into_response(),
        };
        let limit = q.limit;
        match self
            .run(headers, move |s, ctx| s.recently_learned(ctx, since, limit))
            .await
        {
            Ok(items) => ok(serde_json::json!({
                "items": items.iter().map(wire::list_item_json).collect::<Vec<_>>(),
            })),
            Err(e) => refusal(e),
        }
    }

    async fn suggestions(&self, headers: &HeaderMap, q: SuggestionsQuery) -> Response {
        let limit = q.limit;
        match self
            .run(headers, move |s, ctx| s.suggestions(ctx, limit))
            .await
        {
            Ok(items) => ok(wire::suggestions_json(&items)),
            Err(e) => refusal(e),
        }
    }

    fn recently_used(&self, q: RecentlyUsedQuery) -> Response {
        let items = self.service.recently_used(q.limit.unwrap_or(50));
        ok(wire::recently_used_json(&items))
    }

    async fn doctor(&self, headers: &HeaderMap) -> Response {
        match self.run(headers, |s, ctx| s.doctor(ctx)).await {
            Ok(report) => ok(wire::doctor_report_json(&report)),
            Err(e) => refusal(e),
        }
    }

    async fn create(&self, headers: &HeaderMap, body: CreateMemoryBody) -> Response {
        if body.expected_revision.is_some() {
            return invalid(
                "expected_revision is not a body field: a create has no revision to match, and a \
                 replacement names its target's revision in supersedes.expected_revision",
            )
            .into_response();
        }
        let input = match store_input(body) {
            Ok(i) => i,
            Err(r) => return r.into_response(),
        };
        match self.run(headers, move |s, ctx| s.store(ctx, input)).await {
            Ok(out) => {
                let status = match out.dedup {
                    Dedup::Created => StatusCode::CREATED,
                    Dedup::Existing => StatusCode::OK,
                };
                with_etag(
                    (status, Json(wire::store_output_json(&out))).into_response(),
                    &out.revision,
                )
            }
            Err(e) => refusal(e),
        }
    }

    async fn forget(&self, headers: &HeaderMap, body: ForgetMemoriesBody) -> Response {
        let targets = match body
            .targets
            .into_iter()
            .map(|t| Ok::<_, Refusal>((parse_id(&t.id)?, t.expected_revision)))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(t) => t,
            Err(r) => return r.into_response(),
        };
        if let Err(r) = check_reason(body.reason.as_deref()) {
            return r.into_response();
        }
        let input = ForgetInput {
            targets,
            reason: body.reason,
        };
        match self.run(headers, move |s, ctx| s.forget(ctx, input)).await {
            Ok(out) => ok(wire::forget_output_json(&out)),
            Err(e) => refusal(e),
        }
    }

    async fn sync(&self, headers: &HeaderMap) -> Response {
        match self.run(headers, |s, ctx| s.sync(ctx)).await {
            Ok(report) => ok(wire::sync_report_json(&report)),
            Err(e) => refusal(e),
        }
    }

    async fn read(&self, headers: &HeaderMap, id: &str, q: ReadMemoryQuery) -> Response {
        let id = match parse_id(id) {
            Ok(id) => id,
            Err(r) => return r.into_response(),
        };
        let result = match q.revision {
            Some(revision) => {
                if q.section.is_some() {
                    return invalid("section and revision cannot be combined").into_response();
                }
                self.run(headers, move |s, ctx| s.read_revision(ctx, id, &revision))
                    .await
            }
            None => {
                let input = ReadInput {
                    id,
                    expected_revision: None,
                    section: q.section,
                };
                self.run(headers, move |s, ctx| s.read(ctx, input)).await
            }
        };
        match result {
            Ok(out) => with_etag(ok(wire::read_output_json(&out)), &out.revision),
            Err(e) => refusal(e),
        }
    }

    async fn history(&self, headers: &HeaderMap, id: &str) -> Response {
        let id = match parse_id(id) {
            Ok(id) => id,
            Err(r) => return r.into_response(),
        };
        match self.run(headers, move |s, ctx| s.history(ctx, id)).await {
            Ok(entries) => ok(wire::history_json(&entries)),
            Err(e) => refusal(e),
        }
    }

    async fn update(&self, headers: &HeaderMap, id: &str, body: UpdateMemoryBody) -> Response {
        let id = match parse_id(id) {
            Ok(id) => id,
            Err(r) => return r.into_response(),
        };
        let expected_revision = match required_if_match(headers) {
            Ok(r) => r,
            Err(r) => return r.into_response(),
        };
        if body.expected_revision.is_some() {
            return invalid("expected_revision is not a body field: send If-Match: \"<revision>\"")
                .into_response();
        }
        if let Err(r) = check_reason(body.reason.as_deref()) {
            return r.into_response();
        }
        let patch_given = body.patch.is_some();
        let target_given = body.target_revision.is_some();
        let action = match body.action.as_str() {
            "edit" => {
                let Some(p) = body.patch else {
                    return invalid("`edit` needs a `patch` naming what changes").into_response();
                };
                match patch_input(p) {
                    Ok(patch) => UpdateAction::Edit(patch),
                    Err(r) => return r.into_response(),
                }
            }
            "archive" => UpdateAction::Archive,
            "confirm" => UpdateAction::Confirm {
                target_revision: body.target_revision,
            },
            other => {
                return invalid(format!(
                    "`action` must be `edit`, `archive` or `confirm`, got {other:?}"
                ))
                .into_response();
            }
        };
        if !matches!(action, UpdateAction::Edit(_)) && patch_given {
            return invalid("`patch` goes with `edit` only").into_response();
        }
        if target_given && !matches!(action, UpdateAction::Confirm { .. }) {
            return invalid("`target_revision` goes with `confirm` only").into_response();
        }
        let input = UpdateInput {
            id,
            expected_revision,
            action,
            reason: body.reason,
        };
        match self.run(headers, move |s, ctx| s.update(ctx, input)).await {
            Ok(out) => with_etag(ok(wire::update_output_json(&out)), &out.revision),
            Err(e) => refusal(e),
        }
    }

    async fn undo(&self, headers: &HeaderMap, id: &str) -> Response {
        let id = match parse_id(id) {
            Ok(id) => id,
            Err(r) => return r.into_response(),
        };
        let expected_revision = match required_if_match(headers) {
            Ok(r) => r,
            Err(r) => return r.into_response(),
        };
        match self
            .run(headers, move |s, ctx| s.undo(ctx, id, &expected_revision))
            .await
        {
            Ok(out) => ok(wire::withdraw_output_json(&out)),
            Err(e) => refusal(e),
        }
    }

    async fn delete(&self, headers: &HeaderMap, id: &str) -> Response {
        let id = match parse_id(id) {
            Ok(id) => id,
            Err(r) => return r.into_response(),
        };
        let expected_revision = match required_if_match(headers) {
            Ok(r) => r,
            Err(r) => return r.into_response(),
        };
        let input = ForgetInput {
            targets: vec![(id, expected_revision)],
            reason: None,
        };
        match self.run(headers, move |s, ctx| s.forget(ctx, input)).await {
            Ok(out) => forgotten_one(id, &out),
            Err(e) => refusal(e),
        }
    }
}

/// The single-target answer of `DELETE`: the forget result's one item as a
/// status code — `409` with the current revision as `ETag` when the note
/// moved, `404` when it is gone. (`undo` answers a withdrawal instead,
/// [`wire::withdraw_output_json`].)
fn forgotten_one(id: NoteId, out: &openalpaca_core::memory::service::ForgetOutput) -> Response {
    match out.results.first().map(|r| &r.status) {
        Some(ForgetItemStatus::Forgotten) => ok(serde_json::json!({
            "id": id.to_string(),
            "status": "forgotten",
            "retained": wire::retained_json(out),
        })),
        Some(ForgetItemStatus::RevisionConflict { current_revision }) => {
            refusal(MemoryError::RevisionConflict {
                id,
                current_revision: current_revision.clone(),
            })
        }
        Some(ForgetItemStatus::NotFound) | None => refusal(MemoryError::NotFound(id)),
    }
}

// ── Input parsing ───────────────────────────────────────────────────────

/// A refusal decided before the service is asked: the status, the §7a code
/// and the sentence. Small enough to ride in an `Err` (a `Response` is not).
struct Refusal(StatusCode, &'static str, String);

impl Refusal {
    fn into_response(self) -> Response {
        api_error(self.0, self.1, self.2)
    }
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal(
        StatusCode::BAD_REQUEST,
        "MEMORY_INVALID_INPUT",
        message.into(),
    )
}

fn parse_id(id: &str) -> Result<NoteId, Refusal> {
    NoteId::parse(id).map_err(|_| invalid(format!("`{id}` is not a note id (a UUID)")))
}

fn parse_kind(value: &str) -> Result<NoteKind, Refusal> {
    NoteKind::parse(value).ok_or_else(|| {
        invalid(format!(
            "kind must be one of fact, preference, decision, procedure, reference, episode; got {value:?}"
        ))
    })
}

fn parse_scope(value: &str) -> Result<NoteScope, Refusal> {
    NoteScope::parse(value).ok_or_else(|| {
        invalid(format!(
            "scope must be `global` or `project`, got {value:?}"
        ))
    })
}

fn parse_status(value: &str) -> Result<NoteStatus, Refusal> {
    NoteStatus::parse(value).ok_or_else(|| {
        invalid(format!(
            "status must be `active`, `candidate` or `archived`, got {value:?}"
        ))
    })
}

fn parse_ts(field: &str, value: &str) -> Result<DateTime<Utc>, Refusal> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            invalid(format!(
                "{field}: `{value}` is not an RFC 3339 timestamp: {e}"
            ))
        })
}

/// `?since=`: RFC 3339, or a window `<n>d` / `<n>h` / `<n>m` / `<n>s`. A
/// window the clock cannot subtract is a `400`, never an overflow panic —
/// this is a query string under the bearer token, and the router installs
/// no panic catcher.
fn parse_since(value: &str) -> Result<DateTime<Utc>, Refusal> {
    let trimmed = value.trim();
    if let Some((digits, unit)) = trimmed
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| trimmed.split_at(i))
        && !digits.is_empty()
        && let Ok(n) = digits.parse::<i64>()
    {
        let window = match unit {
            "d" => Some(Duration::try_days(n)),
            "h" => Some(Duration::try_hours(n)),
            "m" => Some(Duration::try_minutes(n)),
            "s" => Some(Duration::try_seconds(n)),
            _ => None,
        };
        if let Some(window) = window {
            return window
                .and_then(|w| Utc::now().checked_sub_signed(w))
                .ok_or_else(|| invalid("since: the window is too large"));
        }
    }
    parse_ts("since", trimmed).map_err(|_| {
        invalid(format!(
            "since: `{value}` is neither an RFC 3339 timestamp nor a window such as 7d, 24h or 30m"
        ))
    })
}

fn check_reason(reason: Option<&str>) -> Result<(), Refusal> {
    match reason {
        Some(r) if r.chars().count() > 500 => Err(invalid("`reason` is at most 500 characters")),
        _ => Ok(()),
    }
}

/// The revision a single-note mutation addresses: `If-Match`, required and
/// concrete — `*` names no revision, and a mutation without one is what the
/// header exists to prevent.
fn required_if_match(headers: &HeaderMap) -> Result<String, Refusal> {
    let Some(revision) = if_match(headers) else {
        return Err(Refusal(
            StatusCode::PRECONDITION_REQUIRED,
            "MEMORY_IF_MATCH_REQUIRED",
            "send If-Match: \"<revision>\" with the revision you read (GET /v1/memories/{id} answers it as ETag)"
                .to_string(),
        ));
    };
    if revision == "*" {
        return Err(invalid(
            "If-Match: * names no revision; send the one you read, so a note that changed \
             underneath you is a 409 and not an overwrite",
        ));
    }
    Ok(revision)
}

fn list_input(q: ListMemoriesQuery) -> Result<ListInput, Refusal> {
    Ok(ListInput {
        kind: q.kind.as_deref().map(parse_kind).transpose()?,
        status: q.status.as_deref().map(parse_status).transpose()?,
        scope: q.scope.as_deref().map(parse_scope).transpose()?,
        cursor: q.cursor,
        limit: q.limit,
        include_history: q.include_history,
        include_candidates: q.include_candidates,
    })
}

/// A date filter or a timeline's instant (design 09 §13.1): RFC 3339, or a
/// `YYYY-MM-DD` taken as that whole UTC day.
fn parse_date(field: &str, value: &str, edge: DayEdge) -> Result<DateTime<Utc>, Refusal> {
    parse_date_bound(value, edge).ok_or_else(|| {
        invalid(format!(
            "{field}: `{value}` is neither an RFC 3339 timestamp nor a date (YYYY-MM-DD)"
        ))
    })
}

/// The search and the detail to render it in — `snippets` unless asked, so
/// the Memory view's call answers as it always has.
fn search_input(q: SearchMemoriesQuery) -> Result<(SearchInput, SearchDetail), Refusal> {
    let detail = match q.detail.as_deref() {
        None => SearchDetail::Snippets,
        Some(v) => SearchDetail::parse(v)
            .ok_or_else(|| invalid(format!("detail must be `index` or `snippets`, got {v:?}")))?,
    };
    let order = match q.order_by.as_deref() {
        None => SearchOrder::Relevance,
        Some(v) => SearchOrder::parse(v).ok_or_else(|| {
            invalid(format!(
                "order_by must be `relevance`, `newest` or `oldest`, got {v:?}"
            ))
        })?,
    };
    let input = SearchInput {
        query: q.q,
        kind: q.kind.as_deref().map(parse_kind).transpose()?,
        scope: q.scope.as_deref().map(parse_scope).transpose()?,
        limit: q.limit,
        historical: q.historical,
        as_of: q
            .as_of
            .as_deref()
            .map(|v| parse_ts("as_of", v))
            .transpose()?,
        date_from: q
            .date_from
            .as_deref()
            .map(|v| parse_date("date_from", v, DayEdge::Start))
            .transpose()?,
        date_to: q
            .date_to
            .as_deref()
            .map(|v| parse_date("date_to", v, DayEdge::End))
            .transpose()?,
        order,
    };
    Ok((input, detail))
}

fn timeline_input(q: TimelineMemoriesQuery) -> Result<TimelineInput, Refusal> {
    let anchor = match (q.anchor, q.q, q.date) {
        (Some(id), None, None) => TimelineAnchor::Note(parse_id(&id)?),
        (None, Some(text), None) => TimelineAnchor::Query(text),
        (None, None, Some(date)) => {
            TimelineAnchor::Date(parse_date("date", &date, DayEdge::Start)?)
        }
        _ => {
            return Err(invalid(
                "give exactly one of anchor (a note id), q (a search) or date",
            ));
        }
    };
    Ok(TimelineInput {
        anchor,
        before: q.before,
        after: q.after,
    })
}

fn store_input(body: CreateMemoryBody) -> Result<StoreInput, Refusal> {
    let sources = body
        .sources
        .into_iter()
        .map(|s| {
            let kind = SourceType::parse(&s.kind).ok_or_else(|| {
                invalid(format!(
                    "sources[].type must be `message` or `task`, got {:?}",
                    s.kind
                ))
            })?;
            Ok(NoteSource {
                kind,
                id: s.id,
                observed_at: None,
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    let supersedes = body
        .supersedes
        .map(|s| Ok::<_, Refusal>((parse_id(&s.id)?, s.expected_revision)))
        .transpose()?;
    Ok(StoreInput {
        title: body.title,
        body: body.body,
        kind: Some(parse_kind(&body.kind)?),
        scope: body.scope.as_deref().map(parse_scope).transpose()?,
        aliases: body.aliases,
        tags: body.tags,
        fact_key: body.fact_key,
        sources,
        valid_from: body
            .valid_from
            .as_deref()
            .map(|v| parse_ts("valid_from", v))
            .transpose()?,
        valid_to: body
            .valid_to
            .as_deref()
            .map(|v| parse_ts("valid_to", v))
            .transpose()?,
        supersedes,
        // Learning's own (design 09 §2.5): never from a route.
        observed_at: None,
        replace_policy: ReplacePolicy::Direct,
        provenance: None,
    })
}

fn nullable_ts(
    field: &str,
    value: Option<Option<String>>,
) -> Result<Option<Option<DateTime<Utc>>>, Refusal> {
    match value {
        None => Ok(None),
        Some(None) => Ok(Some(None)),
        Some(Some(v)) => Ok(Some(Some(parse_ts(field, &v)?))),
    }
}

fn patch_input(p: PatchBody) -> Result<Patch, Refusal> {
    Ok(Patch {
        title: p.title,
        body: p.body,
        aliases: p.aliases,
        tags: p.tags,
        fact_key: p.fact_key,
        valid_from: nullable_ts("patch.valid_from", p.valid_from)?,
        valid_to: nullable_ts("patch.valid_to", p.valid_to)?,
        review_after: nullable_ts("patch.review_after", p.review_after)?,
    })
}

// ── Responses ───────────────────────────────────────────────────────────

fn ok(body: serde_json::Value) -> Response {
    (StatusCode::OK, Json(body)).into_response()
}

fn with_etag(mut response: Response, revision: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(&format!("\"{revision}\"")) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

/// `503 MEMORY_INDEX_REBUILDING`'s message, for a person: the timeline is
/// refused until the rebuild ends; searching and reading notes are not.
const INDEX_REBUILDING_MESSAGE: &str = "the memory search index is being rebuilt, so the \
     timeline cannot be read now; try again in a minute. Searching and reading notes work \
     meanwhile.";

/// The service's refusal as the §7a status and code. A revision conflict
/// carries the current revision in its message and as `ETag`, so a client
/// can re-read or retry without a second request; what the client kept of
/// its edit and what it should do next are the client's words, not the
/// daemon's. Refusals that no client state decides name their own next
/// step: `503 MEMORY_INDEX_REBUILDING` ([`INDEX_REBUILDING_MESSAGE`]: try
/// the timeline again, search and read meanwhile), `MEMORY_QUARANTINED`
/// (`openalpaca memory doctor --repair`), `MEMORY_BUSY` (a write pending on the
/// project: try again) and `MEMORY_NOT_UNDOABLE` (forget it instead).
fn refusal(e: MemoryError) -> Response {
    match e {
        MemoryError::InvalidInput(m) => invalid(m).into_response(),
        MemoryError::NotFound(_) | MemoryError::RevisionUnavailable { .. } => {
            api_error(StatusCode::NOT_FOUND, "MEMORY_NOT_FOUND", e.to_string())
        }
        MemoryError::RevisionConflict {
            ref current_revision,
            ..
        } => {
            let current = current_revision.clone();
            with_etag(
                api_error(
                    StatusCode::CONFLICT,
                    "MEMORY_REVISION_CONFLICT",
                    e.to_string(),
                ),
                &current,
            )
        }
        MemoryError::NotUndoable { .. } => {
            api_error(StatusCode::CONFLICT, "MEMORY_NOT_UNDOABLE", e.to_string())
        }
        MemoryError::Quarantined { .. } => {
            api_error(StatusCode::CONFLICT, "MEMORY_QUARANTINED", e.to_string())
        }
        // Neither can happen on the user-directed surface (design §4): the
        // arms exist because the match is exhaustive, and they say so.
        MemoryError::AuthorityConflict { .. } => api_error(
            StatusCode::CONFLICT,
            "MEMORY_AUTHORITY_CONFLICT",
            e.to_string(),
        ),
        // The two in-chat refusals (design 09 §4.2) are `memory_confirm`'s,
        // a tool's, and never a route's; they read as the confirm refusal
        // they are.
        MemoryError::ConfirmRefused { .. }
        | MemoryError::NotASuggestion { .. }
        | MemoryError::ApprovalRequired { .. } => api_error(
            StatusCode::CONFLICT,
            "MEMORY_CONFIRM_REFUSED",
            e.to_string(),
        ),
        MemoryError::WritePending { .. } => {
            api_error(StatusCode::CONFLICT, "MEMORY_BUSY", e.to_string())
        }
        // The L8 product slice's P8: the timeline answers at once while the
        // index is rebuilt, rather than hold the request for the rebuild.
        // The service's words name the model's tools; a person reading the
        // route's answer is told what they can use meanwhile (repair round 1
        // of the L8 product slice).
        MemoryError::IndexRebuilding => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MEMORY_INDEX_REBUILDING",
            INDEX_REBUILDING_MESSAGE.to_string(),
        ),
        MemoryError::NoOwner(_)
        | MemoryError::CapabilityDenied { .. }
        | MemoryError::Internal(_) => {
            tracing::warn!("memory route refused: {e}");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "MEMORY_INTERNAL",
                e.to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests;
