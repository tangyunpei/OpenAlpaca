//! Telegram Connector Module
//!
//! Provides Telegram Bot integration via teloxide.

mod connector;
mod delivery;
mod handler;

/// Per-chat admission throttling, keyed by Telegram's `ChatId`. Private to this
/// module tree, as Discord's equivalent is (descendants still see it).
type ChatRateLimiter = crate::common::KeyedRateLimiter<i64>;

pub use connector::TelegramConnector;

#[cfg(test)]
mod tests {
    use super::delivery::{TELEGRAM_MAX_LENGTH, chunk_message};

    /// Chunking itself is covered exhaustively in `common::delivery`; this
    /// asserts only that the Telegram wrapper passes Telegram's own limit.
    #[test]
    fn wrapper_chunks_at_the_telegram_limit() {
        let chunks = chunk_message(&"a".repeat(TELEGRAM_MAX_LENGTH + 100));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), TELEGRAM_MAX_LENGTH);
        assert_eq!(chunks[1].len(), 100);
    }
}
