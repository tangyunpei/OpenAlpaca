use super::ConnectorSendLock;
use super::annotations_for_builtin;
use super::helpers::{acting_folder, named_folder, resolve_workspace_path};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use crate::utils::text::{escape_hidden_line, hidden_notice};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::repository::TaskRepository;
use openalpaca_storage::{ArtifactRecord, ArtifactStore, Database};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The tail of a refusal for an argument only a file send needs.
const FILE_ONLY: &str = " (required for action=\"file\")";

struct SendTool {
    provider: ConnectorSendLock,
    /// The artifact store an `artifact_id` is looked up in (R23). Absent
    /// (tests, the CLI) refuses an artifact id at call time.
    db: Option<Database>,
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

/// The refusal for a stored file whose bytes are not there.
fn bytes_gone(id: &str, path: &str) -> String {
    format!("send: the bytes of '{id}' are gone ({path}).")
}

/// A confined path as the text a connector is handed.
fn sendable(id: &str, path: PathBuf) -> Result<String, String> {
    path.into_os_string().into_string().map_err(|_| {
        format!(
            "send: the file of '{id}' resolves to a path that is not UTF-8 text, which no channel \
             can send."
        )
    })
}

/// A by-product's or an upload's stored file, resolved the way the Library's
/// read confines a head (`resolve_version_path`): storage's `confine_to_root`
/// canonicalizes every component, the last included, under the row's own
/// folder in the store — its `artifacts/` or `uploads/`, the home store's or
/// a project's folder under `projects/` (`ArtifactRecord::content_root`; the
/// lead's ruling R-T20). A link put at the file is never followed out of
/// that folder — out of the store, or into the store's own `state/` — and
/// the path returned, which the card names and the connector is handed, is
/// the file the link reaches (interfaces §4.3; S1 review, I2). A folder that
/// is gone holds no bytes, as the Library's read answers. Read only: nothing
/// is written or stamped.
fn confine_in_its_folder(id: &str, record: &ArtifactRecord) -> Result<String, String> {
    let root = record
        .content_root()
        .map_err(|e| format!("Failed to look up artifact '{id}': {e:#}"))?;
    if !root.exists() {
        return Err(bytes_gone(id, &record.storage_path));
    }
    let confined =
        openalpaca_storage::store::confine_to_root(&root, Path::new(&record.storage_path))
            .map_err(|e| {
                format!("send: the file of '{id}' is not inside OpenAlpaca's store ({e:#}).")
            })?;
    sendable(id, confined)
}

/// R37: a result file in the project (a pointer row, ST-12), read through
/// R31's rule under the project its row records — storage's
/// `confine_to_root` resolves every component, the last included — so a link
/// put at the file never leads the send out of the project. Read only, like
/// the rest of the lookup: the card shares it. The caller has already refused
/// a project whose root no longer leads to its own folder (R-T19), so the
/// root canonicalized here is the project's own.
fn confine_in_project(id: &str, record: &ArtifactRecord) -> Result<String, String> {
    let root = record.project_root.as_deref().unwrap_or_default();
    let confined = openalpaca_storage::store::confine_to_root(
        Path::new(root),
        Path::new(&record.storage_path),
    )
    .map_err(|e| format!("send: the file of '{id}' is not inside its project folder ({e:#})."))?;
    sendable(id, confined)
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
    /// locate it the same way, so the card names what is sent; only the send
    /// makes a scratch folder (`make_folder`), the card names it (R34).
    fn locate(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
        make_folder: bool,
    ) -> Result<SendFile, String> {
        let file_path = arguments.get("file_path").and_then(|v| v.as_str());
        let artifact_id = arguments.get("artifact_id").and_then(|v| v.as_str());
        match (file_path, artifact_id) {
            (Some(_), Some(_)) => Err("Give file_path or artifact_id, not both.".to_string()),
            (None, None) => Err(format!(
                "Missing required parameter: file_path or artifact_id{FILE_ONLY}"
            )),
            (Some(file_path), None) => self.resolve(file_path, ctx, make_folder),
            (None, Some(id)) => self.artifact(id, ctx),
        }
    }

    /// R23 and R26 (storage design §10–§11): the file `file_path` names in the
    /// call's working folder — the project, or the conversation's scratch
    /// folder — resolved as `file_read` resolves one: relative, no `..`, every
    /// component (the last included) canonicalized and under the folder, and
    /// in a project nothing under a `.openalpaca` folder (R32). No home-store
    /// rule binds it (R33): the store lies under no working folder. A call the
    /// folder refuses (no context, no conversation, a missing project) gets
    /// that refusal's sentence, and nothing is sent.
    fn resolve(
        &self,
        file_path: &str,
        ctx: &ToolContext,
        make_folder: bool,
    ) -> Result<SendFile, String> {
        let (root, rules) = if make_folder {
            acting_folder(ctx)?
        } else {
            named_folder(ctx)?
        };
        let resolved = resolve_workspace_path(file_path, &root, rules)?;
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

    /// R23: a file of the turn's owner, by its id — a by-product, an upload,
    /// or a result file in the project — refused when it belongs to a
    /// project this turn is not in, when it leads out of its folder, or when
    /// its bytes are gone. Read only. The hand-edit check a Library read
    /// makes (`resolve_content`) can record a version or refresh a result
    /// file's row, and the card shares this lookup and must write nothing.
    fn artifact(&self, id: &str, ctx: &ToolContext) -> Result<SendFile, String> {
        let db = self.db.as_ref().ok_or_else(|| {
            format!("send cannot look up artifact '{id}' here: there is no database.")
        })?;
        let owner = turn_owner(db, ctx)?;
        let record = ArtifactStore::new(db)
            .get(id, &owner)
            .map_err(|e| format!("Failed to look up artifact '{id}': {e}"))?
            .ok_or_else(|| format!("send: no artifact '{id}' of yours."))?;
        // The lead's ruling R-T19 (ST-23): a result file of a project whose
        // root no longer leads to its own folder — gone, renamed by case
        // only, or moved with a link left at the old path — is refused
        // before anything is read, from any turn. R-T3's check below would
        // pass it from a turn at that root (`project_root_of` names the
        // recorded root as written), and `confine_to_root` would follow the
        // link.
        if record.in_project
            && let Some(root) = record.project_root.as_deref()
            && openalpaca_storage::store::projects::root_is_missing(Path::new(root))
        {
            return Err(format!(
                "send: the file of '{id}' is in a project whose folder is gone or has moved \
                 ({root}); re-point the project first."
            ));
        }
        // The lead's ruling R-T3: a file is named by the rule `artifact_read`
        // reads a by-product by (S5) — one of no project is the owner's from
        // any turn, one of a project only from a turn in that project — so a
        // conversation never sends from a project it is not in.
        if let Some(root) = record.project_root.as_deref() {
            let scope = super::artifact_write::scope_from_context(ctx);
            let turn_project = openalpaca_storage::store::project_root_of(&scope)
                .map_err(|e| format!("Failed to look up artifact '{id}': {e:#}"))?;
            if turn_project.as_deref() != Some(root) {
                return Err(format!(
                    "send: '{id}' belongs to another project ({root}); send it from a \
                     conversation in that project."
                ));
            }
        }
        let path = if record.in_project {
            confine_in_project(id, &record)?
        } else {
            confine_in_its_folder(id, &record)?
        };
        let metadata = std::fs::metadata(&path)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .ok_or_else(|| bytes_gone(id, &path))?;
        Ok(SendFile {
            path,
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
                let file = self.locate(arguments, ctx, true)?;
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
        let file = self.locate(arguments, ctx, false)?;
        Ok(Some(file_card(&file, channel, recipient, filename)))
    }
}

pub(super) fn send_tool(provider: ConnectorSendLock, db: Option<Database>) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "send".to_string(),
            description: "Send a message or file to a contact via a connected external channel \
                (Telegram, iMessage, Discord). For messages: provide action='message' \
                with content text. For files: provide action='file' with file_path (a file \
                relative to the working folder — the project, or this conversation's scratch \
                folder) or artifact_id (a by-product you saved, by its \
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
                        "description": "A file to send, relative to the working folder (e.g. 'reports/q3.pdf'); absolute paths and '..' are refused. Give this or artifact_id (action='file')"
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
        backend: ToolBackend::BuiltIn(Arc::new(SendTool { provider, db })),
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
    use std::sync::{Mutex, RwLock};

    /// `send` with no connector: the validation tests never reach one. A
    /// file it is handed is resolved in the call's working folder.
    fn make_tool() -> SendTool {
        SendTool {
            provider: Arc::new(RwLock::new(None)),
            db: None,
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

    /// `send` with `sender` as its connector. It acts in the call's working
    /// folder (storage design §10, R26).
    fn tool_with(sender: &Arc<RecordingSender>) -> SendTool {
        let provider: Arc<dyn ConnectorSendProvider> = sender.clone();
        SendTool {
            provider: Arc::new(RwLock::new(Some(provider))),
            db: None,
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

    /// A guarded temp project holding `reports/q3.pdf` (5 bytes): the working
    /// folder of `gp.ctx()`.
    fn project_with_report() -> crate::test_util::GuardedProject {
        let gp = crate::test_util::GuardedProject::new();
        std::fs::create_dir_all(gp.root.join("reports")).unwrap();
        std::fs::write(gp.root.join("reports/q3.pdf"), b"%PDF-").unwrap();
        gp
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
        let gp = project_with_report();
        let err = make_tool()
            .execute_with_context(&file_args("reports/q3.pdf"), &gp.ctx())
            .await
            .unwrap_err();
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
        let gp = project_with_report();
        let sender = Arc::new(RecordingSender::default());
        let out = tool_with(&sender)
            .execute_with_context(&file_args("reports/q3.pdf"), &gp.ctx())
            .await;
        assert_eq!(out.as_deref(), Ok("Sent file 'q3.pdf'"));
        let sent = gp.root.join("reports/q3.pdf");
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
        let gp = project_with_report();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "not yours").unwrap();
        let absolute = outside.path().join("secret.txt");
        let sender = Arc::new(RecordingSender::default());
        let tool = tool_with(&sender);
        for (path, refusal) in [
            (
                absolute.to_str().unwrap(),
                "Absolute paths are not allowed. Use relative paths within the workspace.",
            ),
            ("../secret.txt", "Path traversal ('..') is not allowed."),
            (
                "reports/../../secret.txt",
                "Path traversal ('..') is not allowed.",
            ),
        ] {
            assert_eq!(
                tool.execute_with_context(&file_args(path), &gp.ctx())
                    .await
                    .unwrap_err(),
                refusal,
                "{path}"
            );
        }
        assert!(
            sender.files.lock().unwrap().is_empty(),
            "nothing reached the connector"
        );
    }

    /// R33 (storage design §11): no home-store rule binds `send` any more. A
    /// project's own `state/` folder is the user's — the daemon's `state/`
    /// lies under no working folder. A folder is still not a file.
    #[tokio::test]
    async fn a_projects_own_state_folder_is_sent_and_a_folder_is_not_a_file() {
        let gp = project_with_report();
        std::fs::create_dir_all(gp.root.join("state")).unwrap();
        std::fs::write(gp.root.join("state/notes.json"), "{}").unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = tool_with(&sender);
        assert_eq!(
            tool.execute_with_context(&file_args("state/notes.json"), &gp.ctx())
                .await
                .as_deref(),
            Ok("Sent file 'q3.pdf'")
        );
        assert_eq!(
            tool.execute_with_context(&file_args("reports"), &gp.ctx())
                .await
                .unwrap_err(),
            "'reports' is not a file."
        );
        assert_eq!(sender.files.lock().unwrap().len(), 1, "the file only");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_out_of_the_folder_is_refused_after_resolution() {
        let gp = project_with_report();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "not yours").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            gp.root.join("reports/link.pdf"),
        )
        .unwrap();
        let sender = Arc::new(RecordingSender::default());
        assert_eq!(
            tool_with(&sender)
                .execute_with_context(&file_args("reports/link.pdf"), &gp.ctx())
                .await
                .unwrap_err(),
            "Path 'reports/link.pdf' resolves outside the workspace boundary"
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

    /// S1 review I2, with the lead's ruling R-T20: a by-product's stored file
    /// is resolved the way the Library's read confines a head — under its own
    /// folder in the store, `artifacts/` here. A link put there that leads out
    /// of the home store is refused, at the card and in the send, and nothing
    /// reaches the connector. One that stays inside that folder is named on
    /// the card by the file it reaches, and that file is the one sent.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_at_an_artifacts_stored_file_is_named_by_its_target_and_never_followed_out_of_its_folder()
     {
        let saved = saved();
        let id = saved.record.id.clone();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.md");
        std::fs::write(&secret, b"not the store's\n").unwrap();
        std::fs::remove_file(&saved.record.storage_path).unwrap();
        std::os::unix::fs::symlink(&secret, &saved.record.storage_path).unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = artifact_tool(&saved, &sender);
        let refused = format!("send: the file of '{id}' is not inside OpenAlpaca's store (");

        let card = tool
            .confirmation_preview(&artifact_args(&id), &owner_ctx(OWNER))
            .await
            .unwrap_err();
        assert!(card.starts_with(&refused), "{card}");
        let err = tool
            .execute_with_context(&artifact_args(&id), &owner_ctx(OWNER))
            .await
            .unwrap_err();
        assert!(err.starts_with(&refused), "{err}");
        assert!(sender.files.lock().unwrap().is_empty(), "nothing was sent");

        let home = saved._home.path().canonicalize().unwrap();
        let elsewhere = home.join("artifacts").join("elsewhere.md");
        std::fs::write(&elsewhere, b"in the store\n").unwrap();
        std::fs::remove_file(&saved.record.storage_path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &saved.record.storage_path).unwrap();
        let elsewhere = elsewhere.to_str().unwrap().to_string();
        assert_eq!(
            tool.confirmation_preview(&artifact_args(&id), &owner_ctx(OWNER))
                .await,
            Ok(Some(format!(
                "Sends {elsewhere} (13 bytes) to telegram recipient default as report.md."
            )))
        );
        tool.execute_with_context(&artifact_args(&id), &owner_ctx(OWNER))
            .await
            .unwrap();
        assert_eq!(
            *sender.files.lock().unwrap(),
            vec![(
                elsewhere,
                "report.md".to_string(),
                saved.record.mime_type.clone()
            )]
        );
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
        let gp = project_with_report();
        let sender = Arc::new(RecordingSender::default());
        let card = tool_with(&sender)
            .confirmation_preview(&file_args("reports/q3.pdf"), &gp.ctx())
            .await;
        assert_eq!(
            card,
            Ok(Some(format!(
                "Sends {}/reports/q3.pdf (5 bytes) to telegram recipient default as q3.pdf.",
                gp.root.display()
            )))
        );
        assert!(
            sender.files.lock().unwrap().is_empty(),
            "a card sends nothing"
        );
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
        let sender = Arc::new(RecordingSender::default());
        {
            let gp = project_with_report();
            let tool = tool_with(&sender);
            assert_eq!(
                tool.confirmation_preview(&file_args("../secret.txt"), &gp.ctx())
                    .await,
                Err("Path traversal ('..') is not allowed.".to_string())
            );
            let mut nameless = file_args("reports/q3.pdf");
            nameless.as_object_mut().unwrap().remove("filename");
            assert_eq!(
                tool.confirmation_preview(&nameless, &gp.ctx()).await,
                Err(
                    "Missing required parameter: filename (required for action=\"file\")"
                        .to_string()
                )
            );
        }
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
        let gp = project_with_report();
        std::fs::write(gp.root.join("reports/q3\u{202E}fdp.exe"), b"%PDF-").unwrap();
        let sender = Arc::new(RecordingSender::default());
        let mut args = file_args("reports/q3\u{202E}fdp.exe");
        args["filename"] = serde_json::json!("report\u{200B}.pdf");
        let card = tool_with(&sender)
            .confirmation_preview(&args, &gp.ctx())
            .await
            .unwrap()
            .expect("a file send has a card");
        assert_eq!(
            card,
            format!(
                "Sends {}/reports/q3<U+202E>fdp.exe (5 bytes) to telegram recipient default as \
                 report<U+200B>.pdf.\nNote: this request contains 2 hidden characters, shown as <U+…>.",
                gp.root.display()
            )
        );
    }

    // ── Storage design §10 (R26, R27, R32, R34): the working folder ──

    #[tokio::test]
    async fn a_file_is_sent_from_the_conversations_scratch_folder() {
        let gp = crate::test_util::GuardedProject::new();
        let scratch = gp.home_root().join("scratch").join("conv-test");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("notes.txt"), "notes").unwrap();
        let sender = Arc::new(RecordingSender::default());

        let out = tool_with(&sender)
            .execute_with_context(&file_args("notes.txt"), &gp.scratch_ctx())
            .await;
        assert_eq!(out.as_deref(), Ok("Sent file 'q3.pdf'"));
        assert_eq!(
            sender.files.lock().unwrap()[0].0,
            scratch.join("notes.txt").to_str().unwrap()
        );
    }

    /// R32's read rule binds `send` like `file_read`: an older OpenAlpaca's
    /// folder inside a project is never sent anywhere.
    #[tokio::test]
    async fn a_file_under_dot_openalpaca_in_a_project_is_refused_before_the_connector() {
        let gp = project_with_report();
        std::fs::create_dir_all(gp.root.join(".openalpaca/uploads")).unwrap();
        std::fs::write(gp.root.join(".openalpaca/uploads/a.txt"), "old").unwrap();
        let sender = Arc::new(RecordingSender::default());

        let err = tool_with(&sender)
            .execute_with_context(&file_args(".openalpaca/uploads/a.txt"), &gp.ctx())
            .await
            .expect_err("refused");
        assert_eq!(
            err,
            "'.openalpaca/uploads/a.txt' is refused: a .openalpaca folder inside a project is an \
             older OpenAlpaca's, and nothing in it is read or written any more."
        );
        assert!(
            sender.files.lock().unwrap().is_empty(),
            "nothing reached the connector"
        );
    }

    /// R27: a missing project is refused at the card and in the body.
    #[tokio::test]
    async fn a_missing_project_refuses_the_send_and_raises_no_card() {
        let gp = project_with_report();
        std::fs::remove_dir_all(&gp.root).unwrap();
        let missing = crate::tools::working_folder::WorkingFolderRefusal::ProjectMissing {
            root: gp.root.display().to_string(),
        }
        .to_string();
        let sender = Arc::new(RecordingSender::default());
        let tool = tool_with(&sender);

        assert_eq!(
            tool.confirmation_preview(&file_args("reports/q3.pdf"), &gp.ctx())
                .await,
            Err(missing.clone())
        );
        assert_eq!(
            tool.execute_with_context(&file_args("reports/q3.pdf"), &gp.ctx())
                .await,
            Err(missing)
        );
        assert!(sender.files.lock().unwrap().is_empty());
    }

    /// R34: the card names a file of the scratch folder without making the
    /// folder — a conversation that never acts gets none.
    #[tokio::test]
    async fn a_file_card_makes_no_scratch_folder() {
        let gp = crate::test_util::GuardedProject::new();
        let sender = Arc::new(RecordingSender::default());

        let err = tool_with(&sender)
            .confirmation_preview(&file_args("notes.txt"), &gp.scratch_ctx())
            .await
            .expect_err("no such file yet");
        assert!(
            err.starts_with("Path resolution failed for 'notes.txt'"),
            "{err}"
        );
        assert!(
            !gp.home_root().join("scratch").exists(),
            "the card made no folder"
        );
    }

    // ── the lead's ruling R-T20: a by-product is confined under its own folder ──

    /// R-T20: a by-product's stored file is confined under its own folder in
    /// the store, not merely under the home store. A link put there that
    /// leads into the store's own `state/` is refused, at the card and in the
    /// send, and nothing reaches the connector.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_at_a_by_products_stored_file_into_the_stores_state_folder_is_refused() {
        let saved = saved();
        let id = saved.record.id.clone();
        let state = saved._home.path().canonicalize().unwrap().join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("x"), b"the daemon's\n").unwrap();
        std::fs::remove_file(&saved.record.storage_path).unwrap();
        std::os::unix::fs::symlink(state.join("x"), &saved.record.storage_path).unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = artifact_tool(&saved, &sender);
        let refused = format!("send: the file of '{id}' is not inside OpenAlpaca's store (");

        let card = tool
            .confirmation_preview(&artifact_args(&id), &owner_ctx(OWNER))
            .await
            .unwrap_err();
        assert!(card.starts_with(&refused), "{card}");
        let err = tool
            .execute_with_context(&artifact_args(&id), &owner_ctx(OWNER))
            .await
            .unwrap_err();
        assert!(err.starts_with(&refused), "{err}");
        assert!(sender.files.lock().unwrap().is_empty(), "nothing was sent");
    }

    /// R-T20: an upload's folder is `uploads/`, and an upload of the turn's
    /// owner is sent from its stored file like a by-product.
    #[tokio::test]
    async fn an_upload_of_the_turns_owner_is_sent_from_its_stored_file() {
        let saved = saved();
        let scope = StoreScope::Home;
        let upload = openalpaca_storage::UploadStore::new(&saved.db)
            .put(openalpaca_storage::NewUpload {
                owner_id: OWNER,
                filename: "notes.txt",
                mime_type: "text/plain",
                data: b"notes\n",
                scope: &scope,
                created: chrono::Utc::now(),
            })
            .unwrap()
            .asset;
        let sender = Arc::new(RecordingSender::default());

        let out = artifact_tool(&saved, &sender)
            .execute_with_context(&artifact_args(&upload.id), &owner_ctx(OWNER))
            .await;
        assert_eq!(out.as_deref(), Ok("Sent file 'report.md'"));
        assert_eq!(
            *sender.files.lock().unwrap(),
            vec![(
                upload.storage_path,
                "report.md".to_string(),
                "text/plain".to_string()
            )]
        );
    }

    /// R-T20, as the Library's read answers it: a by-product whose whole
    /// folder is gone reads as gone — its bytes are not there — not as a file
    /// outside the store.
    #[tokio::test]
    async fn an_artifact_whose_folder_is_gone_is_refused_as_gone() {
        let saved = saved();
        std::fs::remove_dir_all(saved._home.path().join("artifacts")).unwrap();
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

    // ── R37 (S4a): a result file in the project, by its id ──

    /// A result file of the turn's owner, sent by its id, is read from the
    /// path R31's rule resolves under its project; replaced by a link out of
    /// the project it is refused, and nothing more is sent.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_result_file_is_sent_from_inside_its_project_and_never_through_a_link_out() {
        let gp = crate::test_util::GuardedProject::new();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        let file = gp.root.join("docs").join("report.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"# Report\n").unwrap();
        let root = gp.root.to_string_lossy().into_owned();
        let (row, _) = ArtifactStore::new(&db)
            .record_project_file(openalpaca_storage::NewProjectFile {
                owner_id: "owner-test",
                project_root: &root,
                rel_path: "docs/report.md",
                content: b"# Report\n",
                task_id: None,
                agent_id: None,
            })
            .unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = SendTool {
            provider: Arc::new(RwLock::new(Some(
                sender.clone() as Arc<dyn crate::orchestrator::ConnectorSendProvider>
            ))),
            db: Some(db.clone()),
        };

        tool.execute_with_context(&artifact_args(&row.id), &gp.ctx())
            .await
            .unwrap();
        let sent: Vec<String> = sender
            .files
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _, _)| path.clone())
            .collect();
        assert_eq!(sent, vec![file.to_string_lossy().into_owned()]);

        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.md");
        std::fs::write(&secret, b"not the project's\n").unwrap();
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&secret, &file).unwrap();

        let err = tool
            .execute_with_context(&artifact_args(&row.id), &gp.ctx())
            .await
            .unwrap_err();
        assert!(
            err.starts_with(&format!(
                "send: the file of '{}' is not inside its project folder",
                row.id
            )),
            "{err}"
        );
        assert_eq!(
            sender.files.lock().unwrap().len(),
            1,
            "nothing more was sent"
        );
    }

    // ── the lead's ruling R-T3: another project's by-product, by its id ──

    /// `send` by `artifact_id` names a by-product by the rule
    /// `artifact_read` reads it by (S5): another project's is refused, at
    /// the card and in the send, naming that project; from a turn in that
    /// project it goes, and one of no project goes from any turn.
    #[tokio::test]
    async fn a_by_product_of_another_project_is_refused_by_id() {
        let gp = crate::test_util::GuardedProject::new();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        let other = tempfile::tempdir().unwrap();
        let other_root = other.path().canonicalize().unwrap();
        openalpaca_storage::store::projects::register(&other_root).unwrap();
        let other_scope = openalpaca_storage::store::StoreScope::Project(other_root.clone());
        let (theirs, _) = ArtifactStore::new(&db)
            .put(openalpaca_storage::NewArtifact::new(
                "owner-test",
                &other_scope,
                openalpaca_storage::ArtifactKind::Markdown,
                "plan",
                b"theirs\n",
            ))
            .unwrap();
        let home_scope = openalpaca_storage::store::StoreScope::Home;
        let (anywhere, _) = ArtifactStore::new(&db)
            .put(openalpaca_storage::NewArtifact::new(
                "owner-test",
                &home_scope,
                openalpaca_storage::ArtifactKind::Markdown,
                "notes",
                b"mine\n",
            ))
            .unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = SendTool {
            provider: Arc::new(RwLock::new(Some(
                sender.clone() as Arc<dyn crate::orchestrator::ConnectorSendProvider>
            ))),
            db: Some(db.clone()),
        };
        let refused = format!(
            "send: '{}' belongs to another project ({}); send it from a conversation in that project.",
            theirs.id,
            other_root.display()
        );

        assert_eq!(
            tool.confirmation_preview(&artifact_args(&theirs.id), &gp.ctx())
                .await,
            Err(refused.clone())
        );
        assert_eq!(
            tool.execute_with_context(&artifact_args(&theirs.id), &gp.ctx())
                .await,
            Err(refused)
        );
        assert!(sender.files.lock().unwrap().is_empty(), "nothing was sent");

        let other_ctx = ToolContext {
            request_workspace_root: Some(other_root.to_string_lossy().into_owned()),
            workspace_id: Some(other_root.to_string_lossy().into_owned()),
            ..gp.ctx()
        };
        tool.execute_with_context(&artifact_args(&theirs.id), &other_ctx)
            .await
            .unwrap();
        tool.execute_with_context(&artifact_args(&anywhere.id), &gp.ctx())
            .await
            .unwrap();
        assert_eq!(sender.files.lock().unwrap().len(), 2);
    }

    // ── the lead's ruling R-T19: a result file of a missing project, by its id ──

    /// R-T19 (R37 with ST-23): a result file of a project whose folder moved
    /// with a link left at its old root is refused by its id — from a turn
    /// at that root too, which `project_root_of` names as the row's project,
    /// so R-T3's check passes it — at the card and in the send, and nothing
    /// is read through the link; once the folder is back at its root it goes.
    /// The project is a folder of its own beside `GuardedProject`'s (which
    /// holds the home-store guard), whose `TempDir` owns its root.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_result_file_of_a_missing_project_is_never_sent_through_its_old_root() {
        let gp = crate::test_util::GuardedProject::new();
        let scene = tempfile::tempdir().unwrap();
        let root = scene.path().canonicalize().unwrap().join("app");
        let file = root.join("docs").join("report.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"# Report\n").unwrap();
        openalpaca_storage::store::projects::register(&root).unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        let recorded = root.to_string_lossy().into_owned();
        let (row, _) = ArtifactStore::new(&db)
            .record_project_file(openalpaca_storage::NewProjectFile {
                owner_id: "owner-test",
                project_root: &recorded,
                rel_path: "docs/report.md",
                content: b"# Report\n",
                task_id: None,
                agent_id: None,
            })
            .unwrap();
        let sender = Arc::new(RecordingSender::default());
        let tool = SendTool {
            provider: Arc::new(RwLock::new(Some(
                sender.clone() as Arc<dyn crate::orchestrator::ConnectorSendProvider>
            ))),
            db: Some(db.clone()),
        };
        let ctx = ToolContext {
            request_workspace_root: Some(recorded.clone()),
            workspace_id: Some(recorded.clone()),
            ..gp.ctx()
        };
        let moved = root.with_file_name("app-moved");
        std::fs::rename(&root, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &root).unwrap();
        let refused = format!(
            "send: the file of '{}' is in a project whose folder is gone or has moved ({recorded}); \
             re-point the project first.",
            row.id
        );

        assert_eq!(
            tool.confirmation_preview(&artifact_args(&row.id), &ctx)
                .await,
            Err(refused.clone())
        );
        assert_eq!(
            tool.execute_with_context(&artifact_args(&row.id), &ctx)
                .await,
            Err(refused)
        );
        assert!(sender.files.lock().unwrap().is_empty(), "nothing was sent");

        // The person moves the folder back: the file goes.
        std::fs::remove_file(&root).unwrap();
        std::fs::rename(&moved, &root).unwrap();
        tool.execute_with_context(&artifact_args(&row.id), &ctx)
            .await
            .unwrap();
        let sent: Vec<String> = sender
            .files
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _, _)| path.clone())
            .collect();
        assert_eq!(sent, vec![file.to_string_lossy().into_owned()]);
    }
}
