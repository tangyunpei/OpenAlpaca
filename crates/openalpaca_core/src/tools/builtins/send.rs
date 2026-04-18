use super::ConnectorSendLock;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct SendTool {
    provider: ConnectorSendLock,
}

impl SendTool {
    fn acquire_provider(
        &self,
    ) -> Result<Arc<dyn crate::orchestrator::ConnectorSendProvider>, String> {
        let guard = self
            .provider
            .read()
            .map_err(|_| "Failed to read connector send provider".to_string())?;
        guard.clone().ok_or_else(|| {
            "No connector send provider configured. External messaging is not available.".to_string()
        })
    }
}

#[async_trait]
impl BuiltInTool for SendTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let action = arguments
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: action".to_string())?;

        let channel = arguments
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: channel".to_string())?;

        let recipient = arguments
            .get("recipient")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: recipient".to_string())?;

        // Validate action-specific parameters before acquiring the provider.
        match action {
            "message" => {
                let content = arguments
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        "Missing required parameter: content (required for action=\"message\")"
                            .to_string()
                    })?;

                let provider = self.acquire_provider()?;
                provider.send_message(channel, recipient, content).await
            }
            "file" => {
                let file_path = arguments
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        "Missing required parameter: file_path (required for action=\"file\")"
                            .to_string()
                    })?;
                let filename = arguments
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        "Missing required parameter: filename (required for action=\"file\")"
                            .to_string()
                    })?;
                let mime_type = arguments
                    .get("mime_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream");
                let caption = arguments
                    .get("caption")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                let provider = self.acquire_provider()?;
                if !provider
                    .file_capable_channels()
                    .contains(&channel.to_string())
                {
                    return Err(format!(
                        "Channel '{}' does not support file sending. File-capable channels: {:?}",
                        channel,
                        provider.file_capable_channels()
                    ));
                }
                provider
                    .send_file(
                        channel,
                        recipient,
                        file_path,
                        filename,
                        mime_type,
                        caption.as_deref(),
                    )
                    .await
            }
            other => Err(format!(
                "Invalid action '{}'. Must be \"message\" or \"file\".",
                other
            )),
        }
    }
}

pub(super) fn send_tool(provider: ConnectorSendLock) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "send".to_string(),
            description: "Send a message or file to a contact via a connected external channel \
                (Telegram, iMessage, Discord). For messages: provide action='message' \
                with content text. For files: provide action='file' with file_path, \
                filename, and optional mime_type/caption. Use recipient='default' for \
                the most recent conversation, or specify a channel-specific identifier \
                (chat_id for Telegram, phone/email for iMessage, channel_id for \
                Discord). Returns confirmation on success."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["message", "file"],
                        "description": "Action type: 'message' to send text, 'file' to send a file"
                    },
                    "channel": {
                        "type": "string",
                        "description": "Channel identifier: 'telegram', 'imessage', or 'discord'"
                    },
                    "recipient": {
                        "type": "string",
                        "description": "'default' for most recent conversation, or specific: chat_id (Telegram), phone/email (iMessage), channel_id (Discord)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Message text (required for action='message')"
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file on disk (required for action='file')"
                    },
                    "filename": {
                        "type": "string",
                        "description": "Display filename for the recipient (required for action='file')"
                    },
                    "mime_type": {
                        "type": "string",
                        "description": "MIME type of the file, e.g. 'application/pdf' (defaults to 'application/octet-stream')"
                    },
                    "caption": {
                        "type": "string",
                        "description": "Optional caption text to accompany the file"
                    }
                },
                "required": ["action", "channel", "recipient"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({
                    "action": "message",
                    "channel": "telegram",
                    "recipient": "default",
                    "content": "Task completed successfully!"
                }),
                serde_json::json!({
                    "action": "file",
                    "channel": "telegram",
                    "recipient": "default",
                    "file_path": "/workspace/report.pdf",
                    "filename": "report.pdf",
                    "mime_type": "application/pdf",
                    "caption": "Here is the analysis report"
                }),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(SendTool { provider })),
        provides_capabilities: vec!["messaging".into()],
        exempt_from_timeout: false,
        annotations: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock;

    fn make_tool() -> SendTool {
        SendTool {
            provider: Arc::new(RwLock::new(None)),
        }
    }

    #[tokio::test]
    async fn message_no_provider_returns_clear_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "message",
                "channel": "telegram",
                "recipient": "default",
                "content": "hello"
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "Should reach provider check: {err}"
        );
    }

    #[tokio::test]
    async fn file_no_provider_returns_clear_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "file",
                "channel": "telegram",
                "recipient": "default",
                "file_path": "/tmp/test.pdf",
                "filename": "test.pdf"
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "Should reach provider check: {err}"
        );
    }

    #[tokio::test]
    async fn missing_action_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "channel": "telegram",
                "recipient": "default",
                "content": "hello"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter: action"));
    }

    #[tokio::test]
    async fn invalid_action_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "invalid",
                "channel": "telegram",
                "recipient": "default"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid action"));
    }

    #[tokio::test]
    async fn message_missing_content_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "message",
                "channel": "telegram",
                "recipient": "default"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("content"));
    }

    #[tokio::test]
    async fn file_missing_file_path_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "file",
                "channel": "telegram",
                "recipient": "default",
                "filename": "test.pdf"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("file_path"));
    }

    #[tokio::test]
    async fn file_missing_filename_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "file",
                "channel": "telegram",
                "recipient": "default",
                "file_path": "/tmp/test.pdf"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("filename"));
    }

    #[tokio::test]
    async fn telegram_default_passes_validation() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "message",
                "channel": "telegram",
                "recipient": "default",
                "content": "hello"
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "Should reach provider check, not pre-validation: {err}"
        );
    }

    #[tokio::test]
    async fn imessage_at_user_passes_validation() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "message",
                "channel": "imessage",
                "recipient": "@user",
                "content": "hello"
            }))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "iMessage @user should pass validation: {err}"
        );
    }
}
