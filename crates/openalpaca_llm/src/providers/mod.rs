#[cfg(any(feature = "anthropic", feature = "openai"))]
mod utf8;

/// The HTTP client one provider's calls go out on (L7).
///
/// Deliberately **no** total `reqwest` timeout: a total deadline at this layer
/// applies through the response body, so it cuts a healthy stream mid-answer —
/// which is what the hard-coded 120 s did to a local model generating at
/// 20 tok/s. What is bounded here instead:
///
/// * the connect phase, so an unreachable endpoint fails fast rather than
///   hanging;
/// * the gap between reads, so a stalled connection is noticed — it resets on
///   every byte, so a slow-but-alive stream is never cut.
///
/// A non-streaming call carries `request_timeout` as its own per-request
/// deadline (`RequestBuilder::timeout`), and a streamed one is bounded above by
/// the agentic loop's `max_stream_duration`.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
pub fn build_http_client(request_timeout: std::time::Duration) -> reqwest::Client {
    /// The longest a TCP+TLS handshake may take before the endpoint counts as
    /// unreachable. Never longer than the call's own budget.
    const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    reqwest::Client::builder()
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .connect_timeout(CONNECT_TIMEOUT.min(request_timeout))
        .read_timeout(request_timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "ollama")]
pub mod ollama;
