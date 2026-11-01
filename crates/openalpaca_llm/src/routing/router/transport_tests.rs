//! The retry table for real transport failures (M57, under its question's
//! answer (a)): what the key layer, the fallback chain and the streamed path
//! each do with a connect timeout, a request timeout, a lost connection, a
//! refused port and a name that does not resolve — and the bound one call is
//! held to, and the circuit breaker.
//!
//! Every failure is a real reqwest error against a loopback listener this
//! test owns, a port nothing listens on, or a name the test's in-process
//! resolver answers; nothing leaves the machine, and no CLI backend is
//! registered (the bound's C is 0).

use super::*;
use crate::error::TransportKind;
use crate::providers::ollama::OllamaProvider;
use crate::routing::model_registry::ModelInfo;
use crate::test_support::{
    MockHttpServer, MockResponse, RecordedRequest, Resolver, UNRESOLVABLE_HOST,
    client_resolving_with, loopback_client, refused_addr, refused_base_url,
    unpooled_loopback_client,
};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(250);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
/// The deadline and read gap where a connect must time out first: seconds
/// past the connect timeout, so a slow scheduler cannot turn it into the
/// call's own deadline (both clocks start when the request is executed).
const PATIENT: Duration = Duration::from_secs(5);
/// What a test allows past a bound for scheduling.
const SLACK: Duration = Duration::from_millis(1500);
const SILENCE: Duration = Duration::from_secs(30);

const PRIMARY: &str = "m-primary";
const FALLBACK_1: &str = "m-fallback-1";
const FALLBACK_2: &str = "m-fallback-2";

fn request(model: &str) -> RouterRequest {
    RouterRequest {
        model: Some(model.to_string()),
        messages: Arc::new(vec![ChatMessage::user("ping")]),
        tools: Arc::new(vec![]),
        temperature: None,
        max_tokens: None,
        context: RequestContext::default(),
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    }
}

fn completion_body(model: &str, content: &str) -> String {
    serde_json::json!({
        "model": model,
        "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    })
    .to_string()
}

fn model_of(request: &RecordedRequest) -> String {
    serde_json::from_str::<serde_json::Value>(&request.body)
        .ok()
        .and_then(|b| b["model"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// How many chat requests named `model`.
async fn hits(server: &MockHttpServer, model: &str) -> usize {
    server
        .requests()
        .await
        .iter()
        .filter(|r| r.path == "/v1/chat/completions" && model_of(r) == model)
        .count()
}

/// Fast backoff, so a retried row costs milliseconds; the breaker's threshold
/// as the test needs it.
fn rates(circuit_breaker_threshold: u32) -> RateLimitConfig {
    RateLimitConfig {
        backoff_base_ms: 5,
        backoff_cap_ms: 20,
        circuit_breaker_threshold,
        ..RateLimitConfig::default()
    }
}

/// A keyless local provider serving `models`, the first the default and the
/// rest its fallback chain, through `client` with `REQUEST_TIMEOUT` as each
/// non-streaming call's deadline.
fn router(
    base: &str,
    client: reqwest::Client,
    models: &[&str],
    rates: RateLimitConfig,
) -> LlmRouter {
    router_with_deadline(base, client, models, rates, REQUEST_TIMEOUT)
}

/// [`router`] with `deadline` as each non-streaming call's deadline.
fn router_with_deadline(
    base: &str,
    client: reqwest::Client,
    models: &[&str],
    rates: RateLimitConfig,
    deadline: Duration,
) -> LlmRouter {
    let provider = Arc::new(
        OllamaProvider::with_client(
            client,
            models[0].to_string(),
            Some(format!("{base}/v1")),
            None,
        )
        .with_request_timeout(deadline),
    );
    let mut providers = HashMap::new();
    providers.insert(
        ProviderType::Ollama,
        ProviderEntry {
            provider,
            key_pool: Arc::new(ArcSwap::from_pointee(KeyPool::new(
                vec![],
                SelectionStrategy::RoundRobin,
            ))),
        },
    );
    let registry = ModelRegistry::new(HashMap::new());
    for model in models {
        registry.register(
            model.to_string(),
            ModelInfo {
                provider: ProviderType::Ollama,
                input_price_per_million: 0.0,
                output_price_per_million: 0.0,
                context_window: 32_768,
                discovered: true,
                supports_image: false,
                supports_audio: false,
                supports_document: false,
                supports_reasoning: false,
                supports_tools: true,
                declared: true,
            },
        );
    }
    let mut chains = HashMap::new();
    chains.insert(
        models[0].to_string(),
        models[1..]
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>(),
    );
    LlmRouter::new_with_runtime(
        providers,
        registry,
        chains,
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        models[0].to_string(),
        LlmRuntimeConfig::default(),
        rates,
    )
}

fn plain_client() -> reqwest::Client {
    loopback_client(REQUEST_TIMEOUT)
}

// ── request timeout: not retried on the key, taken by the chain ─────────────

/// A silent server: each rung of the ladder is asked once and waits its whole
/// deadline, so one `complete` call takes `(1 + chain length) × request_timeout`
/// (C = 0) — the bound of (a) — and ends with the chain exhausted.
#[tokio::test]
async fn a_request_timeout_is_asked_once_per_rung_and_held_to_the_bound() {
    let server = MockHttpServer::start(|_| MockResponse::json("{}").after(SILENCE)).await;
    let models = [PRIMARY, FALLBACK_1, FALLBACK_2];
    let router = router(&server.base_url, plain_client(), &models, rates(100));
    let chain = router.substitution_ladder(PRIMARY, &[]).len();
    assert_eq!(chain, 2);

    let started = Instant::now();
    let err = router.complete(request(PRIMARY)).await.unwrap_err();
    let elapsed = started.elapsed();

    for model in models {
        assert_eq!(hits(&server, model).await, 1, "{model} asked once: {err}");
    }
    assert!(matches!(err, LlmRouterError::AllFallbacksFailed), "{err}");
    // What the caller is told: the chain's own words, not the provider's; the
    // first rung's cause is only in the router's warning in the log.
    assert_eq!(err.to_string(), "All fallback models failed");
    let bound = REQUEST_TIMEOUT * (1 + chain as u32);
    eprintln!(
        "M57 bound, complete: chain length {chain}, request_timeout {REQUEST_TIMEOUT:?}, \
         C = 0: measured {elapsed:?} against (1 + {chain}) x {REQUEST_TIMEOUT:?} = {bound:?}"
    );
    assert!(
        elapsed >= bound,
        "every rung waited its deadline: {elapsed:?}"
    );
    assert!(elapsed < bound + SLACK, "held to the bound: {elapsed:?}");
}

/// A streamed turn's one loop attempt: the stream fails on its read gap and
/// is returned at once, then the loop's backend retries through `complete`
/// (`runner/agentic_loop/backend.rs`) — `(2 + chain length) × request_timeout`.
#[tokio::test]
async fn a_streamed_attempt_and_its_non_streaming_retry_are_held_to_the_bound() {
    let server = MockHttpServer::start(|_| MockResponse::json("{}").after(SILENCE)).await;
    let models = [PRIMARY, FALLBACK_1, FALLBACK_2];
    let router = router(&server.base_url, plain_client(), &models, rates(100));
    let chain = router.substitution_ladder(PRIMARY, &[]).len();

    let started = Instant::now();
    let streamed = match router.complete_streaming(request(PRIMARY)).await {
        Ok(_) => panic!("a silent server opened a stream"),
        Err(e) => e,
    };
    let after_stream = started.elapsed();
    assert_eq!(
        hits(&server, PRIMARY).await,
        1,
        "returned at once: {streamed}"
    );
    assert!(
        matches!(&streamed, LlmRouterError::Llm(e) if e.is_transient()),
        "{streamed}"
    );

    let err = router.complete(request(PRIMARY)).await.unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(err, LlmRouterError::AllFallbacksFailed), "{err}");
    assert_eq!(hits(&server, PRIMARY).await, 2);
    assert_eq!(hits(&server, FALLBACK_1).await, 1);
    assert_eq!(hits(&server, FALLBACK_2).await, 1);

    let bound = REQUEST_TIMEOUT * (2 + chain as u32);
    eprintln!(
        "M57 bound, one loop attempt of a streamed turn: chain length {chain}, request_timeout \
         {REQUEST_TIMEOUT:?}, C = 0: stream failed after {after_stream:?}, measured {elapsed:?} \
         against (2 + {chain}) x {REQUEST_TIMEOUT:?} = {bound:?}"
    );
    assert!(elapsed >= bound, "{elapsed:?}");
    assert!(elapsed < bound + SLACK, "held to the bound: {elapsed:?}");
}

// ── reset, closed, incomplete body: retried on the key, then the chain ──────

/// A body cut short: the key layer asks again (`max_transient_retries`, 3,
/// for a keyless provider), then the chain answers.
#[tokio::test]
async fn a_body_cut_short_is_retried_on_the_key_and_then_falls_back() {
    let server = MockHttpServer::start(|req| {
        if model_of(req) == PRIMARY {
            MockResponse::json(completion_body(PRIMARY, "never whole")).cut_short()
        } else {
            MockResponse::json(completion_body(FALLBACK_1, "from the fallback"))
        }
    })
    .await;
    let router = router(
        &server.base_url,
        plain_client(),
        &[PRIMARY, FALLBACK_1],
        rates(100),
    );

    let response = router
        .complete(request(PRIMARY))
        .await
        .expect("the chain answers");

    assert_eq!(response.content, "from the fallback");
    assert_eq!(hits(&server, PRIMARY).await, 3);
    assert_eq!(hits(&server, FALLBACK_1).await, 1);
}

/// A connection closed before the answer: the same row.
#[tokio::test]
async fn a_connection_closed_before_the_answer_is_retried_and_then_falls_back() {
    let server = MockHttpServer::start(|req| {
        if model_of(req) == PRIMARY {
            MockResponse::hang_up()
        } else {
            MockResponse::json(completion_body(FALLBACK_1, "from the fallback"))
        }
    })
    .await;
    let router = router(
        &server.base_url,
        plain_client(),
        &[PRIMARY, FALLBACK_1],
        rates(100),
    );

    let response = router
        .complete(request(PRIMARY))
        .await
        .expect("the chain answers");

    assert_eq!(response.content, "from the fallback");
    assert_eq!(hits(&server, PRIMARY).await, 3);
    assert_eq!(hits(&server, FALLBACK_1).await, 1);
}

/// A connection closed before the request is written — on the daemon's
/// multi-thread runtime, now and then handed back by hyper as "connection
/// was not ready" — is the same row. The server closes its first three
/// connections unread, the key layer's three tries (the test above counts
/// them), and answers the fourth, the fallback's; each round a fresh server
/// and fresh connections, so the race is run many times.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_closed_before_the_request_is_written_is_retried_and_then_falls_back() {
    const ROUNDS: usize = 50;
    let mut ended = std::collections::BTreeMap::<String, usize>::new();
    for _ in 0..ROUNDS {
        let server = MockHttpServer::start_closing_first(3, |_| {
            MockResponse::json(completion_body(FALLBACK_1, "from the fallback"))
        })
        .await;
        let router = router(
            &server.base_url,
            unpooled_loopback_client(REQUEST_TIMEOUT),
            &[PRIMARY, FALLBACK_1],
            rates(100),
        );

        match router.complete(request(PRIMARY)).await {
            Ok(response) => {
                assert_eq!(response.content, "from the fallback");
                assert_eq!(server.requests().await.len(), 1, "one request read");
                assert_eq!(hits(&server, FALLBACK_1).await, 1, "the fallback's");
            }
            Err(err) => {
                let text = err.to_string().replace(&server.base_url, "U");
                *ended.entry(text).or_default() += 1;
            }
        }
    }

    let count: usize = ended.values().sum();
    assert_eq!(
        count, 0,
        "{count} of {ROUNDS} calls ended without the fallback: {ended:#?}"
    );
}

// ── connect timeout: retried on the key, then the chain ─────────────────────

/// A connect phase that never completes: three tries on the key per rung,
/// each a lookup the test's resolver counts, on every rung of the chain.
#[tokio::test]
async fn a_connect_timeout_is_retried_on_the_key_and_walks_the_chain() {
    let resolver = Resolver::never();
    let client = client_resolving_with(&resolver, CONNECT_TIMEOUT, PATIENT);
    let base = format!("http://{UNRESOLVABLE_HOST}");
    let router = router_with_deadline(
        &base,
        client,
        &[PRIMARY, FALLBACK_1, FALLBACK_2],
        rates(100),
        PATIENT,
    );

    let err = router.complete(request(PRIMARY)).await.unwrap_err();

    assert!(matches!(err, LlmRouterError::AllFallbacksFailed), "{err}");
    assert_eq!(
        resolver.lookups(),
        3 * 3,
        "three tries on each of three rungs"
    );
}

// ── refused, DNS: neither retried nor taken to the chain ────────────────────

/// Refused: one try, the chain not walked — the caller gets the provider's
/// own error, which says so.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_refused_port_is_neither_retried_nor_taken_to_the_chain() {
    let resolver = Resolver::to(refused_addr());
    let client = client_resolving_with(&resolver, CONNECT_TIMEOUT, REQUEST_TIMEOUT);
    let base = format!("http://{UNRESOLVABLE_HOST}");
    let router = router(
        &base,
        client,
        &[PRIMARY, FALLBACK_1, FALLBACK_2],
        rates(100),
    );

    let err = router.complete(request(PRIMARY)).await.unwrap_err();

    assert_eq!(resolver.lookups(), 1, "one try, no chain: {err}");
    assert!(
        matches!(&err, LlmRouterError::Llm(e) if !e.is_transient()),
        "{err}"
    );
    assert!(matches!(
        &err,
        LlmRouterError::Llm(LlmError::Transport {
            kind: TransportKind::ConnectRefused,
            ..
        })
    ));
    assert_eq!(
        err.to_string(),
        format!(
            "LLM error: HTTP error: error sending request for url ({base}/v1/chat/completions): \
             client error (Connect): tcp connect error: {}",
            std::io::Error::from_raw_os_error(if cfg!(target_os = "macos") { 61 } else { 111 })
        )
    );
}

/// A name that does not resolve: the same row.
#[tokio::test]
async fn an_unresolvable_host_is_neither_retried_nor_taken_to_the_chain() {
    let resolver = Resolver::no_such_name();
    let client = client_resolving_with(&resolver, CONNECT_TIMEOUT, REQUEST_TIMEOUT);
    let base = format!("http://{UNRESOLVABLE_HOST}");
    let router = router(
        &base,
        client,
        &[PRIMARY, FALLBACK_1, FALLBACK_2],
        rates(100),
    );

    let err = router.complete(request(PRIMARY)).await.unwrap_err();

    assert_eq!(resolver.lookups(), 1, "one try, no chain: {err}");
    assert!(
        matches!(&err, LlmRouterError::Llm(e) if !e.is_transient()),
        "{err}"
    );
    assert!(matches!(
        &err,
        LlmRouterError::Llm(LlmError::Transport {
            kind: TransportKind::Dns,
            ..
        })
    ));
    assert_eq!(
        err.to_string(),
        format!(
            "LLM error: HTTP error: error sending request for url ({base}/v1/chat/completions): \
             client error (Connect): dns error: error resolving DNS: no such name (test resolver)"
        )
    );
}

/// An answer that is not HTTP (TLS, other): one try, the chain not walked.
#[tokio::test]
async fn an_answer_that_is_not_http_is_neither_retried_nor_taken_to_the_chain() {
    let server = MockHttpServer::start(|_| MockResponse::not_http()).await;
    let router = router(
        &server.base_url,
        plain_client(),
        &[PRIMARY, FALLBACK_1],
        rates(100),
    );

    let err = router.complete(request(PRIMARY)).await.unwrap_err();

    assert_eq!(hits(&server, PRIMARY).await, 1, "one try: {err}");
    assert_eq!(hits(&server, FALLBACK_1).await, 0, "no chain: {err}");
    assert!(
        matches!(
            &err,
            LlmRouterError::Llm(e @ LlmError::Transport {
                kind: TransportKind::TlsOrOther,
                ..
            }) if !e.is_transient()
        ),
        "{err}"
    );
}

/// The same on the daemon's multi-thread runtime: the server reads the whole
/// request before it answers, so the request is always written and the answer
/// always fails to parse — never handed back unwritten, never retried.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_that_is_not_http_is_not_retried_on_a_multi_thread_runtime() {
    const ROUNDS: usize = 50;
    for _ in 0..ROUNDS {
        let server = MockHttpServer::start(|_| MockResponse::not_http()).await;
        let router = router(
            &server.base_url,
            unpooled_loopback_client(REQUEST_TIMEOUT),
            &[PRIMARY, FALLBACK_1],
            rates(100),
        );

        let err = router.complete(request(PRIMARY)).await.unwrap_err();

        assert_eq!(hits(&server, PRIMARY).await, 1, "one try: {err}");
        assert_eq!(hits(&server, FALLBACK_1).await, 0, "no chain: {err}");
        assert!(
            matches!(
                &err,
                LlmRouterError::Llm(e @ LlmError::Transport {
                    kind: TransportKind::TlsOrOther,
                    ..
                }) if !e.is_transient()
            ),
            "{err}"
        );
    }
}

// ── the streamed path: every failure returned at once ───────────────────────

/// `complete_streaming` tries once and returns whatever the transport did;
/// whether it is transient decides only what the agentic loop does next.
#[tokio::test]
async fn a_streamed_call_returns_each_transport_failure_at_once() {
    async fn streamed(router: &LlmRouter) -> LlmError {
        match router.complete_streaming(request(PRIMARY)).await {
            Ok(_) => panic!("a failed transport opened a stream"),
            Err(LlmRouterError::Llm(e)) => e,
            Err(other) => panic!("not the provider's error: {other}"),
        }
    }
    let models = [PRIMARY, FALLBACK_1];
    let base = format!("http://{UNRESOLVABLE_HOST}");

    fn kind(err: &LlmError) -> Option<TransportKind> {
        match err {
            LlmError::Transport { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    for (resolver, transient, expected) in [
        (
            Resolver::to(refused_addr()),
            false,
            TransportKind::ConnectRefused,
        ),
        (Resolver::no_such_name(), false, TransportKind::Dns),
        (Resolver::never(), true, TransportKind::ConnectTimeout),
    ] {
        let client = client_resolving_with(&resolver, CONNECT_TIMEOUT, PATIENT);
        let err = streamed(&router_with_deadline(
            &base,
            client,
            &models,
            rates(100),
            PATIENT,
        ))
        .await;
        assert_eq!(resolver.lookups(), 1, "{err}");
        assert_eq!(err.is_transient(), transient, "{err}");
        assert_eq!(kind(&err), Some(expected), "{err}");
    }

    for (answer, expected) in [
        (
            MockResponse::json("{}").after(SILENCE),
            TransportKind::RequestTimeout,
        ),
        (MockResponse::hang_up(), TransportKind::ResetOrClosed),
        (MockResponse::reset(), TransportKind::ResetOrClosed),
    ] {
        let server = MockHttpServer::start(move |_| answer.clone()).await;
        let err = streamed(&router(
            &server.base_url,
            plain_client(),
            &models,
            rates(100),
        ))
        .await;
        assert_eq!(server.requests().await.len(), 1, "{err}");
        assert!(err.is_transient(), "{err}");
        assert_eq!(kind(&err), Some(expected), "{err}");
    }
}

// ── the circuit breaker ─────────────────────────────────────────────────────

/// Each rung's timeout reports one failure to the one breaker. Once it is
/// open a `complete` call fails at once on every rung — no request is made —
/// while a streamed call, which neither asks nor tells the breaker, is still
/// made.
#[tokio::test]
async fn an_open_breaker_fails_complete_at_once_on_every_rung_but_not_a_stream() {
    let server = MockHttpServer::start(|_| MockResponse::json("{}").after(SILENCE)).await;
    let models = [PRIMARY, FALLBACK_1, FALLBACK_2];
    let router = router(&server.base_url, plain_client(), &models, rates(3));

    let err = router.complete(request(PRIMARY)).await.unwrap_err();
    assert!(matches!(err, LlmRouterError::AllFallbacksFailed), "{err}");
    assert_eq!(
        server.requests().await.len(),
        3,
        "one failure per rung: {err}"
    );

    let started = Instant::now();
    let err = router.complete(request(PRIMARY)).await.unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(err, LlmRouterError::AllFallbacksFailed), "{err}");
    assert_eq!(server.requests().await.len(), 3, "no rung made a request");
    assert!(elapsed < REQUEST_TIMEOUT, "failed at once: {elapsed:?}");

    let _ = router.complete_streaming(request(PRIMARY)).await;
    assert_eq!(
        hits(&server, PRIMARY).await,
        2,
        "the streamed call was made"
    );
}

// ── discovery: the reason the settings screen shows ─────────────────────────

/// `refresh_models_for` records the provider's error text, which the GUI
/// shows as "could not be reached (…)": it now says what failed.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn an_unreachable_local_provider_says_why_in_its_discovery_error() {
    let base = refused_base_url();
    let unreachable = router(&base, plain_client(), &[PRIMARY], rates(100));
    let status = unreachable.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(
        status.error.as_deref(),
        Some(
            format!(
                "HTTP error: error sending request for url ({base}/api/tags): client error \
                 (Connect): tcp connect error: {}",
                std::io::Error::from_raw_os_error(if cfg!(target_os = "macos") { 61 } else { 111 })
            )
            .as_str()
        )
    );

    let server = MockHttpServer::start(|_| {
        MockResponse::json(r#"{"models":[{"name":"m-primary"}]}"#).cut_short()
    })
    .await;
    let cut_short = router(&server.base_url, plain_client(), &[PRIMARY], rates(100));
    let status = cut_short.refresh_models_for(&ProviderType::Ollama).await;
    assert_eq!(
        status.error.as_deref(),
        Some(
            format!(
                "HTTP error: error decoding response body for url ({}/api/tags): request or \
                 response body error: error reading a body from connection: end of file before \
                 message length reached",
                server.base_url
            )
            .as_str()
        )
    );
}
