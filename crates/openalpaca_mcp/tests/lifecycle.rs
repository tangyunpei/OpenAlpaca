// crates/openalpaca_mcp/tests/lifecycle.rs

//! The lifecycle every MCP connection opens with — rmcp's `Auto` (DU-25 (p4))
//! — read on the wire, through `McpClient::connect`.
//!
//! Each stub records every message it is sent, so these tests assert what a
//! server actually saw, not only the outcome: a modern server is probed with
//! `server/discover` at `2026-07-28` and never sees `initialize`; a legacy one
//! that rejects the probe — with a JSON-RPC error over stdio, with a plain-text
//! 4xx over streamable HTTP — is sent `initialize` offering `2025-11-25` on the
//! same connection; one that ignores the probe is sent it once rmcp's 10 s
//! discover timeout has run out; and a credential rejection in either phase
//! stays `Unauthorized`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use openalpaca_mcp::{
    Implementation, McpClient, McpClientConfig, McpError, ProtocolMode, TransportKind,
};
use serde_json::Value;

/// The version the `server/discover` probe names (`ProtocolVersion::LATEST`).
const MODERN: &str = "2026-07-28";
/// The version the legacy `initialize` offers (`LATEST_WITH_INITIALIZE`).
const LEGACY: &str = "2025-11-25";
/// Where a `server/discover` request carries its protocol version.
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";

fn config(name: &str, transport: TransportKind) -> McpClientConfig {
    McpClientConfig {
        server_name: name.into(),
        transport,
        client_info: Implementation::new("openalpaca-mcp-test", "0.1.0"),
        protocol: ProtocolMode::Auto,
        request_timeout: Duration::from_secs(5),
        max_reconnect_attempts: 0,
        reconnect_backoff_ms: 10,
    }
}

/// The `method` of every recorded message, in the order they arrived.
fn methods(sent: &[Value]) -> Vec<&str> {
    sent.iter().filter_map(|m| m["method"].as_str()).collect()
}

fn sent_one<'a>(sent: &'a [Value], method: &str) -> &'a Value {
    sent.iter()
        .find(|m| m["method"] == method)
        .unwrap_or_else(|| panic!("no {method} was sent; the server saw {sent:?}"))
}

/// The version the `server/discover` probe named.
fn probed(sent: &[Value]) -> &str {
    sent_one(sent, "server/discover")["params"]["_meta"][META_PROTOCOL_VERSION]
        .as_str()
        .expect("the probe names its protocol version in _meta")
}

/// The version the legacy `initialize` offered.
fn offered(sent: &[Value]) -> &str {
    sent_one(sent, "initialize")["params"]["protocolVersion"]
        .as_str()
        .expect("initialize offers a protocol version")
}

// ============================================================================
// stdio
// ============================================================================

#[cfg(unix)]
mod stdio {
    use super::*;

    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use openalpaca_mcp::ProtocolVersion;

    /// How a stub answers the `server/discover` probe.
    enum Probe {
        /// A `DiscoverResult` supporting `2026-07-28`: a modern server.
        Answer,
        /// A JSON-RPC `-32601 Method not found`: a legacy server.
        RejectAsUnknown,
        /// Nothing at all: a legacy server that ignores an unknown method.
        Ignore,
    }

    /// A stdio MCP server in `sh` that appends every line it reads to a log,
    /// answers the probe as `probe` says, and answers `initialize` (echoing
    /// the version offered) and `tools/list` (one tool, `echo`). Its
    /// `initialize` declares `tools.listChanged`, as most servers do, so a
    /// `subscriptions/listen` sent in a legacy session — where the server's
    /// notifications reach the handler and nothing listens (P19b) — would show
    /// in the log the tests read whole.
    struct Stub {
        _dir: tempfile::TempDir,
        script: PathBuf,
        log: PathBuf,
    }

    impl Stub {
        fn new(probe: Probe) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let dir = tempfile::tempdir().expect("tempdir");
            let script = dir.path().join("server.sh");
            let log = dir.path().join("sent.jsonl");
            let discover = match probe {
                Probe::Answer => {
                    let result = rmcp::model::DiscoverResult::new(
                        vec![ProtocolVersion::LATEST],
                        rmcp::model::ServerCapabilities::builder().enable_tools().build(),
                    )
                    .with_server_info(Implementation::new("modern-stub", "1.0.0"));
                    let result = serde_json::to_string(&result).expect("serialize discover");
                    assert!(!result.contains('\''), "the result is quoted for sh");
                    format!(
                        r#"printf '{{"jsonrpc":"2.0","id":%s,"result":%s}}\n' "$id" '{result}'"#
                    )
                }
                Probe::RejectAsUnknown => r#"printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"Method not found"}}\n' "$id""#.to_string(),
                Probe::Ignore => ":".to_string(),
            };
            let body = format!(
                r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=`printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'`
  case "$line" in
    *'"method":"server/discover"'*)
      {discover}
      ;;
    *'"method":"initialize"'*)
      pv=`printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p'`
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"%s","capabilities":{{"tools":{{"listChanged":true}}}},"serverInfo":{{"name":"legacy-stub","version":"1.0.0"}}}}}}\n' "$id" "$pv"
      ;;
    *'"method":"tools/list"'*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"echo","description":"stub","inputSchema":{{"type":"object","properties":{{}}}}}}]}}}}\n' "$id"
      ;;
  esac
done
"#,
                log = log.display(),
            );
            std::fs::write(&script, body).expect("write the stub");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod the stub");
            Self {
                _dir: dir,
                script,
                log,
            }
        }

        fn config(&self) -> McpClientConfig {
            config(
                "stdio-stub",
                TransportKind::Stdio {
                    command: self.script.to_string_lossy().into_owned(),
                    args: vec![],
                    env: Default::default(),
                    cwd: None,
                },
            )
        }

        fn sent(&self) -> Vec<Value> {
            read_log(&self.log)
        }
    }

    fn read_log(log: &Path) -> Vec<Value> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("the stub logs JSON lines"))
            .collect()
    }

    /// A modern server answers the probe: the connection is served at
    /// `2026-07-28` with no `initialize` at all, and its identity comes from
    /// the discover result's `_meta`.
    #[tokio::test]
    async fn a_modern_server_is_served_after_server_discover() {
        let stub = Stub::new(Probe::Answer);
        let client = McpClient::connect(stub.config()).await.expect("connect");

        assert_eq!(
            client
                .protocol_version()
                .map(ToString::to_string)
                .as_deref(),
            Some(MODERN)
        );
        assert_eq!(
            client.server_info().map(|i| i.name.as_str()),
            Some("modern-stub")
        );
        let tools = client
            .list_tools_once()
            .await
            .expect("the session serves requests");
        assert_eq!(tools.iter().map(|t| &*t.name).collect::<Vec<_>>(), ["echo"]);

        let sent = stub.sent();
        assert_eq!(
            methods(&sent),
            ["server/discover", "tools/list"],
            "no initialize"
        );
        assert_eq!(probed(&sent), MODERN);
        client.disconnect().await.expect("disconnect");
    }

    /// A legacy server rejects the probe as an unknown method: the same
    /// connection is sent `initialize` offering `2025-11-25`.
    #[tokio::test]
    async fn a_legacy_server_that_rejects_the_probe_gets_initialize_at_2025_11_25() {
        let stub = Stub::new(Probe::RejectAsUnknown);
        let client = McpClient::connect(stub.config()).await.expect("connect");

        assert_eq!(
            client
                .protocol_version()
                .map(ToString::to_string)
                .as_deref(),
            Some(LEGACY)
        );
        assert_eq!(
            client.server_info().map(|i| i.name.as_str()),
            Some("legacy-stub")
        );
        client
            .list_tools_once()
            .await
            .expect("the session serves requests");

        let sent = stub.sent();
        assert_eq!(
            methods(&sent),
            [
                "server/discover",
                "initialize",
                "notifications/initialized",
                "tools/list"
            ]
        );
        assert_eq!(probed(&sent), MODERN);
        assert_eq!(offered(&sent), LEGACY);
        client.disconnect().await.expect("disconnect");
    }

    /// The identity a connection sends is ours (DU-25 (m-f)): the legacy
    /// `initialize` carries the configured `client_info`, an explicit empty
    /// capability set and the version the lifecycle chose, and the probe's
    /// `_meta` carries the same identity and capabilities.
    #[tokio::test]
    async fn a_legacy_servers_initialize_carries_our_client_info_no_capabilities_and_2025_11_25() {
        let stub = Stub::new(Probe::RejectAsUnknown);
        let client = McpClient::connect(stub.config()).await.expect("connect");
        client
            .list_tools_once()
            .await
            .expect("the session serves requests");

        let sent = stub.sent();
        let ours = serde_json::json!({ "name": "openalpaca-mcp-test", "version": "0.1.0" });
        let params = &sent_one(&sent, "initialize")["params"];
        assert_eq!(params["clientInfo"], ours, "initialize: {params}");
        assert_eq!(
            params["capabilities"],
            serde_json::json!({}),
            "initialize: {params}"
        );
        assert_eq!(offered(&sent), LEGACY);
        let meta = &sent_one(&sent, "server/discover")["params"]["_meta"];
        assert_eq!(
            meta["io.modelcontextprotocol/clientInfo"], ours,
            "probe: {meta}"
        );
        assert_eq!(
            meta["io.modelcontextprotocol/clientCapabilities"],
            serde_json::json!({}),
            "probe: {meta}"
        );
        client.disconnect().await.expect("disconnect");
    }

    /// A legacy server that ignores the probe is found legacy only when rmcp's
    /// 10 s discover timeout runs out — on every connect. The time is printed
    /// so the gate's log records it.
    #[tokio::test]
    async fn a_server_that_ignores_the_probe_is_reached_after_the_10_s_discover_timeout() {
        let stub = Stub::new(Probe::Ignore);
        let started = Instant::now();
        let client = McpClient::connect(stub.config()).await.expect("connect");
        let took = started.elapsed();
        eprintln!("silent stub: connect took {took:?}");

        assert!(
            took >= Duration::from_secs(10) && took < Duration::from_secs(20),
            "the fallback waits out the 10 s discover timeout, took {took:?}"
        );
        assert_eq!(
            client
                .protocol_version()
                .map(ToString::to_string)
                .as_deref(),
            Some(LEGACY)
        );
        // A round trip before the log is read: the stub reads its stdin in
        // order and logs a line before answering it, so once `tools/list` is
        // answered the `notifications/initialized` sent ahead of it is logged.
        // Read straight after the connect, the log raced the notification.
        let tools = client
            .list_tools_once()
            .await
            .expect("the session serves requests");
        assert_eq!(tools.iter().map(|t| &*t.name).collect::<Vec<_>>(), ["echo"]);
        let sent = stub.sent();
        assert_eq!(
            methods(&sent),
            [
                "server/discover",
                "initialize",
                "notifications/initialized",
                "tools/list"
            ]
        );
        assert_eq!(offered(&sent), LEGACY);
        client.disconnect().await.expect("disconnect");
    }
}

// ============================================================================
// streamable HTTP
// ============================================================================

mod http {
    use super::*;

    use axum::extract::State;
    use axum::http::{StatusCode, header};
    use axum::response::{IntoResponse, Response};

    /// How the stub answers.
    #[derive(Clone, Copy)]
    enum Behaviour {
        /// A legacy server: the probe is a plain-text `400`, as a server that
        /// predates `server/discover` answers a pre-session request it does
        /// not know; `initialize` is served, declaring `tools.listChanged`
        /// (no `subscriptions/listen` follows in a legacy session, P19b).
        LegacyPlainText400,
        /// The probe is refused `401` with a challenge.
        UnauthorizedProbe,
        /// Legacy (`400` to the probe), then `initialize` refused `401`.
        UnauthorizedInitialize,
    }

    #[derive(Clone)]
    struct Stub {
        behaviour: Behaviour,
        sent: Arc<Mutex<Vec<Value>>>,
    }

    async fn on_post(State(stub): State<Stub>, body: String) -> Response {
        let message: Value = serde_json::from_str(&body).expect("a JSON-RPC message");
        stub.sent.lock().unwrap().push(message.clone());
        let method = message["method"].as_str().unwrap_or_default();
        let unauthorized = || {
            (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, r#"Bearer realm="stub""#)],
            )
                .into_response()
        };
        let json = |result: Value| {
            (
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (
                        header::HeaderName::from_static("mcp-session-id"),
                        "stub-session",
                    ),
                ],
                serde_json::json!({"jsonrpc": "2.0", "id": message["id"], "result": result})
                    .to_string(),
            )
                .into_response()
        };
        match (stub.behaviour, method) {
            (Behaviour::UnauthorizedProbe, "server/discover")
            | (Behaviour::UnauthorizedInitialize, "initialize") => unauthorized(),
            (_, "server/discover") => (
                StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "text/plain")],
                "Bad Request: No valid session ID provided",
            )
                .into_response(),
            (_, "initialize") => json(serde_json::json!({
                "protocolVersion": message["params"]["protocolVersion"],
                "capabilities": {"tools": {"listChanged": true}},
                "serverInfo": {"name": "legacy-http-stub", "version": "1.0.0"},
            })),
            (_, "tools/list") => json(serde_json::json!({"tools": [{
                "name": "echo",
                "description": "stub",
                "inputSchema": {"type": "object", "properties": {}},
            }]})),
            // A notification: accepted, nothing to answer.
            _ => StatusCode::ACCEPTED.into_response(),
        }
    }

    /// Serve the stub on a loopback port; returns its URL and what it was sent.
    async fn serve(behaviour: Behaviour) -> (url::Url, Arc<Mutex<Vec<Value>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let app = axum::Router::new()
            .route(
                "/mcp",
                axum::routing::post(on_post)
                    // No server-initiated stream and no session teardown to do.
                    .get(|| async { StatusCode::METHOD_NOT_ALLOWED })
                    .delete(|| async { StatusCode::OK }),
            )
            .with_state(Stub {
                behaviour,
                sent: Arc::clone(&sent),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("the bound address");
        tokio::spawn(async move { axum::serve(listener, app).await });
        let url = url::Url::parse(&format!("http://{addr}/mcp")).expect("the stub's URL");
        (url, sent)
    }

    fn http_config(url: url::Url) -> McpClientConfig {
        config(
            "http-stub",
            TransportKind::Http {
                url,
                auth: None,
                extra_headers: Default::default(),
            },
        )
    }

    fn snapshot(sent: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
        sent.lock().unwrap().clone()
    }

    /// rmcp turns a pre-session plain-text 4xx to the probe into a correlated
    /// JSON-RPC error, so a legacy streamable-HTTP server falls back too.
    #[tokio::test]
    async fn a_legacy_http_server_answering_a_plain_text_4xx_gets_initialize_at_2025_11_25() {
        let (url, sent) = serve(Behaviour::LegacyPlainText400).await;
        let client = McpClient::connect(http_config(url)).await.expect("connect");

        assert_eq!(
            client
                .protocol_version()
                .map(ToString::to_string)
                .as_deref(),
            Some(LEGACY)
        );
        assert_eq!(
            client.server_info().map(|i| i.name.as_str()),
            Some("legacy-http-stub")
        );
        client
            .list_tools_once()
            .await
            .expect("the session serves requests");

        let sent = snapshot(&sent);
        assert_eq!(
            methods(&sent),
            [
                "server/discover",
                "initialize",
                "notifications/initialized",
                "tools/list"
            ]
        );
        assert_eq!(probed(&sent), MODERN);
        assert_eq!(offered(&sent), LEGACY);
        client.disconnect().await.expect("disconnect");
    }

    /// A `401` to the probe ends the connect — no fallback — and stays the
    /// typed `Unauthorized` the supervisor classifies as needing authorization.
    #[tokio::test]
    async fn a_401_to_the_probe_is_unauthorized_with_no_fallback() {
        let (url, sent) = serve(Behaviour::UnauthorizedProbe).await;
        let Err(err) = McpClient::connect(http_config(url)).await else {
            panic!("a 401 must fail the connect");
        };
        assert!(matches!(err, McpError::Unauthorized(401)), "got {err:?}");
        assert_eq!(
            methods(&snapshot(&sent)),
            ["server/discover"],
            "no fallback after a 401"
        );
    }

    /// A `401` to the legacy `initialize`, after the probe found the server
    /// legacy: rmcp reports both phases, and the fallback's rejection decides.
    #[tokio::test]
    async fn a_401_to_the_fallback_initialize_is_unauthorized() {
        let (url, sent) = serve(Behaviour::UnauthorizedInitialize).await;
        let Err(err) = McpClient::connect(http_config(url)).await else {
            panic!("a 401 must fail the connect");
        };
        assert!(matches!(err, McpError::Unauthorized(401)), "got {err:?}");
        assert_eq!(methods(&snapshot(&sent)), ["server/discover", "initialize"]);
    }
}
