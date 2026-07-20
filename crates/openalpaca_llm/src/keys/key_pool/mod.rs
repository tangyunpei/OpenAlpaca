//! Multi-key management with round-robin selection, cooldown, and rate-limit tracking.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Provider type for categorizing API keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Ollama,
    Plugin(String),
}

impl ProviderType {
    /// Return all built-in provider variants (excludes dynamic Plugin variants).
    pub fn all() -> &'static [ProviderType] {
        // SAFETY: These are the three unit variants which contain no heap data,
        // so a static slice is fine despite ProviderType not being Copy.
        static BUILTINS: [ProviderType; 3] = [
            ProviderType::Anthropic,
            ProviderType::OpenAI,
            ProviderType::Ollama,
        ];
        &BUILTINS
    }
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::OpenAI => write!(f, "openai"),
            Self::Ollama => write!(f, "ollama"),
            Self::Plugin(name) => write!(f, "plugin:{name}"),
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
    Codex,
    Environment,
    #[default]
    Other,
}

impl KeySource {
    /// Whether this key can authenticate against a provider's standard HTTP API.
    /// Managed keys (ClaudeCode, Codex, ClaudeMaxPro) are session/OAuth tokens.
    pub fn is_api_compatible(&self) -> bool {
        matches!(
            self,
            KeySource::ApiConsole | KeySource::Environment | KeySource::Other
        )
    }
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

    /// Whether this key can authenticate against a provider's standard HTTP API.
    /// Checks both the source metadata AND the secret format (defense-in-depth).
    fn is_api_compatible_key(&self) -> bool {
        if !self.source.is_api_compatible() {
            return false;
        }
        // Secret-format guard: sk-ant-oat* tokens are never API-compatible
        if self.provider == ProviderType::Anthropic && self.secret.starts_with("sk-ant-oat") {
            return false;
        }
        true
    }
}

/// Guard returned by `acquire()` — holds the key id, secret, and rate limit for use.
#[derive(Debug, Clone)]
pub struct KeyGuard {
    pub id: String,
    pub secret: String,
    /// Per-key RPM limit from config (if configured).
    pub rate_limit: Option<u32>,
}

/// Errors from key pool operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum KeyPoolError {
    #[error("No keys configured for this provider")]
    NoKeys,

    #[error("All keys are currently rate-limited")]
    AllKeysRateLimited,

    #[error("No API-compatible keys available (only managed/OAuth keys configured)")]
    NoApiCompatibleKeys,
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
    /// Per-key "last handed out" sequence number for LeastRecentlyUsed
    /// selection (index-aligned with `keys`; 0 = never used).
    last_used: Vec<AtomicU64>,
    /// Monotonic counter stamped onto `last_used` on each LRU selection.
    usage_seq: AtomicU64,
}

impl KeyPool {
    pub fn new(keys: Vec<ApiKey>, strategy: SelectionStrategy) -> Self {
        let last_used = keys.iter().map(|_| AtomicU64::new(0)).collect();
        let keys = keys.into_iter().map(|k| Arc::new(RwLock::new(k))).collect();
        Self {
            keys,
            strategy,
            round_robin_index: AtomicUsize::new(0),
            last_used,
            usage_seq: AtomicU64::new(0),
        }
    }

    /// LeastRecentlyUsed selection: among available keys (optionally restricted
    /// to API-compatible ones), pick the one selected least recently, and stamp
    /// it as just-used. Real usage tracking — not a scan from index 0.
    async fn acquire_lru(&self, api_only: bool) -> Result<KeyGuard, KeyPoolError> {
        let mut best: Option<(usize, u64)> = None;
        let mut has_candidate = false;
        for (idx, key_lock) in self.keys.iter().enumerate() {
            let key = key_lock.read().await;
            if api_only && !key.is_api_compatible_key() {
                continue;
            }
            has_candidate = true;
            if !key.is_available() {
                continue;
            }
            let seq = self.last_used[idx].load(Ordering::Relaxed);
            if best.is_none_or(|(_, b)| seq < b) {
                best = Some((idx, seq));
            }
        }
        if let Some((idx, _)) = best {
            let next = self.usage_seq.fetch_add(1, Ordering::Relaxed) + 1;
            self.last_used[idx].store(next, Ordering::Relaxed);
            let key = self.keys[idx].read().await;
            return Ok(KeyGuard {
                id: key.id.clone(),
                secret: key.secret.clone(),
                rate_limit: key.rate_limit,
            });
        }
        if api_only && !has_candidate {
            Err(KeyPoolError::NoApiCompatibleKeys)
        } else {
            Err(KeyPoolError::AllKeysRateLimited)
        }
    }

    /// Get the current selection strategy.
    pub fn strategy(&self) -> SelectionStrategy {
        self.strategy
    }

    /// Acquire an available API-compatible key from the pool.
    /// Skips managed/OAuth keys (e.g. Claude Code setup-tokens).
    pub async fn acquire(&self) -> Result<KeyGuard, KeyPoolError> {
        if self.keys.is_empty() {
            return Err(KeyPoolError::NoKeys);
        }

        match self.strategy {
            SelectionStrategy::PrimaryFallback => self.acquire_primary_fallback().await,
            _ => self.acquire_standard().await,
        }
    }

    /// Acquire any available key (including managed/OAuth keys).
    /// Use this only when you don't need HTTP API compatibility.
    pub async fn acquire_any(&self) -> Result<KeyGuard, KeyPoolError> {
        if self.keys.is_empty() {
            return Err(KeyPoolError::NoKeys);
        }

        match self.strategy {
            SelectionStrategy::PrimaryFallback => self.acquire_primary_fallback_any().await,
            _ => self.acquire_standard_any().await,
        }
    }

    /// Acquire a key suitable for standard API calls (delegates to `acquire()`).
    pub async fn acquire_api_compatible(&self) -> Result<KeyGuard, KeyPoolError> {
        self.acquire().await
    }

    /// Standard round-robin / LRU acquisition (API-compatible keys only).
    async fn acquire_standard(&self) -> Result<KeyGuard, KeyPoolError> {
        if self.strategy == SelectionStrategy::LeastRecentlyUsed {
            return self.acquire_lru(true).await;
        }
        let len = self.keys.len();
        let start = match self.strategy {
            SelectionStrategy::RoundRobin => {
                self.round_robin_index.fetch_add(1, Ordering::Relaxed) % len
            }
            SelectionStrategy::LeastRecentlyUsed => unreachable!(),
            SelectionStrategy::PrimaryFallback => unreachable!(),
        };

        for i in 0..len {
            let idx = (start + i) % len;
            let key = self.keys[idx].read().await;
            if key.is_available() && key.is_api_compatible_key() {
                return Ok(KeyGuard {
                    id: key.id.clone(),
                    secret: key.secret.clone(),
                    rate_limit: key.rate_limit,
                });
            }
        }

        // Distinguish: no API-compatible keys at all vs all rate-limited
        let mut has_any_api_compatible = false;
        for key_lock in &self.keys {
            let key = key_lock.read().await;
            if key.is_api_compatible_key() {
                has_any_api_compatible = true;
                break;
            }
        }
        if has_any_api_compatible {
            Err(KeyPoolError::AllKeysRateLimited)
        } else {
            Err(KeyPoolError::NoApiCompatibleKeys)
        }
    }

    /// Standard round-robin / LRU acquisition (any key, including managed).
    async fn acquire_standard_any(&self) -> Result<KeyGuard, KeyPoolError> {
        if self.strategy == SelectionStrategy::LeastRecentlyUsed {
            return self.acquire_lru(false).await;
        }
        let len = self.keys.len();
        let start = match self.strategy {
            SelectionStrategy::RoundRobin => {
                self.round_robin_index.fetch_add(1, Ordering::Relaxed) % len
            }
            SelectionStrategy::LeastRecentlyUsed => unreachable!(),
            SelectionStrategy::PrimaryFallback => unreachable!(),
        };

        for i in 0..len {
            let idx = (start + i) % len;
            let key = self.keys[idx].read().await;
            if key.is_available() {
                return Ok(KeyGuard {
                    id: key.id.clone(),
                    secret: key.secret.clone(),
                    rate_limit: key.rate_limit,
                });
            }
        }

        Err(KeyPoolError::AllKeysRateLimited)
    }

    /// PrimaryFallback: try Primary keys first (round-robin among them),
    /// then Fallback keys if all Primary are rate-limited.
    /// Only considers API-compatible keys.
    async fn acquire_primary_fallback(&self) -> Result<KeyGuard, KeyPoolError> {
        let mut primary_indices = Vec::new();
        let mut fallback_indices = Vec::new();
        let mut has_any_api_compatible = false;

        for (i, key_lock) in self.keys.iter().enumerate() {
            let key = key_lock.read().await;
            // Only include API-compatible keys in the candidate lists
            if !key.is_api_compatible_key() {
                continue;
            }
            has_any_api_compatible = true;
            match key.priority {
                KeyPriority::Primary => primary_indices.push(i),
                KeyPriority::Fallback => fallback_indices.push(i),
            }
        }

        // Try primary keys with round-robin
        if !primary_indices.is_empty() {
            let start =
                self.round_robin_index.fetch_add(1, Ordering::Relaxed) % primary_indices.len();
            for i in 0..primary_indices.len() {
                let idx = primary_indices[(start + i) % primary_indices.len()];
                let key = self.keys[idx].read().await;
                if key.is_available() {
                    return Ok(KeyGuard {
                        id: key.id.clone(),
                        secret: key.secret.clone(),
                        rate_limit: key.rate_limit,
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
                    rate_limit: key.rate_limit,
                });
            }
        }

        if has_any_api_compatible {
            Err(KeyPoolError::AllKeysRateLimited)
        } else {
            Err(KeyPoolError::NoApiCompatibleKeys)
        }
    }

    /// PrimaryFallback acquisition for any key (including managed).
    async fn acquire_primary_fallback_any(&self) -> Result<KeyGuard, KeyPoolError> {
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
            let start =
                self.round_robin_index.fetch_add(1, Ordering::Relaxed) % primary_indices.len();
            for i in 0..primary_indices.len() {
                let idx = primary_indices[(start + i) % primary_indices.len()];
                let key = self.keys[idx].read().await;
                if key.is_available() {
                    return Ok(KeyGuard {
                        id: key.id.clone(),
                        secret: key.secret.clone(),
                        rate_limit: key.rate_limit,
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
                    rate_limit: key.rate_limit,
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
                        tracing::warn!(
                            key_id = %key.id,
                            provider = %key.provider,
                            retry_after_ms,
                            consecutive_rate_limits = key.rate_state.consecutive_rate_limits,
                            "Key entered rate-limit cooldown"
                        );
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

    /// Count API-compatible keys that are currently available (not in cooldown).
    /// Used by the router to estimate how many parallel LLM calls can proceed
    /// without contention, so spawn concurrency can be adapted dynamically.
    pub async fn available_api_key_count(&self) -> usize {
        let mut count = 0;
        for key_lock in &self.keys {
            let key = key_lock.read().await;
            if key.is_api_compatible_key() && key.is_available() {
                count += 1;
            }
        }
        count
    }

    /// Returns the shortest remaining cooldown duration among API-compatible
    /// rate-limited keys, or `None` if no keys are cooling down.
    /// Useful for waiting until the next key becomes available.
    pub async fn shortest_cooldown(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut shortest: Option<Duration> = None;

        for key_lock in &self.keys {
            let key = key_lock.read().await;
            if !key.is_api_compatible_key() {
                continue;
            }
            if let Some(until) = key.rate_state.cooldown_until
                && until > now
            {
                let remaining = until - now;
                shortest = Some(match shortest {
                    Some(prev) => prev.min(remaining),
                    None => remaining,
                });
            }
        }

        shortest
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
    // Count/slice by chars, not bytes: a secret containing multi-byte
    // characters (e.g. a smart-quote paste artifact) would panic on a
    // non-char-boundary byte slice.
    let char_count = secret.chars().count();
    if char_count <= 12 {
        return "*".repeat(char_count);
    }
    let prefix: String = secret.chars().take(8).collect();
    let suffix: String = secret.chars().skip(char_count - 4).collect();
    format!("{}...{}", prefix, suffix)
}

#[cfg(test)]
mod tests;
