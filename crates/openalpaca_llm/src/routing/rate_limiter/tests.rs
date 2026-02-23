use super::*;

// ── TokenBucket tests ─────────────────────────────────────────

#[tokio::test]
async fn test_bucket_immediate_acquire() {
    let bucket = TokenBucket::new(10.0, 1.0);
    let wait = bucket.acquire(5.0).await;
    assert_eq!(wait, Duration::ZERO);
    // 5 tokens remaining
    assert!((bucket.available().await - 5.0).abs() < 0.5);
}

#[tokio::test]
async fn test_bucket_try_acquire() {
    let bucket = TokenBucket::new(3.0, 1.0);
    assert!(bucket.try_acquire(2.0).await);
    assert!(bucket.try_acquire(1.0).await);
    // Now empty — try_acquire should fail
    assert!(!bucket.try_acquire(1.0).await);
}

#[tokio::test]
async fn test_bucket_exhaustion_blocks() {
    let bucket = TokenBucket::new(2.0, 10.0); // 10 tokens/sec → fast refill
    // Drain the bucket
    bucket.acquire(2.0).await;
    // Next acquire should block briefly until refill
    let start = Instant::now();
    bucket.acquire(1.0).await;
    let elapsed = start.elapsed();
    // Should have waited ~100ms for 1 token at 10/sec rate
    assert!(elapsed >= Duration::from_millis(50));
    assert!(elapsed < Duration::from_secs(1));
}

#[tokio::test]
async fn test_bucket_refill_rate() {
    let bucket = TokenBucket::new(10.0, 100.0); // 100 tokens/sec
    bucket.acquire(10.0).await; // drain
    assert!(bucket.available().await < 1.0);
    // Wait 100ms → should refill ~10 tokens
    tokio::time::sleep(Duration::from_millis(100)).await;
    let avail = bucket.available().await;
    assert!(
        avail >= 8.0,
        "Expected ~10 tokens after 100ms at 100/sec, got {avail}"
    );
    assert!(avail <= 12.0);
}

#[tokio::test]
async fn test_bucket_does_not_exceed_capacity() {
    let bucket = TokenBucket::new(5.0, 100.0);
    // Wait a long time — should not exceed capacity
    tokio::time::sleep(Duration::from_millis(200)).await;
    let avail = bucket.available().await;
    assert!(
        avail <= 5.0,
        "Tokens should not exceed capacity, got {avail}"
    );
}

// ── backoff_with_jitter tests ─────────────────────────────────

#[test]
fn test_backoff_base_case() {
    let base = Duration::from_millis(500);
    let cap = Duration::from_secs(30);
    // Attempt 0: result should be in [1ms, 500ms]
    for _ in 0..100 {
        let d = backoff_with_jitter(base, 0, cap);
        assert!(
            d >= Duration::from_millis(1),
            "Expected >= 1ms, got {:?}",
            d
        );
        assert!(d <= base, "Expected <= 500ms, got {:?}", d);
    }
}

#[test]
fn test_backoff_increases_range() {
    let base = Duration::from_millis(500);
    let cap = Duration::from_secs(30);
    // Collect max from many samples at attempt 0 vs attempt 3
    let max_0: Duration = (0..200)
        .map(|_| backoff_with_jitter(base, 0, cap))
        .max()
        .unwrap();
    let max_3: Duration = (0..200)
        .map(|_| backoff_with_jitter(base, 3, cap))
        .max()
        .unwrap();
    // Attempt 3 range is 500*8 = 4000ms; attempt 0 range is 500ms
    assert!(
        max_3 > max_0,
        "Higher attempts should have larger range: max_0={:?}, max_3={:?}",
        max_0,
        max_3
    );
}

#[test]
fn test_backoff_respects_cap() {
    let base = Duration::from_millis(500);
    let cap = Duration::from_secs(2);
    for _ in 0..200 {
        let d = backoff_with_jitter(base, 20, cap); // 2^20 would be huge
        assert!(d <= cap, "Expected <= 2s cap, got {:?}", d);
    }
}

#[test]
fn test_backoff_zero_base() {
    let d = backoff_with_jitter(Duration::ZERO, 5, Duration::from_secs(30));
    assert_eq!(d, Duration::ZERO);
}

// ── CircuitBreaker tests ──────────────────────────────────────

#[tokio::test]
async fn test_circuit_starts_closed() {
    let cb = CircuitBreaker::new(3, Duration::from_secs(1));
    assert_eq!(cb.state().await, CircuitState::Closed);
    assert!(cb.check().await.is_ok());
}

#[tokio::test]
async fn test_circuit_trips_after_threshold() {
    let cb = CircuitBreaker::new(3, Duration::from_secs(60));
    cb.report_failure().await;
    cb.report_failure().await;
    assert_eq!(cb.state().await, CircuitState::Closed); // 2 < 3
    cb.report_failure().await;
    assert_eq!(cb.state().await, CircuitState::Open); // 3 >= 3
}

#[tokio::test]
async fn test_circuit_open_rejects() {
    let cb = CircuitBreaker::new(2, Duration::from_secs(60));
    cb.report_failure().await;
    cb.report_failure().await;
    assert_eq!(cb.state().await, CircuitState::Open);
    assert_eq!(cb.check().await, Err(CircuitState::Open));
}

#[tokio::test]
async fn test_circuit_half_open_after_timeout() {
    let cb = CircuitBreaker::new(2, Duration::from_millis(50));
    cb.report_failure().await;
    cb.report_failure().await;
    assert_eq!(cb.state().await, CircuitState::Open);

    // Wait for recovery timeout
    tokio::time::sleep(Duration::from_millis(60)).await;

    // check() should transition to HalfOpen and allow through
    assert!(cb.check().await.is_ok());
    assert_eq!(cb.state().await, CircuitState::HalfOpen);
}

#[tokio::test]
async fn test_circuit_half_open_success_closes() {
    let cb = CircuitBreaker::new(2, Duration::from_millis(50));
    cb.report_failure().await;
    cb.report_failure().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    cb.check().await.unwrap(); // → HalfOpen

    cb.report_success().await;
    assert_eq!(cb.state().await, CircuitState::Closed);
}

#[tokio::test]
async fn test_circuit_half_open_failure_reopens() {
    let cb = CircuitBreaker::new(2, Duration::from_millis(50));
    cb.report_failure().await;
    cb.report_failure().await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    cb.check().await.unwrap(); // → HalfOpen

    cb.report_failure().await;
    assert_eq!(cb.state().await, CircuitState::Open);
}

#[tokio::test]
async fn test_circuit_success_resets_count() {
    let cb = CircuitBreaker::new(3, Duration::from_secs(60));
    cb.report_failure().await;
    cb.report_failure().await; // 2 failures
    cb.report_success().await; // resets
    cb.report_failure().await;
    cb.report_failure().await; // only 2 since reset
    assert_eq!(cb.state().await, CircuitState::Closed);
}

// ── RateLimiterRegistry tests ─────────────────────────────────

#[tokio::test]
async fn test_registry_creates_on_demand() {
    let registry = RateLimiterRegistry::new(RateLimitConfig::default());
    let limiter = registry.get_or_create("key1", None);
    // Should have default RPM bucket
    let available = limiter.rpm_bucket.available().await;
    assert!(
        (available - 50.0).abs() < 1.0,
        "Expected ~50 RPM, got {available}"
    );
}

#[tokio::test]
async fn test_registry_reuses_limiter() {
    let registry = RateLimiterRegistry::new(RateLimitConfig::default());
    let l1 = registry.get_or_create("key1", None);
    // Consume some tokens
    l1.rpm_bucket.acquire(5.0).await;
    let l2 = registry.get_or_create("key1", None);
    // Should be the same instance (tokens consumed)
    let available = l2.rpm_bucket.available().await;
    assert!(available < 50.0, "Should share state, got {available}");
}

#[tokio::test]
async fn test_registry_uses_key_rate_limit() {
    let registry = RateLimiterRegistry::new(RateLimitConfig::default());
    let limiter = registry.get_or_create("key_custom", Some(100));
    let available = limiter.rpm_bucket.available().await;
    assert!(
        (available - 100.0).abs() < 1.0,
        "Expected ~100 RPM, got {available}"
    );
}

#[tokio::test]
async fn test_registry_circuit_breaker() {
    let mut config = RateLimitConfig::default();
    config.circuit_breaker_threshold = 2;
    config.circuit_breaker_recovery_secs = 60;
    let registry = RateLimiterRegistry::new(config);

    assert!(registry.check_circuit().await.is_ok());
    registry.report_failure().await;
    registry.report_failure().await;
    assert!(registry.check_circuit().await.is_err());
}

// ── RateLimitConfig tests ─────────────────────────────────────

#[test]
fn test_config_default() {
    let config = RateLimitConfig::default();
    assert_eq!(config.default_rpm, 50);
    assert_eq!(config.default_tpm, 40_000);
    assert_eq!(config.per_key_concurrency, 5);
    assert_eq!(config.global_concurrency, 10);
    assert_eq!(config.backoff_base_ms, 500);
    assert_eq!(config.backoff_cap_ms, 30_000);
    assert_eq!(config.circuit_breaker_threshold, 5);
    assert_eq!(config.circuit_breaker_recovery_secs, 30);
    assert_eq!(config.max_transient_retries, 3);
}

#[test]
fn test_config_serde_roundtrip() {
    let config = RateLimitConfig {
        default_rpm: 100,
        default_tpm: 80_000,
        per_key_concurrency: 3,
        global_concurrency: 8,
        backoff_base_ms: 1000,
        backoff_cap_ms: 60_000,
        circuit_breaker_threshold: 10,
        circuit_breaker_recovery_secs: 60,
        max_transient_retries: 3,
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: RateLimitConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.default_rpm, 100);
    assert_eq!(parsed.default_tpm, 80_000);
    assert_eq!(parsed.global_concurrency, 8);
}

#[test]
fn test_config_serde_defaults_from_empty() {
    // An empty JSON object should use all defaults
    let parsed: RateLimitConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(parsed.default_rpm, 50);
    assert_eq!(parsed.default_tpm, 40_000);
    assert_eq!(parsed.per_key_concurrency, 5);
}

// ── KeyRateLimiter tests ──────────────────────────────────────

#[tokio::test]
async fn test_key_limiter_acquire() {
    let limiter = KeyRateLimiter::new(60, 100_000, 4);
    // Should be able to acquire immediately (fresh buckets)
    let _permit = limiter.acquire(100).await;
    // permit is held — concurrency slot taken
    assert_eq!(limiter.concurrency.available_permits(), 3);
}

#[tokio::test]
async fn test_key_limiter_rpm_throttle() {
    // 10 RPM = ~0.167 tokens/sec — very slow refill
    let limiter = KeyRateLimiter::new(10, 100_000, 4);
    // Drain RPM bucket
    for _ in 0..10 {
        limiter.rpm_bucket.acquire(1.0).await;
    }
    // Next acquire should take time
    let start = Instant::now();
    let _permit = limiter.acquire(100).await;
    let elapsed = start.elapsed();
    // 1 token at 10/60 = 0.167/sec → ~6 seconds to refill 1 token
    // But we just need it to be > 0 (shows throttling is working)
    assert!(
        elapsed >= Duration::from_millis(100),
        "Expected throttling delay, got {:?}",
        elapsed
    );
}
