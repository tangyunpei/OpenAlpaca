use serenity::http::Http;
use serenity::model::id::ChannelId;

use openalpaca_delivery::chunk_text;

/// Default character limit for Discord messages.
pub const DEFAULT_CHUNK_LIMIT: usize = 2000;

/// Send a reply to a Discord channel, chunking if needed.
pub async fn send_reply(
    http: &Http,
    channel_id: ChannelId,
    text: &str,
    chunk_limit: usize,
) -> Result<Vec<serenity::model::id::MessageId>, serenity::Error> {
    let limit = if chunk_limit == 0 {
        DEFAULT_CHUNK_LIMIT
    } else {
        chunk_limit
    };

    let chunks = chunk_text(text, limit);
    let mut message_ids = Vec::new();

    for chunk in &chunks {
        let msg = channel_id.say(http, chunk).await?;
        message_ids.push(msg.id);
    }

    Ok(message_ids)
}
