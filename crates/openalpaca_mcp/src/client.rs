// crates/openalpaca_mcp/src/client.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::time::Duration;

use rmcp::model::{ClientRequest, Implementation, PingRequest, ProtocolVersion};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use tokio::sync::Mutex;

use crate::error::McpError;
use crate::lifecycle::{ConnectionSnapshot, ConnectionState};
use crate::transport::{Transport, TransportConnection, TransportInner, TransportKind};

/// Configuration for [`McpClient::connect`].
#[derive(Clone, Debug)]
pub struct McpClientConfig {
    pub server_name: String,
    pub transport: TransportKind,
    pub client_info: Implementation,
    pub request_timeout: Duration,
    pub max_reconnect_attempts: u32,
    pub reconnect_backoff_ms: u64,
}

impl Default for McpClientConfig {
    fn default() -> Self {
        Self {
            server_name: "mcp-server".to_string(),
            transport: TransportKind::Stdio {
                command: String::new(),
                args: Vec::new(),
                env: std::collections::HashMap::new(),
                cwd: None,
            },
            client_info: Implementation {
                name: "openalpaca-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            request_timeout: Duration::from_secs(30),
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 100,
        }
    }
}

/// MCP client. Cheaply cloneable (`Arc<ClientInner>`); all clones share lifecycle state.
#[derive(Clone)]
pub struct McpClient {
    pub(crate) inner: Arc<ClientInner>,
}

pub(crate) struct ClientInner {
    pub(crate) config: McpClientConfig,
    pub(crate) state: tokio::sync::RwLock<ConnectionState>,
    /// Holds the rmcp RunningService for the current connection.
    /// `Option` so we can take() it on reconnect/disconnect (RunningService consumes self on close).
    pub(crate) service: Mutex<Option<RunningService<RoleClient, ()>>>,
    pub(crate) server_info: tokio::sync::OnceCell<Implementation>,
    pub(crate) protocol_version: tokio::sync::OnceCell<ProtocolVersion>,
    pub(crate) attempt_counter: AtomicU32,
    /// The close seal. Set once, in [`McpClient::disconnect`], **before** that
    /// method takes the service lock; never cleared. A sealed client refuses to
    /// reconnect and refuses to publish a service, so a clone that outlives the
    /// disconnect (a stale extension snapshot, an in-flight retry loop) can
    /// never respawn the child the owner just disabled.
    pub(crate) closed: AtomicBool,
}

impl ClientInner {
    /// A fresh inner in `state`, never sealed.
    pub(crate) fn new(config: McpClientConfig, state: ConnectionState) -> Self {
        Self {
            config,
            state: tokio::sync::RwLock::new(state),
            service: Mutex::new(None),
            server_info: tokio::sync::OnceCell::new(),
            protocol_version: tokio::sync::OnceCell::new(),
            attempt_counter: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        }
    }

    /// `true` once [`McpClient::disconnect`] has run on any clone.
    pub(crate) fn is_sealed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl McpClient {
    /// Test-only constructor: a client shell that has never connected.
    ///
    /// Gated behind the `test-utils` feature for downstream test consumers
    /// (e.g. `openalpaca_core` bridge tests that need an `Arc<McpClient>` to
    /// build a `ToolBackend::Mcp` without a live server). Any RPC call on the
    /// returned client fails with a not-connected error.
    #[cfg(feature = "test-utils")]
    pub fn disconnected_for_tests(server_name: impl Into<String>) -> Self {
        let config = McpClientConfig {
            server_name: server_name.into(),
            ..Default::default()
        };
        Self {
            inner: Arc::new(ClientInner::new(config, ConnectionState::Disconnected)),
        }
    }

    /// Connect to the configured MCP server. Performs initialize handshake.
    pub async fn connect(config: McpClientConfig) -> Result<Self, McpError> {
        // A fresh inner is never sealed, so re-enabling a previously
        // disconnected server through a new `connect` is unaffected.
        let inner = Arc::new(ClientInner::new(config, ConnectionState::Connecting));

        let client = Self { inner };
        client.do_handshake().await?;
        Ok(client)
    }

    /// Build a Transport from the configured TransportKind, connect, then run rmcp's serve().
    pub(crate) async fn do_handshake(&self) -> Result<(), McpError> {
        let transport = build_transport(&self.inner.config)?;
        let conn = transport.connect().await?;

        tracing::info!(
            server_name = %self.inner.config.server_name,
            transport_kind = %transport.kind(),
            "MCP handshake starting"
        );

        let running = serve_with_conn(conn)
            .await
            .map_err(|e| McpError::HandshakeFailed(format!("rmcp serve() failed: {e:?}")))?;

        // Record server identity (first-time only; immutable across reconnects).
        if let Some(peer_info) = running.peer_info() {
            let _ = self.inner.server_info.set(peer_info.server_info.clone());
            let _ = self
                .inner
                .protocol_version
                .set(peer_info.protocol_version.clone());
            tracing::info!(
                server_name = %self.inner.config.server_name,
                server_version = %peer_info.server_info.version,
                protocol_version = ?peer_info.protocol_version,
                "MCP handshake complete"
            );
        }

        self.install_service(running).await
    }

    /// Publish a freshly handshaken service as the client's current connection.
    ///
    /// Split out of [`Self::do_handshake`] because this is the second place the
    /// close seal has to be honoured (the first is [`Self::reconnect`]'s entry).
    /// `disconnect` stores the seal *before* taking the service lock and this
    /// reads it *while holding* that lock, so the two orderings are the only
    /// ones possible: install-then-disconnect, where `disconnect` finds the
    /// service and closes it normally; or disconnect-then-install, where this
    /// closes the service it was handed. No interleaving leaves a live child
    /// attached to a sealed client.
    pub(crate) async fn install_service(
        &self,
        mut running: RunningService<RoleClient, ()>,
    ) -> Result<(), McpError> {
        use std::sync::atomic::Ordering;
        let mut guard = self.inner.service.lock().await;
        if self.inner.is_sealed() {
            drop(guard);
            tracing::warn!(
                server_name = %self.inner.config.server_name,
                "MCP handshake completed after disconnect; closing the service it spawned"
            );
            // `close_with_timeout` takes `&mut self`; the result is best-effort —
            // rmcp finishes the teardown on drop either way.
            let _ = running.close_with_timeout(Duration::from_secs(2)).await;
            return Err(McpError::Closed);
        }
        *guard = Some(running);
        drop(guard);
        *self.inner.state.write().await = ConnectionState::Connected;
        self.inner.attempt_counter.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// Current connection state, as an owned snapshot.
    ///
    /// The read-only view of the internal lifecycle enum, so a caller can render
    /// *why* a client is down (e.g. "reconnect exhausted after 3 attempts")
    /// instead of inferring it from the next error.
    ///
    /// Async because the state lives under a `tokio::sync::RwLock`.
    pub async fn connection_state(&self) -> ConnectionSnapshot {
        ConnectionSnapshot::from(&*self.inner.state.read().await)
    }

    /// Server identity from the last successful handshake.
    pub fn server_info(&self) -> Option<&Implementation> {
        self.inner.server_info.get()
    }

    /// Negotiated MCP protocol version.
    pub fn protocol_version(&self) -> Option<&ProtocolVersion> {
        self.inner.protocol_version.get()
    }

    /// Health check — sends an MCP `ping`.
    pub async fn ping(&self) -> Result<(), McpError> {
        let guard = self.inner.service.lock().await;
        let running = guard.as_ref().ok_or(McpError::TransportClosed)?;
        // rmcp 0.16 does not expose a typed `ping()` helper on `Peer<RoleClient>`;
        // construct a PingRequest manually and discard the (empty) server response.
        let req: ClientRequest = PingRequest::default().into();
        running.peer().send_request(req).await.map_err(McpError::from)?;
        Ok(())
    }

    /// Graceful disconnect. Consumes self — no operations can follow.
    ///
    /// Seals the shared inner **before** taking the service lock, so every
    /// clone is dead from this instant: a reconnect that has not started is
    /// refused at its entry, and a handshake already in flight closes the
    /// service it spawned rather than installing it. Re-enabling a server means
    /// a new [`Self::connect`], which builds an unsealed inner.
    pub async fn disconnect(self) -> Result<(), McpError> {
        self.inner
            .closed
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let mut guard = self.inner.service.lock().await;
        if let Some(mut service) = guard.take() {
            // `close_with_timeout` takes `&mut self`; swallow JoinError into `Sdk`.
            service
                .close_with_timeout(Duration::from_secs(5))
                .await
                .map_err(|e| McpError::Sdk(format!("close failed: {e:?}")))?;
        }
        *self.inner.state.write().await = ConnectionState::Disconnected;
        Ok(())
    }

    /// Attempt to reconnect by rebuilding the transport and rerunning handshake.
    /// Honours max_reconnect_attempts and backoff. On exhaustion, transitions to Failed.
    ///
    /// Refuses outright — before the attempt counter, before any spawn — when
    /// the client is sealed or its state already says it is down. This is the
    /// cheap half of the seal; [`Self::install_service`] closes the window this
    /// one cannot see (a handshake already past this point).
    pub(crate) async fn reconnect(&self) -> Result<(), McpError> {
        use std::sync::atomic::Ordering;
        use crate::lifecycle::{apply_jitter, backoff_for_attempt, ConnectionState, MAX_BACKOFF};

        // The flag and the state enum must agree: either alone means "do not
        // resurrect this client".
        if self.inner.is_sealed()
            || matches!(
                *self.inner.state.read().await,
                ConnectionState::Disconnected | ConnectionState::Failed { .. }
            )
        {
            tracing::debug!(
                server_name = %self.inner.config.server_name,
                "MCP reconnect refused: client is closed"
            );
            return Err(McpError::Closed);
        }

        let attempt = self.inner.attempt_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let max = self.inner.config.max_reconnect_attempts;
        if attempt > max {
            let reason = McpError::ReconnectExhausted(max);
            *self.inner.state.write().await = ConnectionState::Failed { reason: McpError::ReconnectExhausted(max) };
            tracing::error!(
                server_name = %self.inner.config.server_name,
                attempts = max,
                "MCP reconnect attempts exhausted"
            );
            return Err(reason);
        }

        // 10% deterministic jitter — uses attempt as a poor-person's jitter seed.
        let rand_factor = ((attempt as f64 * 0.37).fract() * 2.0) - 1.0;
        let base = backoff_for_attempt(attempt, self.inner.config.reconnect_backoff_ms, MAX_BACKOFF);
        let delay = apply_jitter(base, rand_factor);

        tracing::warn!(
            server_name = %self.inner.config.server_name,
            attempt,
            delay_ms = delay.as_millis(),
            "MCP reconnect scheduled"
        );

        // Drop the old service (if any) so its subprocess/HTTP connection is released.
        if let Some(mut old) = self.inner.service.lock().await.take() {
            let _ = old.close_with_timeout(Duration::from_secs(2)).await;
        }
        *self.inner.state.write().await = ConnectionState::Reconnecting {
            attempt,
            next_at: std::time::Instant::now() + delay,
        };

        tokio::time::sleep(delay).await;

        // Re-handshake.
        self.do_handshake().await?;
        Ok(())
    }

    /// List all tools the server exposes.
    pub async fn list_tools(
        &self,
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Vec<rmcp::model::Tool>, McpError> {
        use std::sync::atomic::Ordering;
        loop {
            let op = async {
                let guard = self.inner.service.lock().await;
                let running = guard.as_ref().ok_or(McpError::TransportClosed)?;
                let result = running.list_all_tools().await.map_err(McpError::from)?;
                Ok::<_, McpError>(result)
            };
            match with_cancel_and_timeout(op, cancel_token, self.inner.config.request_timeout).await {
                Ok(tools) => {
                    self.inner.attempt_counter.store(0, Ordering::Relaxed);
                    return Ok(tools);
                }
                Err(e) if e.is_cancelled() => return Err(e),
                Err(e) if e.is_retriable() => {
                    tracing::warn!(
                        server_name = %self.inner.config.server_name,
                        operation = "list_tools",
                        error = %e,
                        "operation failed; triggering reconnect"
                    );
                    self.reconnect().await?;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Invoke a tool.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
        use std::sync::atomic::Ordering;
        let params = rmcp::model::CallToolRequestParams {
            meta: None,
            name: name.to_string().into(),
            arguments: match arguments {
                serde_json::Value::Object(map) => Some(map),
                serde_json::Value::Null => None,
                other => {
                    return Err(McpError::InvalidArguments(format!(
                        "call_tool arguments must be an object or null, got: {other}"
                    )));
                }
            },
            task: None,
        };

        loop {
            let op = {
                let params = params.clone();
                async move {
                    let guard = self.inner.service.lock().await;
                    let running = guard.as_ref().ok_or(McpError::TransportClosed)?;
                    let result = running.call_tool(params).await.map_err(McpError::from)?;
                    Ok::<_, McpError>(result)
                }
            };
            match with_cancel_and_timeout(op, cancel_token, self.inner.config.request_timeout).await {
                Ok(result) => {
                    self.inner.attempt_counter.store(0, Ordering::Relaxed);
                    return Ok(result);
                }
                Err(e) if e.is_cancelled() => return Err(e),
                Err(e) if e.is_retriable() => {
                    tracing::warn!(
                        server_name = %self.inner.config.server_name,
                        operation = "call_tool",
                        tool = %name,
                        error = %e,
                        "operation failed; triggering reconnect"
                    );
                    self.reconnect().await?;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// List resources exposed by the server. **P5 feature** — returns an error in P1.
    pub async fn list_resources(
        &self,
        _cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Vec<rmcp::model::Resource>, McpError> {
        Err(McpError::ServerInternal(
            "list_resources not implemented until P5 of the MCP roadmap".into(),
        ))
    }

    /// Read a resource by URI. **P5 feature** — returns an error in P1.
    pub async fn read_resource(
        &self,
        _uri: &str,
        _cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<rmcp::model::ResourceContents, McpError> {
        Err(McpError::ServerInternal(
            "read_resource not implemented until P5 of the MCP roadmap".into(),
        ))
    }

    /// List prompts exposed by the server. **P5 feature** — returns an error in P1.
    pub async fn list_prompts(
        &self,
        _cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Vec<rmcp::model::Prompt>, McpError> {
        Err(McpError::ServerInternal(
            "list_prompts not implemented until P5 of the MCP roadmap".into(),
        ))
    }

    /// Materialise a prompt. **P5 feature** — returns an error in P1.
    pub async fn get_prompt(
        &self,
        _name: &str,
        _arguments: serde_json::Value,
        _cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Vec<rmcp::model::PromptMessage>, McpError> {
        Err(McpError::ServerInternal(
            "get_prompt not implemented until P5 of the MCP roadmap".into(),
        ))
    }
}

/// Apply per-operation cancellation and timeout to an async operation.
///
/// On cancel: returns `McpError::Cancelled` immediately. (Protocol-level
/// cancellation notification is added in a later refinement; for P1 we
/// bail locally — the rmcp transport will carry an implicit cancellation
/// when the future drops.)
///
/// On timeout: returns `McpError::Timeout(duration)`.
async fn with_cancel_and_timeout<F, T>(
    op: F,
    cancel_token: Option<&tokio_util::sync::CancellationToken>,
    timeout: Duration,
) -> Result<T, McpError>
where
    F: std::future::Future<Output = Result<T, McpError>>,
{
    let timed = tokio::time::timeout(timeout, async {
        match cancel_token {
            Some(ct) => tokio::select! {
                biased;
                _ = ct.cancelled() => Err(McpError::Cancelled),
                result = op => result,
            },
            None => op.await,
        }
    });

    match timed.await {
        Ok(result) => result,
        Err(_elapsed) => Err(McpError::Timeout(timeout)),
    }
}

/// Internal: materialise a Transport impl from a TransportKind.
fn build_transport(cfg: &McpClientConfig) -> Result<Box<dyn Transport>, McpError> {
    use crate::transport::{StdioTransport, StreamableHttpTransport};
    match &cfg.transport {
        TransportKind::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let mut t = StdioTransport::new(&cfg.server_name, command).with_args(args.clone());
            for (k, v) in env {
                t = t.with_env(k, v);
            }
            if let Some(cwd) = cwd {
                t = t.with_cwd(cwd.clone());
            }
            Ok(Box::new(t))
        }
        TransportKind::Http {
            url,
            auth,
            extra_headers,
        } => {
            let mut t = StreamableHttpTransport::new(&cfg.server_name, url.clone());
            match auth {
                Some(crate::transport::HttpAuth::Bearer(token)) => t = t.with_bearer(token),
                Some(crate::transport::HttpAuth::ApiKey { header, value }) => {
                    t = t.with_api_key(header, value);
                }
                None => {}
            }
            for (k, v) in extra_headers {
                t = t.with_header(k, v);
            }
            Ok(Box::new(t))
        }
    }
}

/// Internal: hand a TransportConnection to rmcp's serve().
async fn serve_with_conn(
    conn: TransportConnection,
) -> Result<RunningService<RoleClient, ()>, Box<dyn std::error::Error + Send + Sync>> {
    match conn.inner {
        TransportInner::Stdio(child) => Ok(().serve(child).await?),
        TransportInner::Http(http) => Ok(().serve(http).await?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stdio "server" that records every spawn and exits immediately: the
    /// handshake always fails, but the pid log proves whether a child was
    /// spawned at all. Returns `(script, spawn log)`.
    #[cfg(unix)]
    fn spawn_recording_server(dir: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let log = dir.join("spawns.log");
        let script = dir.join("stub-server.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$$\" >> '{}'\nexit 0\n", log.display()),
        )
        .expect("write stub server");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub server");
        std::fs::write(&log, "").expect("create spawn log");
        (script, log)
    }

    #[cfg(unix)]
    fn spawn_count(log: &std::path::Path) -> usize {
        std::fs::read_to_string(log)
            .expect("read spawn log")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    }

    #[cfg(unix)]
    fn stdio_client(command: &std::path::Path, state: ConnectionState) -> McpClient {
        let cfg = McpClientConfig {
            server_name: "sealed".into(),
            transport: TransportKind::Stdio {
                command: command.to_string_lossy().into_owned(),
                args: vec![],
                env: Default::default(),
                cwd: None,
            },
            request_timeout: Duration::from_secs(5),
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1,
            ..McpClientConfig::default()
        };
        McpClient {
            inner: Arc::new(ClientInner::new(cfg, state)),
        }
    }

    /// Minimal in-process MCP server over one half of a duplex pipe: answers the
    /// client's `initialize` request with a valid `InitializeResult`, then reads
    /// until EOF. Resolves to `true` once the client end has gone away — which is
    /// how a test observes that a `RunningService` was really closed.
    async fn stub_initialize_server(stream: tokio::io::DuplexStream) -> bool {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (read_half, mut write_half) = tokio::io::split(stream);
        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<rmcp::model::ClientJsonRpcMessage>(&line) else {
                continue;
            };
            let Some((_request, id)) = msg.into_request() else {
                continue; // the `initialized` notification carries no id
            };
            let result =
                rmcp::model::ServerResult::InitializeResult(rmcp::model::InitializeResult {
                    protocol_version: ProtocolVersion::default(),
                    capabilities: Default::default(),
                    server_info: Implementation {
                        name: "stub".into(),
                        version: "0.0.1".into(),
                        ..Default::default()
                    },
                    instructions: None,
                });
            let mut out =
                serde_json::to_string(&rmcp::model::ServerJsonRpcMessage::response(result, id))
                    .expect("serialize initialize result");
            out.push('\n');
            if write_half.write_all(out.as_bytes()).await.is_err() {
                return false;
            }
        }
        true
    }

    /// Bug D: a client the owner disabled must not resurrect its child. After
    /// `disconnect`, a held clone's `call_tool` refuses with the non-retriable
    /// `Closed` and never reaches `transport.connect()`.
    #[cfg(unix)]
    #[tokio::test]
    async fn sealed_client_refuses_call_tool_and_never_respawns_the_child() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (script, log) = spawn_recording_server(tmp.path());
        let client = stdio_client(&script, ConnectionState::Connected);

        // Control: while the client is live, a call against a dropped transport
        // really does reconnect and really does spawn the child. Without this the
        // assertion below could pass for the wrong reason.
        let _ = client.call_tool("any", serde_json::json!({}), None).await;
        assert_eq!(
            spawn_count(&log),
            1,
            "control: a live client must respawn the child"
        );

        // Seal it. `disconnect` consumes its own clone; `client` is the stale
        // snapshot an extension registry would still be holding.
        client.clone().disconnect().await.expect("disconnect");

        let err = client
            .call_tool("any", serde_json::json!({}), None)
            .await
            .expect_err("a sealed client must refuse");
        assert!(
            matches!(err, McpError::Closed),
            "expected Closed, got {err:?}"
        );
        assert!(!err.is_retriable(), "Closed must never be retriable");
        assert_eq!(
            spawn_count(&log),
            1,
            "a sealed client must not spawn a child"
        );
    }

    /// The same seal on `list_tools`, the other retry loop.
    #[cfg(unix)]
    #[tokio::test]
    async fn sealed_client_refuses_list_tools_and_never_respawns_the_child() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (script, log) = spawn_recording_server(tmp.path());
        let client = stdio_client(&script, ConnectionState::Connected);

        client.clone().disconnect().await.expect("disconnect");

        let err = client
            .list_tools(None)
            .await
            .expect_err("a sealed client must refuse");
        assert!(
            matches!(err, McpError::Closed),
            "expected Closed, got {err:?}"
        );
        assert_eq!(
            spawn_count(&log),
            0,
            "a sealed client must not spawn a child"
        );
    }

    /// The in-flight window: a handshake that started before the disable reaches
    /// its install point after the seal. It must close the service it just
    /// spawned instead of publishing it into the sealed client.
    #[tokio::test]
    async fn handshake_finishing_after_disconnect_never_installs_a_live_service() {
        let client = McpClient {
            inner: Arc::new(ClientInner::new(
                McpClientConfig {
                    server_name: "raced".into(),
                    ..McpClientConfig::default()
                },
                ConnectionState::Connected,
            )),
        };

        // A genuine, completed rmcp handshake — the service is live.
        let (client_end, server_end) = tokio::io::duplex(8 * 1024);
        let server = tokio::spawn(stub_initialize_server(server_end));
        let running = ().serve(client_end).await.expect("stub handshake");
        assert!(
            running.peer_info().is_some(),
            "handshake should have peer info"
        );

        // The race: `disconnect` takes and releases the service lock while the
        // handshake is still in flight (service is `None`, so it sees nothing to
        // close), and only then does the handshake reach its install point.
        client.clone().disconnect().await.expect("disconnect");

        let err = client
            .install_service(running)
            .await
            .expect_err("install into a sealed client must be refused");
        assert!(
            matches!(err, McpError::Closed),
            "expected Closed, got {err:?}"
        );
        assert!(
            client.inner.service.lock().await.is_none(),
            "a sealed client must hold no service"
        );
        assert_eq!(
            client.connection_state().await,
            ConnectionSnapshot::Disconnected,
            "the seal must not resurrect the state either"
        );

        // The service was closed, not leaked: its peer sees the connection go away.
        let closed = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the just-spawned service should have been closed")
            .expect("stub server task");
        assert!(
            closed,
            "the just-spawned service must be closed, not installed"
        );
    }

    /// The flag and the state enum can never disagree: `reconnect` refuses from
    /// `Disconnected` and `Failed{..}` even when the seal was never stored.
    #[tokio::test]
    async fn reconnect_refuses_from_disconnected_and_failed_states() {
        for state in [
            ConnectionState::Disconnected,
            ConnectionState::Failed {
                reason: McpError::ReconnectExhausted(3),
            },
        ] {
            let client = McpClient {
                inner: Arc::new(ClientInner::new(
                    McpClientConfig {
                        max_reconnect_attempts: 3,
                        reconnect_backoff_ms: 1,
                        ..McpClientConfig::default()
                    },
                    state,
                )),
            };
            let err = client.reconnect().await.expect_err("reconnect must refuse");
            assert!(
                matches!(err, McpError::Closed),
                "expected Closed, got {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn connection_state_exposes_an_owned_snapshot() {
        let client = McpClient {
            inner: Arc::new(ClientInner::new(
                McpClientConfig::default(),
                ConnectionState::Connected,
            )),
        };
        assert_eq!(
            client.connection_state().await,
            ConnectionSnapshot::Connected
        );

        *client.inner.state.write().await = ConnectionState::Reconnecting {
            attempt: 2,
            next_at: std::time::Instant::now(),
        };
        assert_eq!(
            client.connection_state().await,
            ConnectionSnapshot::Reconnecting { attempt: 2 }
        );

        *client.inner.state.write().await = ConnectionState::Failed {
            reason: McpError::ReconnectExhausted(3),
        };
        assert_eq!(
            client.connection_state().await,
            ConnectionSnapshot::Failed {
                reason: "reconnect attempts exhausted (tried 3)".into()
            }
        );

        client.clone().disconnect().await.expect("disconnect");
        assert_eq!(
            client.connection_state().await,
            ConnectionSnapshot::Disconnected
        );
    }

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = McpClientConfig::default();
        assert_eq!(cfg.client_info.name, "openalpaca-mcp");
        assert_eq!(cfg.request_timeout, Duration::from_secs(30));
        assert_eq!(cfg.max_reconnect_attempts, 3);
        assert_eq!(cfg.reconnect_backoff_ms, 100);
    }

    #[tokio::test]
    async fn connect_fails_for_nonexistent_command() {
        let cfg = McpClientConfig {
            server_name: "bad".into(),
            transport: TransportKind::Stdio {
                command: "/definitely/not/a/command/xyzzy999".into(),
                args: vec![],
                env: Default::default(),
                cwd: None,
            },
            client_info: Implementation {
                name: "openalpaca-mcp-test".into(),
                version: "0.1.0".into(),
                ..Default::default()
            },
            request_timeout: Duration::from_secs(1),
            max_reconnect_attempts: 0,
            reconnect_backoff_ms: 100,
        };
        // `McpClient` doesn't implement `Debug` (RunningService isn't Debug in our config),
        // so avoid `.unwrap_err()` and match manually.
        match McpClient::connect(cfg).await {
            Ok(_) => panic!("expected connect() to fail for nonexistent command"),
            Err(err) => assert!(
                matches!(err, McpError::Transport(_) | McpError::HandshakeFailed(_)),
                "unexpected error: {err:?}"
            ),
        }
    }

    #[tokio::test]
    async fn call_tool_rejects_non_object_arguments() {
        let cfg = McpClientConfig::default();
        let inner = Arc::new(ClientInner::new(cfg.clone(), ConnectionState::Connected));
        let client = McpClient { inner };

        // Array argument should be rejected.
        let err = client
            .call_tool("any", serde_json::json!([1, 2, 3]), None)
            .await
            .expect_err("expected error");
        assert!(
            matches!(err, McpError::InvalidArguments(_)),
            "expected InvalidArguments, got {err:?}"
        );
    }

    #[tokio::test]
    async fn list_tools_fails_when_not_connected() {
        // max_reconnect_attempts = 0 so the operation returns immediately
        // without trying to reconnect. (Task 11 wraps list_tools in a retry loop;
        // this test would otherwise exhaust retries instead of surfacing TransportClosed.)
        //
        // The state is `Connected` with no service installed: "not connected" in
        // the transport sense, but not sealed. A client whose *state* is
        // `Disconnected` is covered by
        // `reconnect_refuses_from_disconnected_and_failed_states`, which now
        // refuses with `Closed` instead of retrying.
        let cfg = McpClientConfig {
            max_reconnect_attempts: 0,
            ..McpClientConfig::default()
        };
        let inner = Arc::new(ClientInner::new(cfg.clone(), ConnectionState::Connected));
        let client = McpClient { inner };

        let err = client.list_tools(None).await.expect_err("expected error");
        // Accept either: pre-Task-11 returns TransportClosed; post-Task-11 returns
        // ReconnectExhausted(0) because max_attempts=0 means "don't retry at all".
        assert!(
            matches!(err, McpError::TransportClosed | McpError::ReconnectExhausted(0)),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_tools_cancels_immediately_when_token_pre_cancelled() {
        let cfg = McpClientConfig::default();
        let inner = Arc::new(ClientInner::new(cfg.clone(), ConnectionState::Connected));
        let client = McpClient { inner };

        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();
        let err = client
            .list_tools(Some(&token))
            .await
            .expect_err("expected error");
        // Either Cancelled (if select chose cancel first) or TransportClosed (if service check ran first).
        // Biased select should prefer Cancelled.
        assert!(
            matches!(err, McpError::Cancelled | McpError::TransportClosed),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test]
    async fn stub_methods_return_server_internal() {
        let cfg = McpClientConfig::default();
        let inner = Arc::new(ClientInner::new(cfg, ConnectionState::Connected));
        let client = McpClient { inner };

        for result in [
            client.list_resources(None).await.map(|_| ()),
            client.read_resource("mem://x", None).await.map(|_| ()),
            client.list_prompts(None).await.map(|_| ()),
            client.get_prompt("x", serde_json::json!({}), None).await.map(|_| ()),
        ] {
            let err = result.expect_err("expected error");
            assert!(
                matches!(err, McpError::ServerInternal(_)),
                "expected ServerInternal, got {err:?}"
            );
            assert!(err.to_string().contains("P5"), "msg should mention P5: {err}");
        }
    }

    #[tokio::test]
    async fn reconnect_transitions_to_failed_after_max_attempts() {
        // Construct a client that will always fail to reconnect (bad command).
        let cfg = McpClientConfig {
            server_name: "bad".into(),
            transport: TransportKind::Stdio {
                command: "/definitely/not/a/command/xyzzy".into(),
                args: vec![],
                env: Default::default(),
                cwd: None,
            },
            client_info: Implementation {
                name: "openalpaca-mcp-test".into(),
                version: "0.1.0".into(),
                ..Default::default()
            },
            request_timeout: Duration::from_secs(1),
            max_reconnect_attempts: 2,
            reconnect_backoff_ms: 10, // fast test
        };

        // A live client whose call just failed retriably — the only state from
        // which reconnect is allowed to run at all.
        let inner = Arc::new(ClientInner::new(cfg.clone(), ConnectionState::Connected));
        let client = McpClient { inner };

        // Attempt 1: should fail trying to spawn but stay below exhaustion.
        let _ = client.reconnect().await;
        // Attempt 2: also fails.
        let _ = client.reconnect().await;
        // Attempt 3: exceeds max=2, should return ReconnectExhausted.
        let err = client.reconnect().await.expect_err("expected error");
        assert!(
            matches!(err, McpError::ReconnectExhausted(2)),
            "unexpected error: {err:?}"
        );
        // State should be Failed.
        let state = client.inner.state.read().await;
        assert!(matches!(&*state, ConnectionState::Failed { .. }), "expected Failed state");
    }
}
