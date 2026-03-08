use async_trait::async_trait;

use openalpaca_channels::InboundMessage;
use openalpaca_core::types::ChannelId;
use openalpaca_routing::ResolvedRoute;

use crate::error::MessageBusError;

/// Result of dispatching a message to an agent.
#[derive(Debug, Clone)]
pub struct DispatchResult {
    /// The reply text from the agent.
    pub text: String,
    /// Target channel for the reply (usually same as inbound).
    pub channel_id: ChannelId,
    /// Target chat/user to send reply to.
    pub target: String,
    /// Thread ID for threaded replies.
    pub thread_id: Option<String>,
    /// Reply-to message ID.
    pub reply_to_id: Option<String>,
}

/// Trait for dispatching inbound messages to an agent.
///
/// Phase 7 will provide the real agent runtime implementation.
/// For now, EchoDispatcher is provided for testing.
#[async_trait]
pub trait AgentDispatcher: Send + Sync {
    /// Dispatch a message to the appropriate agent and return the reply.
    async fn dispatch(
        &self,
        channel_id: &ChannelId,
        message: &InboundMessage,
        route: &ResolvedRoute,
    ) -> Result<DispatchResult, MessageBusError>;
}

/// Echo dispatcher — returns "Echo: {text}" for testing.
pub struct EchoDispatcher;

#[async_trait]
impl AgentDispatcher for EchoDispatcher {
    async fn dispatch(
        &self,
        channel_id: &ChannelId,
        message: &InboundMessage,
        _route: &ResolvedRoute,
    ) -> Result<DispatchResult, MessageBusError> {
        Ok(DispatchResult {
            text: format!("Echo: {}", message.text),
            channel_id: channel_id.clone(),
            target: message.chat_id.clone(),
            thread_id: message.thread_id.clone(),
            reply_to_id: Some(message.message_id.clone()),
        })
    }
}
