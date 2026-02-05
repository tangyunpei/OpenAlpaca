//! Multi-key management with round-robin selection, cooldown, and rate-limit tracking.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Provider type for categorizing API keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Ollama,
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::OpenAI => write!(f, "openai"),
            Self::Ollama => write!(f, "ollama"),
        }
    }
}

/// Key selection strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionStrategy {
    #[default]
    RoundRobin,
    LeastRecentlyUsed,
}

/// Outcome of an API call, reported back to the pool.
#[derive(Debug, Clone)]
pub enum CallResult {
    Success,
    RateLimited { retry_after_ms: u64 },
    Error(String),
}

/// Internal rate-limit tracking state.
#[derive(Debug, Clone, Default)]
pub struct RateLimitState {
    pub cooldown_until: Option<Instant>,
    pub consecutive_rate_limits: u32,
}

/// An API key with metadata.
#[derive(Debug)]
pub struct ApiKey {
    pub id: String,
    pub provider: ProviderType,
    pub secret: String,
    pub tier: Option<String>,
    pub rate_limit: Option<u32>,
    pub allowed_models: Vec<String>,
    pub monthly_budget: Option<f64>,
    pub rate_state: RateLimitState,
}

impl ApiKey {
    pub fn new(id: String, provider: ProviderType, secret: String) -> Self {
        Self {
            id,
            provider,
            secret,
            tier: None,
            rate_limit: None,
            allowed_models: Vec::new(),
            monthly_budget: None,
            rate_state: RateLimitState::default(),
        }
    }

    fn is_available(&self) -> bool {
        match self.rate_state.cooldown_until {
            Some(until) => Instant::now() >= until,
            None => true,
        }
    }
}

/// Guard returned by `acquire()` — holds the key id and secret for use.
#[derive(Debug, Clone)]
pub struct KeyGuard {
    pub id: String,
    pub secret: String,
}

/// Errors from key pool operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum KeyPoolError {
    #[error("No keys configured for this provider")]
    NoKeys,

    #[error("All keys are currently rate-limited")]
    AllKeysRateLimited,
}

/// Pool of API keys with rotation and cooldown support.
pub struct KeyPool {
    keys: Vec<Arc<RwLock<ApiKey>>>,
    strategy: SelectionStrategy,
    round_robin_index: AtomicUsize,
}

impl KeyPool {
    pub fn new(keys: Vec<ApiKey>, strategy: SelectionStrategy) -> Self {
        let keys = keys.into_iter().map(|k| Arc::new(RwLock::new(k))).collect();
        Self {
            keys,
            strategy,
            round_robin_index: AtomicUsize::new(0),
        }
    }

    /// Acquire an available key from the pool.
    pub async fn acquire(&self) -> Result<KeyGuard, KeyPoolError> {
        if self.keys.is_empty() {
            return Err(KeyPoolError::NoKeys);
        }

        let len = self.keys.len();
        let start = match self.strategy {
            SelectionStrategy::RoundRobin => {
                self.round_robin_index.fetch_add(1, Ordering::Relaxed) % len
            }
            SelectionStrategy::LeastRecentlyUsed => 0,
        };

        for i in 0..len {
            let idx = (start + i) % len;
            let key = self.keys[idx].read().await;
            if key.is_available() {
                return Ok(KeyGuard {
                    id: key.id.clone(),
                    secret: key.secret.clone(),
                });
            }
        }

        Err(KeyPoolError::AllKeysRateLimited)
    }

    /// Report the result of an API call for a specific key.
    pub async fn report_result(&self, key_id: &str, result: CallResult) {
        for key_lock in &self.keys {
            let mut key = key_lock.write().await;
            if key.id == key_id {
                match result {
                    CallResult::Success => {
                        key.rate_state.cooldown_until = None;
                        key.rate_state.consecutive_rate_limits = 0;
                    }
                    CallResult::RateLimited { retry_after_ms } => {
                        key.rate_state.consecutive_rate_limits += 1;
                        let cooldown = Duration::from_millis(retry_after_ms);
                        key.rate_state.cooldown_until = Some(Instant::now() + cooldown);
                    }
                    CallResult::Error(_) => {
                        // Don't cooldown on general errors, just on rate limits
                    }
                }
                break;
            }
        }
    }

    /// Reset all keys' rate-limit state.
    pub async fn reset_all(&self) {
        for key_lock in &self.keys {
            let mut key = key_lock.write().await;
            key.rate_state = RateLimitState::default();
        }
    }

    /// Number of keys in the pool.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(id: &str) -> ApiKey {
        ApiKey::new(id.to_string(), ProviderType::Anthropic, format!("sk-{}", id))
    }

    #[tokio::test]
    async fn test_round_robin_selection() {
        let pool = KeyPool::new(
            vec![make_key("k1"), make_key("k2"), make_key("k3")],
            SelectionStrategy::RoundRobin,
        );

        let g1 = pool.acquire().await.unwrap();
        let g2 = pool.acquire().await.unwrap();
        let g3 = pool.acquire().await.unwrap();
        let g4 = pool.acquire().await.unwrap();

        // Should cycle through k1, k2, k3, k1
        assert_eq!(g1.id, "k1");
        assert_eq!(g2.id, "k2");
        assert_eq!(g3.id, "k3");
        assert_eq!(g4.id, "k1");
    }

    #[tokio::test]
    async fn test_cooldown_skips_key() {
        let pool = KeyPool::new(
            vec![make_key("k1"), make_key("k2")],
            SelectionStrategy::RoundRobin,
        );

        // Rate-limit k1
        pool.report_result("k1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;

        // Next acquire should skip k1 and return k2
        let guard = pool.acquire().await.unwrap();
        assert_eq!(guard.id, "k2");
    }

    #[tokio::test]
    async fn test_all_keys_limited() {
        let pool = KeyPool::new(
            vec![make_key("k1"), make_key("k2")],
            SelectionStrategy::RoundRobin,
        );

        pool.report_result("k1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;
        pool.report_result("k2", CallResult::RateLimited { retry_after_ms: 60_000 }).await;

        let result = pool.acquire().await;
        assert!(matches!(result, Err(KeyPoolError::AllKeysRateLimited)));
    }

    #[tokio::test]
    async fn test_reset_all() {
        let pool = KeyPool::new(
            vec![make_key("k1"), make_key("k2")],
            SelectionStrategy::RoundRobin,
        );

        pool.report_result("k1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;
        pool.report_result("k2", CallResult::RateLimited { retry_after_ms: 60_000 }).await;

        pool.reset_all().await;

        let guard = pool.acquire().await.unwrap();
        assert!(!guard.id.is_empty());
    }

    #[tokio::test]
    async fn test_success_clears_cooldown() {
        let pool = KeyPool::new(
            vec![make_key("k1")],
            SelectionStrategy::RoundRobin,
        );

        pool.report_result("k1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;
        assert!(pool.acquire().await.is_err());

        pool.report_result("k1", CallResult::Success).await;
        assert!(pool.acquire().await.is_ok());
    }

    #[tokio::test]
    async fn test_empty_pool() {
        let pool = KeyPool::new(vec![], SelectionStrategy::RoundRobin);
        assert!(pool.is_empty());
        assert!(matches!(pool.acquire().await, Err(KeyPoolError::NoKeys)));
    }

    #[test]
    fn test_provider_type_display() {
        assert_eq!(ProviderType::Anthropic.to_string(), "anthropic");
        assert_eq!(ProviderType::OpenAI.to_string(), "openai");
        assert_eq!(ProviderType::Ollama.to_string(), "ollama");
    }
}
