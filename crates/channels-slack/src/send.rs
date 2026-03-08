use crate::api::{SlackApi, SlackApiError};

/// Default character limit for Slack messages.
pub const DEFAULT_CHUNK_LIMIT: usize = 3000;

/// Send a reply, chunking if needed.
///
/// Returns the timestamps of the sent messages.
pub async fn send_reply(
    api: &SlackApi,
    channel: &str,
    text: &str,
    thread_ts: Option<&str>,
    chunk_limit: usize,
) -> Result<Vec<String>, SlackApiError> {
    let limit = if chunk_limit == 0 {
        DEFAULT_CHUNK_LIMIT
    } else {
        chunk_limit
    };

    let chunks = openalpaca_delivery::chunk_text(text, limit);
    let mut timestamps = Vec::new();

    for chunk in &chunks {
        let ts = api.post_message(channel, chunk, thread_ts).await?;
        timestamps.push(ts);
    }

    Ok(timestamps)
}
