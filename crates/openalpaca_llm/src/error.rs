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
}
