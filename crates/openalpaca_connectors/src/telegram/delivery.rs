//! Message chunking, formatting, and delivery helpers for Telegram.

use std::time::Duration;
use teloxide::prelude::*;
use tracing::{error, warn};

/// Telegram's max message length
pub(super) const TELEGRAM_MAX_LENGTH: usize = 4096;

/// Split a message into chunks that fit within Telegram's message limit.
/// Prefers splitting at paragraph boundaries (\n\n), then sentence boundaries (. ),
/// then falls back to hard cut at a valid UTF-8 char boundary.
pub(super) fn chunk_message(text: &str) -> Vec<String> {
    crate::common::chunk_message(text, TELEGRAM_MAX_LENGTH)
}

/// Send a message with exponential backoff retry (3 attempts, with 1s and 2s delays).
pub(super) async fn send_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chunks = chunk_message(text);

    for chunk in &chunks {
        let mut attempts = 0;
        let max_retries = 3;

        loop {
            match bot.send_message(chat_id, chunk).await {
                Ok(_) => break,
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        error!(
                            "Failed to send message after {} retries: {}",
                            max_retries, e
                        );
                        return Err(Box::new(e));
                    }
                    let delay = Duration::from_secs(1 << (attempts - 1)); // 1s, 2s
                    warn!(
                        "Send failed (attempt {}/{}), retrying in {:?}: {}",
                        attempts, max_retries, delay, e
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Ok(())
}

/// Download a file from Telegram using the Bot API.
pub(super) async fn download_telegram_file(
    bot: &Bot,
    file_id: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    use teloxide::net::Download;
    use teloxide::types::FileId;
    let file = bot.get_file(FileId(file_id.to_string())).await?;
    let mut buf = Vec::new();
    bot.download_file(&file.path, &mut buf).await?;
    Ok(buf)
}
