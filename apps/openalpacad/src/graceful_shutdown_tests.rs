//! Graceful shutdown with a live long-lived connection, driven for real.
//!
//! The daemon has exactly two response bodies that outlive a request: the
//! `/v1/events` WebSocket and the `/v1/chat/stream/{id}` SSE body. Each test
//! binds `127.0.0.1:0` inside `cargo test`, serves the real handler under
//! `with_graceful_shutdown` on a `CancellationToken` — the same shape as
//! `main.rs` — holds a live client on it, cancels, and asserts that
//! `serve(...)` **resolves within 2 s** and what the client was told.
//!
//! Resolution is the defect. If `serve` does not resolve, `main.rs`'s 10 s
//! watchdog calls `process::exit(1)`, which skips the cost flush, the
//! session-log flush, the MCP/plugin child sweep, the connector stop and the
//! discovery removal.
//!
//! What was measured when this was written (axum 0.8.9, hyper 1.11): the SSE
//! body without a cancel arm held `serve` for the whole window — that is the
//! defect these tests prove gone. The WebSocket did **not**: hyper resolves a
//! connection once it hands the upgraded IO off, so `serve` resolved in under
//! a millisecond even with a socket that never saw the cancel. That socket's
//! defect was silence — no frame, no close, until the process died — which is
//! what (a) and (b) below pin. (c) is kept for the socket too, because the
//! day an upgrade *is* tracked by the server, it becomes the defect.
//!
//! No `AppState`, no store root, no database, no daemon process
//! (collision ruling C11). Nothing here reads or sets `OPENALPACA_HOME_STORE`.

use std::time::Duration;

use axum::{
    Router,
    extract::{State, WebSocketUpgrade},
    response::{IntoResponse, sse::Sse},
    routing::get,
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Message, protocol::frame::coding::CloseCode};
use tokio_util::sync::CancellationToken;

use crate::events::EventBroadcaster;
use crate::routes::chat::{SHUTDOWN_SSE_MESSAGE, make_sse_stream};
use crate::routes::events::{SocketDeps, handle_socket};

/// The bound the defect is measured against. Far inside the daemon's 10 s
/// force-exit window, far outside anything a healthy shutdown needs.
const SERVE_MUST_RESOLVE_WITHIN: Duration = Duration::from_secs(2);

/// Every client read is bounded, so a regression fails instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Serve `app` on an OS-chosen loopback port with the daemon's shutdown shape:
/// `with_graceful_shutdown` on the one token, which is also what the handlers
/// select on.
async fn serve(
    app: Router,
    cancel: &CancellationToken,
) -> (std::net::SocketAddr, JoinHandle<std::io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let shutdown = cancel.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await
    });
    (addr, server)
}

/// Assert (c): the server future resolves, cleanly, inside the bound.
async fn assert_serve_resolves(server: JoinHandle<std::io::Result<()>>, what: &str) {
    let resolved = tokio::time::timeout(SERVE_MUST_RESOLVE_WITHIN, server).await;
    let joined = resolved.unwrap_or_else(|_| {
        panic!(
            "serve(...) did not resolve within {SERVE_MUST_RESOLVE_WITHIN:?} of the cancel \
             with {what} open — the daemon would sit out its watchdog and force-exit"
        )
    });
    joined
        .expect("the serve task panicked")
        .expect("serve returned an error");
}

// ── /v1/events ──────────────────────────────────────────────────────────────

/// The route, minus the token check `events_handler` does before it builds
/// the same `SocketDeps`.
async fn upgrade(ws: WebSocketUpgrade, State(deps): State<SocketDeps>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, deps))
}

#[tokio::test]
async fn graceful_shutdown_completes_with_a_live_events_socket() {
    let cancel = CancellationToken::new();
    // Held for the whole test, as `AppState` holds it for the daemon's life:
    // the broadcast sender never drops, so the socket's own arms never end.
    let broadcaster = EventBroadcaster::new(16, "test-instance".to_string(), None);
    let deps = SocketDeps {
        broadcaster: broadcaster.clone(),
        cancel: cancel.clone(),
        instance_id: "test-instance".to_string(),
    };
    let app = Router::new()
        .route("/v1/events", get(upgrade))
        .with_state(deps);
    let (addr, server) = serve(app, &cancel).await;

    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/events"))
        .await
        .expect("the WebSocket handshake completes");

    // Live, not merely connected: the loop has subscribed and is forwarding.
    // Retried because the upgrade callback may not have subscribed yet when
    // the handshake returns, and a heartbeat sent before it is simply lost.
    let mut live = false;
    for _ in 0..20 {
        broadcaster.heartbeat();
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(100), client.next()).await
        {
            let frame: serde_json::Value = serde_json::from_str(&text).expect("a JSON frame");
            assert_eq!(frame["type"], "heartbeat");
            live = true;
            break;
        }
    }
    assert!(live, "the socket never forwarded a broadcast frame");

    cancel.cancel();

    // (c) — first, and without touching the client, so nothing the client
    // does (a close reply, a disconnect) can be what lets the server go.
    assert_serve_resolves(server, "a live /v1/events socket").await;

    // (a) the farewell frame, with the daemon's own grace.
    let frame = tokio::time::timeout(READ_TIMEOUT, client.next())
        .await
        .expect("the farewell frame arrives")
        .expect("the socket is still readable")
        .expect("a well-formed frame");
    let Message::Text(text) = frame else {
        panic!("expected the daemon_shutting_down text frame, got {frame:?}");
    };
    let frame: serde_json::Value = serde_json::from_str(&text).expect("a JSON frame");
    assert_eq!(frame["type"], "daemon_shutting_down");
    assert_eq!(frame["grace_secs"], crate::FORCE_EXIT_GRACE.as_secs());
    assert_eq!(frame["instance_id"], "test-instance");

    // (b) then the protocol's own close: 1001, going away.
    let close = tokio::time::timeout(READ_TIMEOUT, client.next())
        .await
        .expect("the close frame arrives")
        .expect("the socket is still readable")
        .expect("a well-formed frame");
    let Message::Close(Some(close)) = close else {
        panic!("expected a close frame carrying a code, got {close:?}");
    };
    assert_eq!(close.code, CloseCode::Away, "1001 going away");
    assert_eq!(close.reason.as_str(), "daemon_shutting_down");
}

// ── /v1/chat/stream/{id} ────────────────────────────────────────────────────

/// Read from `stream` until the peer closes it, bounded by [`READ_TIMEOUT`].
async fn read_to_close(stream: &mut TcpStream) -> String {
    let mut body = Vec::new();
    tokio::time::timeout(READ_TIMEOUT, stream.read_to_end(&mut body))
        .await
        .expect("the connection closes after the farewell")
        .expect("the connection reads cleanly");
    String::from_utf8_lossy(&body).into_owned()
}

/// Read until `needle` has arrived, bounded by [`READ_TIMEOUT`].
async fn read_until(stream: &mut TcpStream, needle: &str) -> String {
    let mut seen = String::new();
    let mut buf = [0u8; 4096];
    tokio::time::timeout(READ_TIMEOUT, async {
        while !seen.contains(needle) {
            let n = stream.read(&mut buf).await.expect("read");
            assert!(n > 0, "the connection closed before {needle:?}: {seen:?}");
            seen.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{needle:?} never arrived: {seen:?}"));
    seen
}

#[tokio::test]
async fn graceful_shutdown_completes_with_a_live_chat_stream() {
    let cancel = CancellationToken::new();
    // Held for the whole test, as `ChatStreamManager` holds a turn's sender
    // until its GC — which stops on the same token — would drop it.
    let (tx, _) = tokio::sync::broadcast::channel::<openalpaca_core::chat::ChatStreamEvent>(16);
    let route_tx = tx.clone();
    let route_cancel = cancel.clone();
    let app = Router::new().route(
        "/v1/chat/stream",
        get(move || {
            let rx = route_tx.subscribe();
            let cancel = route_cancel.clone();
            async move { Sse::new(make_sse_stream(rx, cancel)) }
        }),
    );
    let (addr, server) = serve(app, &cancel).await;

    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(format!("GET /v1/chat/stream HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())
        .await
        .expect("send the request");
    read_until(&mut client, "\r\n\r\n").await;

    // Live, not merely open: a frame the turn sends reaches the client.
    tx.send(openalpaca_core::chat::ChatStreamEvent::Delta {
        content: "half an answer".to_string(),
    })
    .expect("the route subscribed");
    read_until(&mut client, "half an answer").await;

    cancel.cancel();

    // (c) — the defect: before the cancel arm, this body never ended and
    // `serve` sat out the whole window.
    assert_serve_resolves(server, "a live chat stream").await;

    // The client was told why, in the one terminal frame both readers know.
    let rest = read_to_close(&mut client).await;
    assert!(
        rest.contains("event: error"),
        "no farewell error frame: {rest:?}"
    );
    let message = serde_json::json!({ "message": SHUTDOWN_SSE_MESSAGE }).to_string();
    assert!(
        rest.contains(&message),
        "the farewell is SHUTDOWN_SSE_MESSAGE: {rest:?}"
    );

    drop(tx);
}
