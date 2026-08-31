use super::reader::{IMessageAttachment, IncomingMessage};
use super::routing::{
    normalize_handle, should_process, strip_trigger_prefix, IMessageConfig, ProcessDecision,
};

fn make_msg(text: &str, sender: &str, chat_id: &str, is_from_me: bool) -> IncomingMessage {
    IncomingMessage {
        rowid: 1,
        text: text.to_string(),
        sender: sender.to_string(),
        chat_id: chat_id.to_string(),
        is_group: chat_id.starts_with("chat"),
        is_from_me,
        account: "e:user@iphone.com".to_string(),
        attachments: Vec::new(),
    }
}

fn make_msg_with_account(
    text: &str,
    sender: &str,
    chat_id: &str,
    is_from_me: bool,
    account: &str,
) -> IncomingMessage {
    IncomingMessage {
        rowid: 1,
        text: text.to_string(),
        sender: sender.to_string(),
        chat_id: chat_id.to_string(),
        is_group: chat_id.starts_with("chat"),
        is_from_me,
        account: account.to_string(),
        attachments: Vec::new(),
    }
}

fn make_msg_with_attachments(
    text: &str,
    sender: &str,
    chat_id: &str,
    is_from_me: bool,
) -> IncomingMessage {
    IncomingMessage {
        rowid: 1,
        text: text.to_string(),
        sender: sender.to_string(),
        chat_id: chat_id.to_string(),
        is_group: chat_id.starts_with("chat"),
        is_from_me,
        account: "e:user@iphone.com".to_string(),
        attachments: vec![IMessageAttachment {
            filename: "photo.jpg".into(),
            mime_type: "image/jpeg".into(),
            transfer_name: "photo.jpg".into(),
            file_path: "/tmp/photo.jpg".into(),
            total_bytes: 1024,
        }],
    }
}

fn owner_config() -> IMessageConfig {
    IMessageConfig {
        allow_from_me: true,
        direct_require_prefix: false,
        group_require_prefix: true,
        owner_handles: vec!["15551234567".into(), "owner@example.com".into()],
        bot_handle: Some("bot@mac.com".into()),
    }
}

fn no_owner_config() -> IMessageConfig {
    IMessageConfig {
        allow_from_me: true,
        direct_require_prefix: false,
        group_require_prefix: true,
        owner_handles: Vec::new(),
        bot_handle: None,
    }
}

fn prefix_required_config() -> IMessageConfig {
    IMessageConfig {
        allow_from_me: true,
        direct_require_prefix: true,
        group_require_prefix: true,
        owner_handles: Vec::new(),
        bot_handle: None,
    }
}

fn is_process(decision: &ProcessDecision) -> bool {
    matches!(decision, ProcessDecision::Process { .. })
}

fn get_content(decision: ProcessDecision) -> String {
    match decision {
        ProcessDecision::Process { content } => content,
        ProcessDecision::Skip(reason) => panic!("Expected Process, got Skip({reason})"),
    }
}

fn get_skip_reason(decision: ProcessDecision) -> &'static str {
    match decision {
        ProcessDecision::Skip(reason) => reason,
        ProcessDecision::Process { .. } => panic!("Expected Skip, got Process"),
    }
}

// ── strip_trigger_prefix ──

#[test]
fn test_strip_trigger_prefix_ask() {
    assert_eq!(
        strip_trigger_prefix("/ask what is rust"),
        Some("what is rust".into())
    );
}

#[test]
fn test_strip_trigger_prefix_at() {
    assert_eq!(
        strip_trigger_prefix("@openalpaca hello"),
        Some("hello".into())
    );
}

#[test]
fn test_strip_trigger_prefix_no_match() {
    assert_eq!(strip_trigger_prefix("hello world"), None);
}

#[test]
fn test_strip_trigger_prefix_empty_after() {
    assert_eq!(strip_trigger_prefix("/ask"), Some(String::new()));
    assert_eq!(strip_trigger_prefix("/ask   "), Some(String::new()));
}

// ── normalize_handle ──

#[test]
fn test_normalize_phone_international() {
    assert_eq!(normalize_handle("+1 (555) 123-4567"), "15551234567");
}

#[test]
fn test_normalize_phone_local() {
    assert_eq!(normalize_handle("5551234567"), "5551234567");
}

#[test]
fn test_normalize_email() {
    assert_eq!(normalize_handle("Owner@Example.COM"), "owner@example.com");
}

#[test]
fn test_normalize_email_whitespace() {
    assert_eq!(normalize_handle("  user@test.com  "), "user@test.com");
}

#[test]
fn test_normalize_empty() {
    assert_eq!(normalize_handle(""), "");
    assert_eq!(normalize_handle("   "), "");
}

// ── should_process: DM from owner (is_from_me) ──

#[test]
fn test_dm_owner_from_me_no_prefix_required() {
    let config = owner_config();
    let msg = make_msg("hello world", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "hello world");
}

#[test]
fn test_dm_owner_from_me_with_prefix() {
    let config = owner_config();
    let msg = make_msg("/ask hello world", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "hello world");
}

#[test]
fn test_dm_owner_from_me_prefix_required() {
    let mut config = owner_config();
    config.direct_require_prefix = true;
    let msg = make_msg("hello world", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "dm_owner_no_prefix");
}

// ── should_process: DM from owner (via owner_handles) ──

#[test]
fn test_dm_owner_handle_match_phone() {
    let config = owner_config();
    let msg = make_msg("hello", "+1 (555) 123-4567", "+15551234567", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "hello");
}

#[test]
fn test_dm_owner_handle_match_email() {
    let config = owner_config();
    let msg = make_msg("hello", "Owner@Example.COM", "+15551234567", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "hello");
}

// ── should_process: DM from non-owner ──

#[test]
fn test_dm_non_owner_no_prefix() {
    let config = owner_config();
    let msg = make_msg("hello", "+19995550000", "+19995550000", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "dm_non_owner_no_prefix");
}

#[test]
fn test_dm_non_owner_with_prefix() {
    let config = owner_config();
    let msg = make_msg("/ask hello", "+19995550000", "+19995550000", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "hello");
}

// ── should_process: Group messages ──

#[test]
fn test_group_with_prefix() {
    let config = owner_config();
    let msg = make_msg("/ask question", "+15551234567", "chat12345", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "question");
}

#[test]
fn test_group_no_prefix_required() {
    let config = owner_config();
    let msg = make_msg("question", "+15551234567", "chat12345", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "group_no_prefix");
}

#[test]
fn test_group_prefix_not_required() {
    let mut config = owner_config();
    config.group_require_prefix = false;
    let msg = make_msg("question", "+15551234567", "chat12345", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "question");
}

#[test]
fn test_group_empty_after_prefix() {
    let config = owner_config();
    let msg = make_msg("/ask", "+15551234567", "chat12345", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "group_empty_after_prefix");
}

// ── should_process: Edge cases ──

#[test]
fn test_dm_empty_text() {
    let config = owner_config();
    let msg = make_msg("", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "dm_empty");
}

#[test]
fn test_dm_whitespace_only() {
    let config = owner_config();
    let msg = make_msg("   ", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "dm_empty");
}

#[test]
fn test_dm_attachment_only_no_prefix_required() {
    let config = owner_config();
    let msg = make_msg_with_attachments("", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert!(is_process(&decision));
    assert_eq!(get_content(decision), "");
}

#[test]
fn test_dm_attachment_only_prefix_required() {
    let mut config = owner_config();
    config.direct_require_prefix = true;
    let msg = make_msg_with_attachments("", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    // Empty text with no prefix match -> skip
    assert_eq!(get_skip_reason(decision), "dm_owner_no_prefix");
}

#[test]
fn test_dm_attachment_with_prefix_only() {
    let config = prefix_required_config();
    let msg = make_msg_with_attachments("/ask", "+15551234567", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert!(is_process(&decision));
    assert_eq!(get_content(decision), "");
}

#[test]
fn test_no_owner_handles_is_from_me_no_prefix_needed() {
    // With no owner_handles, is_from_me=true DMs still work without prefix
    let config = no_owner_config();
    let msg = make_msg("hello", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "hello");
}

#[test]
fn test_no_owner_handles_non_owner_requires_prefix() {
    // With no owner_handles, is_from_me=false requires prefix (can't verify sender)
    let config = no_owner_config();
    let msg = make_msg("hello", "+19995550000", "+19995550000", false);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "dm_non_owner_no_prefix");
}

#[test]
fn test_no_owner_handles_with_prefix_works() {
    let config = no_owner_config();
    let msg = make_msg("/ask hello", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_content(decision), "hello");
}

#[test]
fn test_explicit_prefix_required_forces_prefix() {
    // When direct_require_prefix is explicitly true, prefix is needed even for owner
    let config = prefix_required_config();
    let msg = make_msg("hello", "", "+15551234567", true);
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "dm_owner_no_prefix");
}

// ── should_process: bot_handle filtering ──

#[test]
fn test_bot_handle_skips_own_message() {
    let config = owner_config(); // bot_handle = Some("bot@mac.com")
    // is_from_me=true: this is the bot's own outgoing reply
    let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:bot@mac.com");
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "bot_own_message");
}

#[test]
fn test_bot_handle_does_not_skip_incoming_message() {
    // is_from_me=false: incoming message where account matches bot_handle
    // (because chat.db records the LOCAL receiving account for incoming msgs too).
    // This must NOT be skipped — it's a real incoming message.
    let config = owner_config(); // bot_handle = Some("bot@mac.com")
    let msg = make_msg_with_account(
        "/ask hello",
        "+19995550000",
        "+19995550000",
        false,
        "e:bot@mac.com",
    );
    let decision = should_process(&msg, &config);
    assert!(is_process(&decision));
    assert_eq!(get_content(decision), "hello");
}

#[test]
fn test_bot_handle_allows_other_account() {
    let config = owner_config(); // bot_handle = Some("bot@mac.com")
    let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:user@iphone.com");
    let decision = should_process(&msg, &config);
    assert!(is_process(&decision));
    assert_eq!(get_content(decision), "hello");
}

#[test]
fn test_no_bot_handle_allows_all() {
    let config = no_owner_config(); // bot_handle = None
    let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:bot@mac.com");
    let decision = should_process(&msg, &config);
    assert!(is_process(&decision));
}

#[test]
fn test_bot_handle_empty_account_not_skipped() {
    let config = owner_config(); // bot_handle = Some("bot@mac.com")
    let msg = make_msg_with_account("hello", "", "+15551234567", true, "");
    let decision = should_process(&msg, &config);
    // Empty account should NOT match bot_handle
    assert!(is_process(&decision));
}

#[test]
fn test_bot_handle_suffix_match() {
    // bot_handle uses ends_with() matching to handle "p:"/"e:" prefixes
    let mut config = owner_config();
    config.bot_handle = Some("+1234567890".into());
    let msg = make_msg_with_account("hello", "", "+15551234567", true, "p:+1234567890");
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "bot_own_message");
}

#[test]
fn test_bot_handle_no_over_match() {
    // Short bot_handle should not accidentally match unrelated accounts
    let mut config = owner_config();
    config.bot_handle = Some("bot".into());
    let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:notbot@mac.com");
    let decision = should_process(&msg, &config);
    // "notbot@mac.com" does not end with "bot", so should NOT be skipped
    assert!(is_process(&decision));
}

#[test]
fn test_normalize_digit_prefixed_email() {
    // Digit-prefixed emails should not be misclassified as phone numbers
    assert_eq!(
        normalize_handle("123user@example.com"),
        "123user@example.com"
    );
    assert_eq!(
        normalize_handle("+user@example.com"),
        "+user@example.com"
    );
}

#[test]
fn test_bot_handle_group_message_also_filtered() {
    // Bot's own messages in group chats should also be filtered
    let config = owner_config(); // bot_handle = Some("bot@mac.com")
    let msg = make_msg_with_account("/ask hello", "", "chat12345", true, "e:bot@mac.com");
    let decision = should_process(&msg, &config);
    assert_eq!(get_skip_reason(decision), "bot_own_message");
}

// ── Reply target computation ──

#[test]
fn test_dm_reply_target_uses_sender() {
    // For DMs, reply_target should be the sender handle (not the varying chat_id)
    let msg = make_msg("/ask hello", "+19995550000", "iMessage;-;+19995550000", false);
    // Simulate the reply_target logic from handle_message
    let (reply_target, reply_is_group) = if msg.is_group {
        (msg.chat_id.clone(), true)
    } else if !msg.sender.is_empty() {
        (msg.sender.clone(), false)
    } else {
        (msg.chat_id.clone(), false)
    };
    assert_eq!(reply_target, "+19995550000");
    assert!(!reply_is_group);
}

#[test]
fn test_group_reply_target_uses_chat_id() {
    // For groups, reply_target should be the chat_id (group identifier)
    let msg = make_msg("/ask hello", "+19995550000", "chat12345", false);
    let (reply_target, reply_is_group) = if msg.is_group {
        (msg.chat_id.clone(), true)
    } else if !msg.sender.is_empty() {
        (msg.sender.clone(), false)
    } else {
        (msg.chat_id.clone(), false)
    };
    assert_eq!(reply_target, "chat12345");
    assert!(reply_is_group);
}

#[test]
fn test_dm_empty_sender_falls_back_to_chat_id() {
    // is_from_me=true with empty sender should fall back to chat_id
    let msg = make_msg("hello", "", "+15551234567", true);
    let (reply_target, reply_is_group) = if msg.is_group {
        (msg.chat_id.clone(), true)
    } else if !msg.sender.is_empty() {
        (msg.sender.clone(), false)
    } else {
        (msg.chat_id.clone(), false)
    };
    assert_eq!(reply_target, "+15551234567");
    assert!(!reply_is_group);
}

// --- Tool confirmation: reply-target derivation + intercept/broker roundtrip ---

#[test]
fn test_confirmation_reply_target_group() {
    let (target, is_group) = super::routing::confirmation_reply_target("chat12345");
    assert_eq!(target, "chat12345");
    assert!(is_group);
}

#[test]
fn test_confirmation_reply_target_dm_guid() {
    // GUID-style DM identifier -> handle extracted, buddy addressing
    let (target, is_group) =
        super::routing::confirmation_reply_target("iMessage;-;+15551234567");
    assert_eq!(target, "+15551234567");
    assert!(!is_group);
}

#[test]
fn test_confirmation_reply_target_dm_plain_handle() {
    // Plain-handle DM identifier passes through unchanged
    let (target, is_group) = super::routing::confirmation_reply_target("user@example.com");
    assert_eq!(target, "user@example.com");
    assert!(!is_group);
}

#[test]
fn test_confirmation_intercept_broker_roundtrip() {
    use dashmap::DashMap;
    use openalpaca_core::security::confirmation::{ConfirmationBroker, ConfirmationRequest};
    use std::collections::VecDeque;

    let broker = ConfirmationBroker::new();
    let mut rx = broker.request(&ConfirmationRequest {
        request_id: "req-imsg-1".to_string(),
        agent_id: "agent-1".to_string(),
        tool_name: "shell_exec".to_string(),
        tool_arguments: serde_json::json!({"cmd": "ls"}),
        stream_id: None,
        lane_key: Some("global1:imessage".to_string()),
        timestamp: chrono::Utc::now(),
    });

    // Simulate the listener queuing the request for a chat
    let pending: DashMap<String, VecDeque<String>> = DashMap::new();
    let chat_id = "iMessage;-;+15551234567".to_string();
    pending
        .entry(chat_id.clone())
        .or_default()
        .push_back("req-imsg-1".to_string());

    // A /no from an unrelated chat falls through to normal processing
    assert!(crate::common::intercept_confirmation_reply(
        "/no",
        &"chat999".to_string(),
        &broker,
        &pending
    )
    .is_none());

    // A /no in the prompted chat denies via the broker
    let reply =
        crate::common::intercept_confirmation_reply("/no", &chat_id, &broker, &pending)
            .unwrap();
    assert!(reply.contains("Denied"));
    assert!(!rx.try_recv().unwrap().approved);
}
