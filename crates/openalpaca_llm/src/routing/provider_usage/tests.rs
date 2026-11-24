use super::*;

/// A build with no provider feature has no HTTP client: every fetch is
/// `None`, and nothing is sent anywhere.
#[cfg(not(any(feature = "anthropic", feature = "openai", feature = "ollama")))]
#[tokio::test]
async fn without_a_provider_feature_no_usage_is_fetched() {
    let tracker = ProviderUsageTracker::new();
    let result = tracker
        .get_usage(CredentialSource::ClaudeCode, "invalid-token")
        .await;
    assert!(result.is_none());
}

/// A tracker whose two usage endpoints are a loopback server's, fetched by
/// a client no proxy reaches: no test sends a token to Anthropic's or
/// OpenAI's real usage endpoints, which `LlmRuntimeConfig::default()`
/// names (on the parent this module's one fetch test built
/// `ProviderUsageTracker::new()` and sent its request to
/// api.anthropic.com in every workspace run).
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
fn tracker_on(server: &crate::test_support::MockHttpServer) -> ProviderUsageTracker {
    let mut rt = LlmRuntimeConfig::default();
    rt.endpoints.anthropic_usage = format!("{}/api/oauth/usage", server.base_url);
    rt.endpoints.openai_usage = format!("{}/dashboard/billing/usage", server.base_url);
    let mut tracker =
        ProviderUsageTracker::with_runtime_config(Arc::new(ArcSwap::from_pointee(rt)));
    tracker.client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("a test client");
    tracker
}

/// Each source's usage is fetched from its own endpoint, parsed, and then
/// served from the cache: a second ask sends no second request.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
#[tokio::test]
async fn usage_is_fetched_from_the_configured_endpoint_once_then_cached() {
    use crate::test_support::{MockHttpServer, MockResponse};
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/api/oauth/usage" => MockResponse::json(
            r#"{"total_cost": 12.5, "total_tokens": 50000, "period": "2026-02"}"#,
        ),
        "/dashboard/billing/usage" => {
            MockResponse::json(r#"{"total_usage": 1250.0, "total_tokens": 100000}"#)
        }
        _ => MockResponse::not_found(),
    })
    .await;
    let tracker = tracker_on(&server);

    for _ in 0..2 {
        let anthropic = tracker
            .get_usage(CredentialSource::ClaudeCode, "a-test-token")
            .await
            .expect("Anthropic usage from the loopback server");
        assert!((anthropic.cost_usd - 12.5).abs() < 0.01);
        assert_eq!(anthropic.token_count, 50000);
        let openai = tracker
            .get_usage(CredentialSource::Codex, "a-test-token")
            .await
            .expect("OpenAI usage from the loopback server");
        assert!((openai.cost_usd - 12.5).abs() < 0.01);
        assert_eq!(openai.token_count, 100000);
    }

    assert_eq!(server.hits("/api/oauth/usage").await, 1);
    assert_eq!(server.hits("/dashboard/billing/usage").await, 1);
    assert_eq!(server.requests().await.len(), 2);
}

/// A refused fetch is no usage, and nothing is cached: the next ask
/// fetches again.
#[cfg(any(feature = "anthropic", feature = "openai", feature = "ollama"))]
#[tokio::test]
async fn a_refused_usage_fetch_is_none_and_not_cached() {
    use crate::test_support::{MockHttpServer, MockResponse};
    let server = MockHttpServer::start(|_| MockResponse::error(401, r#"{"error":"no"}"#)).await;
    let tracker = tracker_on(&server);

    for _ in 0..2 {
        assert!(
            tracker
                .get_usage(CredentialSource::ClaudeCode, "invalid-token")
                .await
                .is_none()
        );
    }

    assert_eq!(server.hits("/api/oauth/usage").await, 2);
}

/// GUARD-REACH (pending-decisions *Rev 133* item 5, (b), taken *Rev 134*):
/// a guarded test build refuses the two usage fetches at the real default
/// endpoints `LlmRuntimeConfig::default()` names, as the providers' calls
/// are refused: an error naming the guard and the host, before any lookup,
/// so no connection is tried and no token leaves the process. The client
/// is the test's resolver's, which no proxy reaches and which answers every
/// name with a refused loopback port, so even the parent's send (red: an
/// "HTTP error" after one lookup) left nothing on the wire.
#[cfg(all(
    feature = "openai",
    not(feature = "live-eval"),
    any(target_os = "macos", target_os = "linux")
))]
#[tokio::test]
async fn a_test_build_refuses_the_real_usage_endpoints_before_any_lookup() {
    use crate::test_support::{Resolver, client_resolving_with, refused_addr};
    assert!(crate::providers::test_endpoint_guard_armed());
    let resolver = Resolver::to(refused_addr());
    let mut tracker = ProviderUsageTracker::new();
    tracker.client =
        client_resolving_with(&resolver, Duration::from_secs(5), Duration::from_secs(5));
    let defaults = LlmRuntimeConfig::default().endpoints;
    assert!(
        defaults
            .anthropic_usage
            .starts_with("https://api.anthropic.com/")
    );
    assert!(defaults.openai_usage.starts_with("https://api.openai.com/"));

    let anthropic = tracker
        .fetch_anthropic_usage("a-test-token")
        .await
        .unwrap_err();
    assert!(
        anthropic.contains("test-endpoint-guard") && anthropic.contains("(api.anthropic.com)"),
        "{anthropic}"
    );
    let openai = tracker
        .fetch_openai_usage("a-test-token")
        .await
        .unwrap_err();
    assert!(
        openai.contains("test-endpoint-guard") && openai.contains("(api.openai.com)"),
        "{openai}"
    );
    // Through the public path a refusal is no usage, and nothing is cached.
    assert!(
        tracker
            .get_usage(CredentialSource::ClaudeCode, "a-test-token")
            .await
            .is_none()
    );
    assert_eq!(resolver.lookups(), 0, "no connection was tried");
}

/// A regional host of OpenAI's API is refused too, and a loopback usage
/// endpoint (`tracker_on`'s) still answers: the guard reads the host only.
#[cfg(all(
    feature = "openai",
    not(feature = "live-eval"),
    any(target_os = "macos", target_os = "linux")
))]
#[tokio::test]
async fn the_usage_guard_refuses_only_the_real_hosts() {
    use crate::test_support::{
        MockHttpServer, MockResponse, Resolver, client_resolving_with, refused_addr,
    };
    let resolver = Resolver::to(refused_addr());
    let mut rt = LlmRuntimeConfig::default();
    rt.endpoints.openai_usage = "https://eu.api.openai.com/dashboard/billing/usage".to_string();
    let mut tracker =
        ProviderUsageTracker::with_runtime_config(Arc::new(ArcSwap::from_pointee(rt)));
    tracker.client =
        client_resolving_with(&resolver, Duration::from_secs(5), Duration::from_secs(5));
    let regional = tracker
        .fetch_openai_usage("a-test-token")
        .await
        .unwrap_err();
    assert!(regional.contains("(eu.api.openai.com)"), "{regional}");
    assert_eq!(resolver.lookups(), 0, "no connection was tried");

    let server =
        MockHttpServer::start(|_| MockResponse::json(r#"{"total_cost": 1.0, "total_tokens": 10}"#))
            .await;
    let loopback = tracker_on(&server)
        .fetch_anthropic_usage("a-test-token")
        .await
        .expect("a loopback usage endpoint answers");
    assert_eq!(loopback.token_count, 10);
}

#[test]
fn test_approximate_flag_always_true() {
    let body = r#"{"total_cost": 12.5, "total_tokens": 50000}"#;
    let usage = parse_anthropic_usage_response(body).unwrap();
    assert!(usage.approximate);

    let body = r#"{"total_usage": 1250, "total_tokens": 50000}"#;
    let usage = parse_openai_usage_response(body).unwrap();
    assert!(usage.approximate);
}

#[test]
fn test_parse_anthropic_usage() {
    let body = r#"{"total_cost": 12.5, "total_tokens": 50000, "period": "2026-02"}"#;
    let usage = parse_anthropic_usage_response(body).unwrap();
    assert!((usage.cost_usd - 12.5).abs() < 0.01);
    assert_eq!(usage.token_count, 50000);
    assert_eq!(usage.period, "2026-02");
    assert!(usage.approximate);
}

#[test]
fn test_parse_openai_usage() {
    let body = r#"{"total_usage": 1250.0, "total_tokens": 100000}"#;
    let usage = parse_openai_usage_response(body).unwrap();
    assert!((usage.cost_usd - 12.5).abs() < 0.01); // 1250 cents = $12.50
    assert_eq!(usage.token_count, 100000);
    assert!(usage.approximate);
}

#[test]
fn test_parse_anthropic_usage_empty() {
    let body = r#"{}"#;
    let usage = parse_anthropic_usage_response(body).unwrap();
    assert!((usage.cost_usd - 0.0).abs() < 0.01);
    assert_eq!(usage.token_count, 0);
}

#[test]
fn test_parse_invalid_json() {
    let body = "not json";
    let result = parse_anthropic_usage_response(body);
    assert!(result.is_err());
}

#[test]
fn test_current_period_format() {
    let period = current_period();
    // Should be in YYYY-MM format
    assert!(period.len() >= 6); // at least "YYYY-M"
    assert!(period.contains('-'));
}

#[test]
fn test_cached_usage_staleness() {
    let cached = CachedUsage {
        data: ExternalUsage {
            period: "2026-02".to_string(),
            cost_usd: 0.0,
            token_count: 0,
            rate_limit_remaining: None,
            fetched_at: "0Z".to_string(),
            approximate: true,
        },
        fetched_at: Instant::now(),
    };
    let ttl = Duration::from_secs(300);
    assert!(!cached.is_stale(ttl)); // Just created, should not be stale
}
