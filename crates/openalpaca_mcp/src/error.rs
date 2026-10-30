// crates/openalpaca_mcp/src/error.rs

use std::time::Duration;

/// Errors returned from MCP operations.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("transport closed unexpectedly")]
    TransportClosed,

    /// The client was deliberately closed (see [`crate::McpClient::disconnect`])
    /// and is sealed: it will never reconnect and never respawn its child.
    ///
    /// Terminal *by type* — deliberately **not** in [`McpError::is_retriable`]'s
    /// set, so a retry loop cannot spin a sealed client back to life. Distinct
    /// from [`Self::TransportClosed`], which means "closed unexpectedly".
    #[error("client closed deliberately; no reconnect will be attempted")]
    Closed,

    /// The server rejected the daemon's credentials with an HTTP `401` or
    /// `403` (extension design §3.6 item 1, §4.2).
    ///
    /// Terminal *by type*, like [`Self::Closed`]: it is deliberately **not** in
    /// [`McpError::is_retriable`]'s set, which takes an expired token out of
    /// `call_tool`'s reconnect ladder at the source — otherwise a mid-session
    /// 401 burns the four reconnect entries and ends as `Failed{Crashed}` with
    /// a Retry button that cannot help.
    #[error("server rejected the daemon's credentials (HTTP {0})")]
    Unauthorized(u16),

    /// The streamable-HTTP server answered with a redirect the client refuses
    /// to follow (`transport/http.rs`, the redirect rule): a hop to another
    /// origin, any hop for a server given credentials or extra headers, or a
    /// chain past ten hops. `origin` is the target's scheme, host and port —
    /// never its path or query.
    ///
    /// Terminal *by type*, like [`Self::Unauthorized`]: following the server
    /// again cannot help until the owner sets `url` to the endpoint it
    /// redirected to, so it is not in [`McpError::is_retriable`]'s set and a
    /// call never enters the reconnect ladder. At bring-up the row reads
    /// `Failed{ConfigInvalid}` with this text as its detail.
    #[error("server redirected to {origin}; set `url` to the canonical endpoint")]
    RedirectRefused { origin: String },

    #[error("transport I/O error: {0}")]
    Transport(#[from] std::io::Error),

    #[error("MCP handshake failed: {0}")]
    HandshakeFailed(String),

    /// The connection's lifecycle could not be negotiated (DU-25 (m-b)): the
    /// server supports no version the client offers, answered the probe or the
    /// handshake with something else, closed the transport while the probe
    /// waited, was found legacy and then failed the `initialize` that
    /// followed, or — under the `2026-07-28` pin — refused the probe as a
    /// legacy server does (not with a `2026-07-28` refusal). The text carries
    /// rmcp's error and what to set (`protocol = "legacy"`, or the pin's
    /// alternative); at bring-up the row reads `Failed{ProtocolNegotiationFailed}`
    /// with it as its detail, where before it read `Unreachable`.
    ///
    /// Not in [`McpError::is_retriable`]'s set: the same lifecycle meets the
    /// same answer until the owner changes `protocol`.
    #[error("MCP protocol negotiation failed: {0}")]
    ProtocolNegotiationFailed(String),

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
    ///
    /// [`Self::Closed`] is intentionally absent: a sealed client must never be
    /// retried, or the retry loop would respawn the child the owner just
    /// disabled. So are [`Self::Unauthorized`] and [`Self::RedirectRefused`]: a
    /// retry cannot succeed until the owner fixes the credential or the `url`.
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
            Self::TransportClosed
            | Self::Closed
            | Self::Transport(_)
            | Self::ReconnectExhausted(_) => ErrorCategory::Transport,
            // The server answered — it answered "no".
            Self::Unauthorized(_) => ErrorCategory::Server,
            // The server answered — it answered "go elsewhere".
            Self::RedirectRefused { .. } => ErrorCategory::Server,
            Self::HandshakeFailed(_)
            | Self::ProtocolNegotiationFailed(_)
            | Self::VersionMismatch { .. } => ErrorCategory::Handshake,
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
// rmcp 3.7 `ServiceError` variants (see rmcp/src/service.rs):
//   - McpError(ErrorData)           : JSON-RPC error from the peer.
//   - TransportSend(DynamicTransportError) : local transport send failure.
//   - TransportClosed               : the transport reports EOF / closure.
//   - UnexpectedResponse            : protocol-level mismatch.
//   - SubscriptionLagged { capacity } : a `subscriptions/listen` consumer fell
//                                     behind (we open no subscription).
//   - Cancelled { reason }          : the operation was cancelled.
//   - Timeout { timeout }           : the operation timed out.
//   - InputRequiredRoundsExceeded { max_rounds } : an MRTR `input_required`
//                                     loop did not finish (we answer no input).
//
// Best-effort classification; unknown (`#[non_exhaustive]`) variants fall
// through to `Sdk(...)` with a warn log so we notice rmcp additions.
/// The one place an HTTP status survives into this crate.
///
/// rmcp's reqwest-backed streamable-HTTP client maps a `401` carrying a
/// `www-authenticate` header to [`StreamableHttpError::AuthRequired`] and a
/// `403` carrying one to [`StreamableHttpError::InsufficientScope`]
/// (`rmcp-3.7.0/src/transport/common/reqwest/streamable_http_client.rs:108`,
/// `:121` on the GET stream, `:223`, `:236` on a POST); every other status
/// is folded into an untyped response error before
/// it reaches us. So this recovers exactly the two the design classifies
/// (§4.2) and nothing else — no string matching, no guessing.
///
/// Returns `None` for the stdio transport, which cannot carry a status at all:
/// a stdio server that exits on an expired token is indistinguishable from one
/// that crashed and classifies as `Unreachable` (§4.2).
pub(crate) fn unauthorized_status(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> Option<u16> {
    use rmcp::transport::streamable_http_client::StreamableHttpError;
    match error.downcast_ref::<StreamableHttpError<reqwest::Error>>()? {
        StreamableHttpError::AuthRequired(_) => Some(401),
        StreamableHttpError::InsufficientScope(_) => Some(403),
        _ => None,
    }
}

/// What the streamable-HTTP client's redirect rule hands reqwest for a hop it
/// refuses (`transport/http.rs`). reqwest keeps it as the source of the
/// redirect error it returns, which is how [`typed_http_failure`] finds it.
#[derive(Debug)]
pub(crate) struct RefusedRedirect {
    /// The target's origin — scheme, host and port, never a path or query.
    pub(crate) origin: String,
}

impl std::fmt::Display for RefusedRedirect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "redirect to {} refused", self.origin)
    }
}

impl std::error::Error for RefusedRedirect {}

/// The typed failure a streamable-HTTP transport error carries, if any: a
/// credential the server rejected ([`McpError::Unauthorized`]) or a redirect
/// the client refused ([`McpError::RedirectRefused`]). `None` for anything
/// else, which the caller keeps as its own untyped failure.
///
/// One function for the handshake (`McpClient::do_handshake`) and for a call
/// (`From<ServiceError>` below), so the two cannot classify the same error
/// differently.
pub(crate) fn typed_http_failure(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> Option<McpError> {
    if let Some(status) = unauthorized_status(error) {
        return Some(McpError::Unauthorized(status));
    }
    refused_redirect(error).map(|refused| McpError::RedirectRefused {
        origin: refused.origin.clone(),
    })
}

/// The refusal inside a streamable-HTTP client error: rmcp wraps the reqwest
/// error as [`StreamableHttpError::Client`] (a variant with no `source`, so
/// it is unwrapped by type), and reqwest keeps what the redirect policy
/// returned as its error's source.
///
/// [`StreamableHttpError::Client`]: rmcp::transport::streamable_http_client::StreamableHttpError::Client
fn refused_redirect<'e>(
    error: &'e (dyn std::error::Error + Send + Sync + 'static),
) -> Option<&'e RefusedRedirect> {
    use rmcp::transport::streamable_http_client::StreamableHttpError;
    let StreamableHttpError::Client(client) =
        error.downcast_ref::<StreamableHttpError<reqwest::Error>>()?
    else {
        return None;
    };
    let mut source = std::error::Error::source(client);
    while let Some(error) = source {
        if let Some(refused) = error.downcast_ref::<RefusedRedirect>() {
            return Some(refused);
        }
        source = error.source();
    }
    None
}

impl From<rmcp::service::ServiceError> for McpError {
    fn from(e: rmcp::service::ServiceError) -> Self {
        use rmcp::service::ServiceError as S;
        match e {
            S::McpError(err) => Self::JsonRpc {
                code: err.code.0 as i64,
                message: err.message.to_string(),
            },
            S::TransportSend(err) => {
                // The streamable-HTTP client is the one transport that can
                // report a failure as a *typed* error — a credential the server
                // rejected, the only place a status code survives anywhere in
                // this crate, or a redirect the client refused. Recover it here
                // rather than flattening either into a retriable
                // `TransportClosed` (design §3.6 item 1, X-7; N-MCP-2).
                if let Some(typed) = typed_http_failure(err.error.as_ref()) {
                    tracing::debug!(error = %err, typed = %typed, "MCP transport failure");
                    return typed;
                }
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
            // A deliberately closed (sealed) client must never be retried.
            (McpError::Closed, false),
            // Nor may an authorization failure enter the reconnect ladder.
            (McpError::Unauthorized(401), false),
            (McpError::Unauthorized(403), false),
            // Nor may a redirect the client refused (N-MCP-2).
            (
                McpError::RedirectRefused {
                    origin: "https://other.example".into(),
                },
                false,
            ),
            (
                McpError::Transport(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
                true,
            ),
            (McpError::Timeout(Duration::from_secs(1)), true),
            (McpError::ServerInternal("x".into()), true),
            (McpError::HandshakeFailed("x".into()), false),
            // The same lifecycle meets the same answer (DU-25 (m-b)).
            (McpError::ProtocolNegotiationFailed("x".into()), false),
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
        assert_eq!(McpError::Closed.category(), ErrorCategory::Transport);
        assert_eq!(McpError::Unauthorized(401).category(), ErrorCategory::Server);
        assert_eq!(
            McpError::ReconnectExhausted(3).category(),
            ErrorCategory::Transport
        );
        assert_eq!(
            McpError::HandshakeFailed("x".into()).category(),
            ErrorCategory::Handshake
        );
        assert_eq!(
            McpError::ProtocolNegotiationFailed("x".into()).category(),
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
            McpError::RedirectRefused {
                origin: "https://other.example".into()
            }
            .category(),
            ErrorCategory::Server
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
        // The row's detail at bring-up: the origin and the remedy.
        assert_eq!(
            McpError::RedirectRefused {
                origin: "https://other.example:8443".into()
            }
            .to_string(),
            "server redirected to https://other.example:8443; set `url` to the canonical endpoint"
        );
    }
}
