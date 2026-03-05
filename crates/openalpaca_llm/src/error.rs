#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("Provider overloaded (status {status}), retry_after_ms={retry_after_ms:?}")]
    Overloaded {
        status: u16,
        retry_after_ms: Option<u64>,
    },

    #[error("Provider not configured")]
    NotConfigured,

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    #[error("Credential discovery error: {0}")]
    CredentialDiscovery(String),

    #[error("Token refresh error: {0}")]
    TokenRefresh(String),

    #[error("CLI backend error: {0}")]
    CliBackend(String),

    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("Stream error: {0}")]
    Stream(String),
}

/// Check if an HTTP-level error message indicates a transient network issue.
/// Only timeout, connection reset, broken pipe, and premature close are transient.
/// DNS failures, TLS errors, and connection refused are NOT transient.
fn is_transient_http_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("reset by peer")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("connection closed")
        || lower.contains("incomplete message")
}

impl LlmError {
    /// Whether this error is transient and worth retrying with the same key.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http(msg) => is_transient_http_error(msg),
            Self::RateLimited { .. } => true,
            Self::Overloaded { .. } => true,
            Self::Stream(_) => true,
            Self::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Whether this is an authentication/authorization error (bad key).
    pub fn is_auth_error(&self) -> bool {
        match self {
            Self::Api { status, .. } => *status == 401 || *status == 403,
            Self::AuthenticationFailed(_) => true,
            _ => false,
        }
    }
}
