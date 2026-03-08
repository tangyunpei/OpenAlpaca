use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use openalpaca_channels::{
    ChannelAccountSnapshot, ChannelCapabilities, ChannelError, ChannelGatewayContext, ChannelMeta,
    ChannelPlugin, InboundHandler, OutboundContext, OutboundResult, ReplyToMode,
};
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::types::{AccountId, ChannelId, ChatType};

use crate::api::TelegramApi;
use crate::send;

/// Telegram channel plugin implementing `ChannelPlugin`.
pub struct TelegramChannel {
    id: ChannelId,
    meta: ChannelMeta,
    capabilities: ChannelCapabilities,
    api: Arc<TelegramApi>,
    handler: Arc<dyn InboundHandler>,
    chunk_limit: usize,
    cancel: CancellationToken,
}

impl TelegramChannel {
    pub fn new(
        api: Arc<TelegramApi>,
        handler: Arc<dyn InboundHandler>,
        chunk_limit: Option<usize>,
    ) -> Self {
        let id = ChannelId::new_unchecked("telegram");
        Self {
            meta: ChannelMeta {
                id: id.clone(),
                label: "Telegram".into(),
                blurb: "Telegram Bot API channel".into(),
                order: 1,
                aliases: vec!["tg".into()],
            },
            capabilities: ChannelCapabilities {
                chat_types: vec![ChatType::Direct, ChatType::Group],
                polls: false,
                reactions: false,
                edit: true,
                threads: true,
                media: false, // not yet implemented
                reply: true,
            },
            id,
            api,
            handler,
            chunk_limit: chunk_limit.unwrap_or(send::DEFAULT_CHUNK_LIMIT),
            cancel: CancellationToken::new(),
        }
    }
}

#[async_trait]
impl ChannelPlugin for TelegramChannel {
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
        let api = self.api.clone();
        let handler = self.handler.clone();
        let channel_id = self.id.clone();
        let account_id = ctx.account_id.clone();
        let chunk_limit = self.chunk_limit;
        let cancel = self.cancel.child_token();

        // Delete any existing webhook before starting long-polling
        api.delete_webhook()
            .await
            .map_err(|e| ChannelError::Other(e.to_string()))?;

        tokio::spawn(crate::polling::run_polling_loop(
            api,
            handler,
            channel_id,
            account_id,
            chunk_limit,
            cancel,
        ));

        tracing::info!("telegram: started polling for account {}", ctx.account_id);
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
        let default_id = AccountId::new_unchecked("default");
        let aid = account_id.unwrap_or(&default_id);
        crate::config::resolve_account_config(config, aid)
            .and_then(|c| c.reply_to_mode.as_deref())
            .map(ReplyToMode::parse)
            .unwrap_or(ReplyToMode::Off)
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

    async fn send_text(&self, ctx: &OutboundContext) -> Result<OutboundResult, ChannelError> {
        let reply_to = ctx
            .reply_to_id
            .as_deref()
            .and_then(|id| id.parse::<i64>().ok());
        let thread_id = ctx
            .thread_id
            .as_deref()
            .and_then(|id| id.parse::<i64>().ok());

        let ids = send::send_reply(
            &self.api,
            &ctx.target,
            &ctx.text,
            reply_to,
            thread_id,
            self.chunk_limit,
        )
        .await
        .map_err(|e| ChannelError::Other(e.to_string()))?;

        Ok(OutboundResult {
            message_id: ids.first().map(|id| id.to_string()),
        })
    }

    async fn build_account_snapshot(
        &self,
        config: &OpenAlpacaConfig,
        account_id: &AccountId,
    ) -> Option<ChannelAccountSnapshot> {
        let enabled = self.is_account_enabled(config, account_id);
        Some(ChannelAccountSnapshot {
            account_id: account_id.to_string(),
            name: Some("Telegram Bot".into()),
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
    use reqwest::Client;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn make_test_channel(server: &MockServer) -> TelegramChannel {
        let api = Arc::new(TelegramApi::with_base_url(Client::new(), server.uri()));
        TelegramChannel::new(api, Arc::new(EchoHandler), None)
    }

    #[tokio::test]
    async fn test_plugin_identity() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        assert_eq!(channel.id().as_str(), "telegram");
        assert_eq!(channel.meta().label, "Telegram");
        assert!(channel.capabilities().edit);
        assert!(channel.capabilities().reply);
    }

    #[tokio::test]
    async fn test_send_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 42,
                    "from": {"id": 1, "is_bot": true, "first_name": "Bot"},
                    "chat": {"id": 100, "type": "private"},
                    "text": "hello"
                }
            })))
            .mount(&server)
            .await;

        let channel = make_test_channel(&server).await;
        let ctx = OutboundContext {
            channel_id: ChannelId::new_unchecked("telegram"),
            account_id: AccountId::new_unchecked("default"),
            target: "100".into(),
            text: "hello".into(),
            reply_to_id: None,
            thread_id: None,
        };
        let result = channel.send_text(&ctx).await.unwrap();
        assert_eq!(result.message_id.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn test_is_object_safe() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let _boxed: Box<dyn ChannelPlugin> = Box::new(channel);
    }

    #[tokio::test]
    async fn test_text_chunk_limit() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        assert_eq!(channel.text_chunk_limit(), Some(4000));
    }

    #[tokio::test]
    async fn test_resolve_require_mention() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let config = OpenAlpacaConfig::default();
        assert_eq!(
            channel.resolve_require_mention(&config, None, None),
            Some(true)
        );
    }

    #[tokio::test]
    async fn test_mention_strip_patterns_empty() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        assert!(channel.mention_strip_patterns().is_empty());
    }

    #[tokio::test]
    async fn test_check_ready() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let config = OpenAlpacaConfig::default();
        let aid = AccountId::new_unchecked("default");
        assert!(channel.check_ready(&config, &aid).await.unwrap());
    }

    #[tokio::test]
    async fn test_resolve_reply_to_mode_from_config() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let yaml = r#"
channels:
  telegram:
    bot_token: "123:ABC"
    reply_to_mode: "first"
"#;
        let config: OpenAlpacaConfig = serde_yaml::from_str(yaml).unwrap();
        let aid = AccountId::new_unchecked("default");
        assert_eq!(
            channel.resolve_reply_to_mode(&config, Some(&aid), None),
            ReplyToMode::First
        );
    }

    #[tokio::test]
    async fn test_new_methods_via_trait_object() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let boxed: Box<dyn ChannelPlugin> = Box::new(channel);
        let config = OpenAlpacaConfig::default();
        let aid = AccountId::new_unchecked("default");

        assert_eq!(boxed.text_chunk_limit(), Some(4000));
        assert_eq!(
            boxed.resolve_require_mention(&config, None, None),
            Some(true)
        );
        assert!(boxed.mention_strip_patterns().is_empty());
        assert!(boxed.check_ready(&config, &aid).await.unwrap());
    }
}
