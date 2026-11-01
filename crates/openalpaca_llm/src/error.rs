#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    /// A transport failure described by its text alone. No provider in this
    /// crate builds it any more — they map `reqwest::Error` to
    /// [`Self::Transport`] (M57) — but a plugin's provider bridge does, and so
    /// do tests standing in for a provider.
    #[error("HTTP error: {0}")]
    Http(String),

    /// The request or its answer failed in transit, classified from reqwest's
    /// own predicates rather than from its text (M57).
    ///
    /// `message` is reqwest's text followed by its source chain, joined with
    /// `: ` — reqwest's `Display` writes only its kind and the URL, never the
    /// cause. No source is attached (the variant is `Clone`, and
    /// `reqwest::Error` is not), so a reporter that prints the chain prints
    /// the cause once.
    #[error("HTTP error: {message}")]
    Transport {
        kind: TransportKind,
        message: String,
    },

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

/// Which transport failure a [`LlmError::Transport`] was (M57).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// The connect phase — the lookup, the TCP connect, the TLS handshake —
    /// outlasted the client's connect timeout before the call's own deadline
    /// or read gap ran out.
    ConnectTimeout,
    /// The call's own deadline (a non-streaming call's `request_timeout`) or
    /// the client's read gap ran out. Both clocks start when the request is
    /// executed, so a connect that hangs ends here, not as a
    /// [`Self::ConnectTimeout`], whenever `request_timeout` is no longer than
    /// the connect timeout (30 s at most). reqwest also reports the OS's own
    /// TCP timeout on an open connection — its keepalive probes unanswered —
    /// as a timeout, and that ends here too.
    RequestTimeout,
    /// Nothing listens at the address.
    ConnectRefused,
    /// The host name did not resolve.
    Dns,
    /// The connection was reset or closed before the answer was whole — no
    /// answer at all, or a body shorter than it promised — or closed before
    /// the request was written (hyper-util hands the request back). A reset
    /// or close during the connect phase (the TCP connect, the TLS
    /// handshake) is [`Self::TlsOrOther`].
    ResetOrClosed,
    /// A TLS failure — a reset or close during the handshake among them — an
    /// answer that is not HTTP, or anything else reqwest reports.
    TlsOrOther,
}

impl TransportKind {
    /// Whether trying again may succeed: a timeout or a lost connection may
    /// not happen twice, while a refused port, a name that does not resolve
    /// and a TLS failure fail the same way again.
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            Self::ConnectTimeout | Self::RequestTimeout | Self::ResetOrClosed
        )
    }
}

/// Check if an [`LlmError::Http`] message indicates a transient network issue.
/// Only timeout, connection reset, broken pipe, and premature close are transient.
/// DNS failures, TLS errors, and connection refused are NOT transient.
///
/// It reads text, so it serves only the variant that carries nothing else; a
/// provider's own failures are [`LlmError::Transport`], classified by kind.
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
    /// Whether this error is transient: worth trying again. The fallback chain
    /// and the agentic loop retry every transient error; the router's key
    /// layer retries those [`Self::is_retryable_on_key`] admits — all but a
    /// request timeout.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http(msg) => is_transient_http_error(msg),
            Self::Transport { kind, .. } => kind.is_transient(),
            Self::RateLimited { .. } => true,
            Self::Overloaded { .. } => true,
            Self::Stream(_) => true,
            Self::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Whether the router's key layer tries this error again — on the same key
    /// or the next — before the call goes on to the fallback chain (M57, its
    /// question taken (a)).
    ///
    /// Every transient error but a request timeout: a call that spent its
    /// whole `request_timeout` would spend another if repeated against the
    /// same model, so a request timeout costs one `complete` call at most one
    /// deadline per rung of the ladder. A kind retried here costs a wait per
    /// try, and so can cost more: a rung's three or more 30 s connect
    /// timeouts outlast a `request_timeout` under 90 s. The kind also holds a
    /// connect the deadline cut short and the OS's TCP timeout on an open
    /// connection ([`TransportKind::RequestTimeout`]); neither is repeated
    /// here either. It stays transient, so the chain and the loop still take
    /// it.
    pub fn is_retryable_on_key(&self) -> bool {
        !matches!(
            self,
            Self::Transport {
                kind: TransportKind::RequestTimeout,
                ..
            }
        ) && self.is_transient()
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
