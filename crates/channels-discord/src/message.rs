use openalpaca_channels::InboundMessage;
use openalpaca_core::types::ChatType;
use serenity::model::channel::Message as SerenityMessage;

/// Convert a serenity Message into our InboundMessage format.
///
/// Returns None for bot messages or empty messages.
pub fn parse_discord_message(msg: &SerenityMessage) -> Option<InboundMessage> {
    // Skip bot messages
    if msg.author.bot {
        return None;
    }

    let text = msg.content.clone();
    if text.is_empty() {
        return None;
    }

    let chat_type = if msg.guild_id.is_some() {
        ChatType::Group
    } else {
        ChatType::Direct
    };

    let sender_name = match msg.author.discriminator {
        Some(d) => Some(format!("{}#{:04}", msg.author.name, d)),
        None => Some(msg.author.name.clone()),
    };

    Some(InboundMessage {
        text,
        sender_id: msg.author.id.to_string(),
        sender_name,
        chat_id: msg.channel_id.to_string(),
        chat_type,
        message_id: msg.id.to_string(),
        thread_id: msg.thread.as_ref().map(|t| t.id.to_string()),
        reply_to_message_id: msg.referenced_message.as_ref().map(|r| r.id.to_string()),
        media_urls: msg.attachments.iter().map(|a| a.url.clone()).collect(),
    })
}
