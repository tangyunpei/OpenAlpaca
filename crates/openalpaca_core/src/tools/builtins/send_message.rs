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

        // Pre-validate Telegram recipient format before bridge call
        if channel == "telegram" && recipient != "default" {
            if recipient.starts_with('@') {
                return Err(format!(
                    "Telegram @username ('{}') is not directly sendable. \
                     The Bot API requires a numeric chat_id. \
                     Use \"default\" to send to the user's most recent conversation.",
                    recipient
                ));
            }
            if recipient.parse::<i64>().is_err() {
                return Err(format!(
                    "Invalid Telegram recipient '{}'. \
                     Expected \"default\" or a numeric chat_id (e.g. 123456789).",
                    recipient
                ));
            }
        }

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
            description: "Send a message via a connected channel (iMessage on macOS via AppleScript, Telegram via Bot API). \
                Call this tool directly — use recipient=\"default\" for the user's most recent conversation."
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
                        "description": "Recipient. Use \"default\" (RECOMMENDED) to send to the user's most recent conversation. Only use a specific identifier if the user explicitly provides one: numeric chat_id for Telegram, phone/email for iMessage."
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock;

    fn make_tool() -> SendMessageTool {
        SendMessageTool {
            provider: Arc::new(RwLock::new(None)),
        }
    }

    fn args(channel: &str, recipient: &str, content: &str) -> serde_json::Value {
        serde_json::json!({
            "channel": channel,
            "recipient": recipient,
            "content": content,
        })
    }

    #[tokio::test]
    async fn telegram_at_username_rejected() {
        let tool = make_tool();
        let result = tool.execute(&args("telegram", "@user", "hello")).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("@username"), "Error should mention @username: {err}");
        assert!(err.contains("chat_id"), "Error should suggest chat_id: {err}");
    }

    #[tokio::test]
    async fn telegram_non_numeric_rejected() {
        let tool = make_tool();
        let result = tool.execute(&args("telegram", "abc", "hello")).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("numeric chat_id"), "Error should mention numeric: {err}");
    }

    #[tokio::test]
    async fn telegram_default_passes_validation() {
        let tool = make_tool();
        let result = tool.execute(&args("telegram", "default", "hello")).await;
        // Should pass pre-validation and fail at provider (None provider)
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "Should reach provider check, not pre-validation: {err}"
        );
    }

    #[tokio::test]
    async fn telegram_numeric_passes_validation() {
        let tool = make_tool();
        let result = tool.execute(&args("telegram", "12345", "hello")).await;
        // Should pass pre-validation and fail at provider
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "Should reach provider check: {err}"
        );
    }

    #[tokio::test]
    async fn imessage_at_user_passes_validation() {
        // @username is valid for iMessage (it's an email-like identifier)
        let tool = make_tool();
        let result = tool.execute(&args("imessage", "@user", "hello")).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "iMessage @user should pass validation: {err}"
        );
    }
}
