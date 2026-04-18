// crates/openalpaca_mcp/src/error.rs

use std::time::Duration;

/// Errors returned from MCP operations.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("transport closed unexpectedly")]
    TransportClosed,

    #[error("transport I/O error: {0}")]
    Transport(#[from] std::io::Error),

    #[error("MCP handshake failed: {0}")]
    HandshakeFailed(String),

    #[error("protocol version mismatch: server={server}, client={client}")]
    VersionMismatch { server: String, client: String },

    #[error("JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },

    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),

    #[error("server returned an internal error: {0}")]
    ServerInternal(String),

    #[error("operation timed out after {0:?}")]
    Timeout(Duration),

    #[error("operation cancelled")]
    Cancelled,

    #[error("reconnect attempts exhausted (tried {0})")]
    ReconnectExhausted(u32),

    #[error("rmcp SDK error: {0}")]
    Sdk(String),
}

/// Coarse categorisation of an [`McpError`] for routing/reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Transport,
    Handshake,
    Protocol,
    Server,
    Cancelled,
    Internal,
}

impl McpError {
    /// Returns `true` for errors that a caller should retry after reconnection.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::TransportClosed
                | Self::Transport(_)
                | Self::Timeout(_)
                | Self::ServerInternal(_)
        )
    }

    /// Returns `true` iff the operation was cancelled by caller.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Coarse category for telemetry / error routing.
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::TransportClosed | Self::Transport(_) | Self::ReconnectExhausted(_) => {
                ErrorCategory::Transport
            }
            Self::HandshakeFailed(_) | Self::VersionMismatch { .. } => ErrorCategory::Handshake,
            Self::JsonRpc { .. } | Self::ToolNotFound(_) | Self::InvalidArguments(_) => {
                ErrorCategory::Protocol
            }
            Self::ServerInternal(_) | Self::Timeout(_) => ErrorCategory::Server,
            Self::Cancelled => ErrorCategory::Cancelled,
            Self::Sdk(_) => ErrorCategory::Internal,
        }
    }
}

// rmcp::service::ServiceError → McpError translation.
//
// rmcp 0.16 `ServiceError` variants (see rmcp/src/service.rs):
//   - McpError(ErrorData)           : JSON-RPC error from the peer.
//   - TransportSend(DynamicTransportError) : local transport send failure.
//   - TransportClosed               : the transport reports EOF / closure.
//   - UnexpectedResponse            : protocol-level mismatch.
//   - Cancelled { reason }          : the operation was cancelled.
//   - Timeout { timeout }           : the operation timed out.
//
// Best-effort classification; unknown (`#[non_exhaustive]`) variants fall
// through to `Sdk(...)` with a warn log so we notice rmcp additions.
impl From<rmcp::service::ServiceError> for McpError {
    fn from(e: rmcp::service::ServiceError) -> Self {
        use rmcp::service::ServiceError as S;
        match e {
            S::McpError(err) => Self::JsonRpc {
                code: err.code.0 as i64,
                message: err.message.to_string(),
            },
            S::TransportSend(err) => {
                tracing::debug!(error = %err, "rmcp transport send error");
                Self::TransportClosed
            }
            S::TransportClosed => Self::TransportClosed,
            S::UnexpectedResponse => {
                Self::ServerInternal("rmcp returned unexpected response".into())
            }
            S::Timeout { timeout } => Self::Timeout(timeout),
            S::Cancelled { .. } => Self::Cancelled,
            other => {
                tracing::warn!(error = ?other, "unmapped rmcp ServiceError");
                Self::Sdk(format!("{other:?}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_retriable_classification() {
        let cases = [
            (McpError::TransportClosed, true),
            (
                McpError::Transport(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
                true,
            ),
            (McpError::Timeout(Duration::from_secs(1)), true),
            (McpError::ServerInternal("x".into()), true),
            (McpError::HandshakeFailed("x".into()), false),
            (McpError::ToolNotFound("x".into()), false),
            (McpError::InvalidArguments("x".into()), false),
            (McpError::Cancelled, false),
            (McpError::ReconnectExhausted(3), false),
            (
                McpError::JsonRpc {
                    code: -32700,
                    message: "parse".into(),
                },
                false,
            ),
            (
                McpError::VersionMismatch {
                    server: "1".into(),
                    client: "2".into(),
                },
                false,
            ),
            (McpError::Sdk("x".into()), false),
        ];
        for (err, expected) in cases {
            assert_eq!(err.is_retriable(), expected, "{err:?}");
        }
    }

    #[test]
    fn is_cancelled_only_cancelled() {
        assert!(McpError::Cancelled.is_cancelled());
        assert!(!McpError::TransportClosed.is_cancelled());
        assert!(!McpError::Timeout(Duration::from_secs(1)).is_cancelled());
    }

    #[test]
    fn category_classification() {
        assert_eq!(
            McpError::TransportClosed.category(),
            ErrorCategory::Transport
        );
        assert_eq!(
            McpError::ReconnectExhausted(3).category(),
            ErrorCategory::Transport
        );
        assert_eq!(
            McpError::HandshakeFailed("x".into()).category(),
            ErrorCategory::Handshake
        );
        assert_eq!(
            McpError::VersionMismatch {
                server: "a".into(),
                client: "b".into()
            }
            .category(),
            ErrorCategory::Handshake
        );
        assert_eq!(
            McpError::JsonRpc {
                code: -32603,
                message: "x".into()
            }
            .category(),
            ErrorCategory::Protocol
        );
        assert_eq!(
            McpError::ToolNotFound("x".into()).category(),
            ErrorCategory::Protocol
        );
        assert_eq!(
            McpError::InvalidArguments("x".into()).category(),
            ErrorCategory::Protocol
        );
        assert_eq!(
            McpError::ServerInternal("x".into()).category(),
            ErrorCategory::Server
        );
        assert_eq!(
            McpError::Timeout(Duration::from_secs(1)).category(),
            ErrorCategory::Server
        );
        assert_eq!(McpError::Cancelled.category(), ErrorCategory::Cancelled);
        assert_eq!(McpError::Sdk("x".into()).category(), ErrorCategory::Internal);
    }

    #[test]
    fn display_messages_are_informative() {
        assert_eq!(
            McpError::VersionMismatch {
                server: "2025-06".into(),
                client: "2024-11".into()
            }
            .to_string(),
            "protocol version mismatch: server=2025-06, client=2024-11"
        );
        assert_eq!(
            McpError::ReconnectExhausted(5).to_string(),
            "reconnect attempts exhausted (tried 5)"
        );
    }
}
