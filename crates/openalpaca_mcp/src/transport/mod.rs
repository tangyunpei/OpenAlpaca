// crates/openalpaca_mcp/src/transport/mod.rs

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use url::Url;

use crate::error::McpError;

mod stdio;
mod http;

// pub use http::StreamableHttpTransport; // implemented in Task 5.
pub use stdio::StdioTransport;

/// Abstraction over how bytes reach an MCP server.
///
/// Implementations produce a connected [`TransportConnection`] containing
/// an `rmcp`-level transport handle. Connection lifetime is managed by the
/// [`crate::McpClient`] lifecycle layer, not by the transport itself.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Establish a new connection, consuming no state on success.
    async fn connect(&self) -> Result<TransportConnection, McpError>;

    /// Short identifier for logs, e.g. "stdio" or "streamable-http".
    fn kind(&self) -> &'static str;

    /// Human-readable server identifier used in logs and errors.
    fn server_name(&self) -> &str;
}

/// Opaque handle wrapping an rmcp transport, ready to feed `ServiceExt::serve`.
pub struct TransportConnection {
    pub(crate) inner: TransportInner,
}

/// Internal enum so lifecycle code can match on concrete transport type.
/// Not exposed in public API.
pub(crate) enum TransportInner {
    Stdio(rmcp::transport::TokioChildProcess),
    Http(rmcp::transport::StreamableHttpClientTransport<reqwest::Client>),
}

/// Config shape used by `McpClientConfig`. Lets callers specify the transport
/// declaratively; the client constructs the corresponding `Transport` impl.
#[derive(Clone, Debug)]
pub enum TransportKind {
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: Url,
        auth: Option<HttpAuth>,
        extra_headers: HashMap<String, String>,
    },
}

/// HTTP authentication strategy for `StreamableHttpTransport`.
#[derive(Clone, Debug)]
pub enum HttpAuth {
    Bearer(String),
    ApiKey { header: String, value: String },
}
