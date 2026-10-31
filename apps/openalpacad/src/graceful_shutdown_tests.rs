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
//! session-log flush, the MCP/plugin child sweep and the connector stop (the
//! discovery file it removes itself first, since N-DAEMON-1).
//!
//! The tests after them pin N-DAEMON-1's half of the same window: a stage of
//! the tail after the server — a connector's stop, the wake manager's — ends
//! at its deadline instead of holding the rest, and that deadline is
//! `shutdown::TAIL_RESERVE` before the force-exit instant, so what follows it
//! runs before the watchdog; and the watchdog removes the discovery file it is
//! given before it exits. (The connector manager's own loop is pinned in
//! `managers/connector.rs`.)
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

// ── The shutdown tail's stages and the forced exit (N-DAEMON-1) ─────────────

/// A window far below the daemon's 10 s, so a stage that ignores it fails the
/// test instead of slowing it down.
const STAGE_WINDOW: Duration = Duration::from_millis(200);

/// A connector whose shutdown never returns — Telegram's waits for its
/// dispatcher to finish, and nothing bounded that wait — no longer holds the
/// shutdown: its stage ends at the window, so the wake manager and the
/// discovery file after it are still reached. `ConnectorManager::shutdown_all`
/// runs every handle, and `main.rs` the wake manager, through this one helper;
/// the helper is driven here with a future that never resolves, and the
/// manager's loop with a stop that never returns in `managers/connector.rs`.
#[tokio::test]
async fn graceful_shutdown_a_stage_that_never_returns_ends_at_the_window() {
    let started = std::time::Instant::now();
    let deadline = started + STAGE_WINDOW;
    let finished = tokio::time::timeout(
        SERVE_MUST_RESOLVE_WITHIN,
        crate::shutdown::within(
            "a connector that never stops",
            deadline,
            std::future::pending::<()>(),
        ),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "a stage that never returns held the shutdown past its {STAGE_WINDOW:?} window — \
             the daemon would sit out its watchdog and force-exit"
        )
    });
    assert_eq!(
        finished, None,
        "a stage cut at the window reports that it did not finish"
    );
    assert!(
        started.elapsed() >= STAGE_WINDOW / 2,
        "the stage was given its window, not cut at once: {:?}",
        started.elapsed()
    );
}

/// Past the window a stage is still started once: a connector whose stop is a
/// cancel — Discord's, iMessage's, a plugin connector's — is told to stop even
/// when the stages before it spent the whole window.
#[tokio::test]
async fn graceful_shutdown_a_stage_reached_after_the_window_is_still_run_once() {
    let deadline = std::time::Instant::now();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let token = CancellationToken::new();
    let stop = token.clone();
    let finished =
        crate::shutdown::within("a connector stopped by its token", deadline, async move {
            stop.cancel();
        })
        .await;
    assert_eq!(
        finished,
        Some(()),
        "a stage that is done at its first poll is done"
    );
    assert!(token.is_cancelled(), "the connector was told to stop");
}

/// The force-exit watchdog removes the discovery file before
/// `process::exit(1)`, so no client reads the port and token of a daemon that
/// is gone. The watchdog is handed the path the boot wrote; this drives the
/// same function with a temporary one — no store, no environment.
#[test]
fn graceful_shutdown_the_forced_exit_removes_the_discovery_file_it_is_given() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("discovery.json");
    std::fs::write(&path, b"{\"pid\": 1}").expect("write the file");

    crate::shutdown::before_forced_exit(&path);
    assert!(
        !path.exists(),
        "the forced exit left {} behind",
        path.display()
    );

    // Already gone — removed by the shutdown tail a moment before the
    // watchdog fired: nothing to do, and nothing panics.
    crate::shutdown::before_forced_exit(&path);
    assert!(!path.exists());
}

/// The connector and wake stages end [`TAIL_RESERVE`] before the force-exit
/// instant, not at it: a stop that never returns is given up while the tail
/// still has time for what follows it — the handles after it, the wake
/// manager's stop, the discovery file's removal — instead of meeting the
/// watchdog's timer on the same tick, where either could run first.
///
/// [`TAIL_RESERVE`]: crate::shutdown::TAIL_RESERVE
#[tokio::test]
async fn graceful_shutdown_a_hung_stage_ends_before_the_watchdog_fires() {
    use crate::shutdown::TAIL_RESERVE;

    let force_exit_at = std::sync::OnceLock::new();
    let watchdog_fires = std::time::Instant::now() + TAIL_RESERVE + STAGE_WINDOW;
    force_exit_at
        .set(watchdog_fires)
        .expect("the instant is published once");

    let ends = crate::shutdown::stage_deadline(&force_exit_at);
    assert_eq!(
        ends,
        watchdog_fires - TAIL_RESERVE,
        "the stages end at the force-exit instant itself, in a race with the watchdog"
    );

    let finished = tokio::time::timeout(
        SERVE_MUST_RESOLVE_WITHIN,
        crate::shutdown::within(
            "a connector that never stops",
            ends,
            std::future::pending::<()>(),
        ),
    )
    .await
    .expect("the stage ended at its deadline");
    assert_eq!(finished, None);
    let left = watchdog_fires.saturating_duration_since(std::time::Instant::now());
    assert!(
        left >= TAIL_RESERVE / 2,
        "a hung stage left the tail {left:?} before the watchdog fires"
    );
}

/// The watchdog itself — the task `main.rs` spawns — publishes its instant
/// once the token is cancelled, and when the tail has not returned by then
/// removes the discovery file it was handed **before** it exits. The exit is
/// a closure here, `std::process::exit` in the daemon; it reports whether the
/// file was still there when it was called.
#[tokio::test]
async fn graceful_shutdown_the_watchdog_removes_the_discovery_file_before_it_exits() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("discovery.json");
    std::fs::write(&path, b"{\"pid\": 1}").expect("write the file");
    let cancel = CancellationToken::new();
    let (exited, exit_seen) = tokio::sync::oneshot::channel();
    let probe = path.clone();
    let force_exit_at = crate::shutdown::spawn_force_exit_watchdog(
        cancel.clone(),
        path.clone(),
        STAGE_WINDOW,
        move |code| {
            let _ = exited.send((code, probe.exists()));
        },
    );
    tokio::task::yield_now().await;
    assert!(
        force_exit_at.get().is_none(),
        "nothing is published before the cancellation"
    );

    let cancelled = std::time::Instant::now();
    cancel.cancel();
    let (code, file_was_there) = tokio::time::timeout(SERVE_MUST_RESOLVE_WITHIN, exit_seen)
        .await
        .expect("the watchdog never fired")
        .expect("the watchdog called its exit");
    assert_eq!(code, 1, "a forced exit is exit code 1");
    assert!(
        !file_was_there,
        "the watchdog exited with {} still there",
        path.display()
    );
    let published = *force_exit_at.get().expect("the instant was published");
    assert!(
        published >= cancelled + STAGE_WINDOW,
        "the published instant is the window from the cancellation"
    );
}
