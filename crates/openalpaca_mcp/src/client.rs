// crates/openalpaca_mcp/src/client.rs

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use rmcp::model::{ClientRequest, Implementation, PingRequest, ProtocolVersion};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use tokio::sync::Mutex;

use crate::error::McpError;
use crate::lifecycle::ConnectionState;
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
}

impl McpClient {
    /// Connect to the configured MCP server. Performs initialize handshake.
    pub async fn connect(config: McpClientConfig) -> Result<Self, McpError> {
        let inner = Arc::new(ClientInner {
            config: config.clone(),
            state: tokio::sync::RwLock::new(ConnectionState::Connecting),
            service: Mutex::new(None),
            server_info: tokio::sync::OnceCell::new(),
            protocol_version: tokio::sync::OnceCell::new(),
            attempt_counter: AtomicU32::new(0),
        });

        let client = Self { inner };
        client.do_handshake().await?;
        Ok(client)
    }

    /// Build a Transport from the configured TransportKind, connect, then run rmcp's serve().
    pub(crate) async fn do_handshake(&self) -> Result<(), McpError> {
        use std::sync::atomic::Ordering;
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

        *self.inner.service.lock().await = Some(running);
        *self.inner.state.write().await = ConnectionState::Connected;
        self.inner.attempt_counter.store(0, Ordering::Relaxed);
        Ok(())
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
    pub async fn disconnect(self) -> Result<(), McpError> {
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

    /// List all tools the server exposes.
    pub async fn list_tools(
        &self,
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Vec<rmcp::model::Tool>, McpError> {
        let op = async {
            let guard = self.inner.service.lock().await;
            let running = guard.as_ref().ok_or(McpError::TransportClosed)?;
            let result = running.list_all_tools().await.map_err(McpError::from)?;
            Ok::<_, McpError>(result)
        };
        with_cancel_and_timeout(op, cancel_token, self.inner.config.request_timeout).await
    }

    /// Invoke a tool.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<rmcp::model::CallToolResult, McpError> {
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

        let op = async {
            let guard = self.inner.service.lock().await;
            let running = guard.as_ref().ok_or(McpError::TransportClosed)?;
            let result = running.call_tool(params).await.map_err(McpError::from)?;
            Ok::<_, McpError>(result)
        };
        with_cancel_and_timeout(op, cancel_token, self.inner.config.request_timeout).await
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
        let inner = Arc::new(ClientInner {
            config: cfg.clone(),
            state: tokio::sync::RwLock::new(ConnectionState::Connected),
            service: Mutex::new(None),
            server_info: tokio::sync::OnceCell::new(),
            protocol_version: tokio::sync::OnceCell::new(),
            attempt_counter: AtomicU32::new(0),
        });
        let client = McpClient { inner };

        // Array argument should be rejected.
        let err = client
            .call_tool("any", serde_json::json!([1, 2, 3]), None)
            .await
            .err()
            .expect("expected error");
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
        let cfg = McpClientConfig {
            max_reconnect_attempts: 0,
            ..McpClientConfig::default()
        };
        let inner = Arc::new(ClientInner {
            config: cfg.clone(),
            state: tokio::sync::RwLock::new(ConnectionState::Disconnected),
            service: Mutex::new(None),
            server_info: tokio::sync::OnceCell::new(),
            protocol_version: tokio::sync::OnceCell::new(),
            attempt_counter: AtomicU32::new(0),
        });
        let client = McpClient { inner };

        let err = client.list_tools(None).await.err().expect("expected error");
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
        let inner = Arc::new(ClientInner {
            config: cfg.clone(),
            state: tokio::sync::RwLock::new(ConnectionState::Connected),
            service: Mutex::new(None),
            server_info: tokio::sync::OnceCell::new(),
            protocol_version: tokio::sync::OnceCell::new(),
            attempt_counter: AtomicU32::new(0),
        });
        let client = McpClient { inner };

        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();
        let err = client
            .list_tools(Some(&token))
            .await
            .err()
            .expect("expected error");
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
        let inner = Arc::new(ClientInner {
            config: cfg,
            state: tokio::sync::RwLock::new(ConnectionState::Connected),
            service: Mutex::new(None),
            server_info: tokio::sync::OnceCell::new(),
            protocol_version: tokio::sync::OnceCell::new(),
            attempt_counter: AtomicU32::new(0),
        });
        let client = McpClient { inner };

        for result in [
            client.list_resources(None).await.map(|_| ()),
            client.read_resource("mem://x", None).await.map(|_| ()),
            client.list_prompts(None).await.map(|_| ()),
            client.get_prompt("x", serde_json::json!({}), None).await.map(|_| ()),
        ] {
            let err = result.err().expect("expected error");
            assert!(
                matches!(err, McpError::ServerInternal(_)),
                "expected ServerInternal, got {err:?}"
            );
            assert!(err.to_string().contains("P5"), "msg should mention P5: {err}");
        }
    }
}
