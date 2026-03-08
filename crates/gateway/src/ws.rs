use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};

use crate::protocol::connect::{
    AuthResult, ConnectParams, Features, HelloOk, PROTOCOL_VERSION, ServerInfo,
};
use crate::protocol::frames::GatewayFrame;
use crate::rpc::RpcContext;
use crate::state::GatewayState;

/// WebSocket upgrade handler.
pub async fn ws_upgrade_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state, addr.ip()))
}

async fn handle_ws_connection(
    socket: WebSocket,
    state: Arc<GatewayState>,
    remote_ip: std::net::IpAddr,
) {
    let (mut sender, mut receiver) = socket.split();
    let shutdown = state.app_ctx.shutdown.clone();

    // Step 1: Wait for connect frame (with timeout to prevent DoS)
    let connect_params = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        wait_for_connect(&mut receiver),
    )
    .await
    {
        Ok(Some(params)) => params,
        Ok(None) => {
            tracing::warn!("ws: client disconnected before connect frame");
            return;
        }
        Err(_) => {
            tracing::warn!("ws: connect timeout — closing connection");
            return;
        }
    };

    // Step 1.5: Negotiate protocol version
    let negotiated_version = match crate::protocol::connect::negotiate_version(
        connect_params.min_protocol,
        connect_params.max_protocol,
    ) {
        Some(v) => v,
        None => {
            let err = crate::protocol::error::ErrorShape::new(
                "PROTOCOL_MISMATCH",
                format!(
                    "incompatible protocol: client [{}, {}], server {}",
                    connect_params.min_protocol, connect_params.max_protocol, PROTOCOL_VERSION
                ),
            );
            let frame = GatewayFrame::err_response("connect".into(), err);
            let _ = send_frame(&mut sender, &frame).await;
            return;
        }
    };

    // Step 2: Authenticate
    let auth_result = if let Some(ref auth_params) = connect_params.auth {
        match state
            .auth
            .verify(auth_params.token.as_deref(), remote_ip)
            .await
        {
            Ok(method) => Some(AuthResult {
                method: format!("{method:?}"),
                authenticated: true,
            }),
            Err(err) => {
                let frame = GatewayFrame::err_response("connect".into(), err);
                let _ = send_frame(&mut sender, &frame).await;
                return;
            }
        }
    } else {
        // No auth params provided — check if auth is required
        match state.auth.verify(None, remote_ip).await {
            Ok(_) => None,
            Err(err) => {
                let frame = GatewayFrame::err_response("connect".into(), err);
                let _ = send_frame(&mut sender, &frame).await;
                return;
            }
        }
    };

    // Step 3: Send hello-ok
    let rpc_methods = state.rpc_registry.list_methods();
    let hello = HelloOk {
        protocol: negotiated_version,
        server: ServerInfo {
            name: "openalpaca".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        features: Features {
            channels: vec![],
            rpc_methods,
        },
        auth: auth_result,
    };

    let hello_frame = GatewayFrame::ok_response(
        "connect".into(),
        serde_json::to_value(&hello).unwrap_or_default(),
    );
    if send_frame(&mut sender, &hello_frame).await.is_err() {
        return;
    }

    tracing::info!(
        client = %connect_params.client.name,
        version = %connect_params.client.version,
        "ws: client connected"
    );

    // Step 4: Subscribe to broadcast events
    let mut broadcast_rx = state.broadcast_tx.subscribe();

    // Step 5: Request/response loop
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("ws: shutdown signal received");
                break;
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_text_message(&text, &state, &mut sender).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::info!("ws: client disconnected");
                        break;
                    }
                    Some(Ok(_)) => {} // ignore binary, ping, pong
                    Some(Err(e)) => {
                        tracing::warn!("ws: receive error: {e}");
                        break;
                    }
                }
            }
            event = broadcast_rx.recv() => {
                if let Ok(event) = event {
                    let frame = broadcast_event_to_frame(&event);
                    if send_frame(&mut sender, &frame).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

async fn wait_for_connect(
    receiver: &mut (impl StreamExt<Item = Result<Message, axum::Error>> + Unpin),
) -> Option<ConnectParams> {
    while let Some(msg) = receiver.next().await {
        if let Ok(Message::Text(text)) = msg
            && let Ok(params) = serde_json::from_str::<ConnectParams>(&text)
        {
            return Some(params);
        }
    }
    None
}

async fn handle_text_message(
    text: &str,
    state: &Arc<GatewayState>,
    sender: &mut (impl SinkExt<Message> + Unpin),
) {
    let frame = match serde_json::from_str::<GatewayFrame>(text) {
        Ok(frame) => frame,
        Err(e) => {
            tracing::warn!("ws: invalid frame: {e}");
            return;
        }
    };

    if let GatewayFrame::Request { id, method, params } = frame {
        let ctx = RpcContext {
            state: state.clone(),
        };
        let params = params.unwrap_or(serde_json::Value::Null);
        let response = match state.rpc_registry.dispatch(&method, params, &ctx).await {
            Ok(payload) => GatewayFrame::ok_response(id, payload),
            Err(error) => GatewayFrame::err_response(id, error),
        };
        if send_frame(sender, &response).await.is_err() {
            tracing::debug!("ws: failed to send RPC response (client likely disconnected)");
        }
    }
}

async fn send_frame(
    sender: &mut (impl SinkExt<Message> + Unpin),
    frame: &GatewayFrame,
) -> Result<(), ()> {
    let json = serde_json::to_string(frame).map_err(|_| ())?;
    sender
        .send(Message::Text(json.into()))
        .await
        .map_err(|_| ())
}

fn broadcast_event_to_frame(event: &crate::state::BroadcastEvent) -> GatewayFrame {
    match event {
        crate::state::BroadcastEvent::ConfigChanged { hash } => GatewayFrame::Event {
            event: "config.changed".into(),
            payload: Some(serde_json::json!({ "hash": hash })),
        },
        crate::state::BroadcastEvent::ChannelStatusChanged { channel_id } => GatewayFrame::Event {
            event: "channel.status_changed".into(),
            payload: Some(serde_json::json!({ "channel_id": channel_id })),
        },
    }
}
