//! Message chunking, formatting, and delivery helpers for Telegram.

use std::time::Duration;
use teloxide::payloads::SendMessage;
use teloxide::prelude::*;
use teloxide::requests::JsonRequest;
use teloxide::types::LinkPreviewOptions;
use tracing::{error, warn};

/// Telegram's max message length
pub(super) const TELEGRAM_MAX_LENGTH: usize = 4096;

/// Split a message into chunks that fit within Telegram's message limit.
pub(super) fn chunk_message(text: &str) -> Vec<String> {
    crate::common::chunk_message(text, TELEGRAM_MAX_LENGTH)
}

/// Send a message with exponential backoff retry (3 attempts: 1s, 2s, 4s).
pub(super) async fn send_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_chunks_with_retry(&chunk_message(text), |chunk| {
        bot.send_message(chat_id, chunk)
    })
    .await
}

/// [`send_with_retry`] for an approval prompt: every chunk is an
/// [`approval_message`], split by [`approval_chunks`].
pub(super) async fn send_approval_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_chunks_with_retry(&approval_chunks(text), |chunk| {
        approval_message(bot, chat_id, chunk)
    })
    .await
}

/// An approval prompt split into Telegram messages, only between lines
/// (`common::chunk_approval_prompt`, the lead's ruling R-T32), every one
/// after the first begun with the prompt's own continued line (Y.2).
pub(super) fn approval_chunks(text: &str) -> Vec<String> {
    crate::common::chunk_approval_prompt(
        text,
        TELEGRAM_MAX_LENGTH,
        crate::common::APPROVAL_CONTINUED,
    )
}

/// An approval prompt, or the acknowledgement of a reply to one, as a
/// Telegram message: with no link preview.
///
/// Telegram draws a card under the first URL in a message — the linked
/// page's own title, description and image. In an approval prompt that URL
/// is in the tool's arguments, so the card would put text the site's owner
/// chose ("Read-only: lists /tmp") inside the message the person decides
/// on, where no escape reaches it. The model's ordinary answers keep their
/// previews.
pub(super) fn approval_message(bot: &Bot, chat_id: ChatId, text: &str) -> JsonRequest<SendMessage> {
    bot.send_message(chat_id, text)
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
}

/// Each of `chunks` sent as `message` builds it, with the retries.
async fn send_chunks_with_retry(
    chunks: &[String],
    message: impl Fn(&str) -> JsonRequest<SendMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for chunk in chunks {
        let mut attempts = 0;
        let max_retries = 3;

        loop {
            match message(chunk).await {
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
                    let delay = Duration::from_secs(1 << (attempts - 1)); // 1s, 2s, 4s
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
