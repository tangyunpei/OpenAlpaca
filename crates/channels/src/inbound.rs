use async_trait::async_trait;

use openalpaca_core::types::{AccountId, ChannelId, ChatType};

use crate::error::ChannelError;

/// A message received from an external messaging platform.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub text: String,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub chat_id: String,
    pub chat_type: ChatType,
    pub message_id: String,
    pub thread_id: Option<String>,
    pub reply_to_message_id: Option<String>,
    pub media_urls: Vec<String>,
}

/// Callback trait for handling inbound messages.
///
/// Since the agent runtime isn't available yet (Phase 6),
/// channels use this trait to dispatch inbound messages.
/// The `EchoHandler` implementation is provided for testing.
#[async_trait]
pub trait InboundHandler: Send + Sync {
    async fn handle_message(
        &self,
        channel: &ChannelId,
        account_id: &AccountId,
        message: &InboundMessage,
    ) -> Result<String, ChannelError>;
}

/// Echo handler for testing — returns "Echo: {text}".
pub struct EchoHandler;

#[async_trait]
impl InboundHandler for EchoHandler {
    async fn handle_message(
        &self,
        _channel: &ChannelId,
        _account_id: &AccountId,
        message: &InboundMessage,
    ) -> Result<String, ChannelError> {
        Ok(format!("Echo: {}", message.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_echo_handler() {
        let handler = EchoHandler;
        let msg = InboundMessage {
            text: "hello".into(),
            sender_id: "user1".into(),
            sender_name: None,
            chat_id: "chat1".into(),
            chat_type: ChatType::Direct,
            message_id: "1".into(),
            thread_id: None,
            reply_to_message_id: None,
            media_urls: vec![],
        };
        let channel = ChannelId::new_unchecked("test");
        let account = AccountId::new_unchecked("acc");
        let reply = handler
            .handle_message(&channel, &account, &msg)
            .await
            .unwrap();
        assert_eq!(reply, "Echo: hello");
    }
}
