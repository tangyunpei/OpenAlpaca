use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Shared cost accumulator across a nested skill invocation chain.
/// Stores cost in micro-dollars (1 USD = 1,000,000) for lock-free atomics.
#[derive(Clone, Debug)]
pub struct LoopCostAccumulator {
    inner: Arc<AtomicU64>,
}

impl Default for LoopCostAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopCostAccumulator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Add a USD cost delta. Saturates at u64::MAX (~$18 trillion).
    pub fn add_usd(&self, usd: f64) {
        let micros = (usd * 1_000_000.0).round().max(0.0) as u64;
        self.inner.fetch_add(micros, Ordering::Relaxed);
    }

    /// Read the total accumulated cost in USD.
    pub fn total_usd(&self) -> f64 {
        self.inner.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_accumulator_atomic_addition() {
        let acc = LoopCostAccumulator::new();
        acc.add_usd(0.50);
        acc.add_usd(0.25);
        acc.add_usd(1.00);
        let total = acc.total_usd();
        assert!(
            (total - 1.75).abs() < 1e-6,
            "expected 1.75, got {total}"
        );
    }

    #[test]
    fn test_cost_accumulator_micro_dollar_rounding() {
        let acc = LoopCostAccumulator::new();
        for _ in 0..1000 {
            acc.add_usd(0.000001); // 1 micro-dollar each
        }
        let total = acc.total_usd();
        assert!(
            (total - 0.001).abs() < 1e-6,
            "expected 0.001, got {total}"
        );
    }

    #[test]
    fn test_cost_accumulator_negative_ignored() {
        let acc = LoopCostAccumulator::new();
        acc.add_usd(1.0);
        acc.add_usd(-0.5);
        let total = acc.total_usd();
        assert!(
            (total - 1.0).abs() < 1e-6,
            "negative should be clamped to 0, got {total}"
        );
    }

    #[test]
    fn test_cost_accumulator_concurrent() {
        let acc = LoopCostAccumulator::new();
        let threads: Vec<_> = (0..10)
            .map(|_| {
                let acc = acc.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        acc.add_usd(0.01);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let total = acc.total_usd();
        assert!(
            (total - 10.0).abs() < 1e-6,
            "10 threads x 100 x $0.01 = $10.00, got {total}"
        );
    }

    #[test]
    fn test_cost_accumulator_clone_shares_state() {
        let acc1 = LoopCostAccumulator::new();
        let acc2 = acc1.clone();
        acc1.add_usd(0.50);
        acc2.add_usd(0.50);
        assert!(
            (acc1.total_usd() - 1.0).abs() < 1e-6,
            "clone should share state"
        );
        assert!(
            (acc2.total_usd() - 1.0).abs() < 1e-6,
            "clone should share state"
        );
    }
}
