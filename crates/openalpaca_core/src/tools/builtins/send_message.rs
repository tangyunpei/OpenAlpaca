use super::ConnectorSendLock;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct SendMessageTool {
    provider: ConnectorSendLock,
}

#[async_trait]
impl BuiltInTool for SendMessageTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let channel = arguments
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: channel".to_string())?;

        let recipient = arguments
            .get("recipient")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: recipient".to_string())?;

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        // Read the provider from the shared lock
        let provider = {
            let guard = self
                .provider
                .read()
                .map_err(|_| "Failed to read connector send provider".to_string())?;
            guard.clone()
        };

        let provider = provider.ok_or_else(|| {
            "No connector send provider configured. External messaging is not available.".to_string()
        })?;

        provider.send_message(channel, recipient, content).await
    }
}

pub(super) fn send_message_tool(provider: ConnectorSendLock) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "send_message".to_string(),
            description: "Send a message to a contact via an external communication channel \
                (iMessage, Telegram, etc.). Use this when the user asks to send, forward, \
                or relay a message to someone through a connected platform."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel identifier: \"telegram\" or \"imessage\""
                    },
                    "recipient": {
                        "type": "string",
                        "description": "Recipient identifier — phone/email for iMessage, chat_id for Telegram. Use \"default\" to send to the user's most recent conversation on that channel."
                    },
                    "content": {
                        "type": "string",
                        "description": "Message text to send"
                    }
                },
                "required": ["channel", "recipient", "content"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(SendMessageTool { provider })),
    }
}
