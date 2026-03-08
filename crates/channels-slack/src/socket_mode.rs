use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_util::sync::CancellationToken;

use openalpaca_channels::InboundHandler;
use openalpaca_core::types::{AccountId, ChannelId};

use crate::api::{self, SlackApi};
use crate::events::{self, SocketModeEnvelope};
use crate::send;

/// Parameters for the socket mode loop, bundled to avoid too many arguments.
pub struct SocketModeParams {
    pub http_client: reqwest::Client,
    pub app_token: String,
    pub api: Arc<SlackApi>,
    pub handler: Arc<dyn InboundHandler>,
    pub channel_id: ChannelId,
    pub account_id: AccountId,
    pub chunk_limit: usize,
}

/// Run the Slack Socket Mode connection loop.
///
/// Connects to Slack's Socket Mode endpoint, handles events,
/// dispatches to the InboundHandler, and reconnects on disconnect.
pub async fn run_socket_mode(params: SocketModeParams, cancel: CancellationToken) {
    let base_url = "https://slack.com/api";

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Get WebSocket URL
        let ws_url =
            match api::connections_open(&params.http_client, &params.app_token, base_url).await {
                Ok(url) => url,
                Err(e) => {
                    tracing::warn!("slack: failed to open connection: {e}");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                    }
                }
            };

        tracing::info!("slack: connecting to socket mode");

        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                let (mut ws_sender, mut ws_receiver) = ws_stream.split();

                loop {
                    tokio::select! {
                        () = cancel.cancelled() => {
                            tracing::info!("slack: shutting down socket mode");
                            return;
                        }
                        msg = ws_receiver.next() => {
                            match msg {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    handle_socket_message(
                                        &text,
                                        &mut ws_sender,
                                        &params,
                                    )
                                    .await;
                                }
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                                    tracing::warn!("slack: socket mode disconnected, reconnecting...");
                                    break;
                                }
                                Some(Ok(_)) => {} // ignore ping/pong/binary
                                Some(Err(e)) => {
                                    tracing::warn!("slack: ws error: {e}");
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("slack: ws connect error: {e}");
            }
        }

        // Backoff before reconnect
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
    }
}

async fn handle_socket_message(
    text: &str,
    ws_sender: &mut (impl SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin),
    params: &SocketModeParams,
) {
    let Ok(envelope) = serde_json::from_str::<SocketModeEnvelope>(text) else {
        return;
    };

    // Acknowledge the envelope
    let ack = serde_json::json!({ "envelope_id": envelope.envelope_id });
    let _ = ws_sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            ack.to_string().into(),
        ))
        .await;

    // Process events_api events
    if envelope.envelope_type == "events_api"
        && let Some(payload) = &envelope.payload
        && let Some(inbound) = events::parse_event(payload)
    {
        match params
            .handler
            .handle_message(&params.channel_id, &params.account_id, &inbound)
            .await
        {
            Ok(reply) => {
                let thread_ts = inbound.thread_id.as_deref().or(Some(&inbound.message_id));
                if let Err(e) = send::send_reply(
                    &params.api,
                    &inbound.chat_id,
                    &reply,
                    thread_ts,
                    params.chunk_limit,
                )
                .await
                {
                    tracing::warn!("slack: failed to send reply: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("slack: handler error: {e}");
            }
        }
    }
}
