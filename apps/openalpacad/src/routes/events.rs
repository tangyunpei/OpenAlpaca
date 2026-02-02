//! WebSocket events endpoint
//!
//! GET /v1/events?token=xxx - Real-time event stream
//! Authentication via query parameter (browser WS limitation)

use axum::{
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, sync::Arc};

use crate::AppState;

/// Handle WebSocket upgrade for /v1/events
///
/// Token is validated from query parameter since browsers can't send
/// custom headers on WebSocket connections.
pub async fn events_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Validate token from query parameter
    let token = params.get("token").map(|s| s.as_str()).unwrap_or("");
    if token != state.token {
        return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
    }

    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Handle an individual WebSocket connection
///
/// Note: Heartbeat is generated at daemon level (not per-connection)
/// to avoid duplication when multiple clients are connected.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to event broadcast
    let mut event_rx = state.event_broadcaster.subscribe();

    tracing::info!("WebSocket client connected");

    loop {
        tokio::select! {
            // Forward broadcast events to this client
            Ok(event) = event_rx.recv() => {
                let msg = match serde_json::to_string(&event) {
                    Ok(json) => json,
                    Err(e) => {
                        tracing::error!("Failed to serialize event: {e}");
                        continue;
                    }
                };

                if sender.send(Message::Text(msg.into())).await.is_err() {
                    // Client disconnected
                    break;
                }
            }

            // Handle incoming messages from client (ping/pong, close)
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Ping(data))) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Err(e)) => {
                        tracing::warn!("WebSocket error: {e}");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    tracing::info!("WebSocket client disconnected");
}
