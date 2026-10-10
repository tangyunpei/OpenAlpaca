use super::ConnectorSendLock;
use super::annotations_for_builtin;
use super::helpers::{
    FolderRules, StoreAccess, is_store_owned_path, is_store_owned_resolved, resolve_workspace_path,
};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use crate::utils::text::{escape_hidden_line, hidden_notice};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::repository::TaskRepository;
use openalpaca_storage::{ArtifactStore, Database};
use std::path::PathBuf;
use std::sync::Arc;

/// The tail of a refusal for an argument only a file send needs.
const FILE_ONLY: &str = " (required for action=\"file\")";

struct SendTool {
    provider: ConnectorSendLock,
    /// The artifact store an `artifact_id` is looked up in (R23). Absent
    /// (tests, the CLI) refuses an artifact id at call time.
    db: Option<Database>,
    /// The folder a relative `file_path` is resolved under (R23, storage
    /// design §11): the file tools' root, the daemon's starting directory,
    /// until S3 moves both to the turn's working folder.
    workspace_root: PathBuf,
}

/// The file a `send` with action `file` delivers (R23).
struct SendFile {
    /// Its resolved absolute path: what the card names and the connector is
    /// handed.
    path: String,
    /// Its size, for the card.
    bytes: u64,
    /// A by-product's own MIME type, used when the call names none.
    mime_type: Option<String>,
}

/// A string argument the call cannot go without, refused in one wording for
/// the card and for the send.
fn required<'a>(
    arguments: &'a serde_json::Value,
    key: &str,
    note: &str,
) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing required parameter: {key}{note}"))
}

/// Whose by-products a call may send: the turn's owner, else, on a run's
/// context that threaded none, the run's `created_by`, as `artifact_write`
/// attributes what it saves (`put_tool_artifact`).
fn turn_owner(db: &Database, ctx: &ToolContext) -> Result<String, String> {
    if let Some(owner) = ctx.owner_id.as_deref() {
        return Ok(owner.to_string());
    }
    let task = match ctx.task_id.as_deref() {
        Some(task_id) => TaskRepository::new(db)
            .get(task_id)
            .map_err(|e| format!("Failed to load task: {e}"))?,
        None => None,
    };
    task.map(|task| task.created_by).ok_or_else(|| {
        "send cannot send an artifact on this turn: it has no owner context (it was not sent \
         on a person's behalf), and every artifact belongs to an owner."
            .to_string()
    })
}

/// R23's card (interfaces §4.3): the file as resolved, its size, where it
/// goes and under what name. Each value is shown escaped, because a model or
/// a file on disk chose it (Gotchas). The count of what was escaped goes on
/// its own line.
fn file_card(file: &SendFile, channel: &str, recipient: &str, filename: &str) -> String {
    let path = escape_hidden_line(&file.path);
    let channel = escape_hidden_line(channel);
    let recipient = escape_hidden_line(recipient);
    let filename = escape_hidden_line(filename);
    let mut card = format!(
        "Sends {} ({} bytes) to {} recipient {} as {}.",
        path.text, file.bytes, channel.text, recipient.text, filename.text
    );
    if let Some(notice) =
        hidden_notice(path.hidden + channel.hidden + recipient.hidden + filename.hidden)
    {
        card.push('\n');
        card.push_str(&notice);
    }
    card
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

    /// R23: the file a `send` with action `file` delivers, named by
    /// `file_path` or by `artifact_id` (exactly one). The card and the send
    /// locate it the same way, so the card names what is sent.
    fn locate(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<SendFile, String> {
        let file_path = arguments.get("file_path").and_then(|v| v.as_str());
        let artifact_id = arguments.get("artifact_id").and_then(|v| v.as_str());
        match (file_path, artifact_id) {
            (Some(_), Some(_)) => Err("Give file_path or artifact_id, not both.".to_string()),
            (None, None) => Err(format!(
                "Missing required parameter: file_path or artifact_id{FILE_ONLY}"
            )),
            (Some(file_path), None) => self.resolve(file_path),
            (None, Some(id)) => self.artifact(id, ctx),
        }
    }

    /// R23: the file `file_path` names, resolved as `file_read` resolves one
    /// — relative, no `..`, every component (the last included) canonicalized
    /// and under the root — and refused, before and after resolution, inside
    /// the store's `state/`, as `file_read` refuses it until S3 takes both
    /// tools' store rules away with the root (R33).
    fn resolve(&self, file_path: &str) -> Result<SendFile, String> {
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
        let path = resolved.into_os_string().into_string().map_err(|_| {
            format!(
                "'{file_path}' resolves to a path that is not UTF-8 text, which no channel can send."
            )
        })?;
        Ok(SendFile {
            path,
            bytes: metadata.len(),
            mime_type: None,
        })
    }

    /// R23: a by-product of the turn's owner, by its id: its head file as the
    /// store recorded it, refused when the bytes are gone. Read only. The
    /// hand-edit check a Library read makes (`resolve_content`) can record a
    /// version, and the card shares this lookup and must write nothing.
    fn artifact(&self, id: &str, ctx: &ToolContext) -> Result<SendFile, String> {
        let db = self.db.as_ref().ok_or_else(|| {
            format!("send cannot look up artifact '{id}' here: there is no database.")
        })?;
        let owner = turn_owner(db, ctx)?;
        let record = ArtifactStore::new(db)
            .get(id, &owner)
            .map_err(|e| format!("Failed to look up artifact '{id}': {e}"))?
            .ok_or_else(|| format!("send: no artifact '{id}' of yours."))?;
        let metadata = std::fs::metadata(&record.storage_path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .ok_or_else(|| {
                format!(
                    "send: the bytes of '{id}' are gone ({}).",
                    record.storage_path
                )
            })?;
        Ok(SendFile {
            path: record.storage_path,
            bytes: metadata.len(),
            mime_type: Some(record.mime_type),
        })
    }
}

#[async_trait]
impl BuiltInTool for SendTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        self.execute_with_context(arguments, &ToolContext::default())
            .await
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let action = required(arguments, "action", "")?;
        let channel = required(arguments, "channel", "")?;
        let recipient = required(arguments, "recipient", "")?;

        // Validate action-specific parameters before acquiring the provider.
        match action {
            "message" => {
                let content =
                    required(arguments, "content", " (required for action=\"message\")")?;
                let provider = self.acquire_provider()?;
                provider.send_message(channel, recipient, content).await
            }
            "file" => {
                let filename = required(arguments, "filename", FILE_ONLY)?;
                let caption = arguments
                    .get("caption")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                // R23: located before anything is handed on, so a path that
                // leaves the folder, or another owner's artifact, is refused
                // before the provider is asked.
                let file = self.locate(arguments, ctx)?;
                let mime_type = arguments
                    .get("mime_type")
                    .and_then(|v| v.as_str())
                    .or(file.mime_type.as_deref())
                    .unwrap_or("application/octet-stream");

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
                        &file.path,
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

    /// R23 (interfaces §4.3): a file send's card names the resolved file, its
    /// size and where it goes. A send that would be refused raises no card. A
    /// message keeps the ordinary card, its text shown as written.
    async fn confirmation_preview(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        if arguments.get("action").and_then(|v| v.as_str()) != Some("file") {
            return Ok(None);
        }
        let channel = required(arguments, "channel", "")?;
        let recipient = required(arguments, "recipient", "")?;
        let filename = required(arguments, "filename", FILE_ONLY)?;
        let file = self.locate(arguments, ctx)?;
        Ok(Some(file_card(&file, channel, recipient, filename)))
    }
}

pub(super) fn send_tool(
    provider: ConnectorSendLock,
    db: Option<Database>,
    workspace_root: PathBuf,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "send".to_string(),
            description: "Send a message or file to a contact via a connected external channel \
                (Telegram, iMessage, Discord). For messages: provide action='message' \
                with content text. For files: provide action='file' with file_path (a file \
                relative to OpenAlpaca's folder) or artifact_id (a by-product you saved, by its \
                id), filename, and optional mime_type/caption. Use recipient='default' for the \
                most recent conversation, or specify a channel-specific identifier (chat_id \
                for Telegram, phone/email for iMessage, channel_id for Discord). Returns \
                confirmation on success."
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
                        "description": "A file to send, relative to OpenAlpaca's folder (e.g. 'reports/q3.pdf'); absolute paths and '..' are refused. Give this or artifact_id (action='file')"
                    },
                    "artifact_id": {
                        "type": "string",
                        "description": "A by-product to send, by its id; give this or file_path"
                    },
                    "filename": {
                        "type": "string",
                        "description": "Display filename for the recipient (required for action='file')"
                    },
                    "mime_type": {
                        "type": "string",
                        "description": "MIME type of the file, e.g. 'application/pdf' (defaults to the artifact's own type, else 'application/octet-stream')"
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
            db,
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
    use openalpaca_storage::store::StoreScope;
    use openalpaca_storage::{ArtifactKind, ArtifactRecord, NewArtifact};
    use std::path::Path;
    use std::sync::{Mutex, RwLock};

    /// A root nothing resolves under: the validation tests never reach a
    /// file, and no test may touch the repository.
    const NO_ROOT: &str = "/nonexistent/openalpaca-send-tests";

    fn make_tool() -> SendTool {
        SendTool {
            provider: Arc::new(RwLock::new(None)),
            db: None,
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
            db: None,
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
            db: None,
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
        assert_eq!(
            result.unwrap_err(),
            "Missing required parameter: file_path or artifact_id (required for action=\"file\")"
        );
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

    // ── R23: a by-product by its id (storage design §11) ──

    const OWNER: &str = "owner-send";

    /// A home store of its own, a database, and one markdown artifact of
    /// `OWNER` in it (`# Report\n`, 9 bytes). `_guard` drops first.
    struct Saved {
        _guard: crate::test_util::HomeStoreGuard,
        _home: tempfile::TempDir,
        _db_dir: tempfile::TempDir,
        db: Database,
        record: ArtifactRecord,
    }

    fn saved() -> Saved {
        let home = tempfile::tempdir().unwrap();
        let guard = crate::test_util::HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        let scope = StoreScope::Home;
        let (record, _) = ArtifactStore::new(&db)
            .put(NewArtifact::new(
                OWNER,
                &scope,
                ArtifactKind::Markdown,
                "report",
                b"# Report\n",
            ))
            .unwrap();
        Saved {
            _guard: guard,
            _home: home,
            _db_dir: db_dir,
            db,
            record,
        }
    }

    fn artifact_tool(saved: &Saved, sender: &Arc<RecordingSender>) -> SendTool {
        let provider: Arc<dyn ConnectorSendProvider> = sender.clone();
        SendTool {
            provider: Arc::new(RwLock::new(Some(provider))),
            db: Some(saved.db.clone()),
            workspace_root: PathBuf::from(NO_ROOT),
        }
    }

    fn artifact_args(id: &str) -> serde_json::Value {
        serde_json::json!({
            "action": "file",
            "channel": "telegram",
            "recipient": "default",
            "artifact_id": id,
            "filename": "report.md"
        })
    }

    fn owner_ctx(owner: &str) -> ToolContext {
        ToolContext {
            owner_id: Some(owner.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_artifact_of_the_turns_owner_is_sent_from_its_stored_file() {
        let saved = saved();
        let sender = Arc::new(RecordingSender::default());
        let out = artifact_tool(&saved, &sender)
            .execute_with_context(&artifact_args(&saved.record.id), &owner_ctx(OWNER))
            .await;
        assert_eq!(out.as_deref(), Ok("Sent file 'report.md'"));
        assert_eq!(
            *sender.files.lock().unwrap(),
            vec![(
                saved.record.storage_path.clone(),
                "report.md".to_string(),
                saved.record.mime_type.clone()
            )],
            "the stored file, with the artifact's own type when the call names none"
        );
    }

    /// A run's context that threaded no owner: the run's creator is the
    /// owner, as `artifact_write` attributes what it saves.
    #[tokio::test]
    async fn a_runs_artifact_is_sent_for_the_person_who_started_it() {
        let saved = saved();
        let now = chrono::Utc::now();
        openalpaca_storage::repository::TaskRepository::new(&saved.db)
            .create(&openalpaca_storage::Task {
                id: "t-send-0001".to_string(),
                title: "Send run".to_string(),
                description: None,
                status: openalpaca_storage::TaskStatus::Queued,
                priority: 0,
                progress_current: None,
                progress_total: None,
                result_summary: None,
                created_by: OWNER.to_string(),
                source_lane: "gui".to_string(),
                created_at: now,
                updated_at: now,
                completed_at: None,
                state_json: None,
                state_version: 0,
                outcome_json: None,
                outcome_kind: None,
                artifact_count: 0,
                workspace_id: None,
                source_task_id: None,
                session_id: None,
                unattended: false,
            })
            .unwrap();
        let sender = Arc::new(RecordingSender::default());
        let ctx = ToolContext {
            task_id: Some("t-send-0001".to_string()),
            ..Default::default()
        };
        let out = artifact_tool(&saved, &sender)
            .execute_with_context(&artifact_args(&saved.record.id), &ctx)
            .await;
        assert_eq!(out.as_deref(), Ok("Sent file 'report.md'"));
    }

    #[tokio::test]
    async fn an_artifact_of_another_owner_is_refused() {
        let saved = saved();
        let sender = Arc::new(RecordingSender::default());
        let err = artifact_tool(&saved, &sender)
            .execute_with_context(&artifact_args(&saved.record.id), &owner_ctx("someone-else"))
            .await
            .unwrap_err();
        assert_eq!(err, format!("send: no artifact '{}' of yours.", saved.record.id));
        assert!(sender.files.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_artifact_whose_bytes_are_gone_is_refused() {
        let saved = saved();
        std::fs::remove_file(&saved.record.storage_path).unwrap();
        let sender = Arc::new(RecordingSender::default());
        let err = artifact_tool(&saved, &sender)
            .execute_with_context(&artifact_args(&saved.record.id), &owner_ctx(OWNER))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            format!(
                "send: the bytes of '{}' are gone ({}).",
                saved.record.id, saved.record.storage_path
            )
        );
        assert!(sender.files.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn file_path_and_artifact_id_together_are_refused() {
        let mut args = file_args("reports/q3.pdf");
        args["artifact_id"] = serde_json::json!("a-1");
        assert_eq!(
            make_tool().execute(&args).await.unwrap_err(),
            "Give file_path or artifact_id, not both."
        );
    }

    #[tokio::test]
    async fn an_artifact_on_a_turn_with_no_owner_is_refused() {
        let saved = saved();
        let sender = Arc::new(RecordingSender::default());
        let err = artifact_tool(&saved, &sender)
            .execute(&artifact_args(&saved.record.id))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "send cannot send an artifact on this turn: it has no owner context (it was not \
             sent on a person's behalf), and every artifact belongs to an owner."
        );
    }

    // ── R23: the card names what is sent (storage design §11, interfaces §4.3) ──

    #[tokio::test]
    async fn a_file_card_names_the_resolved_path_its_size_and_where_it_goes() {
        let root = root_with_report();
        let sender = Arc::new(RecordingSender::default());
        let card = tool_at(root.path(), &sender)
            .confirmation_preview(&file_args("reports/q3.pdf"), &ToolContext::default())
            .await;
        let canonical = root.path().canonicalize().unwrap();
        assert_eq!(
            card,
            Ok(Some(format!(
                "Sends {}/reports/q3.pdf (5 bytes) to telegram recipient default as q3.pdf.",
                canonical.display()
            )))
        );
        assert!(sender.files.lock().unwrap().is_empty(), "a card sends nothing");
    }

    #[tokio::test]
    async fn an_artifact_card_names_the_stored_file() {
        let saved = saved();
        let sender = Arc::new(RecordingSender::default());
        let card = artifact_tool(&saved, &sender)
            .confirmation_preview(&artifact_args(&saved.record.id), &owner_ctx(OWNER))
            .await;
        assert_eq!(
            card,
            Ok(Some(format!(
                "Sends {} (9 bytes) to telegram recipient default as report.md.",
                saved.record.storage_path
            )))
        );
    }

    #[tokio::test]
    async fn a_message_raises_the_ordinary_card() {
        let card = make_tool()
            .confirmation_preview(
                &serde_json::json!({
                    "action": "message",
                    "channel": "telegram",
                    "recipient": "default",
                    "content": "hello"
                }),
                &ToolContext::default(),
            )
            .await;
        assert_eq!(card, Ok(None));
    }

    /// A call the send would refuse raises no card: nobody is asked about it.
    #[tokio::test]
    async fn a_send_that_would_be_refused_raises_no_card() {
        let root = root_with_report();
        let sender = Arc::new(RecordingSender::default());
        let tool = tool_at(root.path(), &sender);
        let ctx = ToolContext::default();
        assert_eq!(
            tool.confirmation_preview(&file_args("../secret.txt"), &ctx).await,
            Err("Path traversal ('..') is not allowed.".to_string())
        );
        let mut nameless = file_args("reports/q3.pdf");
        nameless.as_object_mut().unwrap().remove("filename");
        assert_eq!(
            tool.confirmation_preview(&nameless, &ctx).await,
            Err("Missing required parameter: filename (required for action=\"file\")".to_string())
        );
        let saved = saved();
        assert_eq!(
            artifact_tool(&saved, &sender)
                .confirmation_preview(&artifact_args(&saved.record.id), &owner_ctx("someone-else"))
                .await,
            Err(format!("send: no artifact '{}' of yours.", saved.record.id))
        );
    }

    /// Gotchas: an approval surface plants a U+202E and a U+200B. The file's
    /// name on disk carries one, the display name the other.
    #[tokio::test]
    async fn hidden_characters_on_a_send_card_are_escaped_and_counted() {
        let root = root_with_report();
        std::fs::write(root.path().join("reports/q3\u{202E}fdp.exe"), b"%PDF-").unwrap();
        let sender = Arc::new(RecordingSender::default());
        let mut args = file_args("reports/q3\u{202E}fdp.exe");
        args["filename"] = serde_json::json!("report\u{200B}.pdf");
        let card = tool_at(root.path(), &sender)
            .confirmation_preview(&args, &ToolContext::default())
            .await
            .unwrap()
            .expect("a file send has a card");
        let canonical = root.path().canonicalize().unwrap();
        assert_eq!(
            card,
            format!(
                "Sends {}/reports/q3<U+202E>fdp.exe (5 bytes) to telegram recipient default as \
                 report<U+200B>.pdf.\nNote: this request contains 2 hidden characters, shown as <U+…>.",
                canonical.display()
            )
        );
    }
}
