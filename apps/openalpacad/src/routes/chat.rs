//! Chat route handlers
//!
//! POST   /v1/chat                     — Send a message (protected)
//! GET    /v1/chat/stream/:stream_id   — SSE stream (inline auth via ?token=)
//! GET    /v1/chat/history             — Get conversation history (protected)
//! DELETE /v1/chat/history             — Clear conversation history (protected)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse,
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

    let workspace_path = headers
        .get("x-workspace-path")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match chat_service.send_message(body.content, body.attachments, principal, workspace_path) {
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
            })
            .into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "GATEWAY_ERROR",
            &e.to_string(),
        )
        .into_response(),
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
                openalpaca_core::chat::ChatStreamEvent::Done {
                    content,
                    model,
                    tokens_in,
                    tokens_out,
                    duration_ms,
                    attachments_used,
                    citations,
                    artifacts,
                } => {
                    let mut data = serde_json::json!({
                        "content": content,
                        "model": model,
                        "tokens_in": tokens_in,
                        "tokens_out": tokens_out,
                        "duration_ms": duration_ms
                    });
                    if let Some(att) = attachments_used {
                        data["attachments_used"] = serde_json::json!(att);
                    }
                    if let Some(cit) = citations {
                        data["citations"] = serde_json::json!(cit);
                    }
                    if let Some(art) = artifacts {
                        data["artifacts"] = serde_json::json!(art);
                    }
                    Event::default().event("done").data(data.to_string())
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

    match chat_service.get_history(lane_key, limit, offset) {
        Ok((messages, total)) => Json(ChatHistoryResponse {
            messages,
            total,
            lane_key: lane_key.to_string(),
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

    match chat_service.clear_history(lane_key) {
        Ok(deleted) => {
            // Also clear the conversation summary
            let conv_repo = openalpaca_storage::ConversationRepository::new(&state.db);
            let _ = conv_repo.clear_summary(lane_key);
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

// ── GET /v1/conversations ─────────────────────────────────────────

pub async fn list_conversations_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConversationsQuery>,
) -> impl IntoResponse {
    let repo = openalpaca_storage::ConversationRepository::new(&state.db);
    let limit = query.limit.unwrap_or(50);
    let offset = query.offset.unwrap_or(0);

    match repo.list_conversations_for_owner(
        &state.local_user_id,
        query.source.as_deref(),
        limit,
        offset,
    ) {
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
            return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "Conversation not found")
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

    // Verify the caller owns this conversation
    if !is_lane_owned_by(&conv.lane_key, &state.local_user_id) {
        return error_response(StatusCode::FORBIDDEN, "FORBIDDEN", "Access denied").into_response();
    }

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
            approval_scope: None,
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
}
