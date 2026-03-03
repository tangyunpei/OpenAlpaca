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

impl LlmError {
    /// Whether this error is transient and worth retrying with the same key.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http(_) => true,
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
