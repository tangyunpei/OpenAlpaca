//! WebSocket events endpoint
//!
//! GET /v1/events?token=xxx - Real-time event stream
//! Authentication via query parameter (browser WS limitation)

use axum::{
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use openalpaca_api::events::ServerEvent;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::events::EventBroadcaster;

/// Everything one open socket needs, and nothing else.
///
/// Extracted from [`AppState`] at the upgrade so the socket loop can be driven
/// without one: `AppState` is built in exactly one place (`main.rs`) and has no
/// test builder, and standing one up would drag in an orchestrator, a gateway
/// and a store root — none of which this loop reads.
#[derive(Clone)]
pub(crate) struct SocketDeps {
    pub(crate) broadcaster: EventBroadcaster,
    pub(crate) cancel: CancellationToken,
    pub(crate) instance_id: String,
}

/// How long the farewell may take before the socket is dropped regardless.
///
/// The farewell is a courtesy: a client that will not read it must not keep
/// this task writing into the shutdown window, so it is dropped instead.
const FAREWELL_TIMEOUT: Duration = Duration::from_millis(500);

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

    let deps = SocketDeps {
        broadcaster: state.event_broadcaster.clone(),
        cancel: state.cancel_token.clone(),
        instance_id: state.instance_id.clone(),
    };

    ws.on_upgrade(|socket| handle_socket(socket, deps))
}

/// The single frame a client is owed when the daemon goes away under it.
///
/// Pure, so the wire shape is provable without a socket.
fn shutdown_frame(grace_secs: u64, instance_id: &str) -> String {
    serde_json::to_string(&ServerEvent::DaemonShuttingDown {
        grace_secs,
        ts: chrono::Utc::now(),
        instance_id: instance_id.to_string(),
    })
    .unwrap_or_else(|_| {
        // Serializing three owned scalars cannot fail; if it somehow does, a
        // client is still better served by a frame it can parse than by none.
        format!(
            r#"{{"type":"daemon_shutting_down","grace_secs":{grace_secs},"instance_id":"{instance_id}"}}"#
        )
    })
}

/// Handle an individual WebSocket connection
///
/// Note: Heartbeat is generated at daemon level (not per-connection)
/// to avoid duplication when multiple clients are connected.
pub(crate) async fn handle_socket(socket: WebSocket, deps: SocketDeps) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to event broadcast
    let mut event_rx = deps.broadcaster.subscribe();

    tracing::info!("WebSocket client connected");

    loop {
        tokio::select! {
            // Forward broadcast events to this client
            event = event_rx.recv() => {
                // Bind the full Result: an `Ok(event) =` pattern would silently
                // disable this branch on Lagged, and since browsers rarely send
                // frames, select! would then block forever on receiver.next(),
                // freezing event delivery while the UI still shows "connected".
                let event = match event {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WebSocket event stream lagged; dropped {n} events");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
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

            // The daemon is shutting down. Neither arm above ends on its own
            // at shutdown — the broadcast sender lives as long as `AppState`,
            // and a client that is not sending frames leaves
            // `receiver.next()` pending — so without this arm the socket stayed
            // open and silent until the process exited under it, and the
            // client saw a dropped connection it could not tell from a crash.
            // (It never held `with_graceful_shutdown` open: hyper hands an
            // upgraded connection off and stops tracking it.)
            _ = deps.cancel.cancelled() => {
                let frame = shutdown_frame(
                    crate::FORCE_EXIT_GRACE.as_secs(),
                    &deps.instance_id,
                );
                // Bounded: a client that will not read must not hold the exit.
                let _ = tokio::time::timeout(FAREWELL_TIMEOUT, async {
                    let _ = sender.send(Message::Text(frame.into())).await;
                    let _ = sender
                        .send(Message::Close(Some(CloseFrame {
                            code: close_code::AWAY,
                            reason: "daemon_shutting_down".into(),
                        })))
                        .await;
                })
                .await;
                tracing::info!("WebSocket client released for shutdown");
                break;
            }
        }
    }

    tracing::info!("WebSocket client disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame is what a client parses, so its shape is asserted, not its
    /// Rust type: `type` is the discriminator the GUI's `onmessage` switches
    /// on, and `grace_secs` is the number the dialog quotes.
    #[test]
    fn shutdown_frame_carries_the_discriminator_grace_and_instance() {
        let json = shutdown_frame(10, "inst-42");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

        assert_eq!(value["type"], "daemon_shutting_down");
        assert_eq!(value["grace_secs"], 10);
        assert_eq!(value["instance_id"], "inst-42");
        assert!(
            value["ts"].as_str().is_some(),
            "a client orders frames by ts; it must be there"
        );
    }

    /// It round-trips as the enum it is, so the GUI's union and the Rust enum
    /// cannot drift on the field names.
    #[test]
    fn shutdown_frame_round_trips_as_the_server_event() {
        let json = shutdown_frame(10, "inst-42");
        let parsed: ServerEvent = serde_json::from_str(&json).expect("round trip");

        match parsed {
            ServerEvent::DaemonShuttingDown {
                grace_secs,
                instance_id,
                ..
            } => {
                assert_eq!(grace_secs, 10);
                assert_eq!(instance_id, "inst-42");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
