// crates/openalpaca_mcp/src/client.rs

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::time::Duration;

use rmcp::handler::client::ClientHandler;
use rmcp::model::{ClientRequest, Implementation, PingRequest, ProtocolVersion};
use rmcp::service::{NotificationContext, RoleClient, RunningService, ServiceExt};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::error::McpError;
use crate::lifecycle::{ConnectionSnapshot, ConnectionState};
use crate::transport::{Transport, TransportConnection, TransportInner, TransportKind};

/// Something the *server* changed about itself mid-session, announced over one
/// of MCP's `notifications/*_list_changed` methods.
///
/// rmcp receives all three today and OpenAlpaca's unit handler discarded them
/// (`impl ClientHandler for () {}`), which left two silent holes: a tool the
/// server added answered an unattributed *"not found"*, and one it dropped
/// answered a raw JSON-RPC error on every call (extension design §3.7, X-35).
///
/// Only [`Self::ToolList`] has a consumer: MCP resources and prompts are
/// stubbed. The other two variants exist so that un-stubbing them is a
/// supervisor change, never a second refresh route (§2.3, X-36).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerChange {
    ToolList,
    ResourceList,
    PromptList,
}

/// The `ClientHandler` every connection is served with: it forwards the three
/// list-changed notifications onto the client's own channel and does nothing
/// else (design §3.7 "Client (C2)").
///
/// **The sender is not created per handshake.** `do_handshake` runs on
/// `connect` *and* on every in-session `reconnect`, each time building a fresh
/// `RunningService` and therefore a fresh handler; a per-handshake sender would
/// mean the first respawn dropped the only sender and ended the supervisor's
/// receiver while the server was still enabled. The pair lives on
/// [`ClientInner`] and every handshake clones the sender into the handler it
/// serves.
#[derive(Clone)]
pub struct NotifyingHandler {
    server_name: String,
    tx: Option<UnboundedSender<ServerChange>>,
}

impl NotifyingHandler {
    fn send(&self, change: ServerChange) {
        let Some(tx) = &self.tx else { return };
        if tx.send(change).is_err() {
            // The receiver is gone: either nobody took it (`changes()` is a
            // take-once accessor) or the supervisor's reader task has exited.
            // Neither is an error — the notification simply has no consumer.
            tracing::debug!(
                server_name = %self.server_name,
                ?change,
                "MCP server change dropped: no receiver"
            );
        }
    }
}

impl ClientHandler for NotifyingHandler {
    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.send(ServerChange::ToolList);
        std::future::ready(())
    }

    fn on_resource_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.send(ServerChange::ResourceList);
        std::future::ready(())
    }

    fn on_prompt_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        self.send(ServerChange::PromptList);
        std::future::ready(())
    }
}

/// How long a deliberate [`McpClient::disconnect`] waits for rmcp to finish the
/// graceful close (cancel the service task, close the transport, reap the child).
/// The caller asked for this teardown and is waiting on its result, so it is
/// worth waiting out a slow server.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// The shorter bound used when closing a service nobody will ever use: one
/// spawned by a handshake that finished after the client was sealed, or the
/// superseded service dropped at the start of a reconnect. Nothing downstream
/// depends on the result, rmcp's `DropGuard` finishes the teardown regardless,
/// and both call sites sit on a latency path (a handshake, a retry), so they
/// must not stall on a wedged child.
const ABANDONED_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

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
    pub(crate) service: Mutex<Option<RunningService<RoleClient, NotifyingHandler>>>,
    pub(crate) server_info: tokio::sync::OnceCell<Implementation>,
    pub(crate) protocol_version: tokio::sync::OnceCell<ProtocolVersion>,
    pub(crate) attempt_counter: AtomicU32,
    /// The sender half of the server-change channel, created **once** with the
    /// client and cloned into every handshake's handler. `disconnect` `take()`s
    /// it beside the service, so the supervisor's receiver ends when — and only
    /// when — T4 closes the client (design §3.7).
    pub(crate) changes_tx: Mutex<Option<UnboundedSender<ServerChange>>>,
    /// The receiver half, parked here between `connect` and E2 and handed out
    /// once by [`McpClient::changes`].
    pub(crate) changes_rx: Mutex<Option<UnboundedReceiver<ServerChange>>>,
    /// The close seal. Set once, in [`McpClient::disconnect`], **before** that
    /// method takes the service lock; never cleared. A sealed client refuses to
    /// reconnect and refuses to publish a service, so a clone that outlives the
    /// disconnect (a stale extension snapshot, an in-flight retry loop) can
    /// never respawn the child the owner just disabled.
    pub(crate) closed: AtomicBool,
}

impl ClientInner {
    /// A fresh inner in `state`, never sealed.
    ///
    /// The server-change channel is created here so that both constructors —
    /// [`McpClient::connect`] and `disconnected_for_tests` — carry an
    /// identically initialised pair, and so that it outlives every in-session
    /// `reconnect` (design §3.7).
    pub(crate) fn new(config: McpClientConfig, state: ConnectionState) -> Self {
        let (changes_tx, changes_rx) = unbounded_channel();
        Self {
            config,
            state: tokio::sync::RwLock::new(state),
            service: Mutex::new(None),
            server_info: tokio::sync::OnceCell::new(),
            protocol_version: tokio::sync::OnceCell::new(),
            attempt_counter: AtomicU32::new(0),
            closed: AtomicBool::new(false),
            changes_tx: Mutex::new(Some(changes_tx)),
            changes_rx: Mutex::new(Some(changes_rx)),
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

        // Clone — never create — the sender: a per-handshake channel would be
        // severed by the first in-session reconnect (design §3.7).
        let handler = NotifyingHandler {
            server_name: self.inner.config.server_name.clone(),
            tx: self.inner.changes_tx.lock().await.clone(),
        };

        let running = serve_with_conn(conn, handler).await.map_err(|e| {
            // A handshake against a streamable-HTTP server that rejected our
            // credentials is the one bring-up failure that carries a status
            // (design §4.2); everything else stays a handshake failure.
            match &e {
                rmcp::service::ClientInitializeError::TransportError { error, .. } => {
                    match crate::error::unauthorized_status(error.error.as_ref()) {
                        Some(status) => McpError::Unauthorized(status),
                        None => McpError::HandshakeFailed(format!("rmcp serve() failed: {e:?}")),
                    }
                }
                _ => McpError::HandshakeFailed(format!("rmcp serve() failed: {e:?}")),
            }
        })?;

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
    async fn install_service(
        &self,
        mut running: RunningService<RoleClient, NotifyingHandler>,
    ) -> Result<(), McpError> {
        use std::sync::atomic::Ordering;
        let mut guard = self.inner.service.lock().await;
        if self.inner.is_sealed() {
            drop(guard);
            tracing::warn!(
                server_name = %self.inner.config.server_name,
                "MCP handshake completed after disconnect; closing the service it spawned"
            );
            // `close_with_timeout` takes `&mut self`. The outcome cannot change
            // what happens next — the service is abandoned either way, and
            // rmcp's `DropGuard` finishes the teardown when `running` drops —
            // but a close that fails on the seal path is exactly what an
            // incident would need to see.
            if let Err(e) = running.close_with_timeout(ABANDONED_CLOSE_TIMEOUT).await {
                tracing::warn!(
                    server_name = %self.inner.config.server_name,
                    error = ?e,
                    "closing the abandoned MCP service failed; rmcp's drop guard will finish the teardown"
                );
            }
            return Err(McpError::Closed);
        }
        // Publish the service and record the state **under the same guard**.
        // Releasing the lock in between would let a `disconnect` seal, take and
        // close the service and write `Disconnected`, only for the write below
        // to overwrite it back to `Connected` — a sealed, service-less client
        // reporting a live connection. (Lock order is service → state
        // throughout the client: `disconnect` does the same, and no path takes
        // them the other way round, so there is no inversion here.)
        *guard = Some(running);
        *self.inner.state.write().await = ConnectionState::Connected;
        self.inner.attempt_counter.store(0, Ordering::Relaxed);
        drop(guard);
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

    /// Take the server-change receiver. **Once** — a second call returns
    /// `None`.
    ///
    /// The supervisor calls this at E2, with the client, and spawns the
    /// per-server reader task on the result. The channel ends when `disconnect`
    /// takes the sender at T4, so a notification can never outlive the load
    /// that produced it, while a `reconnect` *inside* that load keeps
    /// delivering (design §3.7, §3.3 E5).
    pub async fn changes(&self) -> Option<UnboundedReceiver<ServerChange>> {
        self.inner.changes_rx.lock().await.take()
    }

    /// One-shot `tools/list`: the `op` future of [`Self::list_tools`] under
    /// `request_timeout`, returning the typed error and **never entering
    /// `reconnect()`**.
    ///
    /// Two callers, for the same reason — bounded latency:
    ///
    /// * **E3**, where a client that has just handshaken has no reconnect to
    ///   make, so bring-up stays bounded by `connect_timeout_secs` plus one
    ///   `request_timeout` rather than up to four reconnect cycles under the
    ///   supervisor's per-extension mutex;
    /// * **§3.7 step 2**, the tool-list refresh, where the plain `list_tools`
    ///   would hold its handles for minutes against a dying server.
    pub async fn list_tools_once(&self) -> Result<Vec<rmcp::model::Tool>, McpError> {
        let op = async {
            let guard = self.inner.service.lock().await;
            let running = guard.as_ref().ok_or(McpError::TransportClosed)?;
            let result = running.list_all_tools().await.map_err(McpError::from)?;
            Ok::<_, McpError>(result)
        };
        with_cancel_and_timeout(op, None, self.inner.config.request_timeout).await
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
        let service = guard.take();
        // Take the change sender beside the service: dropping the last sender
        // ends the supervisor's receiver, which is how the per-server reader
        // task learns the load is over (design §3.7, §3.3 E-FAIL).
        drop(self.inner.changes_tx.lock().await.take());
        // Record the state before the close, not after: once the service has
        // been taken out of a sealed client there is no outcome of the close
        // that makes the client anything other than disconnected, and a close
        // that errors must not leave `connection_state()` reporting the
        // connection it just tore down. Still under the service guard, so a
        // handshake finishing concurrently cannot interleave with it.
        *self.inner.state.write().await = ConnectionState::Disconnected;
        if let Some(mut service) = service {
            // `close_with_timeout` takes `&mut self`; swallow JoinError into `Sdk`.
            service
                .close_with_timeout(CLOSE_TIMEOUT)
                .await
                .map_err(|e| McpError::Sdk(format!("close failed: {e:?}")))?;
        }
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
            let _ = old.close_with_timeout(ABANDONED_CLOSE_TIMEOUT).await;
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

/// Internal: hand a TransportConnection to rmcp's serve(), with the handler
/// that forwards the server's list-changed notifications (design §3.7).
async fn serve_with_conn(
    conn: TransportConnection,
    handler: NotifyingHandler,
) -> Result<RunningService<RoleClient, NotifyingHandler>, rmcp::service::ClientInitializeError> {
    match conn.inner {
        TransportInner::Stdio(child) => handler.serve(child).await,
        TransportInner::Http(http) => handler.serve(http).await,
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

    /// Wraps a transport so a test can tell a real `close()` from a bare drop,
    /// and can make that close fail.
    ///
    /// Only `close_with_timeout` awaits the rmcp service task to completion, and
    /// that task calls `Transport::close` on its way out — so `closed` is
    /// guaranteed to be set by the time the call that closed the service
    /// returns. A bare `drop(running)` tears the service down asynchronously
    /// (rmcp's `DropGuard`), leaving the flag false at that instant. That is
    /// what makes an assertion on this flag mutation-proof where an assertion
    /// on the peer's EOF is not: both a close and a drop end in EOF eventually.
    struct WatchedTransport<T> {
        inner: T,
        closed: Arc<AtomicBool>,
        /// When set, `close()` panics — the rmcp service task then ends in a
        /// `JoinError`, which is the one way `close_with_timeout` is observably
        /// fallible (a timeout is reported as `Ok(None)`).
        fail_close: bool,
    }

    /// What the client end of an rmcp connection reads.
    type ClientRx = Option<rmcp::service::RxJsonRpcMessage<RoleClient>>;

    impl<T> rmcp::transport::Transport<RoleClient> for WatchedTransport<T>
    where
        T: rmcp::transport::Transport<RoleClient> + Send,
    {
        type Error = T::Error;

        fn send(
            &mut self,
            item: rmcp::service::TxJsonRpcMessage<RoleClient>,
        ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
            self.inner.send(item)
        }

        fn receive(&mut self) -> impl std::future::Future<Output = ClientRx> + Send {
            self.inner.receive()
        }

        fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
            self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
            let fail = self.fail_close;
            let inner = self.inner.close();
            async move {
                let result = inner.await;
                assert!(
                    !fail,
                    "WatchedTransport: deliberate close failure (expected by this test)"
                );
                result
            }
        }
    }

    fn watched<T, E, A>(
        transport: T,
        closed: Arc<AtomicBool>,
        fail_close: bool,
    ) -> WatchedTransport<impl rmcp::transport::Transport<RoleClient, Error = E> + 'static>
    where
        T: rmcp::transport::IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        WatchedTransport {
            inner: transport.into_transport(),
            closed,
            fail_close,
        }
    }

    /// A genuine, completed rmcp handshake against the in-process stub.
    struct LiveService {
        running: RunningService<RoleClient, NotifyingHandler>,
        /// Resolves to `true` once the client end has gone away.
        server: tokio::task::JoinHandle<bool>,
        /// Set the moment the transport is really closed (see [`WatchedTransport`]).
        closed: Arc<AtomicBool>,
    }

    async fn live_service(fail_close: bool) -> LiveService {
        live_service_with(fail_close, None).await
    }

    async fn live_service_with(
        fail_close: bool,
        tx: Option<UnboundedSender<ServerChange>>,
    ) -> LiveService {
        let (client_end, server_end) = tokio::io::duplex(8 * 1024);
        let server = tokio::spawn(stub_initialize_server(server_end));
        let closed = Arc::new(AtomicBool::new(false));
        let transport = watched(client_end, closed.clone(), fail_close);
        let handler = NotifyingHandler {
            server_name: "stub".into(),
            tx,
        };
        let running = handler.serve(transport).await.expect("stub handshake");
        assert!(
            running.peer_info().is_some(),
            "handshake should have peer info"
        );
        LiveService {
            running,
            server,
            closed,
        }
    }

    fn test_client(server_name: &str, state: ConnectionState) -> McpClient {
        McpClient {
            inner: Arc::new(ClientInner::new(
                McpClientConfig {
                    server_name: server_name.into(),
                    ..McpClientConfig::default()
                },
                state,
            )),
        }
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
        let client = test_client("raced", ConnectionState::Connected);

        // A genuine, completed rmcp handshake — the service is live.
        let live = live_service(false).await;

        // The race: `disconnect` takes and releases the service lock while the
        // handshake is still in flight (service is `None`, so it sees nothing to
        // close), and only then does the handshake reach its install point.
        client.clone().disconnect().await.expect("disconnect");

        let err = client
            .install_service(live.running)
            .await
            .expect_err("install into a sealed client must be refused");
        // Checked before anything else awaits: the sealed branch must have
        // *closed* the service — `close_with_timeout` awaits the service task,
        // which sets this flag on its way out. A bare drop would only cancel in
        // the background and leave the flag false here.
        assert!(
            live.closed.load(std::sync::atomic::Ordering::SeqCst),
            "the sealed install must close the service it was handed, not merely drop it"
        );
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
        let closed = tokio::time::timeout(Duration::from_secs(5), live.server)
            .await
            .expect("the just-spawned service should have been closed")
            .expect("stub server task");
        assert!(
            closed,
            "the just-spawned service must be closed, not installed"
        );
    }

    /// Finding 1(a). Publishing a service and recording `Connected` must be one
    /// step as far as `disconnect` is concerned: both happen under the service
    /// guard. Otherwise a `disconnect` can slot in between them — seal, take and
    /// close the service, write `Disconnected` — and the publish then overwrites
    /// the state back to `Connected`, so `connection_state()` reports
    /// `Connected` for a sealed, service-less client.
    ///
    /// The overwrite itself cannot be forced from a test: tokio's `RwLock` is
    /// FIFO, so a `disconnect` that queues on the state write always lands after
    /// a publish that queued first, and the only real interleaving is a thread
    /// preemption between the guard drop and the state write. The guard is the
    /// invariant that closes the window, so that is what this asserts — while
    /// the publish is unfinished, the lock a racing `disconnect` must take first
    /// is not available.
    #[tokio::test]
    async fn install_publishes_the_service_and_its_state_under_one_guard() {
        let client = test_client("racy-publish", ConnectionState::Connecting);
        let live = live_service(false).await;

        // Park the publish on its state write by holding the state lock.
        let state_guard = client.inner.state.write().await;
        let installer = tokio::spawn({
            let client = client.clone();
            let running = live.running;
            async move { client.install_service(running).await }
        });
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert!(
            client.inner.service.try_lock().is_err(),
            "the publish must still hold the service lock while its state write is pending; \
             a `disconnect` that got the lock here would be overwritten back to Connected"
        );

        // Now let the two race for real: the disconnect seals immediately and
        // blocks on the service lock the publish holds.
        let disconnecter = tokio::spawn({
            let client = client.clone();
            async move { client.disconnect().await }
        });
        drop(state_guard);

        installer
            .await
            .expect("installer task")
            .expect("install must succeed: the seal came after it took the lock");
        disconnecter
            .await
            .expect("disconnecter task")
            .expect("disconnect");

        assert_eq!(
            client.connection_state().await,
            ConnectionSnapshot::Disconnected,
            "a publish racing a disconnect must not leave the client reporting Connected"
        );
        assert!(
            client.inner.service.lock().await.is_none(),
            "the disconnect must have taken the published service"
        );
        assert!(
            live.closed.load(std::sync::atomic::Ordering::SeqCst),
            "the published service must have been closed by the disconnect"
        );
    }

    /// Finding 1(b). A `disconnect` whose close fails still reports a
    /// disconnected client: the error must not skip the state write on a client
    /// that is sealed and service-less either way.
    #[tokio::test]
    async fn disconnect_reports_disconnected_even_when_the_close_fails() {
        let client = test_client("bad-close", ConnectionState::Connected);
        let live = live_service(true).await;
        *client.inner.service.lock().await = Some(live.running);

        let err = client
            .clone()
            .disconnect()
            .await
            .expect_err("the rigged close must fail");
        assert!(
            matches!(err, McpError::Sdk(_)),
            "expected Sdk(close failed), got {err:?}"
        );
        assert_eq!(
            client.connection_state().await,
            ConnectionSnapshot::Disconnected,
            "a failed close must still leave a sealed client reporting Disconnected"
        );
        assert!(
            client.inner.service.lock().await.is_none(),
            "the service must be gone even when its close failed"
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

    /// §3.7: the receiver is handed out **once**, and the notification the
    /// server pushes reaches it through the handler every handshake clones.
    #[tokio::test]
    async fn changes_is_a_take_once_accessor_that_carries_the_notification() {
        let client = test_client("notifier", ConnectionState::Connected);
        let mut rx = client.changes().await.expect("first take");
        assert!(
            client.changes().await.is_none(),
            "the receiver is handed out once"
        );

        // A handler built the way `do_handshake` builds one.
        let handler = NotifyingHandler {
            server_name: "notifier".into(),
            tx: client.inner.changes_tx.lock().await.clone(),
        };
        handler.send(ServerChange::ToolList);
        handler.send(ServerChange::ResourceList);
        assert_eq!(rx.recv().await, Some(ServerChange::ToolList));
        assert_eq!(rx.recv().await, Some(ServerChange::ResourceList));
    }

    /// The sender lives on `ClientInner`, not on the per-handshake handler, so
    /// an in-session reconnect — the same incarnation — keeps delivering, and
    /// only `disconnect` ends the receiver (§3.7, §3.3 E5).
    #[tokio::test]
    async fn the_change_channel_survives_a_reconnect_and_ends_only_at_disconnect() {
        let client = test_client("notifier", ConnectionState::Connected);
        let mut rx = client.changes().await.expect("take");

        // Two successive handshakes' handlers, both cloning the same sender.
        for _ in 0..2 {
            let handler = NotifyingHandler {
                server_name: "notifier".into(),
                tx: client.inner.changes_tx.lock().await.clone(),
            };
            handler.send(ServerChange::ToolList);
            assert_eq!(rx.recv().await, Some(ServerChange::ToolList));
            drop(handler);
        }

        client.clone().disconnect().await.expect("disconnect");
        assert_eq!(
            rx.recv().await,
            None,
            "disconnect takes the sender, so the reader task's receiver ends"
        );
    }

    /// The one-shot fetch never enters `reconnect()`: no child is spawned even
    /// though the client is live and the error is retriable (§3.7 step 2).
    #[cfg(unix)]
    #[tokio::test]
    async fn list_tools_once_never_reconnects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (script, log) = spawn_recording_server(tmp.path());
        let client = stdio_client(&script, ConnectionState::Connected);

        let err = client
            .list_tools_once()
            .await
            .expect_err("no service is installed");
        assert!(
            matches!(err, McpError::TransportClosed),
            "expected the typed error, got {err:?}"
        );
        assert_eq!(
            spawn_count(&log),
            0,
            "list_tools_once must not spawn a child"
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
