// crates/openalpaca_mcp/src/lifecycle.rs

use std::time::{Duration, Instant};

use crate::error::McpError;

/// Connection state for an [`crate::McpClient`]. Stored under a `RwLock` in the inner.
#[derive(Debug)]
pub(crate) enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting { attempt: u32, next_at: Instant },
    Disconnected,
    Failed { reason: McpError },
}

/// Compute the backoff duration for a given attempt number.
///
/// Exponential: `base * 2^(attempt-1)`, capped at `max`.
/// Attempt is 1-based. Attempt 0 returns Duration::ZERO (no backoff before first try).
pub(crate) fn backoff_for_attempt(attempt: u32, base_ms: u64, max: Duration) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    // Clamp shift to avoid overflow for very large attempts.
    let shift = attempt.saturating_sub(1).min(20);
    let ms = base_ms.saturating_mul(1u64 << shift);
    Duration::from_millis(ms).min(max)
}

/// Add ±10% jitter to a duration. Deterministic when `rand_factor` is fixed.
pub(crate) fn apply_jitter(dur: Duration, rand_factor: f64) -> Duration {
    // rand_factor must be in [-1.0, 1.0]; scale by 0.10 for ±10% spread.
    let scaled = rand_factor.clamp(-1.0, 1.0) * 0.10;
    let ms = dur.as_millis() as f64 * (1.0 + scaled);
    Duration::from_millis(ms.max(0.0) as u64)
}

/// Maximum backoff cap used by the default reconnect policy.
pub(crate) const MAX_BACKOFF: Duration = Duration::from_millis(1600);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_exponential() {
        // base=100ms, max=1600ms
        assert_eq!(backoff_for_attempt(0, 100, MAX_BACKOFF), Duration::ZERO);
        assert_eq!(backoff_for_attempt(1, 100, MAX_BACKOFF), Duration::from_millis(100));
        assert_eq!(backoff_for_attempt(2, 100, MAX_BACKOFF), Duration::from_millis(200));
        assert_eq!(backoff_for_attempt(3, 100, MAX_BACKOFF), Duration::from_millis(400));
        assert_eq!(backoff_for_attempt(4, 100, MAX_BACKOFF), Duration::from_millis(800));
        assert_eq!(backoff_for_attempt(5, 100, MAX_BACKOFF), Duration::from_millis(1600));
        // capped
        assert_eq!(backoff_for_attempt(6, 100, MAX_BACKOFF), Duration::from_millis(1600));
        assert_eq!(backoff_for_attempt(100, 100, MAX_BACKOFF), Duration::from_millis(1600));
    }

    #[test]
    fn backoff_handles_overflow_safely() {
        // Very large attempt, very large base — must not panic
        let d = backoff_for_attempt(u32::MAX, u64::MAX / 2, Duration::from_secs(60));
        assert_eq!(d, Duration::from_secs(60)); // capped
    }

    #[test]
    fn jitter_within_10_percent_on_positive_factor() {
        let base = Duration::from_millis(1000);
        let jittered = apply_jitter(base, 1.0); // max positive jitter
        assert_eq!(jittered, Duration::from_millis(1100));
    }

    #[test]
    fn jitter_within_10_percent_on_negative_factor() {
        let base = Duration::from_millis(1000);
        let jittered = apply_jitter(base, -1.0); // max negative jitter
        assert_eq!(jittered, Duration::from_millis(900));
    }

    #[test]
    fn jitter_zero_factor_no_change() {
        let base = Duration::from_millis(500);
        assert_eq!(apply_jitter(base, 0.0), base);
    }

    #[test]
    fn jitter_factor_clamped() {
        // Factors outside [-1,1] are clamped — +5.0 should act like +1.0
        let base = Duration::from_millis(1000);
        assert_eq!(apply_jitter(base, 5.0), Duration::from_millis(1100));
        assert_eq!(apply_jitter(base, -5.0), Duration::from_millis(900));
    }
}
