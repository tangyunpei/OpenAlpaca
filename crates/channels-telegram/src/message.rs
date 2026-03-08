use openalpaca_channels::InboundMessage;
use openalpaca_core::types::ChatType;

use crate::types::{TelegramMessage, Update};

/// Parse a Telegram Update into an InboundMessage.
///
/// Returns None for updates that don't contain processable messages
/// (e.g., callback queries, bot's own messages).
pub fn parse_update(update: &Update) -> Option<InboundMessage> {
    let msg = update.message.as_ref().or(update.edited_message.as_ref())?;
    parse_telegram_message(msg)
}

fn parse_telegram_message(msg: &TelegramMessage) -> Option<InboundMessage> {
    let from = msg.from.as_ref()?;

    // Skip messages from bots
    if from.is_bot {
        return None;
    }

    let text = msg.text.clone().unwrap_or_default();
    if text.is_empty() {
        return None;
    }

    let chat_type = match msg.chat.chat_type.as_str() {
        "private" => ChatType::Direct,
        "group" | "supergroup" => ChatType::Group,
        "channel" => ChatType::Channel,
        _ => ChatType::Group,
    };

    let sender_name = match (&from.first_name, &from.last_name) {
        (first, Some(last)) => Some(format!("{first} {last}")),
        (first, None) => Some(first.clone()),
    };

    let reply_to = msg
        .reply_to_message
        .as_ref()
        .map(|r| r.message_id.to_string());

    Some(InboundMessage {
        text,
        sender_id: from.id.to_string(),
        sender_name,
        chat_id: msg.chat.id.to_string(),
        chat_type,
        message_id: msg.message_id.to_string(),
        thread_id: msg.message_thread_id.map(|id| id.to_string()),
        reply_to_message_id: reply_to,
        media_urls: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_update(text: &str, chat_type: &str, is_bot: bool) -> Update {
        Update {
            update_id: 1,
            message: Some(TelegramMessage {
                message_id: 10,
                from: Some(crate::types::User {
                    id: 42,
                    is_bot,
                    first_name: "Alice".into(),
                    last_name: Some("Smith".into()),
                    username: Some("alice".into()),
                }),
                chat: crate::types::Chat {
                    id: 100,
                    chat_type: chat_type.into(),
                    title: None,
                    username: None,
                },
                text: Some(text.into()),
                reply_to_message: None,
                message_thread_id: None,
                photo: None,
                document: None,
                date: None,
            }),
            edited_message: None,
            callback_query: None,
        }
    }

    #[test]
    fn test_parse_private_message() {
        let update = make_update("hello", "private", false);
        let msg = parse_update(&update).unwrap();
        assert_eq!(msg.text, "hello");
        assert_eq!(msg.sender_id, "42");
        assert_eq!(msg.sender_name.as_deref(), Some("Alice Smith"));
        assert_eq!(msg.chat_id, "100");
        assert!(matches!(msg.chat_type, ChatType::Direct));
    }

    #[test]
    fn test_parse_group_message() {
        let update = make_update("hi group", "supergroup", false);
        let msg = parse_update(&update).unwrap();
        assert!(matches!(msg.chat_type, ChatType::Group));
    }

    #[test]
    fn test_skip_bot_message() {
        let update = make_update("bot reply", "private", true);
        assert!(parse_update(&update).is_none());
    }

    #[test]
    fn test_skip_empty_text() {
        let update = make_update("", "private", false);
        assert!(parse_update(&update).is_none());
    }

    #[test]
    fn test_parse_with_reply() {
        let mut update = make_update("reply text", "private", false);
        if let Some(ref mut msg) = update.message {
            msg.reply_to_message = Some(Box::new(TelegramMessage {
                message_id: 5,
                from: None,
                chat: msg.chat.clone(),
                text: Some("original".into()),
                reply_to_message: None,
                message_thread_id: None,
                photo: None,
                document: None,
                date: None,
            }));
        }
        let msg = parse_update(&update).unwrap();
        assert_eq!(msg.reply_to_message_id.as_deref(), Some("5"));
    }

    #[test]
    fn test_parse_with_thread() {
        let mut update = make_update("threaded", "supergroup", false);
        if let Some(ref mut msg) = update.message {
            msg.message_thread_id = Some(999);
        }
        let msg = parse_update(&update).unwrap();
        assert_eq!(msg.thread_id.as_deref(), Some("999"));
    }
}
