//! Artifact endpoints — the read surface over `ArtifactStore` (plan §4.9).
//!
//! ```text
//! GET /v1/artifacts?task_id=&kind=&origin=&project_root=&pinned=&q=
//!                  &include_missing=&limit=&offset=  -> { artifacts, total }
//! GET /v1/artifacts/{id}                             -> Artifact | 404
//! GET /v1/artifacts/{id}/content[?version=N]         -> bytes | 401 | 404 | 410
//! GET /v1/artifacts/{id}/versions                    -> { versions }
//! GET /v1/artifacts/{id}/versions/{n}/content        -> bytes
//! GET /v1/artifacts/{id}/diff?from=1&to=2            -> ArtifactDiff | 409
//! PUT /v1/artifacts/{id}/pin  {"pinned":true}        -> { id, pinned }
//! ```
//!
//! Three rules run through the whole file.
//!
//! **Every route is owner-scoped, and a row belonging to someone else is a
//! `404`** — never a `403`, exactly as `routes/files.rs` already answers. The
//! two `ArtifactStore` calls that take no `owner_id` (`resolve_content`,
//! `set_pinned`) are therefore preceded by an owner-scoped `get`.
//!
//! **The envelope is Phase 0's shared `api_error`** (`{"error":{"code",
//! "message"}}`), and the `code` is [`ArtifactError::code`] verbatim, so
//! `ARTIFACT_GONE`/`NOT_DIFFABLE` reach the client unrenamed.
//!
//! **The content route authenticates inline** (GAP-11): it lives outside the
//! bearer middleware so a webview `<img src>`/`<iframe src>`, which cannot set
//! a header, can carry the token in the query string. Authorization is not
//! weakened — the owner check below is what it always was; only authentication
//! moved.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use openalpaca_storage::{
    ArtifactError, ArtifactKind, ArtifactOrigin, ArtifactQuery, ArtifactRecord, ArtifactStore,
    ArtifactVersionRow, Database, TaskRepository,
};
use serde::Deserialize;

use super::{api_error, content_response, content_token_ok, invalid_token};
use crate::AppState;

// ── Query and body types ─────────────────────────────────────────

/// `GET /v1/artifacts` — the query string of §4.9, one field per filter.
#[derive(Debug, Default, Deserialize)]
pub struct ListArtifactsParams {
    pub task_id: Option<String>,
    /// An `ArtifactKind` spelling; anything else is a `400`, never a silent
    /// "no filter" that would answer with rows the caller did not ask for.
    pub kind: Option<String>,
    pub origin: Option<String>,
    /// `?project_root=` with an empty value selects the home store — the same
    /// `COALESCE(project_root, '')` the store matches on.
    pub project_root: Option<String>,
    pub pinned: Option<bool>,
    pub q: Option<String>,
    #[serde(default)]
    pub include_missing: bool,
    /// Clamped to [`MAX_LIST_LIMIT`] (R26); non-positive means the default
    /// page size the store applies when no limit is given.
    pub limit: Option<i64>,
    /// Negative is a `400`, not a silent first page.
    pub offset: Option<i64>,
}

/// `?token=` (GAP-11) and `?version=N` on the content route.
#[derive(Debug, Default, Deserialize)]
pub struct ContentParams {
    pub token: Option<String>,
    pub version: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct DiffParams {
    pub from: u32,
    pub to: u32,
}

#[derive(Debug, Deserialize)]
pub struct PinRequest {
    pub pinned: bool,
}

// ── Paging bounds (R26) ──────────────────────────────────────────

/// The largest page `GET /v1/artifacts` will build, whatever `?limit=` asks
/// for. The list route costs one page query, one `COUNT(*)` and one title
/// lookup, all on the connection every other subsystem shares, so the page is
/// bounded rather than left to the caller.
pub(crate) const MAX_LIST_LIMIT: i64 = 500;

/// `?limit=`, resolved against the two bounds.
///
/// A non-positive value is `None` — the store's `DEFAULT_LIST_LIMIT`, never
/// SQLite's "no limit". Anything above [`MAX_LIST_LIMIT`] is *clamped* rather
/// than refused: the caller gets a smaller page and an exact `total`, which is
/// how it learns there is more to fetch.
fn page_limit(requested: Option<i64>) -> Option<i64> {
    requested.filter(|n| *n > 0).map(|n| n.min(MAX_LIST_LIMIT))
}

// ── Status mapping ───────────────────────────────────────────────

/// The §4.9 status codes. Nothing below this line decides one.
///
/// `DiffUnavailable` is a `409` for the same reason `NotDiffable` is: the
/// request is well-formed and the answer is "not from this build". T29 removes
/// the arm by removing the error.
fn artifact_error_status(error: &ArtifactError) -> StatusCode {
    match error {
        ArtifactError::NotFound { .. } | ArtifactError::VersionNotFound { .. } => {
            StatusCode::NOT_FOUND
        }
        ArtifactError::Gone { .. } => StatusCode::GONE,
        ArtifactError::NotDiffable { .. } | ArtifactError::DiffUnavailable { .. } => {
            StatusCode::CONFLICT
        }
    }
}

/// Render a store failure. A typed [`ArtifactError`] keeps its own code; any
/// other `anyhow` error is a database failure by elimination.
fn store_error(error: &anyhow::Error) -> Response {
    match error.downcast_ref::<ArtifactError>() {
        Some(e) => api_error(artifact_error_status(e), e.code(), e.to_string()),
        None => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            error.to_string(),
        ),
    }
}

/// The one shape a row this owner cannot see takes.
fn not_found(id: &str) -> Response {
    api_error(
        StatusCode::NOT_FOUND,
        "ARTIFACT_NOT_FOUND",
        format!("artifact {id} not found"),
    )
}

/// The owner-scoped head record, or the `404` that stands in for every reason
/// it is not visible.
///
/// The `Err` arm is the finished `Response` a handler returns unchanged, which
/// is the point — boxing an `axum::Response` here only to unbox it at five call
/// sites would buy nothing, so `result_large_err` is silenced deliberately.
#[allow(clippy::result_large_err)]
fn visible(db: &Database, owner_id: &str, id: &str) -> Result<ArtifactRecord, Response> {
    match ArtifactStore::new(db).get(id, owner_id) {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(not_found(id)),
        Err(e) => Err(store_error(&e)),
    }
}

// ── Serialisation (§4.9) ─────────────────────────────────────────

/// One `Artifact`: the client's type (`unbacked.ts:39-56`) plus the additive
/// `origin`, `pinned`, `missing`, `path`, `project_root` and `rel_path`.
///
/// `kind` is the stored snake_case spelling; a row whose column is NULL — one
/// that predates migration 036, or an upload written before R25 taught the
/// upload writer to classify one — is projected from its own `mime_type`
/// ([`ArtifactKind::for_mime`]), so this field is never `null` on the wire and
/// the client's non-nullable `ArtifactKind` holds. `metadata` is
/// `metadata_json` *parsed*, so a client never has to `JSON.parse` a string
/// field.
fn artifact_json(record: &ArtifactRecord, task_title: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": record.id,
        "name": record.name,
        "kind": record
            .kind
            .unwrap_or_else(|| ArtifactKind::for_mime(&record.mime_type))
            .as_str(),
        "mime_type": record.mime_type,
        "size_bytes": record.size_bytes,
        "task_id": record.task_id,
        "task_title": task_title,
        "agent_id": record.agent_id,
        "agent_template_id": record.agent_template_id,
        "version": record.version,
        "version_count": record.version_count,
        "summary": record.summary,
        "metadata": record
            .metadata_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok()),
        "created_at": record.created_at,
        "updated_at": record.updated_at,
        // Additive — the client type compiles unchanged against these.
        "origin": record.origin.as_str(),
        "pinned": record.pinned,
        "missing": record.missing(),
        "path": record.storage_path,
        "project_root": record.project_root,
        "rel_path": record.rel_path,
    })
}

/// One `ArtifactVersion` (`unbacked.ts:62-70`). `note` is coalesced from `NULL`
/// to `""`: the client types it as `string`, and "no note" is an empty note.
fn version_json(row: &ArtifactVersionRow) -> serde_json::Value {
    serde_json::json!({
        "version": row.version,
        "note": row.note.clone().unwrap_or_default(),
        "author_agent_id": row.author_agent_id,
        "created_at": row.created_at,
        "size_bytes": row.size_bytes,
        "added_lines": row.added_lines,
        "removed_lines": row.removed_lines,
    })
}

/// `task.title` for each distinct `task_id` on the page — **one** lookup for
/// the whole page (R26), whatever the page size.
///
/// [`TaskRepository::titles_for`] reads two columns; the `get`-per-row this
/// replaced materialized a whole `Task` (`state_json` and `outcome_json`
/// included) and took the global connection mutex once per row, to produce one
/// short string each. A missing entry covers both a loose artifact and a run
/// whose task row is gone, and a failed lookup degrades to no titles rather
/// than to no page.
fn task_titles(db: &Database, records: &[ArtifactRecord]) -> HashMap<String, String> {
    let ids: Vec<String> = records
        .iter()
        .filter_map(|r| r.task_id.clone())
        .collect::<HashSet<String>>()
        .into_iter()
        .collect();

    match TaskRepository::new(db).titles_for(&ids) {
        Ok(titles) => titles,
        Err(e) => {
            tracing::warn!("failed to load task titles for an artifact page: {e}");
            HashMap::new()
        }
    }
}

// ── The routes ───────────────────────────────────────────────────

/// `GET /v1/artifacts` — one page plus the unpaged total (§7: a paginated list
/// is an envelope, never a bare array).
///
/// The page is at most [`MAX_LIST_LIMIT`] rows and costs three queries no
/// matter how many: the page, the `COUNT(*)`, and one `titles_for` lookup.
pub(crate) fn list_artifacts(
    db: &Database,
    owner_id: &str,
    params: ListArtifactsParams,
) -> Response {
    let mut query = ArtifactQuery::new(owner_id);

    if let Some(kind) = params.kind.as_deref() {
        match ArtifactKind::parse(kind) {
            Some(k) => query.kind = Some(k),
            None => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "INVALID_KIND",
                    format!("unknown artifact kind '{kind}'"),
                );
            }
        }
    }
    if let Some(origin) = params.origin.as_deref() {
        // `ArtifactOrigin::parse` folds anything unknown to `Upload` — right
        // for reading a column, wrong for a filter, where it would answer with
        // uploads for `?origin=generated`.
        match origin {
            "upload" => query.origin = Some(ArtifactOrigin::Upload),
            "produced" => query.origin = Some(ArtifactOrigin::Produced),
            _ => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "INVALID_ORIGIN",
                    format!("unknown artifact origin '{origin}'"),
                );
            }
        }
    }

    query.task_id = params.task_id;
    query.project_root = params.project_root;
    query.pinned = params.pinned;
    query.q = params.q;
    query.include_missing = params.include_missing;
    query.limit = page_limit(params.limit);
    // A negative offset is a request nobody can serve — refusing it beats
    // silently answering with the first page, which is a different question.
    let offset = params.offset.unwrap_or(0);
    if offset < 0 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_OFFSET",
            format!("offset must not be negative, got {offset}"),
        );
    }
    query.offset = offset;

    let (records, total) = match ArtifactStore::new(db).list(&query) {
        Ok(page) => page,
        Err(e) => return store_error(&e),
    };

    let titles = task_titles(db, &records);
    let artifacts: Vec<serde_json::Value> = records
        .iter()
        .map(|record| {
            let title = record
                .task_id
                .as_deref()
                .and_then(|id| titles.get(id))
                .map(String::as_str);
            artifact_json(record, title)
        })
        .collect();

    Json(serde_json::json!({ "artifacts": artifacts, "total": total })).into_response()
}

/// `GET /v1/artifacts/{id}`.
pub(crate) fn get_artifact(db: &Database, owner_id: &str, id: &str) -> Response {
    let record = match visible(db, owner_id, id) {
        Ok(record) => record,
        Err(response) => return response,
    };
    let titles = task_titles(db, std::slice::from_ref(&record));
    let title = record
        .task_id
        .as_deref()
        .and_then(|id| titles.get(id))
        .map(String::as_str);
    Json(artifact_json(&record, title)).into_response()
}

/// `GET /v1/artifacts/{id}/content` and `…/versions/{n}/content` — the same
/// body, differing only in where `version` came from.
///
/// Authenticates inline (GAP-11), then answers exactly as the store does:
/// `404` for a row this owner cannot see or a version that does not exist,
/// `410` `ARTIFACT_GONE` when the bytes are missing — which is also what
/// stamps `missing_since` on the row.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn artifact_content(
    db: &Database,
    owner_id: &str,
    expected_token: &str,
    headers: &HeaderMap,
    query_token: Option<&str>,
    id: &str,
    version: Option<u32>,
) -> Response {
    if !content_token_ok(headers, query_token, expected_token) {
        return invalid_token();
    }
    let record = match visible(db, owner_id, id) {
        Ok(record) => record,
        Err(response) => return response,
    };
    let path = match ArtifactStore::new(db).resolve_content(id, version) {
        Ok(path) => path,
        Err(e) => return store_error(&e),
    };
    content_response(&path, &record.mime_type, &record.name).await
}

/// `GET /v1/artifacts/{id}/versions` — newest first.
pub(crate) fn list_artifact_versions(db: &Database, owner_id: &str, id: &str) -> Response {
    if let Err(response) = visible(db, owner_id, id) {
        return response;
    }
    match ArtifactStore::new(db).versions(id) {
        Ok(rows) => {
            let versions: Vec<serde_json::Value> = rows.iter().map(version_json).collect();
            Json(serde_json::json!({ "versions": versions })).into_response()
        }
        Err(e) => store_error(&e),
    }
}

/// `GET /v1/artifacts/{id}/diff?from=&to=`.
///
/// Until T29 lands the unified patch every diffable artifact answers `409`
/// `DIFF_UNAVAILABLE`; an image or a binary answers `409` `NOT_DIFFABLE`, and
/// that answer is final.
pub(crate) fn artifact_diff(db: &Database, owner_id: &str, id: &str, from: u32, to: u32) -> Response {
    if let Err(response) = visible(db, owner_id, id) {
        return response;
    }
    match ArtifactStore::new(db).diff(id, from, to) {
        Ok(diff) => Json(serde_json::json!({
            "from": diff.from,
            "to": diff.to,
            "added_lines": diff.added_lines,
            "removed_lines": diff.removed_lines,
            "format": diff.format,
            "patch": diff.patch,
        }))
        .into_response(),
        Err(e) => store_error(&e),
    }
}

/// `PUT /v1/artifacts/{id}/pin` (GAP-12).
pub(crate) fn pin_artifact(db: &Database, owner_id: &str, id: &str, pinned: bool) -> Response {
    if let Err(response) = visible(db, owner_id, id) {
        return response;
    }
    match ArtifactStore::new(db).set_pinned(id, pinned) {
        Ok(()) => Json(serde_json::json!({ "id": id, "pinned": pinned })).into_response(),
        Err(e) => store_error(&e),
    }
}

// ── Handlers ─────────────────────────────────────────────────────

pub async fn list_artifacts_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListArtifactsParams>,
) -> Response {
    list_artifacts(&state.db, &state.local_user_id, params)
}

pub async fn get_artifact_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    get_artifact(&state.db, &state.local_user_id, &id)
}

pub async fn get_artifact_content_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<ContentParams>,
    headers: HeaderMap,
) -> Response {
    artifact_content(
        &state.db,
        &state.local_user_id,
        &state.token,
        &headers,
        params.token.as_deref(),
        &id,
        params.version,
    )
    .await
}

pub async fn get_artifact_version_content_handler(
    State(state): State<Arc<AppState>>,
    Path((id, version)): Path<(String, u32)>,
    Query(params): Query<super::TokenParams>,
    headers: HeaderMap,
) -> Response {
    artifact_content(
        &state.db,
        &state.local_user_id,
        &state.token,
        &headers,
        params.token.as_deref(),
        &id,
        Some(version),
    )
    .await
}

pub async fn list_artifact_versions_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    list_artifact_versions(&state.db, &state.local_user_id, &id)
}

pub async fn get_artifact_diff_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<DiffParams>,
) -> Response {
    artifact_diff(
        &state.db,
        &state.local_user_id,
        &id,
        params.from,
        params.to,
    )
}

pub async fn pin_artifact_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PinRequest>,
) -> Response {
    pin_artifact(&state.db, &state.local_user_id, &id, body.pinned)
}

#[cfg(test)]
mod tests;
