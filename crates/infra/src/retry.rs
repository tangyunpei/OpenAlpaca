use std::future::Future;
use std::time::Duration;

pub struct RetryConfig {
    pub attempts: u32,
    pub min_delay: Duration,
    pub max_delay: Duration,
    /// Jitter factor from 0.0 (none) to 1.0 (full jitter).
    pub jitter: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            attempts: 3,
            min_delay: Duration::from_millis(300),
            max_delay: Duration::from_secs(30),
            jitter: 0.0,
        }
    }
}

/// Retry an async operation with exponential backoff.
pub async fn retry_async<T, E, F, Fut>(config: &RetryConfig, f: F) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut last_err = None;
    for attempt in 0..config.attempts {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < config.attempts {
                    let delay = compute_delay(config, attempt);
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Err(last_err.unwrap())
}

fn compute_delay(config: &RetryConfig, attempt: u32) -> Duration {
    let base_ms = config.min_delay.as_millis() as f64 * 2.0f64.powi(attempt as i32);
    let capped_ms = base_ms.min(config.max_delay.as_millis() as f64);

    let final_ms = if config.jitter > 0.0 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let random_factor = (nanos % 1000) as f64 / 1000.0;
        let min_ms = capped_ms * (1.0 - config.jitter);
        min_ms + (capped_ms - min_ms) * random_factor
    } else {
        capped_ms
    };

    Duration::from_millis(final_ms.max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let config = RetryConfig::default();
        let result = retry_async(&config, || async { Ok::<_, &str>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let config = RetryConfig {
            attempts: 3,
            min_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
        };

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let result = retry_async(&config, || {
            let counter = counter_clone.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 { Err("not yet") } else { Ok(42) }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_all_attempts_fail() {
        let config = RetryConfig {
            attempts: 2,
            min_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter: 0.0,
        };

        let result = retry_async(&config, || async { Err::<(), _>("fail") }).await;
        assert_eq!(result.unwrap_err(), "fail");
    }

    #[test]
    fn test_compute_delay_exponential() {
        let config = RetryConfig {
            attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            jitter: 0.0,
        };

        let d0 = compute_delay(&config, 0);
        let d1 = compute_delay(&config, 1);
        let d2 = compute_delay(&config, 2);

        assert_eq!(d0.as_millis(), 100);
        assert_eq!(d1.as_millis(), 200);
        assert_eq!(d2.as_millis(), 400);
    }

    #[test]
    fn test_compute_delay_caps_at_max() {
        let config = RetryConfig {
            attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(300),
            jitter: 0.0,
        };

        let d3 = compute_delay(&config, 3);
        assert_eq!(d3.as_millis(), 300);
    }
}
