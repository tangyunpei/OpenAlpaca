// crates/openalpaca_mcp/src/transport/http_tests.rs

//! The streamable-HTTP client against loopback stub servers (N-MCP-2).
//!
//! Two contracts, each proved on the wire rather than on the builder:
//!
//! * **Idleness bounds a body, never a total deadline.** A tool answer that
//!   streams for many times the idle bound completes, and a server-initiated
//!   GET stream held open past it stays open. The stubs keep one byte arriving
//!   every [`TICK`], and the client is built with an idle bound of
//!   [`IDLE_BOUND`], so the stream outlives the bound fivefold in 2.5 s: the
//!   production values (a five-minute idle floor, a 30 s total before) would
//!   cost the suite more than half a minute per test to show the same thing.
//!   A silent GET stream is cut once the idle bound passes with no byte, and
//!   held through a short configured request timeout, which is the floor's
//!   job.
//! * **A redirect the client refuses is a typed, non-retriable failure naming
//!   the target origin** (D6 (a), its third form): any hop to another origin
//!   for every server, any hop at all for a server with credentials or extra
//!   headers, more than ten hops for anyone else. The second origin's stub
//!   records every request it receives, so the test sees what a refused hop
//!   would have carried there.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use url::Url;

use super::{HttpAuth, StreamableHttpTransport, Transport, TransportInner, TransportKind};
use crate::lifecycle::ConnectionSnapshot;
use crate::{McpClient, McpClientConfig, McpError};

/// How often a streaming stub sends a byte.
const TICK: Duration = Duration::from_millis(100);

/// The idle bound the transport-level tests build the client with.
const IDLE_BOUND: Duration = Duration::from_millis(500);

/// How long the streaming stubs stream: five idle bounds.
const STREAM_FOR: Duration = Duration::from_millis(2_500);

/// The session id every stub hands out on `initialize`.
const SESSION: &str = "stub-session";

// ============================================================================
// The stub server
// ============================================================================

/// What a stub does with the GET that opens the server-initiated stream.
#[derive(Clone, Copy, Default)]
enum GetStream {
    /// `405 Method Not Allowed`: the server offers no such stream.
    #[default]
    NotOffered,
    /// An SSE stream that sends a comment every [`TICK`] until the client goes.
    Ticking,
    /// An SSE stream that sends its headers and then nothing.
    Silent,
}

#[derive(Clone, Default)]
struct StubConfig {
    /// Every request to one of these paths is answered `307` to the URL given.
    redirects: Vec<(&'static str, String)>,
    /// `tools/call` alone is answered `307` to this URL.
    call_redirect: Option<String>,
    /// `tools/call` is answered over SSE: a comment every [`TICK`] for this
    /// long, then the result. `None` answers it as one JSON body.
    call_streams_for: Option<Duration>,
    get: GetStream,
}

/// One request a stub received.
#[derive(Clone, Debug)]
struct Seen {
    method: Method,
    path: String,
    /// The JSON-RPC method of a POST body, when it has one.
    rpc: Option<String>,
    headers: HeaderMap,
}

struct StubState {
    config: StubConfig,
    seen: Mutex<Vec<Seen>>,
    /// GET streams the stub is serving right now.
    open_gets: Arc<AtomicUsize>,
}

/// A running stub. Every path that is not redirected is an MCP endpoint.
#[derive(Clone)]
struct Stub {
    /// `http://127.0.0.1:<port>` — the stub's origin, as the error names it.
    origin: String,
    state: Arc<StubState>,
}

impl Stub {
    /// Bind a loopback port and serve the config `make` builds from the
    /// stub's own origin (so a stub can redirect to itself).
    async fn spawn(make: impl FnOnce(&str) -> StubConfig) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let origin = format!("http://{}", listener.local_addr().expect("local addr"));
        let state = Arc::new(StubState {
            config: make(&origin),
            seen: Mutex::new(Vec::new()),
            open_gets: Arc::new(AtomicUsize::new(0)),
        });
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&state));
        tokio::spawn(async move { axum::serve(listener, app).await });
        Self { origin, state }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    fn seen(&self) -> Vec<Seen> {
        self.state.seen.lock().unwrap().clone()
    }

    fn count(&self, method: Method) -> usize {
        self.seen().iter().filter(|s| s.method == method).count()
    }

    fn rpc_count(&self, rpc: &str) -> usize {
        self.seen()
            .iter()
            .filter(|s| s.rpc.as_deref() == Some(rpc))
            .count()
    }

    fn open_gets(&self) -> usize {
        self.state.open_gets.load(Ordering::SeqCst)
    }
}

/// Counts one GET stream as open for as long as its body lives: hyper drops
/// the body when the client goes away (the next tick's write fails).
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

async fn handle(
    State(stub): State<Arc<StubState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let message: Option<Value> = serde_json::from_slice(&body).ok();
    let rpc = message
        .as_ref()
        .and_then(|m| m.get("method"))
        .and_then(Value::as_str)
        .map(str::to_string);
    stub.seen.lock().unwrap().push(Seen {
        method: method.clone(),
        path: uri.path().to_string(),
        rpc: rpc.clone(),
        headers,
    });

    let config = &stub.config;
    if let Some((_, to)) = config
        .redirects
        .iter()
        .find(|(path, _)| *path == uri.path())
    {
        return redirect(to);
    }

    match method {
        Method::POST => {
            let id = message
                .as_ref()
                .and_then(|m| m.get("id"))
                .cloned()
                .unwrap_or(Value::Null);
            match rpc.as_deref() {
                Some("initialize") => {
                    let version = message
                        .as_ref()
                        .and_then(|m| m.pointer("/params/protocolVersion"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let result = json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "stub", "version": "0.0.1" },
                    });
                    let mut response = rpc_json(id, result);
                    response.headers_mut().insert(
                        "mcp-session-id",
                        SESSION.parse().expect("session header value"),
                    );
                    response
                }
                Some("tools/list") => rpc_json(
                    id,
                    json!({ "tools": [{
                        "name": "slow",
                        "description": "a stub tool",
                        "inputSchema": { "type": "object", "properties": {} },
                    }] }),
                ),
                Some("tools/call") => {
                    if let Some(to) = &config.call_redirect {
                        return redirect(to);
                    }
                    let result = json!({
                        "content": [{ "type": "text", "text": "done" }],
                        "isError": false,
                    });
                    match config.call_streams_for {
                        Some(length) => streamed_answer(id, result, length),
                        None => rpc_json(id, result),
                    }
                }
                Some("ping") => rpc_json(id, json!({})),
                // A notification (`notifications/initialized`) has no answer.
                _ => StatusCode::ACCEPTED.into_response(),
            }
        }
        Method::GET => match config.get {
            GetStream::NotOffered => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            GetStream::Ticking => {
                let open = OpenStream::new(&stub.open_gets);
                let ticks = futures_util::stream::unfold(open, |open| async move {
                    tokio::time::sleep(TICK).await;
                    Some((Ok::<_, Infallible>(Event::default().comment("tick")), open))
                });
                Sse::new(ticks).into_response()
            }
            GetStream::Silent => {
                let open = OpenStream::new(&stub.open_gets);
                let silence = futures_util::stream::unfold(open, |open| async move {
                    std::future::pending::<()>().await;
                    Some((Ok::<_, Infallible>(Event::default()), open))
                });
                Sse::new(silence).into_response()
            }
        },
        // The session teardown on close.
        Method::DELETE => StatusCode::OK.into_response(),
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

fn redirect(to: &str) -> Response {
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, to.to_string())],
    )
        .into_response()
}

fn rpc_json(id: Value, result: Value) -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
    )
        .into_response()
}

/// A comment every [`TICK`] for `length`, then the JSON-RPC result as the
/// stream's one message.
fn streamed_answer(id: Value, result: Value, length: Duration) -> Response {
    let ticks = (length.as_millis() / TICK.as_millis()) as u32;
    let answer = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
    let events = futures_util::stream::unfold(0u32, move |tick| {
        let answer = answer.clone();
        async move {
            if tick < ticks {
                tokio::time::sleep(TICK).await;
                Some((
                    Ok::<_, Infallible>(Event::default().comment("working")),
                    tick + 1,
                ))
            } else if tick == ticks {
                Some((Ok(Event::default().data(answer)), tick + 1))
            } else {
                None
            }
        }
    });
    Sse::new(events).into_response()
}

// ============================================================================
// Helpers
// ============================================================================

/// A client config for the streamable-HTTP server at `url`.
fn http_config(
    url: &str,
    auth: Option<HttpAuth>,
    extra_headers: &[(&str, &str)],
) -> McpClientConfig {
    McpClientConfig {
        server_name: "remote".into(),
        transport: TransportKind::Http {
            url: Url::parse(url).expect("stub url"),
            auth,
            extra_headers: extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
        },
        request_timeout: Duration::from_secs(5),
        max_reconnect_attempts: 2,
        reconnect_backoff_ms: 10,
        ..Default::default()
    }
}

/// Serve the transport's connection with rmcp directly — the transport-level
/// tests set the client's idle bound, which `McpClient` derives itself.
async fn serve(
    transport: StreamableHttpTransport,
) -> rmcp::service::RunningService<rmcp::service::RoleClient, ()> {
    use rmcp::ServiceExt;
    let conn = transport.connect().await.expect("transport connects");
    let TransportInner::Http(http) = conn.inner else {
        unreachable!("an http transport yields an http connection")
    };
    ().serve(http).await.expect("handshake")
}

/// Wait until `check` holds, or `within` passes. Returns whether it held.
async fn eventually(within: Duration, check: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    check()
}

/// The request headers no hop the client refused may have carried: the
/// configured credential and extra header.
fn credential_headers(seen: &[Seen]) -> Vec<String> {
    seen.iter()
        .flat_map(|s| {
            ["x-api-key", "authorization", "x-tenant"]
                .into_iter()
                .filter(|name| s.headers.contains_key(*name))
                .map(|name| format!("{} {} carried {name}", s.method, s.path))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn refused_origin(error: &McpError) -> Option<&str> {
    match error {
        McpError::RedirectRefused { origin } => Some(origin),
        _ => None,
    }
}

// ============================================================================
// Idleness, never a total deadline
// ============================================================================

/// A tool answer streamed over SSE for five idle bounds completes: the bound
/// resets on every byte, and nothing else ends the body.
#[tokio::test]
async fn a_streamed_tool_answer_is_not_cut_by_a_total_deadline() {
    let stub = Stub::spawn(|_| StubConfig {
        call_streams_for: Some(STREAM_FOR),
        ..Default::default()
    })
    .await;
    let transport = StreamableHttpTransport::new("remote", Url::parse(&stub.url("/mcp")).unwrap())
        .with_idle_timeout(IDLE_BOUND);
    let running = serve(transport).await;

    let started = Instant::now();
    // Built from its wire form: the struct's fields differ between rmcp 0.16
    // and 3.5, where it is also `#[non_exhaustive]`, and G-base's step 6
    // compiles this file against 3.5 with no patch for it.
    let params: rmcp::model::CallToolRequestParams =
        serde_json::from_value(json!({ "name": "slow" })).expect("call params");
    let answer = tokio::time::timeout(STREAM_FOR * 3, running.call_tool(params))
        .await
        .expect("the streamed answer arrives — it was cut and never resumed")
        .expect("the call succeeds");

    assert!(
        started.elapsed() >= STREAM_FOR,
        "the answer took the whole stream: {:?}",
        started.elapsed()
    );
    let text = serde_json::to_value(&answer.content).unwrap();
    assert_eq!(
        text[0]["text"], "done",
        "the stream's one message: {answer:?}"
    );
    assert_eq!(stub.rpc_count("tools/call"), 1, "one call, never re-sent");
    let _ = running.cancel().await;
}

/// The server-initiated GET stream stays open past five idle bounds while the
/// server keeps it alive: one GET, still being served.
#[tokio::test]
async fn a_get_stream_held_open_past_the_idle_bound_stays_open() {
    let stub = Stub::spawn(|_| StubConfig {
        get: GetStream::Ticking,
        ..Default::default()
    })
    .await;
    let transport = StreamableHttpTransport::new("remote", Url::parse(&stub.url("/mcp")).unwrap())
        .with_idle_timeout(IDLE_BOUND);
    let running = serve(transport).await;

    assert!(
        eventually(Duration::from_secs(2), || stub.open_gets() == 1).await,
        "the client opens the GET stream"
    );
    tokio::time::sleep(STREAM_FOR).await;

    assert_eq!(
        stub.count(Method::GET),
        1,
        "the GET stream was never cut and re-opened: {:?}",
        stub.seen()
            .iter()
            .map(|s| (&s.method, &s.path))
            .collect::<Vec<_>>()
    );
    assert_eq!(stub.open_gets(), 1, "the first GET stream is still open");
    let _ = running.cancel().await;
}

/// The idle bound is applied on the wire: a GET stream that sends its headers
/// and then nothing is cut once the bound passes with no byte, and rmcp opens
/// it again. Without the bound a half-open connection, or a dead server behind
/// a proxy, would hold the stream for ever with no error. (The next test is
/// the other half: the bound is never shorter than its floor.)
#[tokio::test]
async fn a_silent_get_stream_is_cut_at_the_idle_bound_and_reopened() {
    let stub = Stub::spawn(|_| StubConfig {
        get: GetStream::Silent,
        ..Default::default()
    })
    .await;
    let transport = StreamableHttpTransport::new("remote", Url::parse(&stub.url("/mcp")).unwrap())
        .with_idle_timeout(IDLE_BOUND);
    let running = serve(transport).await;

    assert!(
        eventually(Duration::from_secs(3), || stub.count(Method::GET) >= 2).await,
        "the silent GET stream was never cut at the idle bound: {} GET(s)",
        stub.count(Method::GET)
    );
    let _ = running.cancel().await;
}

/// A *silent* GET stream outlives a short configured request timeout: the
/// configured timeout only ever raises the client's idle bound, whose floor
/// is minutes — a stream the server holds open without a byte is a stream
/// with nothing to say, not a dead one.
#[tokio::test]
async fn a_silent_get_stream_outlives_a_short_request_timeout() {
    let stub = Stub::spawn(|_| StubConfig {
        get: GetStream::Silent,
        ..Default::default()
    })
    .await;
    let mut config = http_config(&stub.url("/mcp"), None, &[]);
    config.request_timeout = Duration::from_millis(300);
    let client = McpClient::connect(config).await.expect("connects");

    assert!(
        eventually(Duration::from_secs(2), || stub.open_gets() == 1).await,
        "the client opens the GET stream"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(
        stub.count(Method::GET),
        1,
        "the silent stream was never cut"
    );
    assert_eq!(stub.open_gets(), 1);
    let _ = client.disconnect().await;
}

// ============================================================================
// Redirects
// ============================================================================

/// **Cross-origin, credentialed.** A server whose URL redirects to a second
/// port is refused at the handshake with the typed error naming that origin,
/// and the second origin receives nothing — no API key, no extra header.
#[tokio::test]
async fn a_redirect_to_another_origin_is_refused_and_carries_no_credential() {
    let second = Stub::spawn(|_| StubConfig::default()).await;
    let first = Stub::spawn(|_| StubConfig {
        redirects: vec![("/mcp", second.url("/mcp"))],
        ..Default::default()
    })
    .await;
    let config = http_config(
        &first.url("/mcp"),
        Some(HttpAuth::ApiKey {
            header: "X-API-Key".into(),
            value: "k-secret".into(),
        }),
        &[("X-Tenant", "t-1")],
    );

    let result = McpClient::connect(config).await;

    assert_eq!(
        credential_headers(&second.seen()),
        Vec::<String>::new(),
        "the second origin received a credential"
    );
    assert!(second.seen().is_empty(), "the refused hop was never made");
    let error = result.err().expect("the handshake is refused");
    assert_eq!(
        refused_origin(&error),
        Some(second.origin.as_str()),
        "{error:?}"
    );
    assert!(
        !error.is_retriable(),
        "a refused redirect never enters the reconnect ladder"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "server redirected to {}; set `url` to the canonical endpoint",
            second.origin
        )
    );
    assert_eq!(
        first.count(Method::POST),
        1,
        "one initialize, never retried"
    );
}

/// **Cross-origin, unauthenticated.** The cross-origin rule binds every
/// server, not only a credentialed one.
#[tokio::test]
async fn an_unauthenticated_server_is_refused_a_redirect_to_another_origin() {
    let second = Stub::spawn(|_| StubConfig::default()).await;
    let first = Stub::spawn(|_| StubConfig {
        redirects: vec![("/mcp", second.url("/mcp"))],
        ..Default::default()
    })
    .await;

    let error = McpClient::connect(http_config(&first.url("/mcp"), None, &[]))
        .await
        .err()
        .expect("the handshake is refused");

    assert!(second.seen().is_empty(), "the refused hop was never made");
    assert_eq!(
        refused_origin(&error),
        Some(second.origin.as_str()),
        "{error:?}"
    );
}

/// **Same origin, credentialed.** A server with credentials is refused even a
/// redirect to another path on its own origin; the path is never requested.
#[tokio::test]
async fn a_credentialed_server_is_refused_a_redirect_on_its_own_origin() {
    let stub = Stub::spawn(|origin| StubConfig {
        redirects: vec![("/mcp", format!("{origin}/mcp/v2"))],
        ..Default::default()
    })
    .await;
    let config = http_config(
        &stub.url("/mcp"),
        Some(HttpAuth::Bearer("t-secret".into())),
        &[],
    );

    let error = McpClient::connect(config)
        .await
        .err()
        .expect("the handshake is refused");

    assert!(
        !stub.seen().iter().any(|s| s.path == "/mcp/v2"),
        "the redirect's target was never requested: {:?}",
        stub.seen()
    );
    assert_eq!(
        refused_origin(&error),
        Some(stub.origin.as_str()),
        "{error:?}"
    );
    assert!(!error.is_retriable());
}

/// **Same origin, extra headers only.** An `extra_headers` entry counts as a
/// credential: any hop is refused.
#[tokio::test]
async fn a_server_with_extra_headers_is_refused_a_redirect_on_its_own_origin() {
    let stub = Stub::spawn(|origin| StubConfig {
        redirects: vec![("/mcp", format!("{origin}/mcp/v2"))],
        ..Default::default()
    })
    .await;
    let config = http_config(&stub.url("/mcp"), None, &[("X-Tenant", "t-1")]);

    let error = McpClient::connect(config)
        .await
        .err()
        .expect("the handshake is refused");

    assert!(!stub.seen().iter().any(|s| s.path == "/mcp/v2"));
    assert_eq!(
        refused_origin(&error),
        Some(stub.origin.as_str()),
        "{error:?}"
    );
}

/// **Same origin, unauthenticated.** Followed, as before: the server connects
/// through the redirect and answers.
#[tokio::test]
async fn an_unauthenticated_server_follows_a_redirect_on_its_own_origin() {
    let stub = Stub::spawn(|origin| StubConfig {
        redirects: vec![("/mcp", format!("{origin}/mcp/v2"))],
        ..Default::default()
    })
    .await;

    let client = McpClient::connect(http_config(&stub.url("/mcp"), None, &[]))
        .await
        .expect("a same-origin redirect is followed");
    let tools = client
        .list_tools_once()
        .await
        .expect("lists through the redirect");

    assert_eq!(tools.len(), 1);
    assert!(
        stub.rpc_count("initialize") >= 2,
        "the redirect carried the body"
    );
    let _ = client.disconnect().await;
}

/// **The hop limit.** An unauthenticated same-origin chain past reqwest's ten
/// hops is the same typed error, naming the origin it was sent to.
#[tokio::test]
async fn a_redirect_chain_past_ten_hops_is_refused() {
    let stub = Stub::spawn(|origin| StubConfig {
        redirects: vec![("/mcp", format!("{origin}/mcp"))],
        ..Default::default()
    })
    .await;

    let error = McpClient::connect(http_config(&stub.url("/mcp"), None, &[]))
        .await
        .err()
        .expect("the handshake is refused");

    assert_eq!(
        refused_origin(&error),
        Some(stub.origin.as_str()),
        "{error:?}"
    );
    // The first request and ten followed hops; the eleventh is refused.
    assert_eq!(stub.count(Method::POST), 11);
}

/// **Mid-session.** A call the server redirects elsewhere comes back as the
/// typed error at once: no reconnect, no second handshake, and the second
/// origin receives nothing.
#[tokio::test]
async fn a_call_redirected_mid_session_is_refused_without_a_reconnect() {
    let second = Stub::spawn(|_| StubConfig::default()).await;
    let first = Stub::spawn(|_| StubConfig {
        call_redirect: Some(second.url("/mcp")),
        ..Default::default()
    })
    .await;
    let client = McpClient::connect(http_config(&first.url("/mcp"), None, &[]))
        .await
        .expect("connects");

    let error = client
        .call_tool("slow", Value::Null, None)
        .await
        .expect_err("the call is refused");

    assert!(second.seen().is_empty(), "the refused hop was never made");
    assert_eq!(
        refused_origin(&error),
        Some(second.origin.as_str()),
        "{error:?}"
    );
    assert_eq!(
        client.inner.attempt_counter.load(Ordering::SeqCst),
        0,
        "the reconnect counter did not move"
    );
    assert_eq!(
        client.connection_state().await,
        ConnectionSnapshot::Connected
    );
    assert_eq!(first.rpc_count("initialize"), 1, "no second handshake");
    let _ = client.disconnect().await;
}
