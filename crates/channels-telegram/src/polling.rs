use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use openalpaca_channels::{InboundHandler, InboundMessage};
use openalpaca_core::types::{AccountId, ChannelId};

use crate::api::TelegramApi;
use crate::message::parse_update;
use crate::send;

/// Run the Telegram long-polling loop.
///
/// Fetches updates, parses them into inbound messages, dispatches to the handler,
/// and sends the reply back. Retries on error with a 5-second delay.
pub async fn run_polling_loop(
    api: Arc<TelegramApi>,
    handler: Arc<dyn InboundHandler>,
    channel_id: ChannelId,
    account_id: AccountId,
    chunk_limit: usize,
    cancel: CancellationToken,
) {
    let mut offset: Option<i64> = None;

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                tracing::info!("telegram: polling loop cancelled");
                break;
            }
            result = api.get_updates(offset, 30) => {
                match result {
                    Ok(updates) => {
                        for update in &updates {
                            offset = Some(update.update_id + 1);

                            if let Some(inbound) = parse_update(update) {
                                handle_inbound(
                                    &api,
                                    &handler,
                                    &channel_id,
                                    &account_id,
                                    &inbound,
                                    chunk_limit,
                                )
                                .await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("telegram: polling error: {e}");
                        tokio::select! {
                            () = cancel.cancelled() => break,
                            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                        }
                    }
                }
            }
        }
    }
}

async fn handle_inbound(
    api: &TelegramApi,
    handler: &Arc<dyn InboundHandler>,
    channel_id: &ChannelId,
    account_id: &AccountId,
    inbound: &InboundMessage,
    chunk_limit: usize,
) {
    // Send typing indicator
    let _ = api.send_chat_action(&inbound.chat_id, "typing").await;

    match handler
        .handle_message(channel_id, account_id, inbound)
        .await
    {
        Ok(reply) => {
            let reply_to = inbound.message_id.parse::<i64>().ok();
            let thread_id = inbound.thread_id.as_deref().and_then(|t| t.parse().ok());

            if let Err(e) = send::send_reply(
                api,
                &inbound.chat_id,
                &reply,
                reply_to,
                thread_id,
                chunk_limit,
            )
            .await
            {
                tracing::warn!("telegram: failed to send reply: {e}");
            }
        }
        Err(e) => {
            tracing::warn!("telegram: handler error: {e}");
        }
    }
}
