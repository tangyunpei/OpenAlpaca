use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use openalpaca_channels::{
    ChannelAccountSnapshot, ChannelCapabilities, ChannelError, ChannelGatewayContext, ChannelMeta,
    ChannelPlugin, InboundHandler, OutboundContext, OutboundResult, ReplyToMode,
};
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::types::{AccountId, ChannelId, ChatType};

use crate::api::SlackApi;
use crate::send;

/// Slack channel plugin implementing `ChannelPlugin`.
pub struct SlackChannel {
    id: ChannelId,
    meta: ChannelMeta,
    capabilities: ChannelCapabilities,
    api: Arc<SlackApi>,
    handler: Arc<dyn InboundHandler>,
    http_client: reqwest::Client,
    app_token: String,
    chunk_limit: usize,
    cancel: CancellationToken,
}

impl SlackChannel {
    pub fn new(
        api: Arc<SlackApi>,
        handler: Arc<dyn InboundHandler>,
        http_client: reqwest::Client,
        app_token: String,
        chunk_limit: Option<usize>,
    ) -> Self {
        let id = ChannelId("slack".into());
        Self {
            meta: ChannelMeta {
                id: id.clone(),
                label: "Slack".into(),
                blurb: "Slack workspace channel".into(),
                order: 3,
                aliases: vec![],
            },
            capabilities: ChannelCapabilities {
                chat_types: vec![ChatType::Direct, ChatType::Group],
                polls: false,
                reactions: true,
                edit: true,
                threads: true,
                media: false,
                reply: true,
            },
            id,
            api,
            handler,
            http_client,
            app_token,
            chunk_limit: chunk_limit.unwrap_or(send::DEFAULT_CHUNK_LIMIT),
            cancel: CancellationToken::new(),
        }
    }
}

#[async_trait]
impl ChannelPlugin for SlackChannel {
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
        let cancel = self.cancel.child_token();
        let params = crate::socket_mode::SocketModeParams {
            http_client: self.http_client.clone(),
            app_token: self.app_token.clone(),
            api: self.api.clone(),
            handler: self.handler.clone(),
            channel_id: self.id.clone(),
            account_id: ctx.account_id.clone(),
            chunk_limit: self.chunk_limit,
        };

        tokio::spawn(crate::socket_mode::run_socket_mode(params, cancel));

        tracing::info!("slack: started socket mode for account {}", ctx.account_id);
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
        chat_type: Option<&ChatType>,
    ) -> ReplyToMode {
        let default_id = AccountId("default".into());
        let aid = account_id.unwrap_or(&default_id);
        if let Some(mode) = crate::config::resolve_account_config(config, aid)
            .and_then(|c| c.reply_to_mode.as_deref())
        {
            return ReplyToMode::parse(mode);
        }
        match chat_type {
            Some(ChatType::Group) => ReplyToMode::All,
            _ => ReplyToMode::Off,
        }
    }

    fn resolve_require_mention(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: Option<&AccountId>,
        _group_id: Option<&str>,
    ) -> Option<bool> {
        Some(false)
    }

    async fn check_ready(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: &AccountId,
    ) -> Result<bool, ChannelError> {
        Ok(!self.cancel.is_cancelled())
    }

    async fn send_text(&self, ctx: &OutboundContext) -> Result<OutboundResult, ChannelError> {
        let thread_ts = ctx.thread_id.as_deref();
        let timestamps = send::send_reply(
            &self.api,
            &ctx.target,
            &ctx.text,
            thread_ts,
            self.chunk_limit,
        )
        .await
        .map_err(|e| ChannelError::Other(e.to_string()))?;

        Ok(OutboundResult {
            message_id: timestamps.first().cloned(),
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
            name: Some("Slack Bot".into()),
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

    async fn make_test_channel(server: &MockServer) -> SlackChannel {
        let api = Arc::new(SlackApi::with_base_url(
            Client::new(),
            "xoxb-test",
            server.uri(),
        ));
        SlackChannel::new(
            api,
            Arc::new(EchoHandler),
            Client::new(),
            "xapp-test".into(),
            None,
        )
    }

    #[tokio::test]
    async fn test_plugin_identity() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        assert_eq!(channel.id().0.as_str(), "slack");
        assert_eq!(channel.meta().label, "Slack");
        assert!(channel.capabilities().threads);
        assert!(channel.capabilities().edit);
    }

    #[tokio::test]
    async fn test_send_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1234567890.123456",
                "channel": "C123"
            })))
            .mount(&server)
            .await;

        let channel = make_test_channel(&server).await;
        let ctx = OutboundContext {
            channel_id: ChannelId("slack".into()),
            account_id: AccountId("default".into()),
            target: "C123".into(),
            text: "hello slack".into(),
            reply_to_id: None,
            thread_id: None,
        };
        let result = channel.send_text(&ctx).await.unwrap();
        assert_eq!(result.message_id.as_deref(), Some("1234567890.123456"));
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
        assert_eq!(channel.text_chunk_limit(), Some(3000));
    }

    #[tokio::test]
    async fn test_resolve_reply_to_mode_group_default_all() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let config = OpenAlpacaConfig::default();
        assert_eq!(
            channel.resolve_reply_to_mode(&config, None, Some(&ChatType::Group)),
            ReplyToMode::All
        );
    }

    #[tokio::test]
    async fn test_resolve_reply_to_mode_direct_default_off() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let config = OpenAlpacaConfig::default();
        assert_eq!(
            channel.resolve_reply_to_mode(&config, None, Some(&ChatType::Direct)),
            ReplyToMode::Off
        );
    }

    #[tokio::test]
    async fn test_resolve_reply_to_mode_config_override() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let yaml = r#"
channels:
  slack:
    bot_token: "xoxb-test"
    reply_to_mode: "first"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let aid = AccountId("default".into());
        // Config override should take priority over chat_type default
        assert_eq!(
            channel.resolve_reply_to_mode(&config, Some(&aid), Some(&ChatType::Group)),
            ReplyToMode::First
        );
    }

    #[tokio::test]
    async fn test_resolve_require_mention() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let config = OpenAlpacaConfig::default();
        assert_eq!(
            channel.resolve_require_mention(&config, None, None),
            Some(false)
        );
    }

    #[tokio::test]
    async fn test_check_ready() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let config = OpenAlpacaConfig::default();
        let aid = AccountId("default".into());
        assert!(channel.check_ready(&config, &aid).await.unwrap());
    }

    #[tokio::test]
    async fn test_new_methods_via_trait_object() {
        let server = MockServer::start().await;
        let channel = make_test_channel(&server).await;
        let boxed: Box<dyn ChannelPlugin> = Box::new(channel);
        let config = OpenAlpacaConfig::default();
        let aid = AccountId("default".into());

        assert_eq!(boxed.text_chunk_limit(), Some(3000));
        assert_eq!(
            boxed.resolve_reply_to_mode(&config, None, Some(&ChatType::Group)),
            ReplyToMode::All
        );
        assert_eq!(
            boxed.resolve_require_mention(&config, None, None),
            Some(false)
        );
        assert!(boxed.check_ready(&config, &aid).await.unwrap());
    }
}
