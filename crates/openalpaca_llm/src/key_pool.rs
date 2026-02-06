//! Multi-key management with round-robin selection, cooldown, and rate-limit tracking.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

/// Provider type for categorizing API keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Ollama,
}

impl ProviderType {
    /// Return all known provider variants.
    pub fn all() -> &'static [ProviderType] {
        &[ProviderType::Anthropic, ProviderType::OpenAI, ProviderType::Ollama]
    }
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

/// Key priority for PrimaryFallback strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPriority {
    #[default]
    Primary,
    Fallback,
}

/// Source of an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    ApiConsole,
    ClaudeCode,
    ClaudeMaxPro,
    Environment,
    #[default]
    Other,
}

/// Key selection strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStrategy {
    #[default]
    RoundRobin,
    LeastRecentlyUsed,
    PrimaryFallback,
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
    pub priority: KeyPriority,
    pub source: KeySource,
    pub notes: Option<String>,
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
            priority: KeyPriority::default(),
            source: KeySource::default(),
            notes: None,
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

/// Health status of a key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyHealthStatus {
    Healthy,
    RateLimited,
    Error,
    Unknown,
}

/// Status information for a single key (for API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyStatus {
    pub id: String,
    pub health: KeyHealthStatus,
    pub consecutive_rate_limits: u32,
    pub is_available: bool,
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

    /// Get the current selection strategy.
    pub fn strategy(&self) -> SelectionStrategy {
        self.strategy
    }

    /// Acquire an available key from the pool.
    pub async fn acquire(&self) -> Result<KeyGuard, KeyPoolError> {
        if self.keys.is_empty() {
            return Err(KeyPoolError::NoKeys);
        }

        match self.strategy {
            SelectionStrategy::PrimaryFallback => self.acquire_primary_fallback().await,
            _ => self.acquire_standard().await,
        }
    }

    /// Standard round-robin / LRU acquisition.
    async fn acquire_standard(&self) -> Result<KeyGuard, KeyPoolError> {
        let len = self.keys.len();
        let start = match self.strategy {
            SelectionStrategy::RoundRobin => {
                self.round_robin_index.fetch_add(1, Ordering::Relaxed) % len
            }
            SelectionStrategy::LeastRecentlyUsed => 0,
            SelectionStrategy::PrimaryFallback => unreachable!(),
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

    /// PrimaryFallback: try Primary keys first (round-robin among them),
    /// then Fallback keys if all Primary are rate-limited.
    async fn acquire_primary_fallback(&self) -> Result<KeyGuard, KeyPoolError> {
        // Try primary keys first
        let mut primary_indices = Vec::new();
        let mut fallback_indices = Vec::new();

        for (i, key_lock) in self.keys.iter().enumerate() {
            let key = key_lock.read().await;
            match key.priority {
                KeyPriority::Primary => primary_indices.push(i),
                KeyPriority::Fallback => fallback_indices.push(i),
            }
        }

        // Try primary keys with round-robin
        if !primary_indices.is_empty() {
            let start = self.round_robin_index.fetch_add(1, Ordering::Relaxed) % primary_indices.len();
            for i in 0..primary_indices.len() {
                let idx = primary_indices[(start + i) % primary_indices.len()];
                let key = self.keys[idx].read().await;
                if key.is_available() {
                    return Ok(KeyGuard {
                        id: key.id.clone(),
                        secret: key.secret.clone(),
                    });
                }
            }
        }

        // All primaries rate-limited, try fallback keys
        for idx in &fallback_indices {
            let key = self.keys[*idx].read().await;
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

    /// Get the status of all keys.
    pub async fn key_statuses(&self) -> Vec<KeyStatus> {
        let mut statuses = Vec::with_capacity(self.keys.len());
        for key_lock in &self.keys {
            let key = key_lock.read().await;
            let health = if key.rate_state.consecutive_rate_limits > 0 {
                if key.is_available() {
                    KeyHealthStatus::Healthy
                } else {
                    KeyHealthStatus::RateLimited
                }
            } else {
                KeyHealthStatus::Healthy
            };
            statuses.push(KeyStatus {
                id: key.id.clone(),
                health,
                consecutive_rate_limits: key.rate_state.consecutive_rate_limits,
                is_available: key.is_available(),
            });
        }
        statuses
    }
}

/// Mask a secret key for display — shows first 8 + last 4 characters.
pub fn mask_secret(secret: &str) -> String {
    if secret.len() <= 12 {
        return "*".repeat(secret.len());
    }
    let prefix = &secret[..8];
    let suffix = &secret[secret.len() - 4..];
    format!("{}...{}", prefix, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(id: &str) -> ApiKey {
        ApiKey::new(id.to_string(), ProviderType::Anthropic, format!("sk-{}", id))
    }

    fn make_key_with_priority(id: &str, priority: KeyPriority) -> ApiKey {
        let mut key = make_key(id);
        key.priority = priority;
        key
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

    #[tokio::test]
    async fn test_primary_fallback_prefers_primary() {
        let pool = KeyPool::new(
            vec![
                make_key_with_priority("primary1", KeyPriority::Primary),
                make_key_with_priority("fallback1", KeyPriority::Fallback),
                make_key_with_priority("primary2", KeyPriority::Primary),
            ],
            SelectionStrategy::PrimaryFallback,
        );

        // Should prefer primary keys
        let g1 = pool.acquire().await.unwrap();
        let g2 = pool.acquire().await.unwrap();
        assert!(g1.id == "primary1" || g1.id == "primary2");
        assert!(g2.id == "primary1" || g2.id == "primary2");
    }

    #[tokio::test]
    async fn test_primary_fallback_falls_back() {
        let pool = KeyPool::new(
            vec![
                make_key_with_priority("primary1", KeyPriority::Primary),
                make_key_with_priority("fallback1", KeyPriority::Fallback),
            ],
            SelectionStrategy::PrimaryFallback,
        );

        // Rate-limit all primary keys
        pool.report_result("primary1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;

        // Should fall back to fallback key
        let guard = pool.acquire().await.unwrap();
        assert_eq!(guard.id, "fallback1");
    }

    #[tokio::test]
    async fn test_primary_fallback_all_limited() {
        let pool = KeyPool::new(
            vec![
                make_key_with_priority("primary1", KeyPriority::Primary),
                make_key_with_priority("fallback1", KeyPriority::Fallback),
            ],
            SelectionStrategy::PrimaryFallback,
        );

        pool.report_result("primary1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;
        pool.report_result("fallback1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;

        let result = pool.acquire().await;
        assert!(matches!(result, Err(KeyPoolError::AllKeysRateLimited)));
    }

    #[test]
    fn test_mask_secret() {
        // Long key
        assert_eq!(mask_secret("sk-ant-api03-abcdefghij1234567890"), "sk-ant-a...7890");
        // Short key (<=12 chars)
        assert_eq!(mask_secret("sk-short"), "********");
        // Exactly 12 chars
        assert_eq!(mask_secret("123456789012"), "************");
        // 13 chars - should mask
        assert_eq!(mask_secret("1234567890123"), "12345678...0123");
    }

    #[tokio::test]
    async fn test_key_statuses() {
        let pool = KeyPool::new(
            vec![make_key("k1"), make_key("k2")],
            SelectionStrategy::RoundRobin,
        );

        pool.report_result("k1", CallResult::RateLimited { retry_after_ms: 60_000 }).await;

        let statuses = pool.key_statuses().await;
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].id, "k1");
        assert_eq!(statuses[0].health, KeyHealthStatus::RateLimited);
        assert!(!statuses[0].is_available);
        assert_eq!(statuses[1].id, "k2");
        assert_eq!(statuses[1].health, KeyHealthStatus::Healthy);
        assert!(statuses[1].is_available);
    }

    #[test]
    fn test_strategy_getter() {
        let pool = KeyPool::new(vec![], SelectionStrategy::PrimaryFallback);
        assert_eq!(pool.strategy(), SelectionStrategy::PrimaryFallback);
    }

    #[test]
    fn test_key_priority_serde() {
        let json = serde_json::to_string(&KeyPriority::Primary).unwrap();
        assert_eq!(json, "\"primary\"");
        let parsed: KeyPriority = serde_json::from_str("\"fallback\"").unwrap();
        assert_eq!(parsed, KeyPriority::Fallback);
    }

    #[test]
    fn test_key_source_serde() {
        let json = serde_json::to_string(&KeySource::ApiConsole).unwrap();
        assert_eq!(json, "\"api_console\"");
        let parsed: KeySource = serde_json::from_str("\"claude_code\"").unwrap();
        assert_eq!(parsed, KeySource::ClaudeCode);
    }
}
