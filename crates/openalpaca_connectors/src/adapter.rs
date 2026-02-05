use openalpaca_api::events::{EventPayload, EventSource, UnifiedEvent};

/// An inbound message from any connector, before conversion to UnifiedEvent.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub source: EventSource,
    pub content: String,
}

/// Trait for converting connector-native messages into UnifiedEvents.
pub trait ConnectorAdapter: Send + Sync {
    /// Convert an inbound message into a UnifiedEvent.
    fn to_unified_event(&self, msg: InboundMessage) -> UnifiedEvent;
}

/// Default adapter that wraps the content as a UserMessage payload.
pub struct DefaultAdapter;

impl ConnectorAdapter for DefaultAdapter {
    fn to_unified_event(&self, msg: InboundMessage) -> UnifiedEvent {
        let payload = if msg.content.starts_with('/') {
            // Parse as command: "/cmd arg1 arg2"
            let mut parts = msg.content[1..].splitn(2, ' ');
            let name = parts.next().unwrap_or("").to_string();
            let args = parts
                .next()
                .map(|a| a.split_whitespace().map(String::from).collect())
                .unwrap_or_default();
            EventPayload::Command { name, args }
        } else {
            EventPayload::UserMessage {
                content: msg.content,
            }
        };

        UnifiedEvent::new(msg.source, payload)
    }
}

/// Helper to create an InboundMessage from Telegram-style inputs.
#[cfg(feature = "telegram")]
pub fn telegram_inbound(chat_id: &str, user_id: &str, content: &str) -> InboundMessage {
    InboundMessage {
        source: EventSource::Telegram {
            chat_id: chat_id.into(),
            user_id: user_id.into(),
        },
        content: content.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_adapter_user_message() {
        let adapter = DefaultAdapter;
        let msg = InboundMessage {
            source: EventSource::Cli {
                session_id: "s1".into(),
            },
            content: "hello world".into(),
        };

        let event = adapter.to_unified_event(msg);
        assert_eq!(
            event.payload,
            EventPayload::UserMessage {
                content: "hello world".into()
            }
        );
        assert_eq!(
            event.source,
            EventSource::Cli {
                session_id: "s1".into()
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
}
