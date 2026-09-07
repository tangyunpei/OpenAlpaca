//! Chat route handlers
//!
//! POST   /v1/chat                     — Send a message (protected)
//! GET    /v1/chat/stream/:stream_id   — SSE stream (inline auth via ?token=)
//! GET    /v1/chat/history             — Get one session's transcript (protected)
//! DELETE /v1/chat/history             — Clear one session's transcript (protected)
//!
//! Migration 039 made a lane hold many conversations, so all three retarget to
//! a **session**: the lane's active one unless the request names another. The
//! `/v1/conversations` reads that used to live here are gone (P19) — deleted,
//! not aliased; `/v1/sessions` replaces them.
//!
//! **A `POST /v1/chat` that names a `session_id` addresses that session, full
//! stop (R49).** The `x-workspace-path` header is the window's *current*
//! project; the named conversation's binding is the one that governs the turn,
//! so the project switch §5.1 applies to an unnamed turn (R48) is skipped and
//! the turn's workspace root is the session's `workspace_id` when it has one
//! (the header when it does not, which binds it). Without that, resuming a
//! conversation from one project while the window shows another would archive
//! the session the same request just re-opened and file the turn in a third.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use chrono::Utc;
use futures_util::stream::Stream;
use openalpaca_core::events::SystemEvent;
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use super::chat_types::*;
use crate::AppState;

// ── Session targeting ───────────────────────────────────────────────

/// Resolve `POST /v1/chat`'s `session_id` into the conversation this turn will
/// land in, activating it when the caller asked for that. `Ok` carries that
/// session's project binding, which R49 makes the turn's workspace root.
///
/// The refusals, and why each one is what it is:
/// - **404** for an id that does not exist, *and* for one belonging to another
///   owner — a route that answered `403` would confirm that someone else's
///   conversation exists (R40's line, on the injecting side).
/// - **409 `SESSION_LANE_MISMATCH`** for a session on one of this owner's other
///   lanes. Chat sends on `{principal}:gui`; appending a GUI turn to a
///   Telegram conversation would silently re-home it.
/// - **409 `SESSION_ARCHIVED`** for a closed conversation the caller did not
///   ask to re-open. Re-opening archives whatever is live on that lane, which
///   is a decision, not a side effect of naming an id.
#[allow(clippy::result_large_err)]
fn target_session(
    db: &openalpaca_storage::Database,
    bus: &openalpaca_core::bus::EventBus,
    owner: &str,
    session_id: &str,
    activate: bool,
) -> Result<Option<String>, Response> {
    let repo = openalpaca_storage::ConversationRepository::new(db);
    let session = match repo.get_session(session_id) {
        Ok(Some(session)) => session,
        Ok(None) => {
            return Err(
                error_response(StatusCode::NOT_FOUND, "SESSION_NOT_FOUND", "Session not found")
                    .into_response(),
            );
        }
        Err(e) => {
            return Err(
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", &e.to_string())
                    .into_response(),
            );
        }
    };

    if !is_lane_owned_by(&session.lane_key, owner) {
        return Err(
            error_response(StatusCode::NOT_FOUND, "SESSION_NOT_FOUND", "Session not found")
                .into_response(),
        );
    }
    let chat_lane = format!("{owner}:gui");
    if session.lane_key != chat_lane {
        return Err(error_response(
            StatusCode::CONFLICT,
            "SESSION_LANE_MISMATCH",
            &format!(
                "Session belongs to lane '{}', but chat sends on '{chat_lane}'",
                session.lane_key
            ),
        )
        .into_response());
    }

    if session.status == openalpaca_storage::SESSION_ACTIVE {
        return Ok(session.workspace_id);
    }
    if !activate {
        return Err(error_response(
            StatusCode::CONFLICT,
            "SESSION_ARCHIVED",
            "This conversation is archived. Send `activate: true` to re-open it.",
        )
        .into_response());
    }

    match repo.activate_session(session_id) {
        Ok(true) => {
            let _ = bus.publish(openalpaca_core::events::SystemEvent::SessionChanged {
                session_id: session_id.to_string(),
                lane_key: session.lane_key.clone(),
                status: openalpaca_storage::SESSION_ACTIVE.to_string(),
                task_id: None,
                timestamp: Utc::now(),
            });
            Ok(session.workspace_id)
        }
        Ok(false) => Err(error_response(
            StatusCode::NOT_FOUND,
            "SESSION_NOT_FOUND",
            "Session not found",
        )
        .into_response()),
        Err(e) => Err(
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", &e.to_string())
                .into_response(),
        ),
    }
}

/// The conversation one `POST /v1/chat` turn addresses, and the workspace root
/// it runs in (R49). Returns that root; the refusals come from
/// [`target_session`].
///
/// **Naming a `session_id` addresses that session, full stop.** The
/// `x-workspace-path` header is the *window's* current project, which need not
/// be the project of the conversation the client just asked to resume. Taking
/// the header there would hand §5.1's project switch a mismatch it answers by
/// archiving the session this very request re-activated and opening a third
/// one — the turn would land somewhere the client never asked for, and the
/// conversation it explicitly re-opened would close again milliseconds later.
/// So a named session's own binding is the turn's workspace root: the message,
/// the resolved workspace and any run the turn starts all name one project,
/// which is the "one workspace per session" rule read forwards.
///
/// A named session that is not bound yet takes the header and is bound by it
/// downstream — a first binding, not a re-point. A request that names no
/// session keeps R48 exactly: the header is the turn's project, and a project
/// that differs from the lane's active session opens a new conversation.
#[allow(clippy::result_large_err)]
fn resolve_turn_target(
    db: &openalpaca_storage::Database,
    bus: &openalpaca_core::bus::EventBus,
    owner: &str,
    session_id: Option<&str>,
    activate: bool,
    header_workspace: Option<String>,
) -> Result<Option<String>, Response> {
    let Some(session_id) = session_id else {
        return Ok(header_workspace);
    };
    let bound = target_session(db, bus, owner, session_id, activate)?;
    Ok(bound.or(header_workspace))
}

/// Which session the two history routes act on: the one the query names, else
/// the lane's active one. `Ok(None)` means the lane has never held a turn.
///
/// A named session is checked against the lane the caller already passed
/// ownership for, so the query cannot reach across lanes by naming an id.
#[allow(clippy::result_large_err)]
fn resolve_history_session(
    db: &openalpaca_storage::Database,
    lane_key: &str,
    session_id: Option<&str>,
) -> Result<Option<String>, Response> {
    let repo = openalpaca_storage::ConversationRepository::new(db);
    match session_id {
        Some(id) => match repo.get_session(id) {
            Ok(Some(session)) if session.lane_key == lane_key => Ok(Some(session.id)),
            // A session on another lane is not this request's to read, and
            // saying so precisely would confirm it exists.
            Ok(_) => Err(error_response(
                StatusCode::NOT_FOUND,
                "SESSION_NOT_FOUND",
                "Session not found",
            )
            .into_response()),
            Err(e) => Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                &e.to_string(),
            )
            .into_response()),
        },
        None => match repo.active_session_id(lane_key) {
            Ok(id) => Ok(id),
            Err(e) => Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                &e.to_string(),
            )
            .into_response()),
        },
    }
}

// ── The turn's model (GAP-13) ───────────────────────────────────────

/// Resolve what `POST /v1/chat`'s optional `model` means for this turn, and
/// refuse an id the daemon cannot serve **before** anything is dispatched.
///
/// Refusing early is the point. A bogus id is not inert downstream:
/// `handle_simple_query` sizes the turn's trimming budget from
/// `model_registry().get_model_info(model)` and falls back to a 200k window
/// when the lookup misses, so an unrecognised name would quietly change how
/// much context the turn keeps instead of failing — silent degradation, which
/// the rules reject.
///
/// **A model whose provider is disabled is refused by the same lookup, with
/// the same code.** R58b takes a disabled provider's rows *out* of the
/// registry — neither its `[models]` entries nor its compiled defaults are
/// registered — so "not in the registry" is the one true answer for both
/// cases, and the message says so rather than pretending the two are
/// distinguishable here. The picker the GUI offers is built from
/// `GET /v1/models`, which reads the same registry, so neither kind of
/// refused id is selectable in the first place.
///
/// Returns the model the turn will run on: the named one, or the router's
/// current default when the request named none.
#[allow(clippy::result_large_err)]
fn resolve_turn_model(
    router: Option<&openalpaca_llm::LlmRouter>,
    requested: Option<&str>,
) -> Result<Option<String>, Response> {
    let Some(requested) = requested else {
        return Ok(router.map(|r| r.default_model()));
    };
    let Some(router) = router else {
        // Nothing to validate against, and nothing that could run it.
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured, so this daemon cannot run a named model",
        )
        .into_response());
    };
    if router.model_registry().get_model_info(requested).is_none() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "UNKNOWN_MODEL",
            &format!(
                "Unknown model '{requested}': it is not in the model registry. \
                 A model whose provider is disabled is not registered either — \
                 re-enable the provider to make its models selectable."
            ),
        )
        .into_response());
    }
    Ok(Some(requested.to_string()))
}

// ── POST /v1/chat ───────────────────────────────────────────────────

pub async fn send_chat_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChatSendRequest>,
) -> impl IntoResponse {
    let chat_service = match &state.chat_service {
        Some(svc) => svc,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "CHAT_NOT_CONFIGURED",
                "Chat service is not configured",
            )
            .into_response();
        }
    };

    // Validate attachment count
    {
        let config = state.daemon_config.load();
        if body.attachments.len() > config.upload.max_files_per_message {
            return error_response(
                StatusCode::BAD_REQUEST,
                "TOO_MANY_ATTACHMENTS",
                &format!(
                    "Too many attachments: {} provided, maximum is {}",
                    body.attachments.len(),
                    config.upload.max_files_per_message
                ),
            )
            .into_response();
        }
    }

    let principal = &state.local_user_id;

    // Which conversation this turn belongs to, and which project it runs in.
    // Naming a session is optional and the default is exactly today's
    // behaviour — the lane's active session, created on demand by the
    // persistence path below, in the project this header names. Naming one is
    // how a client resumes, and R49 makes that resumed conversation's own
    // project the turn's: see `resolve_turn_target`.
    let workspace_path = headers
        .get("x-workspace-path")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let workspace_path = match resolve_turn_target(
        &state.db,
        &state.gateway.bus,
        principal,
        body.session_id.as_deref(),
        body.activate,
        workspace_path,
    ) {
        Ok(path) => path,
        Err(response) => return response,
    };

    // GAP-13: which model this one turn runs on, refused here if the daemon
    // cannot serve it. `model_used` is echoed so the client never has to guess
    // whether its override took.
    let model_used = match resolve_turn_model(
        state
            .llm_settings_service
            .as_ref()
            .map(|s| s.router().as_ref()),
        body.model.as_deref(),
    ) {
        Ok(model) => model,
        Err(response) => return response,
    };

    match chat_service.send_message(
        body.content,
        body.attachments,
        principal,
        workspace_path,
        body.model,
    ) {
        Ok(resp) => {
            // Publish to EventBus; bridge forwards to WebSocket clients
            let _ = state.gateway.bus.publish(SystemEvent::ChatStreamStarted {
                stream_id: resp.stream_id.clone(),
                lane_key: resp.lane_key.clone(),
                timestamp: Utc::now(),
            });

            Json(ChatSendResponseBody {
                stream_id: resp.stream_id,
                lane_key: resp.lane_key,
                model_used,
            })
            .into_response()
        }
        Err(e) => {
            // A bad/foreign attachment id is a client error, not a gateway
            // failure — map it to 4xx so the GUI/CLI can distinguish
            // report-vs-retry instead of treating everything as a 500.
            let msg = e.to_string();
            let (status, code) = if msg.starts_with("Attachment not found") {
                (StatusCode::NOT_FOUND, "ATTACHMENT_NOT_FOUND")
            } else if msg.starts_with("Access denied to attachment") {
                (StatusCode::FORBIDDEN, "ATTACHMENT_ACCESS_DENIED")
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "GATEWAY_ERROR")
            };
            error_response(status, code, &msg).into_response()
        }
    }
}

// ── GET /v1/chat/stream/:stream_id ──────────────────────────────────

pub async fn chat_stream_handler(
    Path(stream_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Inline auth (same pattern as WebSocket events)
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    if token != state.token {
        return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
    }

    let chat_service = match &state.chat_service {
        Some(svc) => svc,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "CHAT_NOT_CONFIGURED",
                "Chat service is not configured",
            )
            .into_response();
        }
    };

    let rx = match chat_service.stream_manager().get_receiver(&stream_id) {
        Some(rx) => rx,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                "STREAM_NOT_FOUND",
                "Stream not found or expired",
            )
            .into_response();
        }
    };

    let stream = make_sse_stream(rx);

    let sse_keep_alive_secs = state.daemon_config.load().server.sse_keep_alive_secs;
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(sse_keep_alive_secs)))
        .into_response()
}

fn make_sse_stream(
    rx: tokio::sync::broadcast::Receiver<openalpaca_core::chat::ChatStreamEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let stream = BroadcastStream::new(rx);

    stream.filter_map(|result| match result {
        Ok(event) => {
            let sse_event = match &event {
                openalpaca_core::chat::ChatStreamEvent::Thinking => {
                    Event::default().event("thinking").data("{}")
                }
                openalpaca_core::chat::ChatStreamEvent::Delta { content } => Event::default()
                    .event("delta")
                    .data(serde_json::json!({"content": content}).to_string()),
                openalpaca_core::chat::ChatStreamEvent::Done { .. } => {
                    Event::default().event("done").data(done_event_data(&event))
                }
                openalpaca_core::chat::ChatStreamEvent::Error { message } => Event::default()
                    .event("error")
                    .data(serde_json::json!({"message": message}).to_string()),
                openalpaca_core::chat::ChatStreamEvent::ConfirmationRequested {
                    request_id,
                    tool_name,
                    tool_arguments,
                } => Event::default()
                    .event("confirmation_requested")
                    .data(serde_json::json!({
                        "request_id": request_id,
                        "tool_name": tool_name,
                        "tool_arguments": tool_arguments,
                    }).to_string()),
            };
            Some(Ok(sse_event))
        }
        Err(_) => None,
    })
}

/// SSE data payload for a `done` event: the serde form of the event minus the
/// `"event"` tag. Optional fields (attachments_used, delegation) appear only
/// when present.
fn done_event_data(event: &openalpaca_core::chat::ChatStreamEvent) -> String {
    let mut value = serde_json::to_value(event).unwrap_or_default();
    if let Some(obj) = value.as_object_mut() {
        obj.remove("event");
    }
    value.to_string()
}

// ── GET /v1/chat/history ────────────────────────────────────────────

pub async fn get_chat_history_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    let chat_service = match &state.chat_service {
        Some(svc) => svc,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "CHAT_NOT_CONFIGURED",
                "Chat service is not configured",
            )
            .into_response();
        }
    };

    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);
    let lane_key = query.lane_key.as_deref().unwrap_or(&state.default_lane_key);

    // Verify the caller owns this lane (lane_key format: "{user_id}:{source_name}")
    if !is_lane_owned_by(lane_key, &state.local_user_id) {
        return error_response(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Access denied to this lane",
        )
        .into_response();
    }

    let session_id = match resolve_history_session(&state.db, lane_key, query.session_id.as_deref()) {
        Ok(id) => id,
        Err(response) => return response,
    };
    // A lane that has never held a turn has no session and therefore no
    // transcript. That is an empty history, not an error.
    let Some(session_id) = session_id else {
        return Json(ChatHistoryResponse {
            messages: Vec::new(),
            total: 0,
            lane_key: lane_key.to_string(),
            session_id: None,
        })
        .into_response();
    };

    match chat_service.get_history(&session_id, limit, offset) {
        // GAP-23: the run link rides the row; the chips are one extra query
        // for the whole page, never one per message.
        Ok((messages, total)) => Json(ChatHistoryResponse {
            messages: with_artifacts(&state.db, messages),
            total,
            lane_key: lane_key.to_string(),
            session_id: Some(session_id),
        })
        .into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "HISTORY_NOT_FOUND",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── DELETE /v1/chat/history ─────────────────────────────────────────

pub async fn delete_chat_history_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DeleteHistoryQuery>,
) -> impl IntoResponse {
    let chat_service = match &state.chat_service {
        Some(svc) => svc,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "CHAT_NOT_CONFIGURED",
                "Chat service is not configured",
            )
            .into_response();
        }
    };

    let lane_key = query.lane_key.as_deref().unwrap_or(&state.default_lane_key);

    // Verify the caller owns this lane (lane_key format: "{user_id}:{source_name}")
    if !is_lane_owned_by(lane_key, &state.local_user_id) {
        return error_response(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "Access denied to this lane",
        )
        .into_response();
    }

    let session_id = match resolve_history_session(&state.db, lane_key, query.session_id.as_deref()) {
        Ok(id) => id,
        Err(response) => return response,
    };
    // Nothing to clear on a lane that has never held a turn.
    let Some(session_id) = session_id else {
        return Json(ChatDeleteResponse { deleted: 0 }).into_response();
    };

    match chat_service.clear_history(&session_id) {
        Ok(deleted) => {
            // Also clear that conversation's summary — the compactor's notes
            // describe messages that no longer exist. The session row itself
            // survives: this empties a conversation, it does not delete one
            // (`DELETE /v1/sessions/{id}` does).
            let conv_repo = openalpaca_storage::ConversationRepository::new(&state.db);
            let _ = conv_repo.clear_summary_for_session(&session_id);
            Json(ChatDeleteResponse { deleted }).into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "GATEWAY_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── PUT /v1/chat/messages/:message_id/feedback ────────────────────

pub async fn upsert_feedback_handler(
    State(state): State<Arc<AppState>>,
    Path(message_id): Path<i64>,
    Json(body): Json<FeedbackRequest>,
) -> impl IntoResponse {
    if body.feedback != "positive" && body.feedback != "negative" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "INVALID_FEEDBACK",
            "feedback must be 'positive' or 'negative'",
        )
        .into_response();
    }

    let repo = openalpaca_storage::MessageFeedbackRepository::new(&state.db);
    match repo.upsert(message_id, &body.feedback, body.comment.as_deref()) {
        Ok(()) => Json(FeedbackResponse {
            message_id,
            feedback: body.feedback,
            comment: body.comment,
        })
        .into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── GET /v1/chat/messages/:message_id/feedback ────────────────────

pub async fn get_feedback_handler(
    State(state): State<Arc<AppState>>,
    Path(message_id): Path<i64>,
) -> impl IntoResponse {
    let repo = openalpaca_storage::MessageFeedbackRepository::new(&state.db);
    match repo.get_by_message(message_id) {
        Ok(Some(fb)) => Json(FeedbackResponse {
            message_id: fb.message_id,
            feedback: fb.feedback,
            comment: fb.comment,
        })
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── DELETE /v1/chat/messages/:message_id/feedback ─────────────────

pub async fn delete_feedback_handler(
    State(state): State<Arc<AppState>>,
    Path(message_id): Path<i64>,
) -> impl IntoResponse {
    let repo = openalpaca_storage::MessageFeedbackRepository::new(&state.db);
    match repo.delete(message_id) {
        Ok(deleted) => Json(FeedbackDeleteResponse { deleted }).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── POST /v1/chat/confirmations/:request_id ──────────────────────

pub async fn confirm_tool(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
    Json(body): Json<ConfirmationBody>,
) -> impl IntoResponse {
    let broker = match &state.confirmation_broker {
        Some(b) => b,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "CONFIRMATION_NOT_CONFIGURED",
                "Confirmation broker is not configured",
            )
            .into_response();
        }
    };

    match broker.respond(
        &request_id,
        openalpaca_core::security::confirmation::ConfirmationResponse {
            approved: body.approved,
            approval_scope: body.approval_scope,
        },
    ) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_lane_owned_by_exact_match() {
        assert!(is_lane_owned_by("user1:gui", "user1"));
        assert!(is_lane_owned_by("user1:telegram", "user1"));
    }

    #[test]
    fn test_is_lane_owned_by_rejects_prefix_overlap() {
        // user1 must NOT match user10's lanes
        assert!(!is_lane_owned_by("user10:gui", "user1"));
    }

    #[test]
    fn test_is_lane_owned_by_empty_and_edge_cases() {
        assert!(!is_lane_owned_by("", "user1"));
        assert!(!is_lane_owned_by("user1", "user1")); // no colon separator
        assert!(is_lane_owned_by("user1:", "user1")); // empty source, still valid format
    }

    fn make_done(
        delegation: Option<openalpaca_core::gateway::DelegationInfo>,
    ) -> openalpaca_core::chat::ChatStreamEvent {
        openalpaca_core::chat::ChatStreamEvent::Done {
            content: "ack".to_string(),
            model: "router".to_string(),
            tokens_in: 0,
            tokens_out: 0,
            duration_ms: 42,
            attachments_used: None,
            delegation,
        }
    }

    #[test]
    fn test_done_event_data_without_delegation() {
        let data: serde_json::Value =
            serde_json::from_str(&done_event_data(&make_done(None))).unwrap();
        assert_eq!(data["content"], "ack");
        assert_eq!(data["model"], "router");
        assert_eq!(data["tokens_in"], 0);
        assert_eq!(data["tokens_out"], 0);
        assert_eq!(data["duration_ms"], 42);
        // The SSE payload carries no event tag and omits absent optionals.
        assert!(data.get("event").is_none());
        assert!(data.get("delegation").is_none());
        assert!(data.get("attachments_used").is_none());
    }

    #[test]
    fn test_done_event_data_with_delegation() {
        let event = make_done(Some(openalpaca_core::gateway::DelegationInfo {
            task_id: "task-123".to_string(),
            title: "Research Rust".to_string(),
        }));
        let data: serde_json::Value = serde_json::from_str(&done_event_data(&event)).unwrap();
        assert_eq!(data["delegation"]["task_id"], "task-123");
        assert_eq!(data["delegation"]["title"], "Research Rust");
    }

    // ── Phase 7a: which conversation a chat turn lands in ───────────

    fn sessions_db() -> (tempfile::TempDir, openalpaca_storage::Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).expect("db");
        (dir, db)
    }

    async fn refusal(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// The default: no `session_id`, so nothing is targeted and the lane's
    /// active session (created on demand downstream) takes the turn.
    #[test]
    fn an_absent_session_id_targets_nothing() {
        let (_dir, db) = sessions_db();
        let repo = openalpaca_storage::ConversationRepository::new(&db);
        assert!(repo.active_session_id("user1:gui").unwrap().is_none());
    }

    #[tokio::test]
    async fn an_active_session_on_the_chat_lane_is_accepted() {
        let (_dir, db) = sessions_db();
        let bus = openalpaca_core::bus::EventBus::default();
        let session = openalpaca_storage::ConversationRepository::new(&db)
            .get_or_create_active_session("user1:gui", "gui", None)
            .expect("session");

        assert_eq!(
            target_session(&db, &bus, "user1", &session.id, false).expect("accepted"),
            None,
            "an unbound session brings no project of its own"
        );

        // And a bound one hands its project back, for R49 to run the turn in.
        let bound = openalpaca_storage::ConversationRepository::new(&db)
            .create_session("user1:gui", "gui", Some("/repo/one"), None)
            .expect("session");
        assert_eq!(
            target_session(&db, &bus, "user1", &bound.id, false).expect("accepted"),
            Some("/repo/one".to_string())
        );
    }

    // ── GAP-13: the turn's model ────────────────────────────────

    /// A router with no providers at all — enough to answer "is this id
    /// registered?" and "what is the default?", which is all the route asks.
    fn router_with(disabled: &[openalpaca_llm::ProviderType]) -> openalpaca_llm::LlmRouter {
        let disabled: std::collections::HashSet<_> = disabled.iter().cloned().collect();
        openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::with_defaults_and_config(
                &std::collections::HashMap::new(),
                &disabled,
            ),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::with_defaults(),
            )),
            "claude-sonnet-4-6".to_string(),
        )
    }

    /// The default: no `model`, so the turn runs on the router's own default
    /// and the response says which that is.
    #[tokio::test]
    async fn a_turn_that_names_no_model_reports_the_daemon_default() {
        let router = router_with(&[]);
        assert_eq!(
            resolve_turn_model(Some(&router), None).expect("accepted"),
            Some("claude-sonnet-4-6".to_string())
        );
    }

    /// A registered id is accepted and echoed back as the turn's model.
    #[tokio::test]
    async fn a_registered_model_is_accepted_and_echoed() {
        let router = router_with(&[]);
        assert_eq!(
            resolve_turn_model(Some(&router), Some("claude-opus-4-6")).expect("accepted"),
            Some("claude-opus-4-6".to_string())
        );
    }

    /// An id the registry does not know is refused *before* dispatch, rather
    /// than silently resizing the turn's trimming budget downstream.
    #[tokio::test]
    async fn an_unknown_model_is_400_unknown_model() {
        let router = router_with(&[]);
        let err = resolve_turn_model(Some(&router), Some("gpt-9-turbo")).expect_err("refused");
        let (status, body) = refusal(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "UNKNOWN_MODEL");
        assert!(
            body["error"]["message"]
                .as_str()
                .expect("message")
                .contains("gpt-9-turbo"),
            "the refusal must name the id it refused"
        );
        // An empty string is not a model id either.
        let err = resolve_turn_model(Some(&router), Some("")).expect_err("refused");
        assert_eq!(refusal(err).await.0, StatusCode::BAD_REQUEST);
    }

    /// A disabled provider's models are refused by the same lookup: R58b takes
    /// its rows out of the registry, so there is nothing here to select.
    #[tokio::test]
    async fn a_disabled_providers_model_is_refused_too() {
        let router = router_with(&[openalpaca_llm::ProviderType::OpenAI]);
        // Sanity: the same id is fine while its provider is on.
        assert!(resolve_turn_model(Some(&router_with(&[])), Some("gpt-5.2")).is_ok());

        let err = resolve_turn_model(Some(&router), Some("gpt-5.2")).expect_err("refused");
        let (status, body) = refusal(err).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "UNKNOWN_MODEL");
        assert!(
            body["error"]["message"]
                .as_str()
                .expect("message")
                .contains("provider is disabled"),
            "the refusal must say why an id that used to work no longer does"
        );
        // The provider that is still on keeps its models.
        assert!(resolve_turn_model(Some(&router), Some("claude-opus-4-6")).is_ok());
    }

    /// No router at all: naming a model is refused rather than accepted and
    /// dropped, but a turn that names none is still fine (`model_used: null`).
    #[tokio::test]
    async fn naming_a_model_without_an_llm_router_is_503() {
        assert_eq!(resolve_turn_model(None, None).expect("accepted"), None);

        let err = resolve_turn_model(None, Some("claude-opus-4-6")).expect_err("refused");
        let (status, body) = refusal(err).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "LLM_NOT_CONFIGURED");
    }

    #[tokio::test]
    async fn an_unknown_or_foreign_session_is_404() {
        let (_dir, db) = sessions_db();
        let bus = openalpaca_core::bus::EventBus::default();
        let theirs = openalpaca_storage::ConversationRepository::new(&db)
            .get_or_create_active_session("someone-else:gui", "gui", None)
            .expect("session");

        for id in ["no-such-session", theirs.id.as_str()] {
            let err = target_session(&db, &bus, "user1", id, false).expect_err("refused");
            let (status, body) = refusal(err).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(body["error"]["code"], "SESSION_NOT_FOUND");
        }
    }

    #[tokio::test]
    async fn a_session_on_another_of_my_lanes_is_409() {
        let (_dir, db) = sessions_db();
        let bus = openalpaca_core::bus::EventBus::default();
        let cli = openalpaca_storage::ConversationRepository::new(&db)
            .get_or_create_active_session("user1:cli", "cli", None)
            .expect("session");

        let err = target_session(&db, &bus, "user1", &cli.id, false).expect_err("refused");
        let (status, body) = refusal(err).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "SESSION_LANE_MISMATCH");
    }

    #[tokio::test]
    async fn an_archived_session_is_409_until_activate_says_otherwise() {
        let (_dir, db) = sessions_db();
        let bus = openalpaca_core::bus::EventBus::default();
        let mut rx = bus.subscribe();
        let repo = openalpaca_storage::ConversationRepository::new(&db);
        let archived = repo
            .get_or_create_active_session("user1:gui", "gui", None)
            .expect("session");
        let live = repo
            .create_session("user1:gui", "gui", None, None)
            .expect("session");

        let err = target_session(&db, &bus, "user1", &archived.id, false).expect_err("refused");
        let (status, body) = refusal(err).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "SESSION_ARCHIVED");
        assert_eq!(
            repo.active_session_id("user1:gui").unwrap().as_deref(),
            Some(live.id.as_str()),
            "a refused target must not have moved the lane"
        );

        // With `activate`, the conversation re-opens and the incumbent steps
        // down — announced, so a second window follows.
        assert!(target_session(&db, &bus, "user1", &archived.id, true).is_ok());
        assert_eq!(
            repo.active_session_id("user1:gui").unwrap().as_deref(),
            Some(archived.id.as_str())
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(openalpaca_core::events::SystemEvent::SessionChanged { ref session_id, .. })
                if session_id == &archived.id
        ));
    }

    // ── R49: a named session brings its own workspace ───────────────

    fn project_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect(".git marker");
        dir
    }

    fn project_path(dir: &tempfile::TempDir) -> String {
        dir.path().to_string_lossy().to_string()
    }

    /// The canonical project root a turn carrying `path` runs in — the one
    /// resolver (R22), and the value a run this turn starts records as
    /// `task.workspace_id`.
    fn root(path: Option<&str>) -> Option<String> {
        openalpaca_core::memory::scope_context::MemoryScopeContext::for_request(path)
            .request_workspace_root
    }

    /// What the gateway does with the workspace the route resolved: the turn's
    /// user half, persisted exactly as `Gateway::handle_event` persists it.
    fn persist_turn(
        db: &openalpaca_storage::Database,
        workspace: Option<&str>,
    ) -> openalpaca_core::gateway::persistence::PersistedUserMessage {
        openalpaca_core::gateway::persistence::GatewayPersistence::new(db.clone())
            .persist_user_message("user1:gui", "carry on", "gui", root(workspace).as_deref())
            .expect("persist the user message")
    }

    fn session_count(db: &openalpaca_storage::Database) -> i64 {
        db.with_connection(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get(0))?)
        })
        .expect("count sessions")
    }

    /// R49: naming a session addresses *that* session. The window's header
    /// still says the project it is currently showing, and taking it would
    /// archive the conversation this same request re-opened (§5.1's project
    /// switch) and file the turn in a third one.
    #[tokio::test]
    async fn resuming_a_named_session_keeps_its_workspace_and_does_not_switch() {
        let (_dir, db) = sessions_db();
        let bus = openalpaca_core::bus::EventBus::default();
        let mut rx = bus.subscribe();
        let repo = openalpaca_storage::ConversationRepository::new(&db);

        let one = project_dir();
        let two = project_dir();
        let resumed = repo
            .create_session(
                "user1:gui",
                "gui",
                root(Some(&project_path(&one))).as_deref(),
                None,
            )
            .expect("session");
        // "New chat" on project two archived it; the window is showing two.
        repo.create_session(
            "user1:gui",
            "gui",
            root(Some(&project_path(&two))).as_deref(),
            None,
        )
        .expect("session");

        let workspace = resolve_turn_target(
            &db,
            &bus,
            "user1",
            Some(&resumed.id),
            true,
            Some(project_path(&two)),
        )
        .expect("the resume is accepted");

        assert_eq!(
            workspace, resumed.workspace_id,
            "the named session's binding is the turn's workspace, not the header"
        );
        assert_eq!(
            root(workspace.as_deref()),
            resumed.workspace_id,
            "so a run this turn starts records the session's project"
        );

        let turn = persist_turn(&db, workspace.as_deref());
        assert_eq!(
            turn.session_id, resumed.id,
            "the turn lands in the conversation it named"
        );
        assert!(
            !turn.project_switched,
            "naming a session skips the project switch"
        );
        assert_eq!(
            repo.get_session(&resumed.id)
                .unwrap()
                .expect("session")
                .status,
            openalpaca_storage::SESSION_ACTIVE,
            "the conversation it re-opened stays open"
        );
        assert_eq!(session_count(&db), 2, "no third conversation was opened");

        let announced = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|e| match e {
                openalpaca_core::events::SystemEvent::SessionChanged {
                    session_id,
                    status,
                    ..
                } => Some((session_id, status)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            announced,
            vec![(resumed.id.clone(), "active".to_string())],
            "one announcement, and it is the resume"
        );
    }

    /// A session with no project yet takes the header and is bound by it —
    /// a first binding, not a re-point.
    #[tokio::test]
    async fn a_named_session_with_no_project_takes_the_header() {
        let (_dir, db) = sessions_db();
        let bus = openalpaca_core::bus::EventBus::default();
        let unbound = openalpaca_storage::ConversationRepository::new(&db)
            .get_or_create_active_session("user1:gui", "gui", None)
            .expect("session");

        let two = project_dir();
        let workspace = resolve_turn_target(
            &db,
            &bus,
            "user1",
            Some(&unbound.id),
            false,
            Some(project_path(&two)),
        )
        .expect("accepted");
        assert_eq!(workspace, Some(project_path(&two)));

        let turn = persist_turn(&db, workspace.as_deref());
        assert_eq!(turn.session_id, unbound.id);
        assert!(!turn.project_switched);
    }

    /// The other half of R49: a request that names nothing keeps R48 — the
    /// header is the turn's project, and a different one opens a new session.
    #[tokio::test]
    async fn a_turn_naming_no_session_still_switches_project() {
        let (_dir, db) = sessions_db();
        let bus = openalpaca_core::bus::EventBus::default();
        let repo = openalpaca_storage::ConversationRepository::new(&db);

        let one = project_dir();
        let two = project_dir();
        let live = repo
            .create_session(
                "user1:gui",
                "gui",
                root(Some(&project_path(&one))).as_deref(),
                None,
            )
            .expect("session");

        let workspace =
            resolve_turn_target(&db, &bus, "user1", None, false, Some(project_path(&two)))
                .expect("accepted");
        assert_eq!(
            workspace,
            Some(project_path(&two)),
            "with nothing named, the header is the turn's project"
        );

        let turn = persist_turn(&db, workspace.as_deref());
        assert!(turn.project_switched, "R48 still fires");
        assert_ne!(turn.session_id, live.id);
        assert_eq!(session_count(&db), 2);
    }

    // ── The history routes retarget to a session ────────────────────

    #[test]
    fn history_defaults_to_the_lanes_active_session() {
        let (_dir, db) = sessions_db();
        let repo = openalpaca_storage::ConversationRepository::new(&db);

        // A lane that has never held a turn resolves to nothing — an empty
        // transcript, not an error.
        assert_eq!(
            resolve_history_session(&db, "user1:gui", None).expect("ok"),
            None
        );

        let first = repo
            .get_or_create_active_session("user1:gui", "gui", None)
            .expect("session");
        assert_eq!(
            resolve_history_session(&db, "user1:gui", None).expect("ok"),
            Some(first.id.clone())
        );

        // "New chat" moves the default; the old conversation is still
        // readable by naming it.
        let second = repo
            .create_session("user1:gui", "gui", None, None)
            .expect("session");
        assert_eq!(
            resolve_history_session(&db, "user1:gui", None).expect("ok"),
            Some(second.id)
        );
        assert_eq!(
            resolve_history_session(&db, "user1:gui", Some(&first.id)).expect("ok"),
            Some(first.id)
        );
    }

    #[tokio::test]
    async fn history_refuses_a_session_from_another_lane() {
        let (_dir, db) = sessions_db();
        let elsewhere = openalpaca_storage::ConversationRepository::new(&db)
            .get_or_create_active_session("user1:cli", "cli", None)
            .expect("session");

        let err =
            resolve_history_session(&db, "user1:gui", Some(&elsewhere.id)).expect_err("refused");
        let (status, body) = refusal(err).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "SESSION_NOT_FOUND");
    }
}
