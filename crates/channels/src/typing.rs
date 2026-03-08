use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

/// Handle to a running typing indicator loop.
/// Drop or call `stop()` to cancel the loop.
pub struct TypingHandle {
    stopped: Arc<AtomicBool>,
    notify: Arc<Notify>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl TypingHandle {
    /// Stop the typing indicator loop and wait for it to finish.
    pub async fn stop(mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify.notify_one();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for TypingHandle {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }
}

/// Start a typing indicator loop.
///
/// Calls `tick_fn` every `interval`, stops after `max_duration`.
/// Circuit breaker: trips after `max_failures` consecutive failures.
pub fn start_typing_indicator<F, Fut>(
    tick_fn: F,
    interval: Duration,
    max_duration: Duration,
    max_failures: u32,
) -> TypingHandle
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send,
{
    let stopped = Arc::new(AtomicBool::new(false));
    let notify = Arc::new(Notify::new());
    let stopped_clone = stopped.clone();
    let notify_clone = notify.clone();

    let handle = tokio::spawn(async move {
        let mut consecutive_failures: u32 = 0;
        let start = tokio::time::Instant::now();

        loop {
            if stopped_clone.load(Ordering::SeqCst) {
                break;
            }

            if start.elapsed() >= max_duration {
                tracing::debug!("typing indicator: max duration reached");
                break;
            }

            // Execute the tick
            match tick_fn().await {
                Ok(()) => {
                    consecutive_failures = 0;
                }
                Err(e) => {
                    consecutive_failures += 1;
                    tracing::warn!(
                        failures = consecutive_failures,
                        "typing indicator tick failed: {e}"
                    );
                    if consecutive_failures >= max_failures {
                        tracing::warn!("typing indicator: circuit breaker tripped");
                        break;
                    }
                }
            }

            // Wait for interval or stop signal
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                () = notify_clone.notified() => {
                    break;
                }
            }
        }
    });

    TypingHandle {
        stopped,
        notify,
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[tokio::test]
    async fn test_typing_indicator_ticks() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let handle = start_typing_indicator(
            move || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            Duration::from_millis(50),
            Duration::from_secs(10),
            2,
        );

        tokio::time::sleep(Duration::from_millis(180)).await;
        handle.stop().await;

        let ticks = count.load(Ordering::SeqCst);
        assert!(ticks >= 2, "expected at least 2 ticks, got {ticks}");
    }

    #[tokio::test]
    async fn test_typing_indicator_stop() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let handle = start_typing_indicator(
            move || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            Duration::from_millis(50),
            Duration::from_secs(10),
            2,
        );

        // Stop immediately after first tick
        tokio::time::sleep(Duration::from_millis(10)).await;
        handle.stop().await;

        let ticks = count.load(Ordering::SeqCst);
        assert!(ticks <= 2, "should have stopped quickly, got {ticks} ticks");
    }

    #[tokio::test]
    async fn test_typing_indicator_circuit_breaker() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let handle = start_typing_indicator(
            move || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err("simulated failure".into())
                }
            },
            Duration::from_millis(10),
            Duration::from_secs(10),
            2, // trip after 2 failures
        );

        // Wait for circuit breaker to trip
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.stop().await;

        let ticks = count.load(Ordering::SeqCst);
        assert_eq!(
            ticks, 2,
            "should have tripped after 2 failures, got {ticks}"
        );
    }

    #[tokio::test]
    async fn test_typing_indicator_max_duration() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();

        let handle = start_typing_indicator(
            move || {
                let c = count_clone.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            Duration::from_millis(30),
            Duration::from_millis(100), // short max duration
            10,
        );

        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.stop().await;

        let ticks = count.load(Ordering::SeqCst);
        // With 30ms interval and 100ms max, should get ~3-4 ticks max
        assert!(
            ticks <= 5,
            "should have stopped at max duration, got {ticks}"
        );
    }
}
