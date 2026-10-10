use super::ConnectorSendLock;
use super::annotations_for_builtin;
use super::helpers::{
    FolderRules, StoreAccess, is_store_owned_path, is_store_owned_resolved, resolve_workspace_path,
};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::path::PathBuf;
use std::sync::Arc;

struct SendTool {
    provider: ConnectorSendLock,
    /// The folder a relative `file_path` is resolved under (R23, storage
    /// design §11): the file tools' root, the daemon's starting directory,
    /// until S3 moves both to the turn's working folder.
    workspace_root: PathBuf,
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

    /// R23: the absolute path of the file `file_path` names, resolved as
    /// `file_read` resolves one — relative, no `..`, every component (the
    /// last included) canonicalized and under the root — and refused, before
    /// and after resolution, inside the store's `state/`, as `file_read`
    /// refuses it until S3 takes both tools' store rules away with the root
    /// (R33). This path, not the argument, is what the connector is handed.
    fn resolve(&self, file_path: &str) -> Result<String, String> {
        let refuse = |what: &str| {
            format!(
                "Sending '{file_path}' is refused: it is inside {what}, which belongs to the \
                 daemon's own state/ and is not workspace content."
            )
        };
        if let Some(what) = is_store_owned_path(file_path, StoreAccess::Read) {
            return Err(refuse(what));
        }
        let resolved = resolve_workspace_path(file_path, &self.workspace_root, FolderRules::Plain)?;
        if let Some(what) =
            is_store_owned_resolved(&resolved, &self.workspace_root, StoreAccess::Read)
        {
            return Err(refuse(what));
        }
        let metadata = std::fs::metadata(&resolved)
            .map_err(|e| format!("Cannot access file '{file_path}': {e}"))?;
        if !metadata.is_file() {
            return Err(format!("'{file_path}' is not a file."));
        }
        resolved.into_os_string().into_string().map_err(|_| {
            format!(
                "'{file_path}' resolves to a path that is not UTF-8 text, which no channel can send."
            )
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
                // R23: resolved before anything is handed on, so a path that
                // leaves the folder is refused before the provider is asked.
                let file = self.resolve(file_path)?;

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
                        &file,
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

pub(super) fn send_tool(provider: ConnectorSendLock, workspace_root: PathBuf) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "send".to_string(),
            description: "Send a message or file to a contact via a connected external channel \
                (Telegram, iMessage, Discord). For messages: provide action='message' \
                with content text. For files: provide action='file' with file_path (a file \
                relative to OpenAlpaca's folder), filename, and optional mime_type/caption. \
                Use recipient='default' for the most recent conversation, or specify a \
                channel-specific identifier (chat_id for Telegram, phone/email for iMessage, \
                channel_id for Discord). Returns confirmation on success."
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
                        "description": "A file to send, relative to OpenAlpaca's folder (e.g. 'reports/q3.pdf'); absolute paths and '..' are refused (required for action='file')"
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
                    "file_path": "reports/q3.pdf",
                    "filename": "q3.pdf",
                    "mime_type": "application/pdf",
                    "caption": "Here is the analysis report"
                }),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(SendTool {
            provider,
            workspace_root,
        })),
        provides_capabilities: vec!["messaging".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("send"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::ConnectorSendProvider;
    use std::path::Path;
    use std::sync::{Mutex, RwLock};

    /// A root nothing resolves under: the validation tests never reach a
    /// file, and no test may touch the repository.
    const NO_ROOT: &str = "/nonexistent/openalpaca-send-tests";

    fn make_tool() -> SendTool {
        SendTool {
            provider: Arc::new(RwLock::new(None)),
            workspace_root: PathBuf::from(NO_ROOT),
        }
    }

    /// A connector that keeps every file it was handed: (path, filename,
    /// mime type).
    #[derive(Default)]
    struct RecordingSender {
        files: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait]
    impl ConnectorSendProvider for RecordingSender {
        async fn send_message(&self, _: &str, _: &str, _: &str) -> Result<String, String> {
            Ok("sent".to_string())
        }
        fn sendable_channels(&self) -> Vec<String> {
            vec!["telegram".to_string()]
        }
        async fn send_file(
            &self,
            _channel: &str,
            _recipient: &str,
            file_path: &str,
            filename: &str,
            mime_type: &str,
            _caption: Option<&str>,
        ) -> Result<String, String> {
            self.files.lock().unwrap().push((
                file_path.to_string(),
                filename.to_string(),
                mime_type.to_string(),
            ));
            Ok(format!("Sent file '{filename}'"))
        }
        fn file_capable_channels(&self) -> Vec<String> {
            vec!["telegram".to_string()]
        }
    }

    /// `send` over `root`, with `sender` as its connector.
    fn tool_at(root: &Path, sender: &Arc<RecordingSender>) -> SendTool {
        let provider: Arc<dyn ConnectorSendProvider> = sender.clone();
        SendTool {
            provider: Arc::new(RwLock::new(Some(provider))),
            workspace_root: root.to_path_buf(),
        }
    }

    fn file_args(file_path: &str) -> serde_json::Value {
        serde_json::json!({
            "action": "file",
            "channel": "telegram",
            "recipient": "default",
            "file_path": file_path,
            "filename": "q3.pdf"
        })
    }

    /// A folder holding `reports/q3.pdf` (5 bytes).
    fn root_with_report() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("reports")).unwrap();
        std::fs::write(root.path().join("reports/q3.pdf"), b"%PDF-").unwrap();
        root
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
        let root = root_with_report();
        let tool = SendTool {
            provider: Arc::new(RwLock::new(None)),
            workspace_root: root.path().to_path_buf(),
        };
        let err = tool.execute(&file_args("reports/q3.pdf")).await.unwrap_err();
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

    // ── R23: a file is resolved like file_read's (storage design §11) ──

    #[tokio::test]
    async fn a_file_is_sent_by_its_path_resolved_under_the_root() {
        let root = root_with_report();
        let sender = Arc::new(RecordingSender::default());
        let out = tool_at(root.path(), &sender)
            .execute(&file_args("reports/q3.pdf"))
            .await;
        assert_eq!(out.as_deref(), Ok("Sent file 'q3.pdf'"));
        let sent = root.path().canonicalize().unwrap().join("reports/q3.pdf");
        assert_eq!(
            *sender.files.lock().unwrap(),
            vec![(
                sent.to_str().unwrap().to_string(),
                "q3.pdf".to_string(),
                "application/octet-stream".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn a_path_that_leaves_the_folder_is_refused_before_the_provider() {
        let root = root_with_report();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "not yours").unwrap();
        let absolute = outside.path().join("secret.txt");
        let sender = Arc::new(RecordingSender::default());
        let tool = tool_at(root.path(), &sender);
        for (path, refusal) in [
            (
                absolute.to_str().unwrap(),
                "Absolute paths are not allowed. Use relative paths within the workspace.",
            ),
            ("../secret.txt", "Path traversal ('..') is not allowed."),
            ("reports/../../secret.txt", "Path traversal ('..') is not allowed."),
        ] {
            assert_eq!(tool.execute(&file_args(path)).await.unwrap_err(), refusal, "{path}");
        }
        assert!(sender.files.lock().unwrap().is_empty(), "nothing reached the connector");
    }

    #[tokio::test]
    async fn the_stores_state_folder_is_refused_and_a_folder_is_not_a_file() {
        let root = root_with_report();
        std::fs::create_dir_all(root.path().join("state")).unwrap();
        std::fs::write(root.path().join("state/discovery.json"), "{\"token\":\"t\"}").unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = tool_at(root.path(), &sender);
        assert_eq!(
            tool.execute(&file_args("state/discovery.json")).await.unwrap_err(),
            "Sending 'state/discovery.json' is refused: it is inside the store's `state/` \
             directory, which belongs to the daemon's own state/ and is not workspace content."
        );
        assert_eq!(
            tool.execute(&file_args("reports")).await.unwrap_err(),
            "'reports' is not a file."
        );
        assert!(sender.files.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn links_out_of_the_folder_or_into_state_are_refused_after_resolution() {
        let root = root_with_report();
        std::fs::create_dir_all(root.path().join("state")).unwrap();
        std::fs::write(root.path().join("state/discovery.json"), "{}").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "not yours").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join("reports/link.pdf"),
        )
        .unwrap();
        std::os::unix::fs::symlink(root.path().join("state"), root.path().join("reports/s"))
            .unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = tool_at(root.path(), &sender);
        assert_eq!(
            tool.execute(&file_args("reports/link.pdf")).await.unwrap_err(),
            "Path 'reports/link.pdf' resolves outside the workspace boundary"
        );
        assert_eq!(
            tool.execute(&file_args("reports/s/discovery.json")).await.unwrap_err(),
            "Sending 'reports/s/discovery.json' is refused: it is inside the store's `state/` \
             directory, which belongs to the daemon's own state/ and is not workspace content."
        );
        assert!(sender.files.lock().unwrap().is_empty());
    }
}
