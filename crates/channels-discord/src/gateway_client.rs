use std::sync::Arc;

use serenity::async_trait;
use serenity::model::channel::Message as SerenityMessage;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

use openalpaca_channels::InboundHandler;
use openalpaca_core::types::{AccountId, ChannelId};

use crate::message::parse_discord_message;
use crate::send;

/// Shared state for the Discord event handler.
pub struct DiscordHandlerState {
    pub handler: Arc<dyn InboundHandler>,
    pub channel_id: ChannelId,
    pub account_id: AccountId,
    pub chunk_limit: usize,
}

/// Serenity event handler that dispatches messages to the InboundHandler.
pub struct DiscordHandler {
    pub state: Arc<DiscordHandlerState>,
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn message(&self, ctx: Context, msg: SerenityMessage) {
        let Some(inbound) = parse_discord_message(&msg) else {
            return;
        };

        match self
            .state
            .handler
            .handle_message(&self.state.channel_id, &self.state.account_id, &inbound)
            .await
        {
            Ok(reply) => {
                if let Err(e) =
                    send::send_reply(&ctx.http, msg.channel_id, &reply, self.state.chunk_limit)
                        .await
                {
                    tracing::warn!("discord: failed to send reply: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("discord: handler error: {e}");
            }
        }
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        tracing::info!(
            "discord: connected as {} ({})",
            ready.user.name,
            ready.user.id
        );
    }
}
