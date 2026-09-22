use super::delivery::{TELEGRAM_MAX_LENGTH, chunk_message};
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

// ── CON-01: Telegram uses the shared confirmation logic ──────────────
//
// Telegram was the last connector carrying its own copy of the confirmation
// prompt and the `/yes` `/no` intercept. Its copy was also the last one that
// byte-sliced: `&s[..500]` on pretty-printed, model-authored tool arguments
// panicked the listener whenever byte 500 fell inside a character.

use crate::common::{format_confirmation_prompt, intercept_confirmation_reply};
use dashmap::DashMap;
use openalpaca_core::security::confirmation::{ConfirmationBroker, ConfirmationRequest};
use std::collections::VecDeque;

type ResponseRx =
    tokio::sync::oneshot::Receiver<openalpaca_core::security::confirmation::ConfirmationResponse>;

fn queued_request(broker: &ConfirmationBroker, request_id: &str) -> ResponseRx {
    broker.request(&ConfirmationRequest {
        request_id: request_id.to_string(),
        agent_id: "orchestrator".to_string(),
        tool_name: "shell_exec".to_string(),
        tool_arguments: serde_json::json!({"cmd": "ls"}),
        stream_id: None,
        lane_key: Some("global1:telegram".to_string()),
        task_id: None,
        agent_instance_id: None,
        timestamp: chrono::Utc::now(),
    })
}

/// A `/yes` answers the broker through the shared intercept — the same call
/// `handle_message` makes, with Telegram's own key type (`ChatId.0`, an `i64`)
/// — and it does so **without the rate limiter ever seeing the chat**.
///
/// `ChatRateLimiter::check` both reads and stamps, so "the limiter has no
/// record of this chat afterwards" is the observable consequence of the
/// intercept running before it: an operator answering two prompts in a row is
/// never told to wait, and their next real message still gets its full quota.
#[test]
fn a_confirmation_reply_is_answered_before_the_rate_limiter_sees_the_chat() {
    let chat_id: i64 = -100200300;
    let limiter = ChatRateLimiter::new(Duration::from_secs(1));
    let broker = ConfirmationBroker::new();
    let mut rx = queued_request(&broker, "req-telegram-1");

    let pending: DashMap<i64, VecDeque<String>> = DashMap::new();
    pending
        .entry(chat_id)
        .or_default()
        .push_back("req-telegram-1".to_string());

    let reply = intercept_confirmation_reply("/yes", &chat_id, &broker, &pending)
        .expect("a queued prompt is answered");
    assert!(reply.contains("Approved"), "{reply}");
    assert!(rx.try_recv().expect("the broker was told").approved);

    // The chat never reached the limiter, so its first ordinary message still
    // passes. Had the limiter run first, this would already be stamped.
    assert!(
        limiter.check(chat_id).is_none(),
        "answering a confirmation must not spend the chat's rate-limit slot"
    );
}

/// Both fall-through cases `handle_message` relies on: text that is not one of
/// the four commands, and a command in a chat with nothing pending. Either
/// way the turn goes on to normal handling — rate limit, TrustGate, gateway.
#[test]
fn a_non_confirmation_falls_through_to_normal_handling() {
    let broker = ConfirmationBroker::new();
    let pending: DashMap<i64, VecDeque<String>> = DashMap::new();
    let chat_id: i64 = 42;

    // Nothing pending, and a real command.
    assert!(intercept_confirmation_reply("/yes", &chat_id, &broker, &pending).is_none());

    // Something pending, but ordinary text.
    let _rx = queued_request(&broker, "req-telegram-2");
    pending
        .entry(chat_id)
        .or_default()
        .push_back("req-telegram-2".to_string());
    assert!(
        intercept_confirmation_reply("what is the weather", &chat_id, &broker, &pending).is_none()
    );
    // A reply from a chat that was never prompted falls through too.
    assert!(intercept_confirmation_reply("/no", &7i64, &broker, &pending).is_none());
}

/// Two prompts in one chat are answered oldest first, and the first answer
/// says how many are left — the wording Telegram's own copy carried.
#[test]
fn queued_confirmations_are_answered_fifo_with_a_pending_count() {
    let chat_id: i64 = 99;
    let broker = ConfirmationBroker::new();
    let mut first = queued_request(&broker, "req-a");
    let mut second = queued_request(&broker, "req-b");

    let pending: DashMap<i64, VecDeque<String>> = DashMap::new();
    {
        let mut q = pending.entry(chat_id).or_default();
        q.push_back("req-a".to_string());
        q.push_back("req-b".to_string());
    }

    let reply = intercept_confirmation_reply("/y", &chat_id, &broker, &pending).unwrap();
    assert!(reply.contains("Approved"), "{reply}");
    assert!(reply.contains("(1 more pending — reply /yes or /no)"), "{reply}");
    assert!(first.try_recv().unwrap().approved, "the oldest is answered first");
    assert!(second.try_recv().is_err(), "the newer one is still waiting");

    let reply = intercept_confirmation_reply("/n", &chat_id, &broker, &pending).unwrap();
    assert!(reply.contains("Denied"), "{reply}");
    assert!(!reply.contains("more pending"), "{reply}");
    assert!(!second.try_recv().unwrap().approved);
}

/// The prompt's arguments block is cut at 500 **bytes**. Tool arguments are
/// model-authored, so the cut lands inside a Chinese character constantly, and
/// Telegram's own `&s[..500]` panicked the confirmation listener — the prompt
/// was never sent and the run sat blocked until its 300 s timeout.
#[test]
fn a_multibyte_argument_block_is_cut_without_panicking() {
    let args = serde_json::json!({
        "note": "请把这些会议记录整理成一份摘要".repeat(40),
    });
    let pretty = serde_json::to_string_pretty(&args).unwrap();
    assert!(pretty.len() > 500);
    assert!(
        !pretty.is_char_boundary(500),
        "byte 500 must land inside a character for this to be the old panic"
    );

    let prompt = format_confirmation_prompt("shell_exec", &args, 1);

    assert!(prompt.contains("Tool: shell_exec"), "{prompt}");
    assert!(prompt.contains("Reply /yes or /no to approve or deny."));
    assert!(prompt.ends_with("Reply /yes or /no to approve or deny."));
    // One prompt, so no queue hint.
    assert!(!prompt.contains("pending)"), "{prompt}");
    assert!(prompt.contains("..."), "the arguments were truncated");
}
