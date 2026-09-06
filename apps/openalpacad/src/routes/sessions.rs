//! Session routes (plan §5.7) — the surface that replaced `/v1/conversations`.
//!
//! ```http
//! GET    /v1/sessions?workspace_id=&source=&status=&q=&limit=&offset=
//! POST   /v1/sessions {source?, workspace_path?, title?}
//! GET    /v1/sessions/{id}
//! GET    /v1/sessions/{id}/messages?limit=&offset=&before_id=
//! GET    /v1/sessions/{id}/events            → 501 SESSION_EVENTS_NOT_SERVED
//! POST   /v1/sessions/{id}/activate
//! POST   /v1/sessions/{id}/archive
//! PATCH  /v1/sessions/{id} {title?, workspace_path?}   (409 if already bound)
//! DELETE /v1/sessions/{id}
//! ```
//!
//! `GET /v1/sessions/{id}/events` is registered but deliberately **not
//! served**: it reads the per-session JSONL log, which Phase 7b writes, and a
//! route that answered an empty list for every session would be a mock. It is
//! registered anyway so the answer is `501 SESSION_EVENTS_NOT_SERVED` — a
//! client following §5.7 is told what is going on, instead of getting axum's
//! generic `404`, which it could not tell from an unknown session id.
//!
//! **Owner scoping (ruling R40).** Reads are unscoped, matching the task and
//! follow-up reads. Every *write* is scoped to lanes whose user id is the local
//! user and answers `404` otherwise: creating, activating, archiving, renaming
//! or deleting another owner's conversation moves where their next turn lands
//! or destroys their transcript — that is the injecting side of R40's line,
//! and these routes are new, so they inherit no stance to preserve.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use openalpaca_core::bus::EventBus;
use openalpaca_core::context::SharedContext;
use openalpaca_core::events::SystemEvent;
use openalpaca_storage::{
    Conversation, ConversationRepository, Database, SESSION_ACTIVE, SESSION_ARCHIVED, SessionFilter,
};

use super::chat_types::{is_lane_owned_by, with_artifacts};
use super::{api_error, request_project_root, workspace_header};
use crate::AppState;

/// The status `task` rows carry while a run is interrupted (Phase 7b's sweep).
/// Read here so the sidebar can badge a session the moment 7b starts writing
/// it; until then the grouped query simply answers zero for every session.
const TASK_STATUS_INTERRUPTED: &str = "interrupted";

/// The default page size for both list routes.
const DEFAULT_LIMIT: i64 = 50;

// ── Wire shapes ──────────────────────────────────────────────────────

/// One session, as a client sees it (§5.7).
///
/// The stored row minus the summary columns: `summary`, `summary_version` and
/// friends are the context compactor's bookkeeping, not the conversation's
/// description, and a sidebar that rendered them would be showing the model's
/// notes to the user. `active_task_count` and `interrupted_task_count` are
/// derived — the first from the live lane registry, the second from one
/// grouped query over `task.session_id`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub id: String,
    pub lane_key: String,
    pub source: String,
    pub title: String,
    pub workspace_id: Option<String>,
    /// `"active"` | `"archived"`.
    pub status: String,
    pub message_count: i64,
    pub last_message_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
    /// Runs this session's lane has in flight right now.
    pub active_task_count: i64,
    /// Runs started from this session that were left interrupted.
    pub interrupted_task_count: i64,
}

#[derive(Debug, Serialize)]
pub struct SessionsResponse {
    pub sessions: Vec<SessionView>,
    pub total: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListSessionsQuery {
    pub workspace_id: Option<String>,
    pub source: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateSessionRequest {
    /// The lane's source — `"gui"` unless the client says otherwise. The lane
    /// itself is always the caller's own: `{local_user_id}:{source}`.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PatchSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
    /// A path binds the session's project; `""` unbinds it. Absent leaves it.
    /// Present at all on a session that **already has** a project is
    /// `409 SESSION_WORKSPACE_BOUND` (R48) — one project per session.
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SessionMessagesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Keyset cursor for "load older": the page ending just before this id.
    pub before_id: Option<i64>,
}

#[derive(Serialize)]
pub struct SessionMessagesResponse {
    pub messages: Vec<super::chat_types::ConversationMessageView>,
    pub total: i64,
}

// ── Helpers ──────────────────────────────────────────────────────────

/// The four things every session route needs from `AppState`.
///
/// Named rather than threaded through `State<Arc<AppState>>` for the same
/// reason the follow-up routes are: the logic below is status codes and
/// ownership rules, and both are worth testing without standing up a gateway,
/// an orchestrator and a connector manager to do it.
pub(super) struct Deps<'a> {
    pub db: &'a Database,
    pub bus: &'a EventBus,
    /// The live lane registry, for `active_task_count`.
    pub ctx: &'a SharedContext,
    /// The local user — the owner every write is scoped to (R40).
    pub owner: &'a str,
}

impl Deps<'_> {
    fn repo(&self) -> ConversationRepository<'_> {
        ConversationRepository::new(self.db)
    }

    /// Build the client-facing view of a page of sessions in one extra query
    /// for the whole page — never one per row.
    fn views(&self, sessions: Vec<Conversation>) -> Vec<SessionView> {
        let ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
        let interrupted = self
            .repo()
            .task_counts_by_session(&ids, TASK_STATUS_INTERRUPTED)
            .unwrap_or_else(|e| {
                // The counts are a badge; the list is the payload.
                tracing::warn!("Failed to count interrupted runs for a session page: {e}");
                Default::default()
            });

        sessions
            .into_iter()
            .map(|s| SessionView {
                active_task_count: self.ctx.workflows_for_lane(&s.lane_key).len() as i64,
                interrupted_task_count: interrupted.get(&s.id).copied().unwrap_or(0),
                id: s.id,
                lane_key: s.lane_key,
                source: s.source,
                title: s.title,
                workspace_id: s.workspace_id,
                status: s.status,
                message_count: s.message_count,
                last_message_at: s.last_message_at,
                created_at: s.created_at,
                updated_at: s.updated_at,
                ended_at: s.ended_at,
            })
            .collect()
    }

    /// Load a session for a **read**: unscoped by owner (R40); `404` when the
    /// id is unknown.
    #[allow(clippy::result_large_err)]
    fn load_for_read(&self, id: &str) -> Result<Conversation, Response> {
        match self.repo().get_session(id) {
            Ok(Some(session)) => Ok(session),
            Ok(None) => Err(not_found()),
            Err(e) => Err(db_error(e)),
        }
    }

    /// Load a session for a **write**: another owner's row answers `404`, not
    /// `403`, so the route never confirms someone else's conversation exists.
    #[allow(clippy::result_large_err)]
    fn load_for_write(&self, id: &str) -> Result<Conversation, Response> {
        let session = self.load_for_read(id)?;
        if !is_lane_owned_by(&session.lane_key, self.owner) {
            return Err(not_found());
        }
        Ok(session)
    }

    fn announce(&self, session_id: &str, lane_key: &str, status: &str) {
        let _ = self.bus.publish(SystemEvent::SessionChanged {
            session_id: session_id.to_string(),
            lane_key: lane_key.to_string(),
            status: status.to_string(),
            timestamp: Utc::now(),
        });
    }

    /// Re-read a session and answer with its view — what every write returns,
    /// so the client never has to guess what its own change produced.
    fn reread(&self, id: &str) -> Response {
        match self.load_for_read(id) {
            Ok(session) => match self.views(vec![session]).pop() {
                Some(view) => Json(view).into_response(),
                None => not_found(),
            },
            Err(response) => response,
        }
    }
}

/// Put the buffered records on disk before a transcript is archived or
/// deleted — after either, this session's writer may never be asked again,
/// and `emit` is a `try_send` that syncs on §5.4's boundaries and a 5 s timer.
/// The same barrier the daemon's shutdown path awaits.
async fn flush_session_logs(state: &AppState) {
    if let Some(service) = state.gateway.shared_context.session_log() {
        service.flush_all().await;
    }
}

fn deps(state: &AppState) -> Deps<'_> {
    Deps {
        db: &state.db,
        bus: &state.gateway.bus,
        ctx: &state.gateway.shared_context,
        owner: &state.local_user_id,
    }
}

fn not_found() -> Response {
    api_error(
        StatusCode::NOT_FOUND,
        "SESSION_NOT_FOUND",
        "Session not found",
    )
}

fn db_error(e: impl std::fmt::Display) -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string())
}

// ── GET /v1/sessions ─────────────────────────────────────────────────

pub async fn list_sessions_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListSessionsQuery>,
) -> impl IntoResponse {
    list_sessions(&deps(&state), query)
}

pub(super) fn list_sessions(deps: &Deps<'_>, query: ListSessionsQuery) -> Response {
    if let Some(status) = query.status.as_deref()
        && status != SESSION_ACTIVE
        && status != SESSION_ARCHIVED
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "INVALID_STATUS",
            "status must be \"active\" or \"archived\"",
        );
    }

    let filter = SessionFilter {
        workspace_id: query.workspace_id.as_deref(),
        source: query.source.as_deref(),
        status: query.status.as_deref(),
        q: query.q.as_deref().filter(|q| !q.trim().is_empty()),
        limit: query.limit.unwrap_or(DEFAULT_LIMIT),
        offset: query.offset.unwrap_or(0),
    };

    match deps.repo().list_sessions(&filter) {
        Ok((sessions, total)) => Json(SessionsResponse {
            sessions: deps.views(sessions),
            total,
        })
        .into_response(),
        Err(e) => db_error(e),
    }
}

// ── POST /v1/sessions ────────────────────────────────────────────────

pub async fn create_session_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<CreateSessionRequest>>,
) -> impl IntoResponse {
    let mut request = body.map(|Json(b)| b).unwrap_or_default();
    request.workspace_path = request.workspace_path.or_else(|| workspace_header(&headers));
    create_session(&deps(&state), request)
}

pub(super) fn create_session(deps: &Deps<'_>, request: CreateSessionRequest) -> Response {
    let source = request.source.as_deref().unwrap_or("gui");
    // Always the caller's own lane — a client cannot open a conversation in
    // someone else's (R40).
    let lane_key = format!("{}:{}", deps.owner, source);
    let workspace = request_project_root(request.workspace_path.as_deref());

    match deps.repo().create_session(
        &lane_key,
        source,
        workspace.as_deref(),
        request.title.as_deref(),
    ) {
        Ok(session) => {
            deps.announce(&session.id, &session.lane_key, SESSION_ACTIVE);
            let id = session.id.clone();
            match deps.views(vec![session]).pop() {
                Some(view) => (StatusCode::CREATED, Json(view)).into_response(),
                None => {
                    tracing::error!(%id, "created session vanished before it could be read back");
                    not_found()
                }
            }
        }
        Err(e) => db_error(e),
    }
}

// ── GET /v1/sessions/{id} ────────────────────────────────────────────

pub async fn get_session_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    deps(&state).reread(&id)
}

// ── GET /v1/sessions/{id}/messages ───────────────────────────────────

pub async fn get_session_messages_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<SessionMessagesQuery>,
) -> impl IntoResponse {
    get_session_messages(&deps(&state), &id, query)
}

pub(super) fn get_session_messages(
    deps: &Deps<'_>,
    id: &str,
    query: SessionMessagesQuery,
) -> Response {
    if let Err(response) = deps.load_for_read(id) {
        return response;
    }
    let repo = deps.repo();
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);

    let messages = match query.before_id {
        Some(cursor) => repo.list_by_session_before(id, cursor, limit),
        None => repo.list_by_session(id, limit, query.offset.unwrap_or(0)),
    };
    match messages {
        Ok(messages) => {
            let total = repo.count_by_session(id).unwrap_or(0);
            Json(SessionMessagesResponse {
                messages: with_artifacts(deps.db, messages),
                total,
            })
            .into_response()
        }
        Err(e) => db_error(e),
    }
}

// ── GET /v1/sessions/{id}/events ─────────────────────────────────────

pub async fn get_session_events_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    get_session_events(&deps(&state), &id)
}

/// The path exists so the surface tells the truth about itself: §5.7 lists it,
/// nothing serves it yet, and an unregistered path would answer axum's generic
/// `404` — indistinguishable from an unknown session id. The `404` check runs
/// first for that reason. Phase 7b (T42) replaces the body with the log.
pub(super) fn get_session_events(deps: &Deps<'_>, id: &str) -> Response {
    if let Err(response) = deps.load_for_read(id) {
        return response;
    }
    api_error(
        StatusCode::NOT_IMPLEMENTED,
        "SESSION_EVENTS_NOT_SERVED",
        "The per-session event log is served by Phase 7b (T42).",
    )
}

// ── POST /v1/sessions/{id}/activate ──────────────────────────────────

pub async fn activate_session_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    activate_session(&deps(&state), &id)
}

pub(super) fn activate_session(deps: &Deps<'_>, id: &str) -> Response {
    let session = match deps.load_for_write(id) {
        Ok(session) => session,
        Err(response) => return response,
    };
    // The incumbent steps down inside the same transaction, so name it before
    // the switch — afterwards the lane's active session is this one.
    let incumbent = deps
        .repo()
        .active_session_id(&session.lane_key)
        .unwrap_or_default()
        .filter(|current| current != &session.id);

    match deps.repo().activate_session(id) {
        Ok(true) => {
            if let Some(previous) = incumbent {
                deps.announce(&previous, &session.lane_key, SESSION_ARCHIVED);
            }
            deps.announce(&session.id, &session.lane_key, SESSION_ACTIVE);
            deps.reread(id)
        }
        Ok(false) => not_found(),
        Err(e) => db_error(e),
    }
}

// ── POST /v1/sessions/{id}/archive ───────────────────────────────────

pub async fn archive_session_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    flush_session_logs(&state).await;
    archive_session(&deps(&state), &id)
}

pub(super) fn archive_session(deps: &Deps<'_>, id: &str) -> Response {
    let session = match deps.load_for_write(id) {
        Ok(session) => session,
        Err(response) => return response,
    };
    match deps.repo().archive_session(id) {
        Ok(true) => {
            deps.announce(&session.id, &session.lane_key, SESSION_ARCHIVED);
            deps.reread(id)
        }
        Ok(false) => not_found(),
        Err(e) => db_error(e),
    }
}

// ── PATCH /v1/sessions/{id} ──────────────────────────────────────────

pub async fn patch_session_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<PatchSessionRequest>>,
) -> impl IntoResponse {
    patch_session(&deps(&state), &id, body.map(|Json(b)| b).unwrap_or_default())
}

pub(super) fn patch_session(deps: &Deps<'_>, id: &str, request: PatchSessionRequest) -> Response {
    let session = match deps.load_for_write(id) {
        Ok(session) => session,
        Err(response) => return response,
    };

    // R48: a session's project is bound once. Re-pointing it would leave the
    // runs it already started disagreeing with it about which project they
    // belong to, and `GET /v1/sessions?workspace_id=` would not list the
    // conversation those runs came from. Unbinding is refused with it: unbind
    // then bind is a re-point in two calls. §5.1's answer to a project change
    // is a new session — the gateway opens one on the turn that changes
    // project, and a client can ask for one with `POST /v1/sessions`.
    if request.workspace_path.is_some() && session.workspace_id.is_some() {
        return api_error(
            StatusCode::CONFLICT,
            "SESSION_WORKSPACE_BOUND",
            "This conversation is already bound to a project. Start a new session to change it.",
        );
    }

    // Absent leaves the binding alone; an empty string unbinds; a path binds,
    // resolved to its project root the same way a chat turn's header is (R22).
    // A path under no project marker resolves to `None` — which unbinds rather
    // than storing a root the daemon does not believe in. On an unbound
    // session both of those are the same no-op.
    let workspace = request.workspace_path.as_deref().map(|path| {
        if path.trim().is_empty() {
            None
        } else {
            request_project_root(Some(path))
        }
    });

    match deps.repo().update_session(
        id,
        request.title.as_deref(),
        workspace.as_ref().map(|w| w.as_deref()),
    ) {
        Ok(true) => {
            deps.announce(&session.id, &session.lane_key, &session.status);
            deps.reread(id)
        }
        Ok(false) => not_found(),
        Err(e) => db_error(e),
    }
}

// ── DELETE /v1/sessions/{id} ─────────────────────────────────────────

pub async fn delete_session_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    flush_session_logs(&state).await;
    delete_session(&deps(&state), &id)
}

pub(super) fn delete_session(deps: &Deps<'_>, id: &str) -> Response {
    let session = match deps.load_for_write(id) {
        Ok(session) => session,
        Err(response) => return response,
    };

    // A run in flight is still going to write into this transcript — deleting
    // it would strand the completion report the run is about to post. Only
    // runs *this* session started count: another conversation on the same lane
    // may well be busy, and that is not this session's problem.
    let live = deps.ctx.workflows_for_lane(&session.lane_key);
    if !live.is_empty() {
        let tasks = openalpaca_storage::TaskRepository::new(deps.db);
        let belongs_here = live.iter().any(|task_id| {
            matches!(
                tasks.get(task_id),
                Ok(Some(task)) if task.session_id.as_deref() == Some(session.id.as_str())
            )
        });
        if belongs_here {
            return api_error(
                StatusCode::CONFLICT,
                "SESSION_HAS_ACTIVE_WORKFLOWS",
                "This conversation has a run in flight. Cancel it before deleting.",
            );
        }
    }

    match deps.repo().delete_session(id) {
        Ok(true) => {
            deps.announce(&session.id, &session.lane_key, "deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => not_found(),
        Err(e) => db_error(e),
    }
}

#[cfg(test)]
mod tests;
