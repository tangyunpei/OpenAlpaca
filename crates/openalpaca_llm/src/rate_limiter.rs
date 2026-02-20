//! Rate-limiting primitives for the LLM router.
//!
//! Provides token-bucket rate limiters (RPM/TPM), exponential backoff with jitter,
//! a circuit breaker, and a per-key rate limiter registry. These work together to
//! prevent rate-limit stampedes when parallel subagents call the API simultaneously.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};

// ── Token Bucket ──────────────────────────────────────────────────────

/// A token-bucket rate limiter.
///
/// Refills tokens at a constant rate up to a maximum capacity.
/// Callers acquire tokens before making API calls; if tokens are
/// insufficient, the caller sleeps until enough tokens have accumulated.
pub struct TokenBucket {
    inner: Mutex<TokenBucketInner>,
}

struct TokenBucketInner {
    tokens: f64,
    capacity: f64,
    refill_rate: f64, // tokens per second
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new token bucket.
    ///
    /// - `capacity`: maximum burst size (e.g. RPM limit or TPM limit)
    /// - `refill_rate`: tokens added per second (e.g. 50 RPM → 50.0/60.0 ≈ 0.833/s)
    pub fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            inner: Mutex::new(TokenBucketInner {
                tokens: capacity,
                capacity,
                refill_rate,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Acquire `n` tokens, sleeping if necessary until they are available.
    /// Returns the duration waited (zero if tokens were immediately available).
    pub async fn acquire(&self, n: f64) -> Duration {
        let mut total_wait = Duration::ZERO;

        loop {
            let wait = {
                let mut inner = self.inner.lock().await;
                inner.refill();

                if inner.tokens >= n {
                    inner.tokens -= n;
                    return total_wait;
                }

                // Calculate how long to wait for enough tokens
                let deficit = n - inner.tokens;
                if inner.refill_rate > 0.0 {
                    Duration::from_secs_f64(deficit / inner.refill_rate)
                } else {
                    // Zero refill rate — would wait forever; cap at 60s
                    Duration::from_secs(60)
                }
            };

            tokio::time::sleep(wait).await;
            total_wait += wait;
        }
    }

    /// Try to acquire `n` tokens without blocking.
    /// Returns `true` if tokens were available and consumed.
    pub async fn try_acquire(&self, n: f64) -> bool {
        let mut inner = self.inner.lock().await;
        inner.refill();

        if inner.tokens >= n {
            inner.tokens -= n;
            true
        } else {
            false
        }
    }

    /// Peek at the current number of available tokens (for diagnostics).
    pub async fn available(&self) -> f64 {
        let mut inner = self.inner.lock().await;
        inner.refill();
        inner.tokens
    }
}

impl TokenBucketInner {
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill);
        let new_tokens = elapsed.as_secs_f64() * self.refill_rate;
        self.tokens = (self.tokens + new_tokens).min(self.capacity);
        self.last_refill = now;
    }
}

// ── Exponential Backoff with Jitter ───────────────────────────────────

/// Compute exponential backoff with "full jitter".
///
/// Formula: `random(0, min(cap, base * 2^attempt))`
///
/// This is the "Full Jitter" algorithm from the AWS Architecture Blog,
/// which provides the best stampede/thundering-herd prevention because
/// all waiters get randomised, independent delays.
///
/// - `base`: initial backoff (e.g. 500ms)
/// - `attempt`: retry attempt number (0-indexed)
/// - `cap`: maximum backoff (e.g. 30s)
pub fn backoff_with_jitter(base: Duration, attempt: u32, cap: Duration) -> Duration {
    use rand::RngExt;

    let base_ms = base.as_millis() as u64;
    // Cap exponent at 10 to prevent overflow (2^10 = 1024)
    let exp_ms = base_ms.saturating_mul(1u64 << attempt.min(10));
    let capped_ms = exp_ms.min(cap.as_millis() as u64);

    if capped_ms == 0 {
        return Duration::ZERO;
    }

    let jitter_ms: u64 = rand::rng().random_range(1..=capped_ms);
    Duration::from_millis(jitter_ms)
}

// ── Circuit Breaker ───────────────────────────────────────────────────

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Too many failures — all requests rejected immediately.
    Open,
    /// Recovery probe — one request allowed to test if service is back.
    HalfOpen,
}

/// A simple circuit breaker that trips after consecutive failures.
///
/// - `Closed`: requests pass through. On failure, increment counter.
/// - After `failure_threshold` consecutive failures → `Open`.
/// - `Open`: all `check()` calls return `Err(Open)` immediately.
/// - After `recovery_timeout` → `HalfOpen`.
/// - `HalfOpen`: one request allowed. Success → `Closed`; failure → `Open`.
pub struct CircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
}

struct CircuitBreakerInner {
    state: CircuitState,
    failure_count: u32,
    failure_threshold: u32,
    last_failure: Option<Instant>,
    recovery_timeout: Duration,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    ///
    /// - `failure_threshold`: consecutive failures before tripping to Open
    /// - `recovery_timeout`: how long to wait in Open before trying HalfOpen
    pub fn new(failure_threshold: u32, recovery_timeout: Duration) -> Self {
        Self {
            inner: Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                failure_count: 0,
                failure_threshold,
                last_failure: None,
                recovery_timeout,
            }),
        }
    }

    /// Check if a request should proceed.
    ///
    /// Returns `Ok(())` if the request is allowed, `Err(state)` if blocked.
    pub async fn check(&self) -> Result<(), CircuitState> {
        let mut inner = self.inner.lock().await;

        match inner.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                // Check if recovery timeout has elapsed
                if let Some(last) = inner.last_failure
                    && last.elapsed() >= inner.recovery_timeout
                {
                    inner.state = CircuitState::HalfOpen;
                    return Ok(());
                }
                Err(CircuitState::Open)
            }
            CircuitState::HalfOpen => {
                // Allow the probe request through
                Ok(())
            }
        }
    }

    /// Report a successful API call.
    /// Resets the circuit breaker to Closed.
    pub async fn report_success(&self) {
        let mut inner = self.inner.lock().await;
        inner.state = CircuitState::Closed;
        inner.failure_count = 0;
    }

    /// Report a failed API call.
    /// Increments failure counter; may trip to Open.
    pub async fn report_failure(&self) {
        let mut inner = self.inner.lock().await;
        inner.failure_count += 1;
        inner.last_failure = Some(Instant::now());

        match inner.state {
            CircuitState::Closed => {
                if inner.failure_count >= inner.failure_threshold {
                    inner.state = CircuitState::Open;
                    tracing::warn!(
                        failures = inner.failure_count,
                        "Circuit breaker tripped to Open after {} consecutive failures",
                        inner.failure_count
                    );
                }
            }
            CircuitState::HalfOpen => {
                // Probe failed — go back to Open
                inner.state = CircuitState::Open;
                tracing::warn!("Circuit breaker probe failed, returning to Open state");
            }
            CircuitState::Open => {
                // Already open — nothing to do
            }
        }
    }

    /// Get the current circuit state (for diagnostics/logging).
    pub async fn state(&self) -> CircuitState {
        self.inner.lock().await.state
    }
}

// ── Per-Key Rate Limiter ──────────────────────────────────────────────

/// Per-key rate limiting state.
///
/// Each API key gets one of these, holding RPM and TPM token buckets
/// plus a per-key concurrency semaphore.
pub struct KeyRateLimiter {
    /// Requests-per-minute token bucket.
    pub rpm_bucket: TokenBucket,
    /// Tokens-per-minute token bucket (estimated input + output tokens).
    pub tpm_bucket: TokenBucket,
    /// Per-key concurrency limit.
    pub concurrency: Arc<Semaphore>,
}

impl KeyRateLimiter {
    /// Create a new per-key rate limiter.
    ///
    /// - `rpm`: requests per minute limit
    /// - `tpm`: tokens per minute limit
    /// - `max_concurrent`: max concurrent in-flight requests for this key
    pub fn new(rpm: u32, tpm: u32, max_concurrent: usize) -> Self {
        Self {
            rpm_bucket: TokenBucket::new(rpm as f64, rpm as f64 / 60.0),
            tpm_bucket: TokenBucket::new(tpm as f64, tpm as f64 / 60.0),
            concurrency: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Acquire permission to make a request with estimated token count.
    ///
    /// 1. Acquires per-key concurrency permit
    /// 2. Acquires 1 RPM token
    /// 3. Acquires `estimated_tokens` TPM tokens
    ///
    /// Returns the concurrency permit (held until the API call completes).
    pub async fn acquire(&self, estimated_tokens: u32) -> SemaphorePermit<'_> {
        // 1. Concurrency permit (blocks if too many in-flight for this key)
        let permit = self
            .concurrency
            .acquire()
            .await
            .expect("KeyRateLimiter semaphore should never be closed");

        // 2. RPM token (blocks until within RPM budget)
        let rpm_wait = self.rpm_bucket.acquire(1.0).await;
        if rpm_wait > Duration::from_millis(100) {
            tracing::debug!(wait_ms = rpm_wait.as_millis(), "Waited for RPM budget");
        }

        // 3. TPM tokens (blocks until within TPM budget)
        let tpm_wait = self.tpm_bucket.acquire(estimated_tokens as f64).await;
        if tpm_wait > Duration::from_millis(100) {
            tracing::debug!(
                wait_ms = tpm_wait.as_millis(),
                tokens = estimated_tokens,
                "Waited for TPM budget"
            );
        }

        permit
    }
}

// ── Rate Limiter Registry ─────────────────────────────────────────────

/// Configuration for rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Default requests-per-minute per key (when key has no `rate_limit` field).
    /// Default: 50 (conservative for Anthropic Tier 1).
    #[serde(default = "default_rpm")]
    pub default_rpm: u32,

    /// Default tokens-per-minute per key.
    /// Default: 40000 (conservative).
    #[serde(default = "default_tpm")]
    pub default_tpm: u32,

    /// Maximum concurrent requests per key.
    /// Default: 2.
    #[serde(default = "default_per_key_concurrency")]
    pub per_key_concurrency: usize,

    /// Global maximum concurrent LLM API calls.
    /// Default: 4.
    #[serde(default = "default_global_concurrency")]
    pub global_concurrency: usize,

    /// Exponential backoff base delay in milliseconds.
    /// Default: 500.
    #[serde(default = "default_backoff_base_ms")]
    pub backoff_base_ms: u64,

    /// Exponential backoff maximum delay in milliseconds.
    /// Default: 30000 (30 seconds).
    #[serde(default = "default_backoff_cap_ms")]
    pub backoff_cap_ms: u64,

    /// Circuit breaker: consecutive failures before tripping.
    /// Default: 5.
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,

    /// Circuit breaker: recovery timeout in seconds.
    /// Default: 30.
    #[serde(default = "default_circuit_breaker_recovery_secs")]
    pub circuit_breaker_recovery_secs: u64,

    /// Maximum transient retries per `execute_with_retry` call.
    /// Decoupled from key pool size so even a single-key setup gets enough attempts.
    /// Default: 3.
    #[serde(default = "default_max_transient_retries")]
    pub max_transient_retries: usize,
}

fn default_rpm() -> u32 {
    50
}
fn default_tpm() -> u32 {
    40_000
}
fn default_per_key_concurrency() -> usize {
    5
}
fn default_global_concurrency() -> usize {
    10
}
fn default_backoff_base_ms() -> u64 {
    500
}
fn default_backoff_cap_ms() -> u64 {
    30_000
}
fn default_circuit_breaker_threshold() -> u32 {
    5
}
fn default_circuit_breaker_recovery_secs() -> u64 {
    30
}
fn default_max_transient_retries() -> usize {
    3
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            default_rpm: default_rpm(),
            default_tpm: default_tpm(),
            per_key_concurrency: default_per_key_concurrency(),
            global_concurrency: default_global_concurrency(),
            backoff_base_ms: default_backoff_base_ms(),
            backoff_cap_ms: default_backoff_cap_ms(),
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            circuit_breaker_recovery_secs: default_circuit_breaker_recovery_secs(),
            max_transient_retries: default_max_transient_retries(),
        }
    }
}

/// Global rate limiter registry.
///
/// Maps key IDs to per-key rate limiters and owns the global circuit breaker.
/// Lives on `LlmRouter` and is shared across all concurrent callers via `Arc`.
pub struct RateLimiterRegistry {
    limiters: DashMap<String, Arc<KeyRateLimiter>>,
    circuit_breaker: CircuitBreaker,
    config: RateLimitConfig,
}

impl RateLimiterRegistry {
    /// Create a new registry from config.
    pub fn new(config: RateLimitConfig) -> Self {
        let circuit_breaker = CircuitBreaker::new(
            config.circuit_breaker_threshold,
            Duration::from_secs(config.circuit_breaker_recovery_secs),
        );
        Self {
            limiters: DashMap::new(),
            circuit_breaker,
            config,
        }
    }

    /// Get or create a rate limiter for a specific key.
    ///
    /// If the key has a configured `rate_limit` (RPM), that value is used.
    /// Otherwise, `default_rpm` from config is used.
    pub fn get_or_create(&self, key_id: &str, key_rate_limit: Option<u32>) -> Arc<KeyRateLimiter> {
        self.limiters
            .entry(key_id.to_string())
            .or_insert_with(|| {
                let rpm = key_rate_limit.unwrap_or(self.config.default_rpm);
                Arc::new(KeyRateLimiter::new(
                    rpm,
                    self.config.default_tpm,
                    self.config.per_key_concurrency,
                ))
            })
            .clone()
    }

    /// Check the global circuit breaker before making a request.
    pub async fn check_circuit(&self) -> Result<(), CircuitState> {
        self.circuit_breaker.check().await
    }

    /// Report a successful API call to the circuit breaker.
    pub async fn report_success(&self) {
        self.circuit_breaker.report_success().await;
    }

    /// Report a failed API call to the circuit breaker.
    pub async fn report_failure(&self) {
        self.circuit_breaker.report_failure().await;
    }

    /// Get the config (for reading backoff parameters, etc.).
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
}
