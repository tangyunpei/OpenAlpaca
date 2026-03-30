use super::delivery::{TELEGRAM_MAX_LENGTH, chunk_message, escape_markdown_v2};
use super::rate_limiter::ChatRateLimiter;
use std::time::Duration;

#[test]
fn test_chunk_message_short() {
    let text = "Hello, world!";
    let chunks = chunk_message(text);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], "Hello, world!");
}

#[test]
fn test_chunk_message_exact_limit() {
    let text = "a".repeat(TELEGRAM_MAX_LENGTH);
    let chunks = chunk_message(&text);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].len(), TELEGRAM_MAX_LENGTH);
}

#[test]
fn test_chunk_message_paragraph_boundary() {
    let paragraph1 = "a".repeat(2000);
    let paragraph2 = "b".repeat(2000);
    let paragraph3 = "c".repeat(2000);
    let text = format!("{}\n\n{}\n\n{}", paragraph1, paragraph2, paragraph3);
    let chunks = chunk_message(&text);
    assert!(chunks.len() >= 2);
    // First chunk should split at paragraph boundary
    assert!(chunks[0].ends_with("\n\n"));
}

#[test]
fn test_chunk_message_sentence_boundary() {
    // Create a long string with sentence boundaries but no paragraph boundaries
    let sentence = "a".repeat(2000);
    let text = format!("{}. {}. {}", sentence, sentence, sentence);
    let chunks = chunk_message(&text);
    assert!(chunks.len() >= 2);
    // First chunk should split at sentence boundary
    assert!(chunks[0].ends_with(". "));
}

#[test]
fn test_chunk_message_hard_cut() {
    // No boundaries at all
    let text = "a".repeat(5000);
    let chunks = chunk_message(&text);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].len(), TELEGRAM_MAX_LENGTH);
    assert_eq!(chunks[1].len(), 5000 - TELEGRAM_MAX_LENGTH);
}

#[test]
fn test_escape_markdown_v2_basic() {
    assert_eq!(escape_markdown_v2("hello"), "hello");
    assert_eq!(escape_markdown_v2("hello_world"), "hello\\_world");
    assert_eq!(escape_markdown_v2("a*b*c"), "a\\*b\\*c");
    assert_eq!(escape_markdown_v2("test."), "test\\.");
    assert_eq!(escape_markdown_v2("1+1=2"), "1\\+1\\=2");
}

#[test]
fn test_escape_markdown_v2_all_special() {
    let input = "_*[]()~`>#+-=|{}.!";
    let expected = "\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!";
    assert_eq!(escape_markdown_v2(input), expected);
}

#[test]
fn test_escape_markdown_v2_empty() {
    assert_eq!(escape_markdown_v2(""), "");
}

#[test]
fn test_rate_limiter_allows_first_message() {
    let limiter = ChatRateLimiter::new(Duration::from_secs(1));
    assert!(limiter.check(12345).is_none());
}

#[test]
fn test_rate_limiter_blocks_rapid_messages() {
    let limiter = ChatRateLimiter::new(Duration::from_secs(1));
    assert!(limiter.check(12345).is_none());
    // Second check immediately should be rate limited
    let wait = limiter.check(12345);
    assert!(wait.is_some());
    assert!(wait.unwrap() <= Duration::from_secs(1));
}

#[test]
fn test_rate_limiter_independent_chats() {
    let limiter = ChatRateLimiter::new(Duration::from_secs(1));
    assert!(limiter.check(111).is_none());
    assert!(limiter.check(222).is_none()); // Different chat, should pass
    assert!(limiter.check(111).is_some()); // Same chat, should be limited
}

#[test]
fn test_chunk_message_utf8_boundary() {
    // Build a string of multi-byte chars that would cause a panic
    // if we slice at a raw byte offset.
    // Each CJK char is 3 bytes in UTF-8.
    let cjk_char = "\u{4e16}"; // '世' = 3 bytes
    // Fill slightly over the limit with 3-byte chars
    let count = (TELEGRAM_MAX_LENGTH / 3) + 100;
    let text: String = cjk_char.repeat(count);
    assert!(text.len() > TELEGRAM_MAX_LENGTH);
    // Must not panic
    let chunks = chunk_message(&text);
    assert!(chunks.len() >= 2);
    // All chunks must be valid UTF-8 (they are Strings, so this is guaranteed)
    for chunk in &chunks {
        assert!(!chunk.is_empty());
        // Verify each chunk is within the limit
        assert!(chunk.len() <= TELEGRAM_MAX_LENGTH);
    }
}

#[test]
fn test_chunk_message_emoji_boundary() {
    // Emoji are 4 bytes in UTF-8
    let emoji = "\u{1F600}"; // grinning face = 4 bytes
    let count = (TELEGRAM_MAX_LENGTH / 4) + 100;
    let text: String = emoji.repeat(count);
    assert!(text.len() > TELEGRAM_MAX_LENGTH);
    let chunks = chunk_message(&text);
    assert!(chunks.len() >= 2);
    for chunk in &chunks {
        assert!(!chunk.is_empty());
        assert!(chunk.len() <= TELEGRAM_MAX_LENGTH);
    }
}

#[test]
fn test_floor_char_boundary_std() {
    let s = "Hello\u{4e16}\u{754c}"; // "Hello世界" = 5 + 3 + 3 = 11 bytes
    // Boundary in the middle of '世' (bytes 5..8)
    assert_eq!(s.floor_char_boundary(6), 5);
    assert_eq!(s.floor_char_boundary(7), 5);
    assert_eq!(s.floor_char_boundary(8), 8); // exactly on boundary
    assert_eq!(s.floor_char_boundary(100), 11); // beyond end
    assert_eq!(s.floor_char_boundary(0), 0);
}
