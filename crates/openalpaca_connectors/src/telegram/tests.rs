use super::delivery::{TELEGRAM_MAX_LENGTH, approval_chunks, chunk_message};
use crate::common::KeyedRateLimiter;
use std::time::Duration;

/// Telegram cuts at its own 4096-byte limit; the algorithm itself is pinned
/// once, at both platforms' limits, in `common::tests::chunking`.
#[test]
fn chunk_message_uses_the_telegram_limit() {
    assert_eq!(TELEGRAM_MAX_LENGTH, 4096);
    assert_eq!(chunk_message(&"a".repeat(4096)).len(), 1);
    let lens: Vec<usize> = chunk_message(&"a".repeat(4097))
        .iter()
        .map(String::len)
        .collect();
    assert_eq!(lens, [4096, 1]);
}

/// The lead's rulings R-T32 and Y.2 at Telegram's 4,096 bytes: an approval
/// prompt sent in several messages keeps every preview line indented, and
/// every message after the first begins with the prompt's own
/// `(approval prompt, continued)` line, so even with each message's leading
/// whitespace trimmed — as Telegram shows it — no message starts a line at
/// the margin with the model's text: across them all one line starts
/// `Arguments:` and one `Reply /yes`, the prompt's own. No message begins or
/// ends with a blank line, and every one fits.
#[test]
fn a_chunked_approval_prompt_begins_every_later_message_with_its_own_line() {
    let (args, preview) = crate::common::forging_edit_prompt(TELEGRAM_MAX_LENGTH);
    let prompt = crate::common::format_confirmation_prompt("file_edit", &args, Some(&preview), 1);
    let chunks = approval_chunks(&prompt);

    crate::common::assert_approval_messages(&chunks);
    assert!(
        chunks
            .iter()
            .all(|c| !c.is_empty() && c.len() <= TELEGRAM_MAX_LENGTH)
    );
}

/// Telegram keys the shared limiter by i64 chat id, negative for groups; the
/// limiter itself is pinned in `common::tests::rate_limit`.
#[test]
fn test_rate_limiter_independent_chats() {
    let limiter = KeyedRateLimiter::<i64>::new(Duration::from_secs(1));
    assert!(limiter.check(-111).is_none());
    assert!(limiter.check(111).is_none()); // Different chat, should pass
    assert!(limiter.check(-111).is_some()); // Same chat, should be limited
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
        preview: None,
        per_call: false,
        timestamp: chrono::Utc::now(),
    })
}

/// A `/yes` answers the broker through the shared intercept — the same call
/// `handle_message` makes, with Telegram's own key type (`ChatId.0`, an `i64`)
/// — and it does so **without the rate limiter ever seeing the chat**.
///
/// `KeyedRateLimiter::check` both reads and stamps, so "the limiter has no
/// record of this chat afterwards" is the observable consequence of the
/// intercept running before it: an operator answering two prompts in a row is
/// never told to wait, and their next real message still gets its full quota.
#[test]
fn a_confirmation_reply_is_answered_before_the_rate_limiter_sees_the_chat() {
    let chat_id: i64 = -100200300;
    let limiter = KeyedRateLimiter::<i64>::new(Duration::from_secs(1));
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

    let prompt = format_confirmation_prompt("shell_exec", &args, None, 1);

    assert!(prompt.contains("Tool: shell_exec"), "{prompt}");
    assert!(prompt.contains("Reply /yes or /no to approve or deny."));
    assert!(prompt.ends_with("Reply /yes or /no to approve or deny."));
    // One prompt, so no queue hint.
    assert!(!prompt.contains("pending"), "{prompt}");
    assert!(prompt.contains("..."), "the arguments were truncated");
}

// ── The approval prompt carries no link preview ──────────────────────
//
// Telegram draws a card under the first URL in a message: the linked page's
// own title, description and image. Under an approval prompt that card is
// the site owner's text inside the message the person decides on.

/// The request the prompt and its acknowledgement are sent with asks
/// Telegram for no preview; the text is sent as given.
#[test]
fn an_approval_message_asks_telegram_for_no_link_preview() {
    use super::delivery::approval_message;
    use teloxide::prelude::*;
    use teloxide::requests::HasPayload;

    let bot = Bot::new("0:test");
    let text = "Arguments:\n{\n  \"url\": \"https://e.x/a\"\n}";

    let message = approval_message(&bot, ChatId(42), text);

    let sent = serde_json::to_value(message.payload_ref()).unwrap();
    assert_eq!(sent["text"], text);
    assert_eq!(sent["chat_id"], 42);
    assert_eq!(
        sent["link_preview_options"],
        serde_json::json!({"is_disabled": true}),
        "{sent}"
    );
}

// ── teloxide against a loopback stub of the Bot API (A-09 (2)) ──────────
//
// The live round trip with the owner's test bot is P28's gate of record: only
// it reaches api.telegram.org over rustls with the OS roots. This stub, a
// supplement, pins what a stub can: the bot's API URL is honoured, and
// `getMe` and `sendMessage` go out as the Bot API expects and come back
// parsed. Plain HTTP on 127.0.0.1, so no TLS is exercised here.

/// The stub's record of one request: the method's path and its JSON body.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;

/// A loopback Bot API: answers `getMe` and `sendMessage` as Telegram does and
/// records every request. Returns its base URL (`http://127.0.0.1:<port>`)
/// and the record.
async fn bot_api_stub() -> (String, Seen) {
    use axum::extract::{Path, State};
    use axum::{Json, Router, routing::post};
    let seen: Seen = Default::default();
    async fn method(
        State(seen): State<Seen>,
        Path((bot, method)): Path<(String, String)>,
        body: axum::body::Bytes,
    ) -> Json<serde_json::Value> {
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        seen.lock()
            .unwrap()
            .push((format!("{bot}/{method}"), body.clone()));
        // Bot API method names are case-insensitive; teloxide sends `GetMe`.
        Json(match method.to_ascii_lowercase().as_str() {
            "getme" => serde_json::json!({"ok": true, "result": {
                "id": 7, "is_bot": true, "first_name": "Stub", "username": "stub_bot",
                "can_join_groups": true, "can_read_all_group_messages": false,
                "supports_inline_queries": false, "has_main_web_app": false
            }}),
            "sendmessage" => serde_json::json!({"ok": true, "result": {
                "message_id": 1, "date": 1_761_609_600,
                "from": {"id": 7, "is_bot": true, "first_name": "Stub"},
                "chat": {"id": body["chat_id"], "type": "private", "first_name": "Owner"},
                "text": body["text"]
            }}),
            _ => serde_json::json!({"ok": false, "error_code": 404, "description": "Not Found"}),
        })
    }
    let app = Router::new()
        .route("/{bot}/{method}", post(method))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

/// teloxide sends `getMe` and `sendMessage` to the URL it is given, with the
/// token in the path and the chat and text in the body, and parses both
/// answers.
#[tokio::test]
async fn teloxide_round_trips_get_me_and_send_message_against_a_loopback_bot_api() {
    use teloxide::prelude::*;
    let (url, seen) = bot_api_stub().await;
    // `set_api_url` takes teloxide's own reqwest 0.12 `Url`; `parse` names it.
    let bot = Bot::new("0:stub").set_api_url(url.parse().unwrap());

    let me = bot.get_me().await.expect("getMe");
    assert_eq!(me.user.username.as_deref(), Some("stub_bot"));
    let sent = bot
        .send_message(ChatId(42), "round trip")
        .await
        .expect("sendMessage");
    assert_eq!(sent.chat.id, ChatId(42));
    assert_eq!(sent.text(), Some("round trip"));

    let seen = seen.lock().unwrap().clone();
    let paths: Vec<String> = seen.iter().map(|(p, _)| p.to_ascii_lowercase()).collect();
    assert_eq!(paths, ["bot0:stub/getme", "bot0:stub/sendmessage"]);
    assert_eq!(seen[1].1["chat_id"], 42);
    assert_eq!(seen[1].1["text"], "round trip");
}
