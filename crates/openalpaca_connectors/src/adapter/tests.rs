use super::*;

#[test]
fn test_default_adapter_user_message() {
    let adapter = DefaultAdapter;
    let msg = InboundMessage {
        source: EventSource::Api {
            request_id: "r1".into(),
        },
        content: "hello world".into(),
        attachments: Vec::new(),
    };

    let event = adapter.to_unified_event(msg);
    assert_eq!(
        event.payload,
        EventPayload::UserMessage {
            content: "hello world".into(),
            attachment_ids: Vec::new(),
        }
    );
    assert_eq!(
        event.source,
        EventSource::Api {
            request_id: "r1".into()
        }
    );
}

#[test]
fn test_default_adapter_command() {
    let adapter = DefaultAdapter;
    let msg = InboundMessage {
        source: EventSource::Gui {
            connection_id: "c1".into(),
        },
        content: "/help --verbose".into(),
        attachments: Vec::new(),
    };

    let event = adapter.to_unified_event(msg);
    assert_eq!(
        event.payload,
        EventPayload::Command {
            name: "help".into(),
            args: vec!["--verbose".into()],
        }
    );
}

#[test]
fn test_default_adapter_command_no_args() {
    let adapter = DefaultAdapter;
    let msg = InboundMessage {
        source: EventSource::Internal,
        content: "/status".into(),
        attachments: Vec::new(),
    };

    let event = adapter.to_unified_event(msg);
    assert_eq!(
        event.payload,
        EventPayload::Command {
            name: "status".into(),
            args: vec![],
        }
    );
}

#[cfg(feature = "telegram")]
#[test]
fn test_telegram_inbound_helper() {
    let msg = telegram_inbound("123", "456", "hi there");
    assert_eq!(
        msg.source,
        EventSource::Telegram {
            chat_id: "123".into(),
            user_id: "456".into(),
        }
    );
    assert_eq!(msg.content, "hi there");
}

#[cfg(feature = "discord")]
#[test]
fn test_discord_inbound_helper() {
    let msg = discord_inbound("987654321", "user_42", Some("guild_1"), "hello discord");
    assert_eq!(
        msg.source,
        EventSource::Discord {
            channel_id: "987654321".into(),
            user_id: "user_42".into(),
            guild_id: Some("guild_1".into()),
        }
    );
    assert_eq!(msg.content, "hello discord");
    assert!(msg.attachments.is_empty());
}

#[cfg(feature = "discord")]
#[test]
fn test_discord_inbound_helper_no_guild() {
    let msg = discord_inbound("123", "456", None, "dm message");
    assert_eq!(
        msg.source,
        EventSource::Discord {
            channel_id: "123".into(),
            user_id: "456".into(),
            guild_id: None,
        }
    );
    assert_eq!(msg.content, "dm message");
}
