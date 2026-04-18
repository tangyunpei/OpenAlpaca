// crates/openalpaca_mcp/src/transport/mod.rs

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use url::Url;

use crate::error::McpError;

mod stdio;
mod http;

pub use http::StreamableHttpTransport;
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

#[cfg(feature = "test-utils")]
pub mod test_utils {
    //! Testing helpers. Gated behind the `test-utils` Cargo feature.
    //!
    //! P1 ships a scripted `MockTransport` shell. P2 integration tests can
    //! extend this to wire in rmcp's in-memory transport if needed.

    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::{Transport, TransportConnection};
    use crate::error::McpError;

    /// Scripted transport that records every `connect()` call.
    /// Always returns `McpError::TransportClosed` — useful for failure-path tests.
    /// For happy-path tests, P2 should construct a real stdio transport against
    /// a test fixture binary, or extend this mock to carry a full rmcp in-memory pair.
    pub struct MockTransport {
        server_name: String,
        connect_calls: Arc<Mutex<u32>>,
    }

    impl MockTransport {
        pub fn new(server_name: impl Into<String>) -> Self {
            Self {
                server_name: server_name.into(),
                connect_calls: Arc::new(Mutex::new(0)),
            }
        }

        pub fn connect_count(&self) -> u32 {
            *self.connect_calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl Transport for MockTransport {
        async fn connect(&self) -> Result<TransportConnection, McpError> {
            *self.connect_calls.lock().unwrap() += 1;
            Err(McpError::TransportClosed)
        }

        fn kind(&self) -> &'static str {
            "mock"
        }

        fn server_name(&self) -> &str {
            &self.server_name
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn records_connect_calls() {
            let m = MockTransport::new("mock");
            assert_eq!(m.connect_count(), 0);
            let _ = m.connect().await;
            let _ = m.connect().await;
            assert_eq!(m.connect_count(), 2);
        }
    }
}
