//! Simple per-chat rate limiter for Telegram message sending.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Simple per-chat rate limiter. Allows at most 1 message per `min_interval` per chat.
pub(super) struct ChatRateLimiter {
    last_sent: Mutex<HashMap<i64, Instant>>,
    min_interval: Duration,
}

impl ChatRateLimiter {
    pub(super) fn new(min_interval: Duration) -> Self {
        Self {
            last_sent: Mutex::new(HashMap::new()),
            min_interval,
        }
    }

    /// Check if a message can be sent to this chat. Returns wait duration if rate limited.
    pub(super) fn check(&self, chat_id: i64) -> Option<Duration> {
        let mut map = self.last_sent.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(last) = map.get(&chat_id) {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                return Some(self.min_interval - elapsed);
            }
        }
        map.insert(chat_id, Instant::now());
        None
    }
}
