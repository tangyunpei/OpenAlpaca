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

    /// The call succeeded and came back with nothing usable in it.
    ///
    /// This is the signature of a thinking model on a small output budget: the
    /// reasoning fills `max_tokens` and the answer is never emitted, so a
    /// 256-token extraction call hands its caller `""` and the caller reports
    /// it as malformed JSON ("EOF at column 0") instead of as the budget
    /// problem it is (M2). Raised by the router rather than a provider — it is
    /// a property of the response, not of one transport — and deliberately not
    /// transient: the same budget produces the same nothing.
    #[error(
        "{model} returned an empty completion — no content and no tool call, after \
         {output_tokens} output tokens. A thinking model spends its output budget on \
         reasoning before it answers: raise max_tokens for this call, or ask for no \
         reasoning (ThinkingConfig::Disabled)."
    )]
    EmptyCompletion { model: String, output_tokens: u32 },
}

/// Whether this response said nothing a caller can use, and why that is worth
/// an error rather than an empty string (M2).
///
/// Narrow on purpose. Empty content alone is not enough — only an answer that
/// is empty *and* has no tool call, no content parts, and either stopped at the
/// `max_tokens` cap or produced reasoning instead of an answer. Anything looser
/// would turn a model's legitimately terse turn into a failure.
pub fn empty_completion_error(response: &crate::types::ChatResponse) -> Option<LlmError> {
    let nothing_to_use = response.content.trim().is_empty()
        && response.tool_calls.is_empty()
        && response.parts.as_ref().is_none_or(|p| p.is_empty());
    let attributable = response.finish_reason == crate::types::FinishReason::MaxTokens
        || response
            .thinking
            .as_ref()
            .is_some_and(|t| !t.trim().is_empty());

    (nothing_to_use && attributable).then(|| LlmError::EmptyCompletion {
        model: response.model.clone(),
        output_tokens: response.usage.output_tokens,
    })
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
