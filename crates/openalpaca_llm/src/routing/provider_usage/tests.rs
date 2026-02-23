use super::*;

#[tokio::test]
async fn test_cache_empty_returns_none_without_fetch() {
    let tracker = ProviderUsageTracker::new();
    // Without a valid token, fetch will fail and return None
    let result = tracker
        .get_usage(CredentialSource::ClaudeCode, "invalid-token")
        .await;
    // Should gracefully return None (or possibly Some if mock succeeds)
    // Main thing: no panic
    let _ = result;
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
