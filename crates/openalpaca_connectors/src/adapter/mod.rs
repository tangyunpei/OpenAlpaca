use openalpaca_api::events::{EventPayload, EventSource, UnifiedEvent};

/// An inbound attachment from a connector (raw bytes before storage).
#[derive(Debug, Clone)]
pub struct InboundAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// An inbound message from any connector, before conversion to UnifiedEvent.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub source: EventSource,
    pub content: String,
    pub attachments: Vec<InboundAttachment>,
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
                attachment_ids: Vec::new(),
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
        attachments: Vec::new(),
    }
}

/// Helper to create an InboundMessage from iMessage-style inputs.
#[cfg(all(feature = "imessage", target_os = "macos"))]
pub fn imessage_inbound(chat_id: &str, sender: &str, content: &str) -> InboundMessage {
    InboundMessage {
        source: EventSource::IMessage {
            chat_id: chat_id.into(),
            sender: sender.into(),
        },
        content: content.into(),
        attachments: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
