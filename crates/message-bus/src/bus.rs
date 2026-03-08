use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::sync::RwLock;

use openalpaca_channels::{
    ChannelError, ChannelRegistry, InboundHandler, InboundMessage, OutboundContext,
};
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::types::{AccountId, ChannelId};
use openalpaca_routing::{ResolveRouteInput, resolve_route};

use crate::dispatcher::AgentDispatcher;

/// Central message bus that connects inbound messages to agent dispatch and outbound delivery.
pub struct MessageBus {
    /// Agent dispatcher (EchoDispatcher for now, real agent in Phase 7)
    dispatcher: Arc<dyn AgentDispatcher>,
    /// Channel registry for outbound delivery
    channels: Arc<RwLock<ChannelRegistry>>,
    /// Config for routing
    config: Arc<ArcSwap<OpenAlpacaConfig>>,
}

impl MessageBus {
    pub fn new(
        dispatcher: Arc<dyn AgentDispatcher>,
        channels: Arc<RwLock<ChannelRegistry>>,
        config: Arc<ArcSwap<OpenAlpacaConfig>>,
    ) -> Self {
        Self {
            dispatcher,
            channels,
            config,
        }
    }
}

#[async_trait]
impl InboundHandler for MessageBus {
    async fn handle_message(
        &self,
        channel: &ChannelId,
        account_id: &AccountId,
        message: &InboundMessage,
    ) -> Result<String, ChannelError> {
        let config_guard = self.config.load();

        // Step 1: Resolve route
        let input = ResolveRouteInput {
            config: &config_guard,
            channel,
            account_id: Some(account_id.as_str()),
            peer: None,
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };

        let route = resolve_route(&input)
            .map_err(|e| ChannelError::Other(format!("route resolution failed: {e}")))?;

        tracing::debug!(
            channel = %channel.as_str(),
            agent = %route.agent_id.as_str(),
            session = %route.session_key.as_str(),
            "message-bus: resolved route"
        );

        // Step 2: Dispatch to agent
        let result = self
            .dispatcher
            .dispatch(channel, message, &route)
            .await
            .map_err(|e| ChannelError::Other(format!("agent dispatch failed: {e}")))?;

        // Step 3: Deliver outbound via channel
        let registry = self.channels.read().await;
        if let Some(plugin) = registry.get(&result.channel_id, account_id) {
            let ctx = OutboundContext {
                channel_id: result.channel_id.clone(),
                account_id: account_id.clone(),
                target: result.target.clone(),
                text: result.text.clone(),
                reply_to_id: result.reply_to_id.clone(),
                thread_id: result.thread_id.clone(),
            };

            match plugin.send_text(&ctx).await {
                Ok(_) => {
                    tracing::debug!(
                        channel = %result.channel_id.as_str(),
                        "message-bus: delivered outbound"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        channel = %result.channel_id.as_str(),
                        error = %e,
                        "message-bus: outbound delivery failed"
                    );
                }
            }
        } else {
            tracing::warn!(
                channel = %result.channel_id.as_str(),
                account = %account_id.as_str(),
                "message-bus: no channel plugin found for outbound delivery"
            );
        }

        // Return the text for backward compatibility with InboundHandler signature
        Ok(result.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::EchoDispatcher;
    use openalpaca_config::types_agents::{AgentConfig, AgentsConfig};
    use openalpaca_core::types::ChatType;

    fn make_test_config() -> Arc<ArcSwap<OpenAlpacaConfig>> {
        // Need at least one agent for route resolution to succeed
        let config = OpenAlpacaConfig {
            agents: Some(AgentsConfig {
                list: Some(vec![AgentConfig {
                    id: "assistant".into(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        Arc::new(ArcSwap::new(Arc::new(config)))
    }

    fn make_test_message() -> InboundMessage {
        InboundMessage {
            text: "hello".into(),
            sender_id: "user1".into(),
            sender_name: Some("User One".into()),
            chat_id: "chat1".into(),
            chat_type: ChatType::Direct,
            message_id: "msg1".into(),
            thread_id: None,
            reply_to_message_id: None,
            media_urls: vec![],
        }
    }

    #[tokio::test]
    async fn test_message_bus_echo_dispatch() {
        let dispatcher = Arc::new(EchoDispatcher);
        let channels = Arc::new(RwLock::new(ChannelRegistry::new()));
        let config = make_test_config();

        let bus = MessageBus::new(dispatcher, channels, config);
        let channel = ChannelId::new_unchecked("test");
        let account = AccountId::new_unchecked("default");
        let msg = make_test_message();

        let reply = bus.handle_message(&channel, &account, &msg).await.unwrap();
        assert_eq!(reply, "Echo: hello");
    }

    #[tokio::test]
    async fn test_message_bus_routing_failure() {
        let dispatcher = Arc::new(EchoDispatcher);
        let channels = Arc::new(RwLock::new(ChannelRegistry::new()));
        // Empty config with no agents — routing will fail
        let config = Arc::new(ArcSwap::new(Arc::new(OpenAlpacaConfig::default())));

        let bus = MessageBus::new(dispatcher, channels, config);
        let channel = ChannelId::new_unchecked("test");
        let account = AccountId::new_unchecked("default");
        let msg = make_test_message();

        let result = bus.handle_message(&channel, &account, &msg).await;
        assert!(result.is_err());
    }
}
