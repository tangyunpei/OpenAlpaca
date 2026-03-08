use std::sync::Arc;

use async_trait::async_trait;
use serenity::prelude::*;
use tokio_util::sync::CancellationToken;

use openalpaca_channels::{
    ChannelAccountSnapshot, ChannelCapabilities, ChannelError, ChannelGatewayContext, ChannelMeta,
    ChannelPlugin, InboundHandler, OutboundContext, OutboundResult, ReplyToMode,
};
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::types::{AccountId, ChannelId, ChatType};

use crate::gateway_client::{DiscordHandler, DiscordHandlerState};
use crate::send;

/// Discord channel plugin implementing `ChannelPlugin`.
pub struct DiscordChannel {
    id: ChannelId,
    meta: ChannelMeta,
    capabilities: ChannelCapabilities,
    handler: Arc<dyn InboundHandler>,
    bot_token: String,
    chunk_limit: usize,
    mention_patterns: Vec<String>,
    cancel: CancellationToken,
}

impl DiscordChannel {
    pub fn new(
        bot_token: String,
        handler: Arc<dyn InboundHandler>,
        chunk_limit: Option<usize>,
    ) -> Self {
        let id = ChannelId("discord".into());
        Self {
            meta: ChannelMeta {
                id: id.clone(),
                label: "Discord".into(),
                blurb: "Discord Bot channel".into(),
                order: 2,
                aliases: vec!["dc".into()],
            },
            capabilities: ChannelCapabilities {
                chat_types: vec![ChatType::Direct, ChatType::Group],
                polls: false,
                reactions: true,
                edit: true,
                threads: true,
                media: true,
                reply: true,
            },
            id,
            handler,
            bot_token,
            chunk_limit: chunk_limit.unwrap_or(send::DEFAULT_CHUNK_LIMIT),
            mention_patterns: vec![r"<@!?\d+>".into()],
            cancel: CancellationToken::new(),
        }
    }
}

#[async_trait]
impl ChannelPlugin for DiscordChannel {
    fn id(&self) -> &ChannelId {
        &self.id
    }

    fn meta(&self) -> &ChannelMeta {
        &self.meta
    }

    fn capabilities(&self) -> &ChannelCapabilities {
        &self.capabilities
    }

    fn list_account_ids(&self, config: &OpenAlpacaConfig) -> Vec<AccountId> {
        crate::config::list_account_ids(config)
    }

    fn is_account_enabled(&self, config: &OpenAlpacaConfig, account_id: &AccountId) -> bool {
        crate::config::is_account_enabled(config, account_id)
    }

    async fn start_account(&self, ctx: &ChannelGatewayContext) -> Result<(), ChannelError> {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let handler_state = Arc::new(DiscordHandlerState {
            handler: self.handler.clone(),
            channel_id: self.id.clone(),
            account_id: ctx.account_id.clone(),
            chunk_limit: self.chunk_limit,
        });

        let mut client = Client::builder(&self.bot_token, intents)
            .event_handler(DiscordHandler {
                state: handler_state,
            })
            .await
            .map_err(|e| ChannelError::Other(format!("failed to create Discord client: {e}")))?;

        let cancel = self.cancel.child_token();
        tokio::spawn(async move {
            tokio::select! {
                result = client.start() => {
                    if let Err(e) = result {
                        tracing::warn!("discord: client error: {e}");
                    }
                }
                () = cancel.cancelled() => {
                    tracing::info!("discord: shutting down client");
                    client.shard_manager.shutdown_all().await;
                }
            }
        });

        tracing::info!("discord: started for account {}", ctx.account_id);
        Ok(())
    }

    async fn stop_account(&self, _ctx: &ChannelGatewayContext) -> Result<(), ChannelError> {
        self.cancel.cancel();
        Ok(())
    }

    fn text_chunk_limit(&self) -> Option<usize> {
        Some(self.chunk_limit)
    }

    fn resolve_reply_to_mode(
        &self,
        config: &OpenAlpacaConfig,
        account_id: Option<&AccountId>,
        _chat_type: Option<&ChatType>,
    ) -> ReplyToMode {
        let default_id = AccountId("default".into());
        let aid = account_id.unwrap_or(&default_id);
        crate::config::resolve_account_config(config, aid)
            .and_then(|c| c.reply_to_mode.as_deref())
            .map(ReplyToMode::parse)
            .unwrap_or(ReplyToMode::Off)
    }

    fn mention_strip_patterns(&self) -> &[String] {
        &self.mention_patterns
    }

    fn resolve_require_mention(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: Option<&AccountId>,
        _group_id: Option<&str>,
    ) -> Option<bool> {
        Some(true)
    }

    async fn check_ready(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: &AccountId,
    ) -> Result<bool, ChannelError> {
        Ok(!self.cancel.is_cancelled())
    }

    async fn send_text(&self, _ctx: &OutboundContext) -> Result<OutboundResult, ChannelError> {
        // Direct send requires an HTTP client from a running serenity client.
        // For now, outbound is handled in the event handler via serenity context.
        Err(ChannelError::NotSupported(
            "send_text without active context".into(),
        ))
    }

    async fn build_account_snapshot(
        &self,
        config: &OpenAlpacaConfig,
        account_id: &AccountId,
    ) -> Option<ChannelAccountSnapshot> {
        let enabled = self.is_account_enabled(config, account_id);
        Some(ChannelAccountSnapshot {
            account_id: account_id.to_string(),
            name: Some("Discord Bot".into()),
            enabled,
            configured: crate::config::resolve_account_config(config, account_id).is_some(),
            connected: !self.cancel.is_cancelled(),
            running: !self.cancel.is_cancelled(),
            last_error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_channels::EchoHandler;

    fn make_test_channel() -> DiscordChannel {
        DiscordChannel::new("fake-token".into(), Arc::new(EchoHandler), None)
    }

    #[test]
    fn test_plugin_identity() {
        let channel = make_test_channel();
        assert_eq!(channel.id().0.as_str(), "discord");
        assert_eq!(channel.meta().label, "Discord");
        assert!(channel.capabilities().reactions);
        assert!(channel.capabilities().threads);
    }

    #[test]
    fn test_is_object_safe() {
        let channel = make_test_channel();
        let _boxed: Box<dyn ChannelPlugin> = Box::new(channel);
    }

    #[test]
    fn test_config_no_discord() {
        let config = OpenAlpacaConfig::default();
        let channel = make_test_channel();
        assert!(channel.list_account_ids(&config).is_empty());
    }

    #[test]
    fn test_text_chunk_limit() {
        let channel = make_test_channel();
        assert_eq!(channel.text_chunk_limit(), Some(2000));
    }

    #[test]
    fn test_mention_strip_patterns() {
        let channel = make_test_channel();
        let patterns = channel.mention_strip_patterns();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0], r"<@!?\d+>");
    }

    #[test]
    fn test_mention_pattern_regex_compiles_and_matches() {
        let channel = make_test_channel();
        let pattern = &channel.mention_strip_patterns()[0];
        let re = regex::Regex::new(pattern).expect("pattern should be valid regex");
        assert!(re.is_match("<@123456>"));
        assert!(re.is_match("<@!789012>"));
        assert!(!re.is_match("@plainmention"));
    }

    #[test]
    fn test_resolve_require_mention() {
        let channel = make_test_channel();
        let config = OpenAlpacaConfig::default();
        assert_eq!(
            channel.resolve_require_mention(&config, None, None),
            Some(true)
        );
    }

    #[tokio::test]
    async fn test_check_ready() {
        let channel = make_test_channel();
        let config = OpenAlpacaConfig::default();
        let aid = AccountId("default".into());
        assert!(channel.check_ready(&config, &aid).await.unwrap());
    }

    #[tokio::test]
    async fn test_new_methods_via_trait_object() {
        let channel = make_test_channel();
        let boxed: Box<dyn ChannelPlugin> = Box::new(channel);
        let config = OpenAlpacaConfig::default();
        let aid = AccountId("default".into());

        assert_eq!(boxed.text_chunk_limit(), Some(2000));
        assert!(!boxed.mention_strip_patterns().is_empty());
        assert_eq!(
            boxed.resolve_require_mention(&config, None, None),
            Some(true)
        );
        assert!(boxed.check_ready(&config, &aid).await.unwrap());
    }
}
