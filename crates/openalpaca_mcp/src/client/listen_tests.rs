//! P19b: a `2026-07-28` session subscribes to its server's list changes
//! (D5 (a), A1-12).
//!
//! A `2026-07-28` server announces a list change only on a
//! `subscriptions/listen` stream, so each server here is a modern one: it
//! answers the `server/discover` probe and serves the listen stream as its
//! test needs. Most run in-process over one half of a duplex pipe; the last
//! three over a loopback streamable-HTTP stub, one of whose listen streams
//! outlives the idle bound fivefold — the stand-in, as in N-MCP-2's tests,
//! for the 30 s total deadline that once cut every HTTP stream
//! (`transport/http_tests.rs`).

use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use rmcp::model::{
    DiscoverResult, ErrorData, GetMeta, ListToolsResult, RequestId, ServerCapabilities,
    ServerJsonRpcMessage, ServerNotification, ServerResult, SubscriptionFilter,
    SubscriptionsAcknowledgedNotification, SubscriptionsAcknowledgedNotificationParams,
    SubscriptionsListenResult, Tool, ToolListChangedNotification,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ============================================================================
// The messages a modern server sends
// ============================================================================

/// The probe's answer: `2026-07-28`, with tools that announce their changes.
fn discover_answer(id: RequestId) -> ServerJsonRpcMessage {
    let capabilities = ServerCapabilities::builder()
        .enable_tools()
        .enable_tool_list_changed()
        .build();
    let result = DiscoverResult::new(vec![ProtocolVersion::LATEST], capabilities)
        .with_server_info(Implementation::new("listen-stub", "1.0.0"));
    ServerJsonRpcMessage::response(ServerResult::DiscoverResult(result), id)
}

/// A `tools/list` answer the client may cache for a minute (SEP-2549's
/// `ttlMs`), as a server that rarely changes its tools would send it.
fn tools_answer(id: RequestId, names: &[&str]) -> ServerJsonRpcMessage {
    let schema = Arc::new(
        serde_json::json!({ "type": "object" })
            .as_object()
            .expect("an object schema")
            .clone(),
    );
    let tools = names
        .iter()
        .map(|name| Tool::new(name.to_string(), "a stub tool", Arc::clone(&schema)))
        .collect();
    let result = ListToolsResult::with_all_items(tools).with_ttl_ms(60_000);
    ServerJsonRpcMessage::response(ServerResult::ListToolsResult(result), id)
}

/// The acknowledgment of the listen request `listen`, granting `filter`.
fn acknowledgment(listen: &RequestId, filter: SubscriptionFilter) -> ServerJsonRpcMessage {
    let mut ack = ServerNotification::SubscriptionsAcknowledgedNotification(
        SubscriptionsAcknowledgedNotification::new(
            SubscriptionsAcknowledgedNotificationParams::new(filter),
        ),
    );
    ack.get_meta_mut().set_subscription_id(listen.clone());
    ServerJsonRpcMessage::notification(ack)
}

/// `notifications/tools/list_changed` on the stream of `listen`.
fn tool_list_changed(listen: &RequestId) -> ServerJsonRpcMessage {
    let mut changed =
        ServerNotification::ToolListChangedNotification(ToolListChangedNotification {
            method: Default::default(),
            extensions: Default::default(),
        });
    changed.get_meta_mut().set_subscription_id(listen.clone());
    ServerJsonRpcMessage::notification(changed)
}

/// The listen request's final result: the server ends the stream while the
/// connection lives, as rmcp's own server does
/// (`rmcp-3.5.1/src/handler/server.rs:179-181`).
fn listen_complete(listen: &RequestId) -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::response(
        ServerResult::SubscriptionsListenResult(SubscriptionsListenResult::complete(
            listen.clone(),
        )),
        listen.clone(),
    )
}

/// An error for the listen request `listen`, after its acknowledgment: the
/// stream fails while the connection lives.
fn listen_failed(listen: &RequestId) -> ServerJsonRpcMessage {
    ServerJsonRpcMessage::error(
        ErrorData::internal_error("the stub's stream failed", None),
        Some(listen.clone()),
    )
}

// ============================================================================
// The in-process server
// ============================================================================

/// What [`modern_server`] does on each `subscriptions/listen`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnListen {
    /// Acknowledge, then announce one tool-list change on the stream, which
    /// stays open.
    AckThenChange,
    /// Acknowledge and announce nothing.
    AckOnly,
    /// Add a tool once the client has listed the tools — announced nowhere,
    /// since no stream is acknowledged yet (A1-12's window) — and only then
    /// acknowledge.
    ChangeBeforeAck,
    /// Acknowledge, then end the stream with its final result; the next listen
    /// is acknowledged and announces one change.
    EndFirstThenChange,
    /// Acknowledge and end the stream, every time.
    EndEvery,
    /// Acknowledge, then answer the listen with an error: a failed stream.
    AckThenFail,
}

/// What a [`modern_server`] saw.
#[derive(Default)]
struct Seen {
    listens: AtomicUsize,
    lists: AtomicUsize,
}

/// A `2026-07-28` server over one half of a duplex pipe, declaring
/// `tools.listChanged`: it answers the probe, `tools/list` (`echo`, and
/// `added` once [`OnListen::ChangeBeforeAck`] has added it) and
/// `subscriptions/listen` as `on_listen` says.
async fn modern_server(stream: tokio::io::DuplexStream, on_listen: OnListen, seen: Arc<Seen>) {
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut lines = BufReader::new(read_half).lines();
    let mut tools = vec!["echo"];
    let mut listed = false;
    // `ChangeBeforeAck`'s listen, held until the client has listed the tools.
    let mut held: Option<(RequestId, SubscriptionFilter)> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<rmcp::model::ClientJsonRpcMessage>(&line) else {
            continue;
        };
        let Some((request, id)) = msg.into_request() else {
            continue; // a notification: a cancelled subscription, say
        };
        let mut out = Vec::new();
        match request {
            ClientRequest::DiscoverRequest(_) => out.push(discover_answer(id)),
            ClientRequest::ListToolsRequest(_) => {
                seen.lists.fetch_add(1, Ordering::SeqCst);
                out.push(tools_answer(id, &tools));
                listed = true;
                if let Some((listen, filter)) = held.take() {
                    tools.push("added");
                    out.push(acknowledgment(&listen, filter));
                }
            }
            ClientRequest::SubscriptionsListenRequest(listen) => {
                let nth = seen.listens.fetch_add(1, Ordering::SeqCst) + 1;
                let filter = listen.params.notifications;
                match on_listen {
                    OnListen::ChangeBeforeAck if !listed => held = Some((id, filter)),
                    OnListen::ChangeBeforeAck => {
                        tools.push("added");
                        out.push(acknowledgment(&id, filter));
                    }
                    OnListen::AckOnly => out.push(acknowledgment(&id, filter)),
                    OnListen::AckThenChange => {
                        out.push(acknowledgment(&id, filter));
                        out.push(tool_list_changed(&id));
                    }
                    OnListen::EndFirstThenChange if nth > 1 => {
                        out.push(acknowledgment(&id, filter));
                        out.push(tool_list_changed(&id));
                    }
                    OnListen::EndFirstThenChange | OnListen::EndEvery => {
                        out.push(acknowledgment(&id, filter));
                        out.push(listen_complete(&id));
                    }
                    OnListen::AckThenFail => {
                        out.push(acknowledgment(&id, filter));
                        out.push(listen_failed(&id));
                    }
                }
            }
            _ => continue,
        }
        for reply in out {
            let mut text = serde_json::to_string(&reply).expect("serialize the stub's reply");
            text.push('\n');
            if write_half.write_all(text.as_bytes()).await.is_err() {
                return;
            }
        }
    }
}

/// The handler a connection is served with, forwarding onto `tx`.
fn handler(tx: UnboundedSender<ServerChange>) -> NotifyingHandler {
    NotifyingHandler {
        server_name: "listen".into(),
        tx: Some(tx),
        client_info: Implementation::new("openalpaca-mcp", "0.0.0-test"),
    }
}

/// A modern session over [`modern_server`]: the session, its change receiver
/// and what the server saw.
async fn modern_session(
    on_listen: OnListen,
) -> (
    RunningService<RoleClient, NotifyingHandler>,
    UnboundedReceiver<ServerChange>,
    Arc<Seen>,
) {
    let (tx, rx) = unbounded_channel();
    let seen = Arc::new(Seen::default());
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(modern_server(server_end, on_listen, Arc::clone(&seen)));
    let running = serve(handler(tx), client_end, lifecycle(ProtocolMode::Auto, None))
        .await
        .expect("the modern handshake");
    assert_eq!(
        running
            .peer_info()
            .map(|info| info.protocol_version.to_string())
            .as_deref(),
        Some("2026-07-28"),
        "the stub is served in the 2026-07-28 lifecycle"
    );
    (running, rx, seen)
}

/// The next change on `rx`, or `None` when none arrives within `within`.
async fn next_change(
    rx: &mut UnboundedReceiver<ServerChange>,
    within: Duration,
) -> Option<ServerChange> {
    tokio::time::timeout(within, rx.recv()).await.ok().flatten()
}

/// The names `running`'s server lists now, through rmcp's `list_all_tools` —
/// the call the supervisor's refresh makes (`McpClient::list_tools_once`).
async fn listed_names(running: &RunningService<RoleClient, NotifyingHandler>) -> Vec<String> {
    running
        .peer()
        .list_all_tools()
        .await
        .expect("the server lists its tools")
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

/// Collects the message of every `WARN` (and `ERROR`) event on the thread it
/// is the default for. The tests that read it run on tokio's current-thread
/// runtime, so the listener task they spawn logs on that thread too.
#[derive(Clone, Default)]
struct Warnings(Arc<std::sync::Mutex<Vec<String>>>);

impl Warnings {
    fn texts(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl tracing::Subscriber for Warnings {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::WARN
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Message<'a>(&'a mut String);
        impl tracing::field::Visit for Message<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0.push_str(&format!("{value:?}"));
                }
            }
        }
        let mut text = String::new();
        event.record(&mut Message(&mut text));
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(text);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

// ============================================================================
// The subscription
// ============================================================================

/// A `2026-07-28` session hears its server's tool-list change: [`serve`] opens
/// the `subscriptions/listen` stream the change arrives on, and the change
/// reaches the client's change channel. The channel still ends with the
/// session — the stream holds no sender of its own.
#[tokio::test]
async fn a_modern_session_hears_its_list_changes_on_a_subscription() {
    let (mut running, mut rx, _seen) = modern_session(OnListen::AckThenChange).await;
    // Two: the acknowledgment's own refresh (A1-12), then the change.
    for _ in 0..2 {
        assert_eq!(
            next_change(&mut rx, Duration::from_secs(5)).await,
            Some(ServerChange::ToolList),
            "the change arrives on the subscription"
        );
    }

    // As `disconnect` does: close the service, then drop it — and with it the
    // handler's sender, the last strong one.
    running
        .close_with_timeout(CLOSE_TIMEOUT)
        .await
        .expect("close the session");
    drop(running);
    let after = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the channel ends with the session");
    assert_eq!(after, None);
}

/// A1-12: the acknowledgment asks for exactly one `tools/list` refresh.
#[tokio::test]
async fn an_acknowledged_subscription_asks_for_one_tool_list_refresh() {
    let (running, mut rx, _seen) = modern_session(OnListen::AckOnly).await;
    assert_eq!(
        next_change(&mut rx, Duration::from_secs(5)).await,
        Some(ServerChange::ToolList),
        "the acknowledgment asks for a refresh"
    );
    assert_eq!(
        next_change(&mut rx, Duration::from_millis(300)).await,
        None,
        "one refresh, not more"
    );
    drop(running);
}

/// A1-12, the window it closes: a tool the server adds after the client has
/// listed its tools and before it acknowledges the subscription is announced
/// on no stream — and still arrives, since the acknowledgment's refresh reads
/// the list anew, not the copy rmcp cached under the server's `ttlMs`.
#[tokio::test]
async fn a_tool_the_server_adds_before_acknowledging_arrives_with_the_acknowledgment() {
    let (running, mut rx, seen) = modern_session(OnListen::ChangeBeforeAck).await;
    assert_eq!(
        listed_names(&running).await,
        ["echo"],
        "the connect's list, before the change"
    );

    assert_eq!(
        next_change(&mut rx, Duration::from_secs(5)).await,
        Some(ServerChange::ToolList),
        "the acknowledgment asks for a refresh"
    );
    assert_eq!(
        listed_names(&running).await,
        ["echo", "added"],
        "the refresh reads the server's list, not the cached one"
    );
    assert_eq!(
        seen.lists.load(Ordering::SeqCst),
        2,
        "the server was asked twice"
    );
    drop(running);
}

/// D5 (a): a stream the server ends while the connection lives is opened once
/// more, and a change announced on the new one arrives.
#[tokio::test]
async fn a_subscription_the_server_ends_once_is_reopened_and_a_later_change_arrives() {
    let (running, mut rx, seen) = modern_session(OnListen::EndFirstThenChange).await;
    let mut changes = Vec::new();
    while changes.len() < 3 {
        match next_change(&mut rx, Duration::from_secs(5)).await {
            Some(change) => changes.push(change),
            None => break,
        }
    }
    assert_eq!(
        seen.listens.load(Ordering::SeqCst),
        2,
        "the ended stream was reopened once"
    );
    assert_eq!(
        changes,
        vec![ServerChange::ToolList; 3],
        "a refresh per acknowledgment, then the change on the reopened stream"
    );
    drop(running);
}

/// D5 (a): a second end in the same connection is not reopened again — the
/// listener warns instead.
#[tokio::test]
async fn a_subscription_ended_twice_warns_and_is_not_reopened_a_third_time() {
    let warnings = Warnings::default();
    let _default = tracing::subscriber::set_default(warnings.clone());

    let (running, _rx, seen) = modern_session(OnListen::EndEvery).await;
    let reopened = async {
        while seen.listens.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), reopened)
        .await
        .expect("the first end is reopened");
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        seen.listens.load(Ordering::SeqCst),
        2,
        "no third subscription"
    );
    let texts = warnings.texts();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("ended again while the connection lives")),
        "the second end is warned about: {texts:?}"
    );
    drop(running);
}

/// A stream that fails while the connection lives — the server answers the
/// listen with an error after acknowledging it — is a warning at once, and is
/// not opened again.
#[tokio::test]
async fn a_failed_subscription_is_warned_at_once_and_not_reopened() {
    let warnings = Warnings::default();
    let _default = tracing::subscriber::set_default(warnings.clone());

    let (running, _rx, seen) = modern_session(OnListen::AckThenFail).await;
    let listened = async {
        while seen.listens.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), listened)
        .await
        .expect("the listen is sent");
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        seen.listens.load(Ordering::SeqCst),
        1,
        "a failed stream is not opened again"
    );
    let texts = warnings.texts();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("change subscription failed")),
        "the failure is warned about: {texts:?}"
    );
    drop(running);
}

// ============================================================================
// Over streamable HTTP
// ============================================================================

mod over_http {
    use super::*;

    use std::convert::Infallible;

    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{Method, StatusCode, header};
    use axum::response::sse::{Event, Sse};
    use axum::response::{IntoResponse, Response};
    use futures_util::StreamExt;

    /// How often the listen stream sends a byte.
    const TICK: Duration = Duration::from_millis(100);

    /// The idle bound the client is built with.
    const IDLE_BOUND: Duration = Duration::from_millis(500);

    /// How long the listen stream lives before it announces its change: five
    /// idle bounds.
    const STREAM_FOR: Duration = Duration::from_millis(2_500);

    /// Where an [`HttpStub`] ends its first listen stream's body, with no
    /// final result, while the session lives — a stream the server or the
    /// network cuts; the next listen is acknowledged and announces one change
    /// at once.
    #[derive(Clone, Copy)]
    enum Cut {
        /// Right after the acknowledgment.
        AfterAck,
        /// Before it: the body carries no event.
        BeforeAck,
    }

    #[derive(Default)]
    struct HttpStub {
        /// Cut the first listen stream where this says; `None` keeps every
        /// stream open.
        cut_first: Option<Cut>,
        listens: AtomicUsize,
        /// Listen streams the stub is serving right now.
        open: Arc<AtomicUsize>,
    }

    /// Counts one listen stream as open for as long as its body lives.
    struct OpenStream(Arc<AtomicUsize>);

    impl OpenStream {
        fn new(open: &Arc<AtomicUsize>) -> Self {
            open.fetch_add(1, Ordering::SeqCst);
            Self(Arc::clone(open))
        }
    }

    impl Drop for OpenStream {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn json(message: &ServerJsonRpcMessage) -> Response {
        (
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::to_string(message).expect("serialize the stub's answer"),
        )
            .into_response()
    }

    /// A sessionless `2026-07-28` server: the probe and `tools/list` answered
    /// as JSON; `subscriptions/listen` answered over SSE — the acknowledgment,
    /// then a comment every [`TICK`] for [`STREAM_FOR`], then one tool-list
    /// change, then comments until the client goes.
    async fn handle(State(stub): State<Arc<HttpStub>>, method: Method, body: Bytes) -> Response {
        if method == Method::DELETE {
            return StatusCode::OK.into_response();
        }
        if method != Method::POST {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        let Ok(msg) = serde_json::from_slice::<rmcp::model::ClientJsonRpcMessage>(&body) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let Some((request, id)) = msg.into_request() else {
            return StatusCode::ACCEPTED.into_response();
        };
        match request {
            ClientRequest::DiscoverRequest(_) => json(&discover_answer(id)),
            ClientRequest::ListToolsRequest(_) => json(&tools_answer(id, &["echo"])),
            ClientRequest::SubscriptionsListenRequest(listen) => {
                let nth = stub.listens.fetch_add(1, Ordering::SeqCst) + 1;
                let ack = serde_json::to_string(&acknowledgment(&id, listen.params.notifications))
                    .expect("serialize the acknowledgment");
                let changed =
                    serde_json::to_string(&tool_list_changed(&id)).expect("serialize the change");
                if let Some(cut) = stub.cut_first {
                    let events = match (nth, cut) {
                        (1, Cut::AfterAck) => vec![Event::default().data(ack)],
                        (1, Cut::BeforeAck) => vec![],
                        _ => vec![Event::default().data(ack), Event::default().data(changed)],
                    };
                    let open = OpenStream::new(&stub.open);
                    let cut =
                        futures_util::stream::iter(events.into_iter().map(Ok::<_, Infallible>))
                            .chain(futures_util::stream::unfold(open, move |open| async move {
                                if nth == 1 {
                                    return None; // the body ends: no final result
                                }
                                tokio::time::sleep(TICK).await;
                                Some((Ok(Event::default().comment("tick")), open))
                            }));
                    return Sse::new(cut).into_response();
                }
                let ticks = (STREAM_FOR.as_millis() / TICK.as_millis()) as u32;
                let open = OpenStream::new(&stub.open);
                let events = futures_util::stream::unfold((0u32, open), move |(n, open)| {
                    let (ack, changed) = (ack.clone(), changed.clone());
                    async move {
                        let event = if n == 0 {
                            Event::default().data(ack)
                        } else {
                            tokio::time::sleep(TICK).await;
                            if n == ticks + 1 {
                                Event::default().data(changed)
                            } else {
                                Event::default().comment("tick")
                            }
                        };
                        Some((Ok::<_, Infallible>(event), (n + 1, open)))
                    }
                });
                Sse::new(events).into_response()
            }
            _ => StatusCode::ACCEPTED.into_response(),
        }
    }

    /// A modern session over a loopback [`HttpStub`], the client built with
    /// an idle bound of [`IDLE_BOUND`]: the session, its change receiver and
    /// the stub.
    async fn http_session(
        stub: HttpStub,
    ) -> (
        RunningService<RoleClient, NotifyingHandler>,
        UnboundedReceiver<ServerChange>,
        Arc<HttpStub>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let url = format!(
            "http://{}/mcp",
            listener.local_addr().expect("the bound address")
        );
        let stub = Arc::new(stub);
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&stub));
        tokio::spawn(async move { axum::serve(listener, app).await });

        let transport = crate::transport::StreamableHttpTransport::new(
            "remote",
            url::Url::parse(&url).unwrap(),
        )
        .with_idle_timeout(IDLE_BOUND);
        let conn = transport.connect().await.expect("the transport connects");
        let TransportInner::Http(http) = conn.inner else {
            unreachable!("an http transport yields an http connection")
        };
        let (tx, rx) = unbounded_channel();
        let running = serve(handler(tx), http, lifecycle(ProtocolMode::Auto, None))
            .await
            .expect("the modern handshake over HTTP");
        assert_eq!(
            running
                .peer_info()
                .map(|info| info.protocol_version.to_string())
                .as_deref(),
            Some("2026-07-28")
        );
        (running, rx, stub)
    }

    /// D5 (a)'s first condition, on the wire: over HTTP a listen stream that
    /// outlives the idle bound fivefold keeps delivering — the bound resets on
    /// every byte and nothing else ends the stream — so a change it announces
    /// after [`STREAM_FOR`] arrives, on the one stream the client opened.
    #[tokio::test]
    async fn over_http_a_listen_stream_that_outlives_the_idle_bound_keeps_delivering() {
        let started = std::time::Instant::now();
        let (running, mut rx, stub) = http_session(HttpStub::default()).await;

        assert_eq!(
            next_change(&mut rx, Duration::from_secs(5)).await,
            Some(ServerChange::ToolList),
            "the acknowledgment's refresh"
        );
        assert_eq!(
            next_change(&mut rx, STREAM_FOR * 3).await,
            Some(ServerChange::ToolList),
            "the change the stream announced after {STREAM_FOR:?}"
        );
        assert!(
            started.elapsed() >= STREAM_FOR,
            "the change came at the stream's end: {:?}",
            started.elapsed()
        );
        assert_eq!(
            stub.listens.load(Ordering::SeqCst),
            1,
            "one listen stream, never cut and reopened"
        );
        assert_eq!(
            stub.open.load(Ordering::SeqCst),
            1,
            "the stream is still open"
        );
        let _ = running.cancel().await;
    }

    /// D5 (a)'s second condition over HTTP: a listen stream cut while the
    /// session lives — its body ends with no final result, which rmcp reads
    /// as `Abrupt`, as it reads the connection's own end — is opened once
    /// more, and a change announced on the new stream arrives.
    #[tokio::test]
    async fn over_http_a_listen_stream_cut_while_the_session_lives_is_reopened() {
        let (running, mut rx, stub) = http_session(HttpStub {
            cut_first: Some(Cut::AfterAck),
            ..HttpStub::default()
        })
        .await;
        let mut changes = Vec::new();
        while changes.len() < 3 {
            match next_change(&mut rx, Duration::from_secs(5)).await {
                Some(change) => changes.push(change),
                None => break,
            }
        }
        assert_eq!(
            stub.listens.load(Ordering::SeqCst),
            2,
            "the cut stream was reopened once"
        );
        assert_eq!(
            changes,
            vec![ServerChange::ToolList; 3],
            "a refresh per acknowledgment, then the change on the reopened stream"
        );
        let _ = running.cancel().await;
    }

    /// D5 (a)'s second condition over HTTP, before the acknowledgment: a
    /// listen stream cut while the session lives before it is acknowledged
    /// fails the listen with `TransportClosed`, as the connection's own end
    /// does — and is opened once more; the new stream's refresh and change
    /// arrive.
    #[tokio::test]
    async fn over_http_a_listen_stream_cut_before_its_acknowledgment_is_reopened() {
        let (running, mut rx, stub) = http_session(HttpStub {
            cut_first: Some(Cut::BeforeAck),
            ..HttpStub::default()
        })
        .await;
        let mut changes = Vec::new();
        while changes.len() < 2 {
            match next_change(&mut rx, Duration::from_secs(5)).await {
                Some(change) => changes.push(change),
                None => break,
            }
        }
        assert_eq!(
            stub.listens.load(Ordering::SeqCst),
            2,
            "the cut stream was reopened once"
        );
        assert_eq!(
            changes,
            vec![ServerChange::ToolList; 2],
            "the reopened stream's refresh, then its change"
        );
        let _ = running.cancel().await;
    }
}
