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
/// the agentic loop's `max_stream_duration` (`max_keyless_stream_duration` for
/// a provider that needs no key).
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

/// The one way a provider turns a `reqwest::Error` into an [`LlmError`] (M57),
/// for a failed send and a failed body read alike.
///
/// reqwest's own predicates say what happened; its text does not — its
/// `Display` writes only the kind and the URL, never the cause, which is why
/// a classifier reading that text never saw a timeout or a reset. A body
/// that arrived whole but is not JSON is the answer's content, not its
/// transport, and stays [`LlmError::Serialization`] with its text unchanged.
/// Every other failure is [`LlmError::Transport`].
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
pub(crate) fn reqwest_error(e: reqwest::Error) -> crate::error::LlmError {
    use std::error::Error as _;
    // `Response::json` tags its read of the body as a decode failure too
    // (reqwest 0.13.5's `do_bytes`), so the decode kind alone does not say the
    // JSON was bad: only a source from the JSON parser does.
    if e.is_decode() && e.source().is_some_and(|s| s.is::<serde_json::Error>()) {
        return crate::error::LlmError::Serialization(e.to_string());
    }
    crate::error::LlmError::Transport {
        kind: transport_kind(&e),
        message: transport_message(&e),
    }
}

/// reqwest's text followed by its source chain, joined with `: `; a link that
/// repeats the one before it (a body wrapped once per timeout) is written once.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
pub(crate) fn transport_message(e: &reqwest::Error) -> String {
    let mut message = e.to_string();
    let mut previous = message.clone();
    let mut source = std::error::Error::source(e);
    while let Some(link) = source {
        let text = link.to_string();
        if text != previous {
            message.push_str(": ");
            message.push_str(&text);
        }
        previous = text;
        source = link.source();
    }
    message
}

/// Which failure `e` was, from reqwest's predicates and the OS's own error
/// kind in its source chain.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
fn transport_kind(e: &reqwest::Error) -> crate::error::TransportKind {
    use crate::error::TransportKind;
    use std::io::ErrorKind;

    if e.is_dns() {
        return TransportKind::Dns;
    }
    if e.is_timeout() {
        // The connect timeout fires inside the connector, so its error is a
        // connect error too; the call's deadline and the read gap are not.
        return if e.is_connect() {
            TransportKind::ConnectTimeout
        } else {
            TransportKind::RequestTimeout
        };
    }
    let mut io_kind = None;
    let mut closed_before_answer = false;
    let mut handed_back = false;
    let mut source = std::error::Error::source(e);
    while let Some(link) = source {
        if let Some(io) = link.downcast_ref::<std::io::Error>() {
            io_kind.get_or_insert(io.kind());
        }
        let text = link.to_string();
        // hyper's error for a connection closed before the answer carries no
        // OS error, and hyper is not a dependency of this crate, so its own
        // fixed texts are the one place this function reads one.
        closed_before_answer |= text == "connection closed before message completed";
        // A request hyper hands back unwritten, the connection gone before it
        // took the request, carries none either: hyper-util's `Canceled` link
        // marks every request handed back, and these two texts of hyper's
        // beneath it say only that the connection had closed. Any other text
        // there is the connection's own error, read as above.
        closed_before_answer |= handed_back
            && matches!(
                text.as_str(),
                "connection was not ready" | "connection closed"
            );
        handed_back |= text == "client error (Canceled)";
        source = link.source();
    }
    if e.is_connect() {
        return if io_kind == Some(ErrorKind::ConnectionRefused) {
            TransportKind::ConnectRefused
        } else {
            TransportKind::TlsOrOther
        };
    }
    match io_kind {
        Some(
            ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof,
        ) => TransportKind::ResetOrClosed,
        None if closed_before_answer => TransportKind::ResetOrClosed,
        _ => TransportKind::TlsOrOther,
    }
}

/// Whether this build refuses the providers' real hosts (LLM-TEST-GUARD,
/// pending-decisions *Rev 127* items 8 and 9, (b), taken *Rev 128*): this
/// crate's own tests and the test builds of the crates that turn on
/// `test-endpoint-guard` through their `[dev-dependencies]`, unless the
/// build carries `live-eval` (openalpaca_core's live evals call the real
/// providers on purpose). Never a release, `cargo run` or sidecar build.
#[doc(hidden)]
pub const fn test_endpoint_guard_armed() -> bool {
    cfg!(all(
        any(test, feature = "test-endpoint-guard"),
        not(feature = "live-eval")
    ))
}

/// Whether this build carries `live-eval`, which switches the guard off. A
/// dependant's armed test asserts `armed == !yields`, so a workspace run that
/// unifies core's `live-eval` on still passes it.
#[doc(hidden)]
pub const fn test_endpoint_guard_yields() -> bool {
    cfg!(feature = "live-eval")
}

/// A guarded test build never sends to a provider's real host: a `base`
/// whose host is one of `real_hosts` is refused with a configuration error
/// naming the guard and the host, before any lookup, so no connection is
/// tried and no test's key leaves the process. Any other base — a loopback
/// mock, a LAN or other compatible server — passes. Not transient and not an
/// auth error, so the router neither retries nor counts a refused key.
#[cfg(all(
    any(feature = "anthropic", feature = "openai"),
    any(test, feature = "test-endpoint-guard"),
    not(feature = "live-eval")
))]
pub(crate) fn refuse_real_host(
    provider: &str,
    base: &str,
    real_hosts: &[&str],
) -> Result<(), crate::error::LlmError> {
    let host = reqwest::Url::parse(base)
        .ok()
        .and_then(|url| url.host_str().map(|h| h.trim_end_matches('.').to_owned()));
    match host {
        Some(host) if real_hosts.contains(&host.as_str()) => {
            Err(crate::error::LlmError::Config(format!(
                "a test build of openalpaca_llm (test-endpoint-guard) refuses {provider}'s \
                 real endpoint ({host}): point the provider at a loopback server"
            )))
        }
        _ => Ok(()),
    }
}

/// Release, `cargo run`, sidecar and `live-eval` builds: no check.
#[cfg(all(
    any(feature = "anthropic", feature = "openai"),
    not(all(any(test, feature = "test-endpoint-guard"), not(feature = "live-eval")))
))]
#[inline(always)]
pub(crate) fn refuse_real_host(
    _provider: &str,
    _base: &str,
    _real_hosts: &[&str],
) -> Result<(), crate::error::LlmError> {
    Ok(())
}

pub mod openai_tools;

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "ollama")]
pub mod ollama;

#[cfg(all(test, feature = "openai"))]
mod transport_tests;
