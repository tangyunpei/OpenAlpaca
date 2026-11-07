//! What a provider makes of a real transport failure (M57).
//!
//! Every error here is one reqwest produced: against a loopback listener the
//! test owns, a loopback port nothing listens on, or a name the test's own
//! in-process resolver refuses or never answers. Nothing leaves the machine
//! and no test reaches a provider.
//!
//! reqwest's `Display` writes only its kind and the URL, never the cause, so
//! before M57 every one of these read "error sending request for url (…)" and
//! the classifier, which looked for words such as "timed out" in that text,
//! never saw a timeout, a reset or a close.

use super::openai::OpenAiProvider;
use crate::LlmProvider;
use crate::error::{LlmError, TransportKind};
use crate::test_support::{
    MockHttpServer, MockResponse, Resolver, UNRESOLVABLE_HOST, client_resolving_with,
    loopback_client, refused_base_url, unpooled_loopback_client,
};
use crate::types::*;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A non-streaming call's deadline, and the read gap of the client.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(250);
/// The connect timeout of a client whose lookups never complete.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
/// The deadline and read gap where a connect must time out first: seconds
/// past the connect timeout, so a slow scheduler cannot turn it into the
/// call's own deadline (both clocks start when the request is executed).
const PATIENT: Duration = Duration::from_secs(5);
/// What a test allows past a deadline for scheduling.
const SLACK: Duration = Duration::from_millis(1500);
/// Long enough that a silent server is still silent when the client gives up.
const SILENCE: Duration = Duration::from_secs(30);

fn request() -> ChatRequest {
    ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("ping")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    }
}

fn completion_body() -> String {
    serde_json::json!({
        "model": "m",
        "choices": [{"message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    })
    .to_string()
}

/// An OpenAI-compatible provider on `base` with a client shaped like the
/// router's own and its deadline (L7) — the shape `build_router` gives every
/// provider — that no proxy reaches.
fn provider(base: &str) -> OpenAiProvider {
    provider_with(loopback_client(REQUEST_TIMEOUT), base, REQUEST_TIMEOUT)
}

fn provider_with(client: reqwest::Client, base: &str, deadline: Duration) -> OpenAiProvider {
    OpenAiProvider::new_without_auth_with_client(
        client,
        "m".to_string(),
        format!("{base}/v1"),
        None,
    )
    .with_request_timeout(deadline)
}

/// The text `std::io::Error` gives an errno on this OS.
#[cfg(target_os = "macos")]
fn os_error(macos: i32, _linux: i32) -> String {
    std::io::Error::from_raw_os_error(macos).to_string()
}

#[cfg(target_os = "linux")]
fn os_error(_macos: i32, linux: i32) -> String {
    std::io::Error::from_raw_os_error(linux).to_string()
}

/// The kind a provider's error was classified as, if it was a transport one.
fn kind(err: &LlmError) -> Option<TransportKind> {
    match err {
        LlmError::Transport { kind, .. } => Some(*kind),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn connection_refused() -> String {
    os_error(61, 111)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn connection_reset() -> String {
    os_error(54, 104)
}

// ── `.send()`: what fails before an answer arrives ──────────────────────────

/// Refused: not transient, and the text says it was refused.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_refused_port_says_so_and_is_not_transient() {
    let base = refused_base_url();
    let err = provider(&base).chat(request()).await.unwrap_err();

    assert!(!err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ConnectRefused));
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({base}/v1/chat/completions): client \
             error (Connect): tcp connect error: {}",
            connection_refused()
        )
    );
}

/// A server that takes the request and never answers: the call's own
/// deadline ends it, and that is transient.
#[tokio::test]
async fn a_silent_server_is_a_timeout_and_transient() {
    let server = MockHttpServer::start(|_| MockResponse::json("{}").after(SILENCE)).await;
    let started = Instant::now();
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();
    let elapsed = started.elapsed();

    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::RequestTimeout));
    assert!(
        !err.is_retryable_on_key(),
        "the key layer does not try it again"
    );
    assert!(
        elapsed >= REQUEST_TIMEOUT && elapsed < REQUEST_TIMEOUT + SLACK,
        "{elapsed:?}"
    );
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({}/v1/chat/completions): operation timed \
             out",
            server.base_url
        )
    );
}

/// The streamed call carries no deadline of its own; the client's read gap
/// ends it, with the same text.
#[tokio::test]
async fn a_silent_server_ends_a_streamed_call_on_the_read_gap() {
    let server = MockHttpServer::start(|_| MockResponse::json("{}").after(SILENCE)).await;
    let started = Instant::now();
    let err = match provider(&server.base_url)
        .chat_streaming_with_key("", request())
        .await
    {
        Ok(_) => panic!("a silent server opened a stream"),
        Err(e) => e,
    };
    let elapsed = started.elapsed();

    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::RequestTimeout));
    assert!(
        !err.is_retryable_on_key(),
        "the key layer does not try it again"
    );
    assert!(
        elapsed >= REQUEST_TIMEOUT && elapsed < REQUEST_TIMEOUT + SLACK,
        "{elapsed:?}"
    );
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({}/v1/chat/completions): operation timed \
             out",
            server.base_url
        )
    );
}

/// A name that does not resolve: not transient. The lookup is answered by the
/// test's resolver, in-process.
#[tokio::test]
async fn an_unresolvable_host_says_so_and_is_not_transient() {
    let resolver = Resolver::no_such_name();
    let client = client_resolving_with(&resolver, CONNECT_TIMEOUT, REQUEST_TIMEOUT);
    let base = format!("http://{UNRESOLVABLE_HOST}");
    let err = provider_with(client, &base, REQUEST_TIMEOUT)
        .chat(request())
        .await
        .unwrap_err();

    assert!(!err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::Dns));
    assert_eq!(resolver.lookups(), 1);
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({base}/v1/chat/completions): client \
             error (Connect): dns error: error resolving DNS: no such name (test resolver)"
        )
    );
}

/// A connect phase that outlasts the connect timeout — here a lookup that
/// never completes — is transient.
#[tokio::test]
async fn a_connect_that_never_completes_is_a_connect_timeout_and_transient() {
    let resolver = Resolver::never();
    let client = client_resolving_with(&resolver, CONNECT_TIMEOUT, PATIENT);
    let base = format!("http://{UNRESOLVABLE_HOST}");
    let started = Instant::now();
    let err = provider_with(client, &base, PATIENT)
        .chat(request())
        .await
        .unwrap_err();
    let elapsed = started.elapsed();

    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ConnectTimeout));
    assert!(err.is_retryable_on_key());
    assert!(
        elapsed >= CONNECT_TIMEOUT && elapsed < CONNECT_TIMEOUT + SLACK,
        "{elapsed:?}"
    );
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({base}/v1/chat/completions): client \
             error (Connect): operation timed out"
        )
    );
}

/// The server closes the connection once it has read the request.
#[tokio::test]
async fn a_connection_closed_before_the_answer_is_transient() {
    let server = MockHttpServer::start(|_| MockResponse::hang_up()).await;
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();

    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ResetOrClosed));
    assert!(err.is_retryable_on_key());
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({}/v1/chat/completions): client error \
             (SendRequest): connection closed before message completed",
            server.base_url
        )
    );
}

/// The server resets the connection once it has read the request.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_connection_reset_before_the_answer_is_transient() {
    let server = MockHttpServer::start(|_| MockResponse::reset()).await;
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();

    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ResetOrClosed));
    assert!(err.is_retryable_on_key());
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({}/v1/chat/completions): client error \
             (SendRequest): connection error: {}",
            server.base_url,
            connection_reset()
        )
    );
}

/// The server closes a fresh connection before it reads a byte. On a
/// multi-thread runtime, the daemon's, the connection can die between
/// hyper-util's ready check and the dispatch, and hyper hands the request
/// back unwritten, its cause dropped: "client error (Canceled): operation was
/// canceled: connection was not ready". A lost connection all the same, and
/// a request never written is safe to send again. A current-thread runtime
/// never meets the race, so the test asks for workers and repeats it over
/// fresh connections.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_closed_before_the_request_is_written_is_transient() {
    const TRIALS: usize = 200;
    let server =
        MockHttpServer::start_closing_first(usize::MAX, |_| MockResponse::not_found()).await;
    let provider = provider_with(
        unpooled_loopback_client(REQUEST_TIMEOUT),
        &server.base_url,
        REQUEST_TIMEOUT,
    );
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    let mut misread = 0;
    for _ in 0..TRIALS {
        let err = provider.chat(request()).await.unwrap_err();
        if kind(&err) != Some(TransportKind::ResetOrClosed) || !err.is_retryable_on_key() {
            misread += 1;
        }
        let text = err.to_string().replace(&server.base_url, "U");
        *seen
            .entry(format!("{:?} | {text}", kind(&err)))
            .or_default() += 1;
    }

    eprintln!("M57 follow-up, a request never written: {seen:#?}");
    assert_eq!(
        misread, 0,
        "{misread} of {TRIALS} not a lost connection retried on the key: {seen:#?}"
    );
    assert!(server.requests().await.is_empty(), "no request was read");
}

/// A server that speaks first — a line that is not HTTP, written as it
/// accepts, then a close — leaves one of a few texts, which one a race on
/// the daemon's multi-thread runtime, each read the same way every time. Its
/// answer read ("received unexpected message from connection", "invalid
/// HTTP version parsed") is not transient, even under hyper-util's
/// `Canceled` link, which hands that request back unwritten: only hyper's
/// texts for a connection already closed ("connection was not ready",
/// "connection closed") make that link a lost connection. The test pins
/// that narrowing per error, by its text; which texts a run meets is the
/// race's, so it counts none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_speaks_first_is_read_by_the_text_it_leaves() {
    const TRIALS: usize = 200;
    const HANDED_BACK: &str = "client error (Canceled): operation was canceled: ";
    let server = MockHttpServer::start_speaking_first().await;
    let provider = provider_with(
        unpooled_loopback_client(REQUEST_TIMEOUT),
        &server.base_url,
        REQUEST_TIMEOUT,
    );
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    let mut misread = Vec::new();
    for _ in 0..TRIALS {
        let err = provider.chat(request()).await.unwrap_err();
        let text = err.to_string().replace(&server.base_url, "U");
        let lost = if text.ends_with("received unexpected message from connection")
            || text.ends_with("invalid HTTP version parsed")
        {
            Some(false)
        } else if text.ends_with(&format!("{HANDED_BACK}connection was not ready"))
            || text.ends_with(&format!("{HANDED_BACK}connection closed"))
        {
            Some(true)
        } else {
            None
        };
        let read_as = (kind(&err), err.is_retryable_on_key());
        let wanted = lost.map(|lost| {
            if lost {
                (Some(TransportKind::ResetOrClosed), true)
            } else {
                (Some(TransportKind::TlsOrOther), false)
            }
        });
        if wanted.is_some_and(|wanted| wanted != read_as) {
            misread.push(format!("{read_as:?} | {text}"));
        }
        *seen
            .entry(format!("{:?} | {text}", kind(&err)))
            .or_default() += 1;
    }

    eprintln!("M57 follow-up, a server that speaks first: {seen:#?}");
    assert!(
        misread.is_empty(),
        "{} of {TRIALS} misread: {misread:#?}",
        misread.len()
    );
}

/// A server that answers something other than HTTP: no OS error, and asking
/// again gets the same answer — not transient.
#[tokio::test]
async fn an_answer_that_is_not_http_is_not_transient() {
    let server = MockHttpServer::start(|_| MockResponse::not_http()).await;
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();

    assert!(!err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::TlsOrOther));
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({}/v1/chat/completions): client error \
             (SendRequest): invalid HTTP version parsed",
            server.base_url
        )
    );
}

// ── the body read: what fails after the head arrived ────────────────────────

/// A body shorter than its `content-length`: the connection was lost, which
/// is transient — not a malformed answer, which is not.
#[tokio::test]
async fn a_body_cut_short_is_a_lost_connection_and_transient() {
    let server = MockHttpServer::start(|_| MockResponse::json(completion_body()).cut_short()).await;
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();

    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ResetOrClosed));
    assert!(err.is_retryable_on_key());
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error decoding response body for url ({}/v1/chat/completions): request \
             or response body error: error reading a body from connection: end of file before \
             message length reached",
            server.base_url
        )
    );
}

/// A body that stops arriving: the call's deadline ends it mid-body, and
/// that is the same request timeout as a silent server.
#[tokio::test]
async fn a_body_that_stalls_is_a_timeout_and_transient() {
    let server = MockHttpServer::start(|_| {
        MockResponse::sse(vec!["{\"choices\":".to_string(), "[]}".to_string()]).every(SILENCE)
    })
    .await;
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();

    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::RequestTimeout));
    assert!(
        !err.is_retryable_on_key(),
        "the key layer does not try it again"
    );
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error decoding response body for url ({}/v1/chat/completions): request \
             or response body error: operation timed out",
            server.base_url
        )
    );
}

/// A whole body that is not JSON is the answer's content, not its transport:
/// unchanged, a `Serialization` error, not transient.
#[tokio::test]
async fn a_body_that_is_not_json_stays_a_serialization_error() {
    let server = MockHttpServer::start(|_| MockResponse::json("hello")).await;
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();

    assert!(!err.is_transient(), "{err}");
    assert!(matches!(err, LlmError::Serialization(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!(
            "Serialization error: error decoding response body for url ({}/v1/chat/completions)",
            server.base_url
        )
    );
}

// ── the stream: what fails mid-answer ───────────────────────────────────────

/// A stream that ends before its last chunk: still a `Stream` error and still
/// transient, now saying why.
#[tokio::test]
async fn a_stream_cut_mid_answer_says_why() {
    let frame = format!(
        "data: {}\n\n",
        serde_json::json!({"choices": [{"index": 0, "delta": {"content": "hel"}}]})
    );
    let server =
        MockHttpServer::start(move |_| MockResponse::sse(vec![frame.clone()]).cut_short()).await;
    let stream = provider(&server.base_url)
        .chat_streaming_with_key("", request())
        .await
        .expect("the head arrives whole");
    let events: Vec<_> = stream.collect().await;
    let err = match events.last() {
        Some(Err(e)) => e.clone(),
        other => panic!("the stream ended without an error: {other:?}"),
    };

    assert!(err.is_transient(), "{err}");
    assert!(matches!(err, LlmError::Stream(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "Stream error: error decoding response body: request or response body error: error \
         reading a body from connection: unexpected EOF during chunk size line"
    );
}

// ── the other providers' send sites ─────────────────────────────────────────

/// Anthropic's three calls go to its one fixed URL; the test's resolver sends
/// that name to a refused loopback port, to nowhere, or to a lookup that never
/// completes, so each call fails in-process before any TLS.
#[cfg(all(feature = "anthropic", any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn anthropics_calls_say_what_failed() {
    use super::anthropic::AnthropicProvider;
    use crate::test_support::refused_addr;
    let anthropic = |resolver: &Resolver, deadline: Duration| {
        AnthropicProvider::with_client(
            client_resolving_with(resolver, CONNECT_TIMEOUT, deadline),
            "sk-test".to_string(),
            None,
            None,
        )
        .with_request_timeout(deadline)
    };

    let refused = Resolver::to(refused_addr());
    let err = anthropic(&refused, REQUEST_TIMEOUT)
        .chat(request())
        .await
        .unwrap_err();
    assert!(!err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ConnectRefused));
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url (https://api.anthropic.com/v1/messages): \
             client error (Connect): tcp connect error: {}",
            connection_refused()
        )
    );

    let nowhere = Resolver::no_such_name();
    let err = match anthropic(&nowhere, REQUEST_TIMEOUT)
        .chat_streaming_with_key("sk-test", request())
        .await
    {
        Ok(_) => panic!("an unresolvable host opened a stream"),
        Err(e) => e,
    };
    assert!(!err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::Dns));
    assert_eq!(
        err.to_string(),
        "HTTP error: error sending request for url (https://api.anthropic.com/v1/messages): \
         client error (Connect): dns error: error resolving DNS: no such name (test resolver)"
    );

    let hanging = Resolver::never();
    let err = anthropic(&hanging, PATIENT)
        .list_models_with_key("sk-test")
        .await
        .unwrap_err();
    assert!(err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ConnectTimeout));
    assert_eq!(
        err.to_string(),
        "HTTP error: error sending request for url (https://api.anthropic.com/v1/models): \
         client error (Connect): operation timed out"
    );
}

/// The OpenAI-compatible model list (discovery) maps its failure the same way.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn the_model_list_says_what_failed() {
    let base = refused_base_url();
    let err = provider(&base).list_models_with_key("").await.unwrap_err();

    assert!(!err.is_transient(), "{err}");
    assert_eq!(kind(&err), Some(TransportKind::ConnectRefused));
    assert_eq!(
        err.to_string(),
        format!(
            "HTTP error: error sending request for url ({base}/v1/models): client error \
             (Connect): tcp connect error: {}",
            connection_refused()
        )
    );
}

/// The cause is in the message, and nowhere else: no source is attached, so a
/// reporter that prints the chain (anyhow's `{:#}`, a `tracing` error field)
/// prints it once.
#[tokio::test]
async fn a_transport_error_prints_its_cause_once() {
    let server = MockHttpServer::start(|_| MockResponse::hang_up()).await;
    let err = provider(&server.base_url)
        .chat(request())
        .await
        .unwrap_err();

    assert_eq!(kind(&err), Some(TransportKind::ResetOrClosed));
    assert!(std::error::Error::source(&err).is_none());
    assert_eq!(
        err.to_string()
            .matches("connection closed before message completed")
            .count(),
        1
    );
}
