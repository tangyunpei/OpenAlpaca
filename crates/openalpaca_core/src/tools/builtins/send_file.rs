use super::ConnectorSendLock;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct SendFileTool {
    provider: ConnectorSendLock,
}

#[async_trait]
impl BuiltInTool for SendFileTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let channel = arguments
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: channel".to_string())?;

        let recipient = arguments
            .get("recipient")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: recipient".to_string())?;

        let file_path = arguments
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: file_path".to_string())?;

        let filename = arguments
            .get("filename")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: filename".to_string())?;

        let mime_type = arguments
            .get("mime_type")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream");

        let caption = arguments
            .get("caption")
            .and_then(|v| v.as_str())
            .map(String::from);

        let provider = {
            let guard = self
                .provider
                .read()
                .map_err(|_| "Failed to read connector send provider".to_string())?;
            guard.clone()
        };

        let provider = provider.ok_or_else(|| {
            "No connector send provider configured. File sending is not available.".to_string()
        })?;

        // Verify the channel supports file sending
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
}

pub(super) fn send_file_tool(provider: ConnectorSendLock) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "send_file".to_string(),
            description: "Send a file to a contact via a file-capable channel. \
                Supported channels: telegram, imessage (macOS), discord. \
                Supported types: images, documents, archives, audio, video."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "Channel identifier: \"telegram\", \"imessage\", or \"discord\""
                    },
                    "recipient": {
                        "type": "string",
                        "description": "\"default\" for most recent conversation, or specific: chat_id (Telegram), phone/email (iMessage), channel_id (Discord)."
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file on disk"
                    },
                    "filename": {
                        "type": "string",
                        "description": "Display filename for the recipient"
                    },
                    "mime_type": {
                        "type": "string",
                        "description": "MIME type of the file (e.g. \"application/pdf\", \"image/png\")"
                    },
                    "caption": {
                        "type": "string",
                        "description": "Optional caption text to accompany the file"
                    }
                },
                "required": ["channel", "recipient", "file_path", "filename"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(SendFileTool { provider })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock;

    fn make_tool() -> SendFileTool {
        SendFileTool {
            provider: Arc::new(RwLock::new(None)),
        }
    }

    fn args(
        channel: &str,
        recipient: &str,
        file_path: &str,
        filename: &str,
        mime_type: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "channel": channel,
            "recipient": recipient,
            "file_path": file_path,
            "filename": filename,
            "mime_type": mime_type,
        })
    }

    #[tokio::test]
    async fn no_provider_returns_clear_error() {
        let tool = make_tool();
        let result = tool
            .execute(&args(
                "telegram",
                "default",
                "/tmp/test.pdf",
                "test.pdf",
                "application/pdf",
            ))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("No connector send provider"),
            "Should reach provider check: {err}"
        );
    }

    #[tokio::test]
    async fn missing_channel_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "recipient": "default",
                "file_path": "/tmp/test.pdf",
                "filename": "test.pdf",
                "mime_type": "application/pdf",
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter: channel"));
    }

    #[tokio::test]
    async fn missing_file_path_returns_error() {
        let tool = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "channel": "telegram",
                "recipient": "default",
                "filename": "test.pdf",
                "mime_type": "application/pdf",
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter: file_path"));
    }
}
