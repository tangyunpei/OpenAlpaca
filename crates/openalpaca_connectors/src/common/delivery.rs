//! Byte-bounded message chunks and per-conversation admission throttling.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Preserve the input exactly while preferring paragraph, sentence and line
/// boundaries. Limits are bytes, as in the original platform implementations.
pub(crate) fn chunk_message(text: &str, max_bytes: usize) -> Vec<String> {
    assert!(max_bytes >= 4, "chunk limit must fit any UTF-8 character");
    if text.len() <= max_bytes {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut remaining = text;
    while remaining.len() > max_bytes {
        let boundary = remaining.floor_char_boundary(max_bytes);
        let slice = &remaining[..boundary];
        let split_at = slice
            .rfind("\n\n")
            .map(|i| i + 2)
            .or_else(|| slice.rfind(". ").map(|i| i + 2))
            .or_else(|| slice.rfind('\n').map(|i| i + 1))
            .unwrap_or(boundary);
        chunks.push(remaining[..split_at].to_string());
        remaining = &remaining[split_at..];
    }
    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }
    chunks
}

/// Only admitted messages advance the per-key timestamp; denied retries do not
/// extend the waiting period. Keys remain until this connector is dropped.
pub(crate) struct KeyedRateLimiter<K> {
    last_sent: Mutex<HashMap<K, Instant>>,
    min_interval: Duration,
}

impl<K: Eq + Hash> KeyedRateLimiter<K> {
    pub(crate) fn new(min_interval: Duration) -> Self {
        Self {
            last_sent: Mutex::new(HashMap::new()),
            min_interval,
        }
    }

    pub(crate) fn check(&self, key: K) -> Option<Duration> {
        self.check_at(key, Instant::now())
    }

    fn check_at(&self, key: K, now: Instant) -> Option<Duration> {
        let mut map = self.last_sent.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(last) = map.get(&key) {
            let elapsed = now.saturating_duration_since(*last);
            if elapsed < self.min_interval {
                return Some(self.min_interval - elapsed);
            }
        }
        map.insert(key, now);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_preserve_all_text_at_both_platform_limits() {
        for limit in [2000, 4096] {
            for text in [
                "".to_string(),
                "a".repeat(limit),
                "世界😀\n\n".repeat(900),
                "word. ".repeat(1500),
                "😀".repeat(3000),
            ] {
                let chunks = chunk_message(&text, limit);
                assert_eq!(chunks.concat(), text);
                assert!(chunks.iter().all(|chunk| chunk.len() <= limit));
                if !text.is_empty() {
                    assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
                }
            }
        }
        assert_eq!(
            chunk_message("abc\n\ndef. ghi", 10),
            ["abc\n\n", "def. ghi"]
        );
        assert_eq!(chunk_message("abc. defghi", 8), ["abc. ", "defghi"]);
        assert_eq!(chunk_message("abc\ndefghi", 8), ["abc\n", "defghi"]);
    }

    #[test]
    fn denied_messages_do_not_extend_the_interval_and_keys_are_independent() {
        assert_eq!(KeyedRateLimiter::new(Duration::ZERO).check(1_i64), None);
        let limiter = KeyedRateLimiter::new(Duration::from_secs(1));
        let now = Instant::now();
        assert_eq!(limiter.check_at(1_i64, now), None);
        assert_eq!(limiter.check_at(2, now), None);
        assert_eq!(
            limiter.check_at(1, now + Duration::from_millis(250)),
            Some(Duration::from_millis(750))
        );
        assert_eq!(
            limiter.check_at(1, now + Duration::from_millis(900)),
            Some(Duration::from_millis(100))
        );
        assert_eq!(limiter.check_at(1, now + Duration::from_secs(1)), None);
    }
}
