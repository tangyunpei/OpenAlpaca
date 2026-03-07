use super::{detect_active_send_hints, sanitize_parts_for_dispatch};
use openalpaca_llm::{ChatMessage, ContentPart, ImageSource};

#[test]
fn sanitize_parts_for_dispatch_drops_empty_text_parts() {
    let parts = vec![
        ContentPart::Text {
            text: "".to_string(),
        },
        ContentPart::Text {
            text: "  \n\t".to_string(),
        },
        ContentPart::Text {
            text: "keep me".to_string(),
        },
    ];

    let sanitized = sanitize_parts_for_dispatch(parts);
    assert_eq!(sanitized.len(), 1);
    match &sanitized[0] {
        ContentPart::Text { text } => assert_eq!(text, "keep me"),
        other => panic!("expected text part, got {other:?}"),
    }
}

#[test]
fn sanitize_parts_for_dispatch_replaces_unresolved_file_asset_images() {
    let parts = vec![ContentPart::Image {
        source: ImageSource::FileAsset {
            file_id: "f1".to_string(),
            media_type: "image/jpeg".to_string(),
        },
        detail: None,
    }];

    let sanitized = sanitize_parts_for_dispatch(parts);
    assert_eq!(sanitized.len(), 1);
    match &sanitized[0] {
        ContentPart::Text { text } => {
            assert_eq!(text, "[image attached — unresolved file asset reference]")
        }
        other => panic!("expected placeholder text, got {other:?}"),
    }
}

// --- detect_active_send_hints tests ---

#[test]
fn detect_send_flow_with_telegram_and_send_keyword() {
    let messages = vec![ChatMessage::assistant(
        "I can help you send a message via Telegram using the send tool.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_no_channel_no_send() {
    let messages = vec![ChatMessage::assistant(
        "Sure, I'll help you with that task.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_empty_messages() {
    assert!(!detect_active_send_hints(&[], &[]).send);
}

#[test]
fn detect_send_flow_user_only_messages() {
    let messages = vec![ChatMessage::user(
        "send message to telegram",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_channel_without_send_keyword() {
    let messages = vec![ChatMessage::assistant(
        "Telegram is a messaging platform.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_chinese_context_without_tool_name() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram发送消息给您的联系人。",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_chinese_context_with_tool_name() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram使用`send`工具发送消息。",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_only_recent_messages() {
    let mut messages = Vec::new();
    messages.push(ChatMessage::assistant(
        "I'll send via Telegram using `send`.",
    ));
    for _ in 0..2 {
        messages.push(ChatMessage::assistant("Here is some other info."));
    }
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_survives_bursty_user_messages() {
    let messages = vec![
        ChatMessage::assistant("你的 Telegram 收件人是？"),
        ChatMessage::user("等一下"),
        ChatMessage::user("用 default"),
    ];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_discussion_about_telegram_no_tool() {
    let messages = vec![ChatMessage::assistant(
        "Telegram is great for groups and channels.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_tier2_chinese_recipient_solicitation() {
    let messages = vec![ChatMessage::assistant(
        "你的 Telegram 收件人是？",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_tier2_english_recipient_solicitation() {
    let messages = vec![ChatMessage::assistant(
        "Who should I send it to on Telegram?",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_tier2_no_match_without_recipient_keyword() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram发送消息给您的联系人。",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_with_quoted_send_tool_name() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool to send your photo via Telegram.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_send_parens() {
    let messages = vec![ChatMessage::assistant(
        "Let me call send( for your iMessage attachment.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_recipient_followup_with_send_tool() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool to send your photo via Telegram. What's the recipient?",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_recipient_followup_with_call_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll call send to send your text via Telegram. Who should I send it to?",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_send_tool_mentioned_twice() {
    let messages = vec![ChatMessage::assistant(
        "I can use send for text or call send for attachments via Telegram.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_recipient_only_defaults_to_send() {
    let messages = vec![ChatMessage::assistant(
        "你的 Telegram 收件人是？",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_tier2_cross_message_backtrack() {
    let messages = vec![
        ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via Telegram.",
        ),
        ChatMessage::user("等一下"),
        ChatMessage::assistant(
            "好的，你的 Telegram 收件人是？",
        ),
    ];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn tier2_cross_channel_still_detects_send() {
    // Both turns mention send-related content → send should be active
    let messages = vec![
        ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via iMessage.",
        ),
        ChatMessage::user("等一下"),
        ChatMessage::assistant(
            "好的，你的 Telegram 收件人是？",
        ),
    ];
    let hints = detect_active_send_hints(&messages, &[]);
    // Turn N-1 has "send" (Tier 1), Turn N has Tier 2 → both set send=true
    assert!(hints.send);
}

// --- resolve_send_tool_choice unit tests ---

use super::resolve_send_tool_choice;
use openalpaca_llm::ToolChoice;

#[test]
fn tool_choice_send_present_pins_send() {
    let choice = resolve_send_tool_choice(true);
    assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
}

#[test]
fn tool_choice_no_send_returns_none() {
    let choice = resolve_send_tool_choice(false);
    assert_eq!(choice, None);
}

// --- apply_send_keepalive direct tests ---

use super::apply_send_keepalive;

#[test]
fn detect_send_hints_custom_channel_via_active_channels() {
    // "matrix" is NOT in FALLBACK_CHANNEL_KW
    let messages = vec![ChatMessage::assistant(
        "Your Matrix recipient is?",
    )];
    // With active_channels containing "matrix": should detect Tier 2 hit
    let active = vec!["matrix".to_string()];
    let hints = detect_active_send_hints(&messages, &active);
    assert!(hints.send);
    // Without: should NOT detect (proves dynamic path works)
    let hints_empty = detect_active_send_hints(&messages, &[]);
    assert!(!hints_empty.send);
}

#[test]
fn apply_keepalive_injects_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool via Telegram.",
    )];
    let mut tool_names = vec!["web_fetch".to_string()];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(!intent_send);
    assert!(tool_names.contains(&"send".to_string()));
}

#[test]
fn apply_keepalive_skips_when_intent_has_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the send tool via Telegram.",
    )];
    let mut tool_names = vec!["send".to_string()];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(intent_send);
    // No duplicate
    assert_eq!(
        tool_names.iter().filter(|t| *t == "send").count(),
        1
    );
}

#[test]
fn apply_keepalive_injects_send_when_hinted() {
    let messages = vec![ChatMessage::assistant(
        "I can use send for text or call send for attachments via Telegram.",
    )];
    let mut tool_names = vec![];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(!intent_send);
    assert!(tool_names.contains(&"send".to_string()));
}

// --- end-to-end conflict path: intent + keep-alive → tool_choice ---

#[test]
fn keepalive_plain_text_continuation_resolves_to_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool via Telegram.",
    )];
    let parser = crate::orchestrator::intent::IntentParser;
    let mut tool_names = parser.suggest_tools("好的，发吧");
    let _intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);

    let has_send = tool_names.contains(&"send".to_string());
    let choice = resolve_send_tool_choice(has_send);
    assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
}

#[test]
fn keepalive_text_send_continuation_resolves_to_send() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram使用`send`工具发送消息。",
    )];
    let parser = crate::orchestrator::intent::IntentParser;
    let mut tool_names = parser.suggest_tools("发消息给他");
    let _intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);

    let has_send = tool_names.contains(&"send".to_string());
    let choice = resolve_send_tool_choice(has_send);
    assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
}

#[test]
fn apply_keepalive_cross_channel_injects_send() {
    let messages = vec![
        ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via iMessage.",
        ),
        ChatMessage::user("等一下"),
        ChatMessage::assistant(
            "好的，你的 Telegram 收件人是？",
        ),
    ];
    let mut tool_names = vec!["web_fetch".to_string()];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(!intent_send);
    assert!(tool_names.contains(&"send".to_string()));
    assert_eq!(tool_names.len(), 2); // web_fetch + send
}
