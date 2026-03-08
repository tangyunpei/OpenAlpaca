use async_trait::async_trait;

use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::types::{AccountId, ChannelId, ChatType};

use crate::error::ChannelError;
use crate::meta::{ChannelCapabilities, ChannelMeta};
use crate::plugin::{ChannelPlugin, OutboundContext, OutboundResult};

/// Echo test channel — reflects messages back for integration testing.
pub struct EchoChannel {
    id: ChannelId,
    meta: ChannelMeta,
    capabilities: ChannelCapabilities,
}

impl EchoChannel {
    pub fn new() -> Self {
        let id = ChannelId::new_unchecked("echo");
        Self {
            meta: ChannelMeta {
                id: id.clone(),
                label: "Echo".into(),
                blurb: "Test channel that echoes messages back".into(),
                order: 999,
                aliases: vec![],
            },
            capabilities: ChannelCapabilities {
                chat_types: vec![ChatType::Direct],
                ..Default::default()
            },
            id,
        }
    }
}

impl Default for EchoChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelPlugin for EchoChannel {
    fn id(&self) -> &ChannelId {
        &self.id
    }

    fn meta(&self) -> &ChannelMeta {
        &self.meta
    }

    fn capabilities(&self) -> &ChannelCapabilities {
        &self.capabilities
    }

    fn list_account_ids(&self, _config: &OpenAlpacaConfig) -> Vec<AccountId> {
        vec![AccountId::new_unchecked("default")]
    }

    fn is_account_enabled(&self, _config: &OpenAlpacaConfig, _account_id: &AccountId) -> bool {
        true
    }

    async fn send_text(&self, ctx: &OutboundContext) -> Result<OutboundResult, ChannelError> {
        tracing::info!(
            target = %ctx.target,
            text = %ctx.text,
            "echo: reflecting message"
        );
        Ok(OutboundResult {
            message_id: Some(format!("echo-{}", ctx.target)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_channel_identity() {
        let echo = EchoChannel::new();
        assert_eq!(echo.id().as_str(), "echo");
        assert_eq!(echo.meta().label, "Echo");
        assert_eq!(echo.capabilities().chat_types, vec![ChatType::Direct]);
    }

    #[test]
    fn test_echo_channel_accounts() {
        let echo = EchoChannel::new();
        let config = OpenAlpacaConfig::default();
        let accounts = echo.list_account_ids(&config);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].as_str(), "default");
        assert!(echo.is_account_enabled(&config, &AccountId::new_unchecked("default")));
    }

    #[tokio::test]
    async fn test_echo_channel_send() {
        let echo = EchoChannel::new();
        let ctx = OutboundContext {
            channel_id: ChannelId::new_unchecked("echo"),
            account_id: AccountId::new_unchecked("default"),
            target: "user123".into(),
            text: "hello world".into(),
            reply_to_id: None,
            thread_id: None,
        };
        let result = echo.send_text(&ctx).await.unwrap();
        assert_eq!(result.message_id, Some("echo-user123".to_string()));
    }

    #[test]
    fn test_echo_channel_is_object_safe() {
        // Verify the trait can be used as a trait object
        let echo = EchoChannel::new();
        let _boxed: Box<dyn ChannelPlugin> = Box::new(echo);
    }

    #[test]
    fn test_echo_delivery_mode_default() {
        use crate::plugin::DeliveryMode;
        let echo = EchoChannel::new();
        assert_eq!(echo.delivery_mode(), DeliveryMode::Direct);
    }

    #[test]
    fn test_echo_text_chunk_limit_default() {
        let echo = EchoChannel::new();
        assert_eq!(echo.text_chunk_limit(), None);
    }

    #[tokio::test]
    async fn test_echo_send_media_not_supported() {
        use crate::plugin::MediaOutboundContext;
        let echo = EchoChannel::new();
        let ctx = MediaOutboundContext {
            channel_id: ChannelId::new_unchecked("echo"),
            account_id: AccountId::new_unchecked("default"),
            target: "user123".into(),
            media_url: "https://example.com/image.png".into(),
            caption: None,
            reply_to_id: None,
            thread_id: None,
        };
        let result = echo.send_media(&ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_echo_resolve_reply_to_mode_default() {
        use crate::plugin::ReplyToMode;
        let echo = EchoChannel::new();
        let config = OpenAlpacaConfig::default();
        assert_eq!(
            echo.resolve_reply_to_mode(&config, None, None),
            ReplyToMode::Off
        );
    }

    #[test]
    fn test_echo_mention_strip_patterns_empty() {
        let echo = EchoChannel::new();
        assert!(echo.mention_strip_patterns().is_empty());
    }

    #[test]
    fn test_echo_resolve_require_mention_default() {
        let echo = EchoChannel::new();
        let config = OpenAlpacaConfig::default();
        assert_eq!(echo.resolve_require_mention(&config, None, None), None);
    }

    #[test]
    fn test_echo_normalize_target() {
        let echo = EchoChannel::new();
        assert_eq!(echo.normalize_target("user123"), Some("user123".into()));
        assert_eq!(echo.normalize_target("  spaced  "), Some("spaced".into()));
        assert_eq!(echo.normalize_target(""), None);
        assert_eq!(echo.normalize_target("   "), None);
    }

    #[tokio::test]
    async fn test_echo_check_ready_default() {
        let echo = EchoChannel::new();
        let config = OpenAlpacaConfig::default();
        let result = echo
            .check_ready(&config, &AccountId::new_unchecked("default"))
            .await
            .unwrap();
        assert!(result);
    }

    #[test]
    fn test_echo_collect_status_issues_default() {
        let echo = EchoChannel::new();
        assert!(echo.collect_status_issues(&[]).is_empty());
    }

    #[test]
    fn test_echo_resolve_default_to_default() {
        let echo = EchoChannel::new();
        let config = OpenAlpacaConfig::default();
        assert_eq!(
            echo.resolve_default_to(&config, &AccountId::new_unchecked("default")),
            None
        );
    }

    #[tokio::test]
    async fn test_echo_new_methods_via_trait_object() {
        use crate::plugin::{DeliveryMode, MediaOutboundContext, ReplyToMode};
        let echo = EchoChannel::new();
        let boxed: Box<dyn ChannelPlugin> = Box::new(echo);
        let config = OpenAlpacaConfig::default();
        let aid = AccountId::new_unchecked("default");

        assert_eq!(boxed.delivery_mode(), DeliveryMode::Direct);
        assert_eq!(boxed.text_chunk_limit(), None);
        assert_eq!(
            boxed.resolve_reply_to_mode(&config, None, None),
            ReplyToMode::Off
        );
        assert!(boxed.mention_strip_patterns().is_empty());
        assert_eq!(boxed.resolve_require_mention(&config, None, None), None);
        assert_eq!(boxed.normalize_target("test"), Some("test".into()));
        assert!(boxed.check_ready(&config, &aid).await.unwrap());
        assert!(boxed.collect_status_issues(&[]).is_empty());
        assert_eq!(boxed.resolve_default_to(&config, &aid), None);

        let media_ctx = MediaOutboundContext {
            channel_id: ChannelId::new_unchecked("echo"),
            account_id: aid,
            target: "user".into(),
            media_url: "https://example.com/img.png".into(),
            caption: None,
            reply_to_id: None,
            thread_id: None,
        };
        assert!(boxed.send_media(&media_ctx).await.is_err());
    }
}
