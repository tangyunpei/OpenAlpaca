//! Chat route handlers
//!
//! POST   /v1/chat                     — Send a message (protected)
//! GET    /v1/chat/stream/:stream_id   — SSE stream (inline auth via ?token=)
//! GET    /v1/chat/history             — Get conversation history (protected)
//! DELETE /v1/chat/history             — Clear conversation history (protected)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::AppState;

// ── Request / Response types ────────────────────────────────────────

#[derive(Deserialize)]
pub struct ChatSendRequest {
    pub content: String,
}

#[derive(Serialize)]
pub struct ChatSendResponseBody {
    pub stream_id: String,
    pub lane_key: String,
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub lane_key: Option<String>,
}

#[derive(Deserialize)]
pub struct ConversationsQuery {
    pub source: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub struct ConversationsResponse {
    pub conversations: Vec<openalpaca_storage::Conversation>,
}

#[derive(Serialize)]
pub struct ConversationMessagesResponse {
    pub messages: Vec<openalpaca_storage::ConversationMessage>,
    pub total: i64,
}

#[derive(Serialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<openalpaca_storage::ConversationMessage>,
    pub total: i64,
}

#[derive(Serialize)]
pub struct ChatDeleteResponse {
    pub deleted: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

fn error_response(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
    (
        status,
        Json(ErrorResponse {
            error: ErrorDetail {
                code: code.to_string(),
                message: message.to_string(),
            },
        }),
    )
}

// ── POST /v1/chat ───────────────────────────────────────────────────

pub async fn send_chat_handler(
    State(state): State<Arc<AppState>>,
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

    let principal = "gui_user";

    match chat_service.send_message(body.content, principal) {
        Ok(resp) => {
            // Broadcast WS events
            state
                .event_broadcaster
                .chat_stream_started(&resp.stream_id, &resp.lane_key);

            Json(ChatSendResponseBody {
                stream_id: resp.stream_id,
                lane_key: resp.lane_key,
            })
            .into_response()
        }
        Err(e) => {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "GATEWAY_ERROR",
                &e.to_string(),
            )
            .into_response()
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

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
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
                openalpaca_core::chat::ChatStreamEvent::Done {
                    content,
                    model,
                    tokens_in,
                    tokens_out,
                    duration_ms,
                } => Event::default().event("done").data(
                    serde_json::json!({
                        "content": content,
                        "model": model,
                        "tokens_in": tokens_in,
                        "tokens_out": tokens_out,
                        "duration_ms": duration_ms
                    })
                    .to_string(),
                ),
                openalpaca_core::chat::ChatStreamEvent::Error { message } => Event::default()
                    .event("error")
                    .data(serde_json::json!({"message": message}).to_string()),
            };
            Some(Ok(sse_event))
        }
        Err(_) => None,
    })
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
    let lane_key = query.lane_key.as_deref().unwrap_or("gui_user:gui");

    match chat_service.get_history(lane_key, limit, offset) {
        Ok((messages, total)) => Json(ChatHistoryResponse { messages, total }).into_response(),
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

    let lane_key = "gui_user:gui";

    match chat_service.clear_history(lane_key) {
        Ok(deleted) => Json(ChatDeleteResponse { deleted }).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "GATEWAY_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── GET /v1/conversations ─────────────────────────────────────────

pub async fn list_conversations_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConversationsQuery>,
) -> impl IntoResponse {
    let repo = openalpaca_storage::ConversationRepository::new(&state.db);
    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);

    match repo.list_conversations(query.source.as_deref(), limit, offset) {
        Ok(conversations) => Json(ConversationsResponse { conversations }).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── GET /v1/conversations/{id}/messages ───────────────────────────

pub async fn get_conversation_messages_handler(
    Path(id): Path<String>,
    Query(query): Query<HistoryQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let repo = openalpaca_storage::ConversationRepository::new(&state.db);

    // Look up the conversation to get its lane_key
    let conv = match repo.get_conversation(&id) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                "Conversation not found",
            )
            .into_response();
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                &e.to_string(),
            )
            .into_response();
        }
    };

    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);

    match repo.list_by_lane(&conv.lane_key, limit, offset) {
        Ok(messages) => {
            let total = repo.count_by_lane(&conv.lane_key).unwrap_or(0);
            Json(ConversationMessagesResponse { messages, total }).into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}
