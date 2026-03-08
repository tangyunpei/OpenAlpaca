use serde::Deserialize;

use openalpaca_channels::InboundMessage;
use openalpaca_core::types::ChatType;

/// A Socket Mode envelope received over the WebSocket.
#[derive(Debug, Deserialize)]
pub struct SocketModeEnvelope {
    pub envelope_id: String,
    #[serde(rename = "type")]
    pub envelope_type: String,
    pub payload: Option<serde_json::Value>,
}

/// Slack event wrapper.
#[derive(Debug, Deserialize)]
pub struct EventCallback {
    pub event: SlackEvent,
}

/// A Slack event.
#[derive(Debug, Deserialize)]
pub struct SlackEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub user: Option<String>,
    pub text: Option<String>,
    pub channel: Option<String>,
    pub ts: Option<String>,
    pub thread_ts: Option<String>,
    pub channel_type: Option<String>,
    pub bot_id: Option<String>,
}

/// Parse a Socket Mode event payload into an InboundMessage.
pub fn parse_event(payload: &serde_json::Value) -> Option<InboundMessage> {
    let callback: EventCallback = serde_json::from_value(payload.clone()).ok()?;
    let event = &callback.event;

    // Only handle message events
    if event.event_type != "message" && event.event_type != "app_mention" {
        return None;
    }

    // Skip bot messages
    if event.bot_id.is_some() {
        return None;
    }

    let text = event.text.clone()?;
    if text.is_empty() {
        return None;
    }

    let user = event.user.clone()?;
    let channel = event.channel.clone()?;
    let ts = event.ts.clone()?;

    let chat_type = match event.channel_type.as_deref() {
        Some("im") => ChatType::Direct,
        _ => ChatType::Group,
    };

    Some(InboundMessage {
        text,
        sender_id: user,
        sender_name: None,
        chat_id: channel,
        chat_type,
        message_id: ts,
        thread_id: event.thread_ts.clone(),
        reply_to_message_id: None,
        media_urls: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dm_event() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "user": "U123",
                "text": "hello bot",
                "channel": "D456",
                "ts": "1234567890.123456",
                "channel_type": "im"
            }
        });
        let msg = parse_event(&payload).unwrap();
        assert_eq!(msg.text, "hello bot");
        assert_eq!(msg.sender_id, "U123");
        assert_eq!(msg.chat_id, "D456");
        assert!(matches!(msg.chat_type, ChatType::Direct));
    }

    #[test]
    fn test_parse_channel_event() {
        let payload = serde_json::json!({
            "event": {
                "type": "app_mention",
                "user": "U789",
                "text": "<@B123> help",
                "channel": "C111",
                "ts": "999.000",
                "channel_type": "channel"
            }
        });
        let msg = parse_event(&payload).unwrap();
        assert_eq!(msg.text, "<@B123> help");
        assert!(matches!(msg.chat_type, ChatType::Group));
    }

    #[test]
    fn test_parse_threaded_event() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "user": "U123",
                "text": "in thread",
                "channel": "C111",
                "ts": "999.001",
                "thread_ts": "999.000",
                "channel_type": "channel"
            }
        });
        let msg = parse_event(&payload).unwrap();
        assert_eq!(msg.thread_id.as_deref(), Some("999.000"));
    }

    #[test]
    fn test_skip_bot_message() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "bot_id": "B123",
                "text": "bot says hi",
                "channel": "C111",
                "ts": "999.002"
            }
        });
        assert!(parse_event(&payload).is_none());
    }

    #[test]
    fn test_skip_non_message_event() {
        let payload = serde_json::json!({
            "event": {
                "type": "reaction_added",
                "user": "U123"
            }
        });
        assert!(parse_event(&payload).is_none());
    }
}
