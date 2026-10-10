use crate::daemon_config::{DaemonConfig, SessionsConfig};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use crate::tools::working_folder::{WorkingFolder, WorkingFolderRefusal, working_folder};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::store::{
    ContentKind, FILE_NAME_SLUG_BYTES, StoreScope, kind_for_file_name, slugify, store_root,
};
use openalpaca_storage::{ArtifactKind, Database};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::annotations_for_builtin;
use super::approval_preview::{WriteLanding, edit_card, write_card};
use super::artifact_write::{
    ToolArtifact, put_tool_artifact, record_tool_project_file, split_name,
};
use super::helpers::{
    FolderRules, MAX_FILE_READ_SIZE, acting_folder, resolve_workspace_path,
    resolve_workspace_path_for_write, validate_workspace_path,
};
use super::pre_edit::{EditVerb, PreEdit, check_pre_edit, keep_pre_edit_copy};

// --- file_read ---

/// Reads one file of the call's working folder (storage design §10, R26):
/// the project's root, or the conversation's scratch folder. It holds no
/// root of its own — the folder is the call's, read from its context.
struct FileReadTool;

#[async_trait]
impl BuiltInTool for FileReadTool {
    /// R26: a call reached with no context names no folder, and is refused
    /// rather than read from the daemon's own directory.
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err(WorkingFolderRefusal::NoContext.to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        // R26, R27, R44: the project's folder, or the conversation's scratch
        // folder; a missing project, or a call with neither, is refused with
        // its sentence before anything is read.
        let (root, rules) = acting_folder(ctx)?;
        // R31: relative, no `..`, resolved whole and under the root. R32 in a
        // project: nothing under a `.openalpaca` folder. `.git` and `.env`
        // are read like any file — reading never asks (ST-16).
        let full_path = resolve_workspace_path(path, &root, rules)?;

        // Guard against OOM: reject files larger than 10 MB
        let metadata = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| format!("Cannot access file '{}': {}", path, e))?;
        if metadata.len() > MAX_FILE_READ_SIZE {
            return Err(format!(
                "File '{}' is {} bytes, exceeding the {} byte limit",
                path,
                metadata.len(),
                MAX_FILE_READ_SIZE
            ));
        }

        tokio::fs::read_to_string(&full_path)
            .await
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))
    }
}

pub(super) fn file_read_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file from the working folder — the project, or this \
                conversation's scratch folder. Returns its full content as UTF-8; files over \
                10MB are refused. Paths are relative to the working folder; '..' is refused, \
                and in a project so is any .openalpaca folder. Reading never asks. For binary \
                files, use shell_execute."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file, relative to the working folder (e.g. 'src/main.rs', 'config/settings.toml'). Absolute paths and '..' are refused."
                    }
                },
                "required": ["path"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"path": "src/main.rs"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileReadTool)),
        provides_capabilities: vec!["file_read".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("file_read"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

// --- file_list ---

/// The most entries one `file_list` call shows (R40). Past it the listing
/// ends with one line saying how many more there are: a folder of thousands
/// of files is listed a subfolder at a time, not poured into the context.
const FILE_LIST_MAX_ENTRIES: usize = 500;

/// `file_list` (R40): one folder of the working folder — the project, or the
/// conversation's scratch folder — never recursive and never asking (ST-16).
/// Resolved like `file_read`'s path: relative, no `..`, under the canonical
/// root, and in a project `.openalpaca` refused (R32).
struct FileListTool;

#[async_trait]
impl BuiltInTool for FileListTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err(WorkingFolderRefusal::NoContext.to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        // R26, R27, R44: the project's folder, or the conversation's scratch
        // folder (made on first use), and R32's rules for it — the resolution
        // `file_read` makes; a refusal is its own sentence, before anything
        // is listed.
        let (root, rules) = acting_folder(ctx)?;
        let dir = resolve_workspace_path(path, &root, rules)?;
        let metadata = tokio::fs::metadata(&dir)
            .await
            .map_err(|e| format!("Cannot access '{path}': {e}"))?;
        if !metadata.is_dir() {
            return Err(format!(
                "'{path}' is a file, not a folder; read it with file_read."
            ));
        }
        list_folder(path, &dir).await
    }
}

/// Read `dir`'s entries and lay them out. `DirEntry::metadata` does not
/// follow a link, so a link is shown as one (`@`) rather than as what it
/// points at; a directory gets a trailing `/`, a file its size, anything
/// else (a socket, a pipe) its bare name.
async fn list_folder(path: &str, dir: &Path) -> Result<String, String> {
    let cannot = |e: std::io::Error| format!("Cannot list '{path}': {e}");
    let mut reader = tokio::fs::read_dir(dir).await.map_err(cannot)?;
    let mut entries: Vec<(String, String)> = Vec::new();
    while let Some(entry) = reader.next_entry().await.map_err(cannot)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let shown = match entry.metadata().await {
            Ok(meta) if meta.file_type().is_symlink() => format!("{name}@"),
            Ok(meta) if meta.is_dir() => format!("{name}/"),
            Ok(meta) if meta.is_file() => format!("{name}  {} bytes", meta.len()),
            _ => name.clone(),
        };
        entries.push((name, shown));
    }
    Ok(format_listing(path, dir, entries))
}

/// The listing's text: `{path} ({abs}): {n} entries`, then one line per
/// entry sorted by name in byte order, at most [`FILE_LIST_MAX_ENTRIES`] of
/// them, then — past that — how many were left out. Names are not escaped:
/// this is a tool result, not an approval surface.
fn format_listing(path: &str, dir: &Path, mut entries: Vec<(String, String)>) -> String {
    // By name in byte order: the tuple sorts on its first field, and names in
    // one folder are unique (and `sort()` keeps clippy's sort lints quiet).
    entries.sort();
    let total = entries.len();
    let mut out = format!("{path} ({}): {total} entries", dir.display());
    for (_, shown) in entries.iter().take(FILE_LIST_MAX_ENTRIES) {
        out.push('\n');
        out.push_str(shown);
    }
    if total > FILE_LIST_MAX_ENTRIES {
        out.push_str(&format!(
            "\n... and {} more entries not shown — list a subfolder",
            total - FILE_LIST_MAX_ENTRIES
        ));
    }
    out
}

pub(super) fn file_list_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_list".to_string(),
            description: "List one folder under the working folder (the project, or this \
                conversation's scratch folder): its files with their sizes, its folders \
                marked with a trailing /, links with @. Not recursive — list a subfolder to \
                go deeper. Read a file with file_read."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The folder to list, relative to the working folder; '.' is the working folder itself."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            // Closed and fully required: `strict_mode_compatible` accepts it,
            // so it is one of the two builtins that keep the flag on the wire.
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"path": "."}),
                serde_json::json!({"path": "src"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileListTool)),
        // R42: comes with `file_read`, so a template that reads files lists
        // them with no template edit — and one that denies `file_read` is
        // handed neither (`resolve_capabilities`' provided-capability filter).
        provides_capabilities: vec!["file_read".into(), "file_list".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("file_list"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

// --- file_write ---

/// The most one `file_write` call writes (10 MB).
const MAX_FILE_WRITE_SIZE: u64 = 10 * 1024 * 1024;

struct FileWriteTool {
    /// Where a result file's pointer row is recorded (R35) and a by-product
    /// is saved (no project). Absent (tests, the CLI): a project write still
    /// writes the file and says the app could not list it (R36); a
    /// by-product is refused, as `artifact_write` refuses one.
    db: Option<Database>,
    /// `[orchestrator.sessions] snapshot_max_bytes` — the cap on the
    /// pre-edit copy (ST-17) — and `[execution.artifacts]`, the caps on a
    /// by-product. Absent (tests, the CLI) means the documented defaults.
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
}

/// Where one `file_write` call lands, decided before a byte is written: the
/// card (R34) and the write read the same answer, so a person is never asked
/// about a write that would then be refused, and every refusal comes before
/// anything changes.
enum WritePlan {
    /// A real file in the project (ST-10, ST-12): `target` is where the write
    /// resolver says it lands under `root` (R31, R32); `replaces` says a file
    /// stands there now.
    Project {
        root: PathBuf,
        target: PathBuf,
        replaces: bool,
    },
    /// No project: a by-product in the home store's artifact store (ST-10).
    ByProduct,
}

/// The two arguments, or the refusal naming the one missing.
fn write_arguments(arguments: &serde_json::Value) -> Result<(&str, &str), String> {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required parameter: path".to_string())?;
    let content = arguments
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required parameter: content".to_string())?;
    Ok((path, content))
}

impl FileWriteTool {
    fn snapshot_max_bytes(&self) -> u64 {
        match self.daemon_config {
            Some(ref config) => config.load().orchestrator.sessions.snapshot_max_bytes,
            None => SessionsConfig::default().snapshot_max_bytes,
        }
    }

    /// Every refusal the write gives before it touches a byte, and where it
    /// lands. Reads two small files at most and creates nothing.
    fn plan(&self, path: &str, content: &str, ctx: &ToolContext) -> Result<WritePlan, String> {
        if content.len() as u64 > MAX_FILE_WRITE_SIZE {
            return Err(format!(
                "Content size {} bytes exceeds the {} byte write limit",
                content.len(),
                MAX_FILE_WRITE_SIZE
            ));
        }
        validate_workspace_path(path)?;
        // A path with no file name at its end ('', '.', 'reports/', a blank
        // one) names a folder, not a file.
        let last = path.rsplit(['/', '\\']).next().unwrap_or_default().trim();
        if last.is_empty() || last == "." {
            return Err(format!(
                "Path '{path}' names no file. Give a file name, e.g. 'reports/summary.md'."
            ));
        }
        match working_folder(ctx) {
            Ok(WorkingFolder::Project { root }) => {
                let target = resolve_workspace_path_for_write(path, &root, FolderRules::Project)?;
                let replaces = check_pre_edit(
                    path,
                    &target,
                    ctx,
                    self.snapshot_max_bytes(),
                    EditVerb::Overwrite,
                )?;
                Ok(WritePlan::Project {
                    root,
                    target,
                    replaces,
                })
            }
            // A by-product's place is its owner's artifact store, not a
            // folder: with no project it is saved whether or not the call
            // names a conversation, and no scratch folder is made for it.
            Ok(WorkingFolder::Scratch { .. }) | Err(WorkingFolderRefusal::NoConversation) => {
                Ok(WritePlan::ByProduct)
            }
            // R27: a project whose folder is gone, or a root that names no
            // project any more, is refused before anything runs.
            Err(refusal) => Err(refusal.to_string()),
        }
    }

    async fn write(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let (path, content) = write_arguments(arguments)?;
        let (root, target) = match self.plan(path, content, ctx)? {
            WritePlan::ByProduct => return self.save_as_artifact(path, content, ctx),
            WritePlan::Project { root, target, .. } => (root, target),
        };
        // ST-17 + R46: the bytes this write is about to replace are kept in
        // the conversation's log first; `Err` means nothing was written.
        let kept = keep_pre_edit_copy(
            path,
            &target,
            &root,
            ctx,
            self.snapshot_max_bytes(),
            EditVerb::Overwrite,
        )
        .await?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {e}"))?;
        }
        tokio::fs::write(&target, content)
            .await
            .map_err(|e| format!("Failed to write file '{path}': {e}"))?;
        // R35, R36: the file is the result and the row a view of it — a row
        // that cannot be written does not undo the write.
        let listed = match self.db.as_ref() {
            Some(db) => record_tool_project_file(db, ctx, &root, &target, content).map(|_| ()),
            None => Err("no database is wired to this tool".to_string()),
        };
        if let Err(e) = &listed {
            tracing::warn!(
                path,
                target = %target.display(),
                "file_write: the file is written but not listed: {e}"
            );
        }
        Ok(Self::project_write_result(
            path,
            content.len(),
            &target,
            kept == PreEdit::Kept,
            listed,
        ))
    }

    /// What the model reads after a project write (interfaces §5.6): the
    /// write is done, where, whether it replaced a file whose old content the
    /// log keeps, whether the Library lists it (R36), and how to change it.
    fn project_write_result(
        path: &str,
        bytes: usize,
        abs: &Path,
        replaced: bool,
        listed: Result<(), String>,
    ) -> String {
        let head = if replaced {
            format!(
                "Replaced {path} in the project ({bytes} bytes), at {}; its previous content is \
                 kept in this conversation's log.",
                abs.display()
            )
        } else {
            format!(
                "Wrote {path} in the project ({bytes} bytes), at {}.",
                abs.display()
            )
        };
        let listing = match listed {
            Ok(()) => "It is listed in the Library with the project's results.".to_string(),
            Err(e) => format!("The file is written, but the app could not list it ({e})."),
        };
        format!(
            "{head} {listing} Tell the user the file is at that path. To change it, call \
             file_write with \"{path}\" again."
        )
    }

    /// No project (ST-10): the write is a by-product in the home store's
    /// artifact store, on `artifact_write`'s own store path — named for
    /// `path` ([`artifact_name`]), with the path as written for its summary
    /// and the kind its extension picks (R21's sets). The content is a UTF-8
    /// string by construction, so an extension no kind admits — `build.sh`,
    /// `Makefile`, `logo.svg` — is `code`, stored as `.bin`. The same path
    /// written again in one run is the same artifact's next version.
    fn save_as_artifact(
        &self,
        path: &str,
        content: &str,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let db = self.db.as_ref().ok_or_else(|| {
            "file_write requires database context to save an artifact".to_string()
        })?;
        let record = put_tool_artifact(
            db,
            ctx,
            &StoreScope::Home,
            self.daemon_config.as_ref(),
            ToolArtifact {
                tool: "file_write",
                name: &artifact_name(path),
                kind: kind_for_file_name(path).unwrap_or(ArtifactKind::Code),
                content,
                note: None,
                summary: Some(path),
                metadata: None,
            },
        )?;
        Ok(format!(
            "Saved: {path} is the artifact \"{}\", version {}, at {}. This conversation has no \
             project folder, so its files are kept in OpenAlpaca's artifact store. It is listed \
             in the Library. This file is the deliverable: tell the user it is saved at that \
             absolute path, and do not write another copy with the shell or any other tool. \
             file_read cannot open it from this conversation; what you passed is what was saved, \
             so there is nothing to check. To change it, call file_write with \"{path}\" again; \
             that saves its next version.",
            record.name, record.version, record.storage_path
        ))
    }
}

#[async_trait]
impl BuiltInTool for FileWriteTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        self.write(arguments, &ToolContext::default()).await
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        self.write(arguments, ctx).await
    }

    /// R34: the card says where the write lands — the project folder (and
    /// whether a file is there now) or the artifact store — from the same
    /// plan the write follows; a write the plan refuses raises no card.
    async fn confirmation_preview(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        let (path, content) = write_arguments(arguments)?;
        let card = match self.plan(path, content, ctx)? {
            WritePlan::Project { root, replaces, .. } => write_card(
                path,
                WriteLanding::Project {
                    root: &root,
                    replaces,
                },
            ),
            WritePlan::ByProduct => {
                let store = store_root(&StoreScope::Home)
                    .map_err(|e| format!("The artifact store could not be found: {e:#}"))?
                    .join(ContentKind::Artifacts.dir_name());
                write_card(path, WriteLanding::ByProduct { store: &store })
            }
        };
        Ok(Some(card))
    }
}

/// The artifact name `file_write` saves `path` under (G2): the path itself,
/// which the grammar slugifies (`reports/pasta-night.md` →
/// `01-reports-pasta-night.md`) — when the grammar carries the path whole.
/// A name keeps ASCII letters and digits only, at most
/// `FILE_NAME_SLUG_BYTES` (60) of them, and an extension only when a kind
/// admits it (R21; any other, or none, is stored as `.bin`). So `文档.md` and
/// `笔记.md`, two files of one deep directory, or `go.mod` and `go.sum` would
/// each be one artifact's two versions — and past the version cap the first
/// file's bytes are deleted. A path the grammar cannot carry whole gets as
/// its title its slug, shortened to leave room, plus the first eight hex
/// digits of the whole path's SHA-256 (`artifact-1a2b3c4d.md`,
/// `go-1a2b3c4d.mod`, stored as `.bin`). The same path always gets the same
/// name, so writing it again is still its next version. Two carried paths
/// that differ only in letter case or in characters that are not ASCII
/// letters or digits (`Notes.md` and `notes.md`, `a/b.md` and `a-b.md`,
/// `__main__.py` and `main.py`) still reach one name.
fn artifact_name(path: &str) -> String {
    let (title, _) = split_name(path);
    let carried = path.is_ascii()
        && slugify(title, usize::MAX).len() <= FILE_NAME_SLUG_BYTES
        && kind_for_file_name(path).is_some();
    if carried {
        return path.to_string();
    }
    let extension = &path[title.len()..];
    let hash: String = Sha256::digest(path.as_bytes())[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let slug = slugify(title, FILE_NAME_SLUG_BYTES - hash.len() - 1);
    format!("{slug}-{hash}{extension}")
}

pub(super) fn file_write_tool(
    db: Option<Database>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_write".to_string(),
            description: "Write a whole file. In a project, this writes the real file in the \
                project folder: the path is relative to that folder, missing parent folders are \
                created, and '..', a .git folder and a .openalpaca folder are refused. Replacing \
                an existing file first keeps its current content in this conversation's log, and \
                a file larger than the snapshot limit (10MB by default) cannot be replaced. The \
                file is listed in the Library with the project's results. With no project, this \
                saves a by-product in OpenAlpaca's artifact store instead, named after the path \
                (plus a short hash of the path when the name cannot carry it whole), its \
                extension picking its kind; writing the same path again in one run saves its \
                next version. Content is limited to 10MB. The result says which happened and \
                gives the absolute path to tell the user. To change part of an existing file in \
                the project, use file_edit."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file's path relative to the project folder (e.g. 'src/main.rs', 'docs/report.md'); with no project, the by-product's name. Absolute paths and '..' are refused."
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file as a UTF-8 string"
                    }
                },
                "required": ["path", "content"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"path": "output/result.txt", "content": "Hello, world!"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileWriteTool { db, daemon_config })),
        provides_capabilities: vec!["file_write".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("file_write"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

// --- file_edit (ST-22; storage design §13, R47–R49) ---

/// R48: `file_edit` changes a file in a project, and nowhere else.
const EDIT_NEEDS_A_PROJECT: &str = "file_edit changes a file in a project, and this \
     conversation has no project. Use file_write to save a file instead.";
const EDIT_EMPTY_OLD: &str =
    "old_string is empty. Give the exact text to replace; to create a file, use file_write.";
const EDIT_IDENTICAL: &str =
    "old_string and new_string are the same, so there is nothing to change.";

/// R47: a missing file's refusal names the tool that creates one.
fn edit_missing(path: &str) -> String {
    format!(
        "'{path}' does not exist in the project. file_edit changes an existing file; use \
         file_write to create it."
    )
}

/// Change part of one file in the project (ST-22): the exact text to find and
/// the text to put in its place, every other byte kept. The same folder,
/// confinement, card, pre-edit copy and pointer row as `file_write`'s real
/// write (storage design §13, R47–R49).
struct FileEditTool {
    /// Where the pointer row is recorded (R35). Absent (tests, the CLI): the
    /// edit still lands, and the result says the app could not list it (R36).
    db: Option<Database>,
    /// `[orchestrator.sessions] snapshot_max_bytes`, the cap on the pre-edit
    /// copy (ST-17). Absent: the documented default.
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
}

/// R47's arguments, read once for the card and the body alike.
struct EditArgs<'a> {
    path: &'a str,
    old_string: &'a str,
    new_string: &'a str,
    /// Default false; anything but `true` is false.
    replace_all: bool,
}

fn edit_args(arguments: &serde_json::Value) -> Result<EditArgs<'_>, String> {
    fn required<'a>(arguments: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("Missing required parameter: {key}"))
    }
    Ok(EditArgs {
        path: required(arguments, "path")?,
        old_string: required(arguments, "old_string")?,
        new_string: required(arguments, "new_string")?,
        replace_all: arguments
            .get("replace_all")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

/// What an edit will do, worked out without writing anything — the card's
/// whole and the body's first half (one plan answers both, interfaces §4.3).
struct EditPlan {
    /// The project's canonical root.
    root: PathBuf,
    /// The file, as the write resolver returned it.
    target: PathBuf,
    /// The whole file after the edit.
    text: String,
    replacements: usize,
    first_line: usize,
}

/// What R47's match made of a file's text.
#[derive(Debug, PartialEq, Eq)]
struct Edited {
    text: String,
    replacements: usize,
    /// 1-based line of the first occurrence.
    first_line: usize,
}

/// R47's match: exact, non-overlapping occurrences of `old_string` — exactly
/// one unless `replace_all` — replaced by `new_string`, every other byte of
/// `text` kept. A file holding `\r\n`, asked with an `old_string` that holds
/// `\n` but no `\r\n` and is not found as given, is searched again with every
/// lone `\n` of both strings written `\r\n`: a model writes LF, and the file
/// keeps its endings. In a file whose every line break is `\r\n` (no lone
/// `\n`), both strings' lone `\n` are written `\r\n` even when `old_string`
/// is found as given (the lead's ruling R-T11) — `new_string`'s so the file
/// stays all-CRLF, `old_string`'s so one that starts with a line break is
/// matched with the `\r` before it and no `\r\r\n` is left behind.
fn apply_edit(
    path: &str,
    text: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<Edited, String> {
    if old_string.is_empty() {
        return Err(EDIT_EMPTY_OLD.to_string());
    }
    let retry = !text.contains(old_string)
        && text.contains("\r\n")
        && old_string.contains('\n')
        && !old_string.contains("\r\n");
    // The lead's ruling R-T11: a file whose every line break is `\r\n` stays
    // so — `new_string`'s lone `\n` are written `\r\n` there even when
    // `old_string` holds no line break. A file that mixes the two keeps what
    // the model wrote. In such a file every `\n` follows a `\r`, so an
    // `old_string` found as given can hold a lone `\n` only as its first
    // character; written `\r\n` it matches the same place, one byte earlier.
    let all_crlf = text.contains("\r\n") && !has_lone_lf(text);
    let crlf = retry || all_crlf;
    let old = if crlf {
        lf_to_crlf(old_string)
    } else {
        old_string.to_string()
    };
    let new = if crlf {
        lf_to_crlf(new_string)
    } else {
        new_string.to_string()
    };
    if old == new {
        return Err(EDIT_IDENTICAL.to_string());
    }
    let Some(first) = text.find(old.as_str()) else {
        return Err(format!(
            "old_string was not found in '{path}'. Read the file with file_read and copy the \
             exact text, whitespace and line breaks included."
        ));
    };
    let count = text.matches(old.as_str()).count();
    if count > 1 && !replace_all {
        return Err(format!(
            "old_string occurs {count} times in '{path}'. Give more of the surrounding text so \
             it occurs once, or set replace_all to true to change every one."
        ));
    }
    // A byte slice up to a match's start, never a `str` cut.
    let first_line = 1 + text.as_bytes()[..first]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count();
    let (text, replacements) = if replace_all {
        (text.replace(old.as_str(), &new), count)
    } else {
        (text.replacen(old.as_str(), &new, 1), 1)
    };
    Ok(Edited {
        text,
        replacements,
        first_line,
    })
}

/// Every `\n` that does not already end a `\r\n`, written `\r\n`.
fn lf_to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut after_cr = false;
    for c in text.chars() {
        if c == '\n' && !after_cr {
            out.push('\r');
        }
        out.push(c);
        after_cr = c == '\r';
    }
    out
}

/// Whether `text` holds a `\n` that does not end a `\r\n` — a file with none
/// is all-CRLF (the lead's ruling R-T11).
fn has_lone_lf(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(i, byte)| *byte == b'\n' && (i == 0 || bytes[i - 1] != b'\r'))
}

impl FileEditTool {
    fn snapshot_max_bytes(&self) -> u64 {
        match self.daemon_config {
            Some(ref config) => config.load().orchestrator.sessions.snapshot_max_bytes,
            None => SessionsConfig::default().snapshot_max_bytes,
        }
    }

    /// Everything but the copy, the write and the row: the folder (R48), the
    /// path (R31, R32), the file (it exists, is a regular file — a folder is
    /// refused here, before any card — is UTF-8, and fits the 10 MB bound
    /// before and after), the copy's preconditions (ST-17's cap and R46, from
    /// the size alone, before a byte is read) and the match. Creates nothing,
    /// so the card can call it.
    async fn plan(&self, args: &EditArgs<'_>, ctx: &ToolContext) -> Result<EditPlan, String> {
        let root = match working_folder(ctx) {
            Ok(WorkingFolder::Project { root }) => root,
            Ok(WorkingFolder::Scratch { .. }) => return Err(EDIT_NEEDS_A_PROJECT.to_string()),
            // R26, R27: no context, a missing project (ST-23 included) or a
            // root that names no project — each its own sentence.
            Err(refusal) => return Err(refusal.to_string()),
        };
        let path = args.path;
        let target = resolve_workspace_path_for_write(path, &root, FolderRules::Project)?;
        let metadata = match tokio::fs::metadata(&target).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(edit_missing(path)),
            Err(e) => return Err(format!("Cannot access '{path}': {e}")),
        };
        if metadata.is_dir() {
            return Err(format!("'{path}' is a folder, not a file."));
        }
        if !metadata.is_file() {
            return Err(format!(
                "'{path}' is not a regular file, so file_edit cannot change it."
            ));
        }
        check_pre_edit(
            path,
            &target,
            ctx,
            self.snapshot_max_bytes(),
            EditVerb::Edit,
        )?;
        let limit = MAX_FILE_READ_SIZE;
        if metadata.len() > limit {
            return Err(format!(
                "File '{path}' is {} bytes, exceeding the {limit} byte limit",
                metadata.len()
            ));
        }
        let bytes = tokio::fs::read(&target)
            .await
            .map_err(|e| format!("Failed to read file '{path}': {e}"))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("'{path}' is not UTF-8 text, so file_edit cannot change it."))?;
        let edited = apply_edit(
            path,
            &text,
            args.old_string,
            args.new_string,
            args.replace_all,
        )?;
        if edited.text.len() as u64 > limit {
            return Err(format!(
                "The edited '{path}' would be {} bytes, above the {limit} byte limit file_read \
                 and file_write keep to. The file is unchanged.",
                edited.text.len()
            ));
        }
        Ok(EditPlan {
            root,
            target,
            text: edited.text,
            replacements: edited.replacements,
            first_line: edited.first_line,
        })
    }

    /// R47's order: the plan, the pre-edit copy (ST-17, R46), the file, the
    /// pointer row (R35; R36 when it cannot be recorded).
    async fn edit(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args = edit_args(arguments)?;
        let plan = self.plan(&args, ctx).await?;
        let path = args.path;
        // The current bytes go into this conversation's log first; `Err` —
        // R46, the cap, a copy that failed — leaves the file untouched.
        match keep_pre_edit_copy(
            path,
            &plan.target,
            &plan.root,
            ctx,
            self.snapshot_max_bytes(),
            EditVerb::Edit,
        )
        .await?
        {
            PreEdit::Kept => {}
            // The plan read the file a moment ago and it is gone now.
            PreEdit::NewFile => return Err(edit_missing(path)),
        }
        tokio::fs::write(&plan.target, &plan.text)
            .await
            .map_err(|e| format!("Failed to write file '{path}': {e}"))?;
        let listed = match self.db.as_ref() {
            Some(db) => {
                record_tool_project_file(db, ctx, &plan.root, &plan.target, &plan.text).map(|_| ())
            }
            // `file_write` words this case the same way.
            None => Err("no database is wired to this tool".to_string()),
        };
        let mut result = format!(
            "Edited {path} in the project: {} replacement{}, the first at line {}; {} bytes at {}. \
             Its previous content is kept in this conversation's log.",
            plan.replacements,
            if plan.replacements == 1 { "" } else { "s" },
            plan.first_line,
            plan.text.len(),
            plan.target.display()
        );
        // R36: the file is the result; the row is a view of it.
        if let Err(e) = listed {
            tracing::warn!(
                path,
                target = %plan.target.display(),
                "file_edit: the file is written but not listed: {e}"
            );
            result.push_str(&format!(
                " The file is written, but the app could not list it ({e})."
            ));
        }
        Ok(result)
    }
}

#[async_trait]
impl BuiltInTool for FileEditTool {
    /// R26: with no context there is no folder to edit in.
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err(WorkingFolderRefusal::NoContext.to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        self.edit(arguments, ctx).await
    }

    /// R49: the folder, the file and the line, and both strings escaped and
    /// cut — from the same plan as the body, so a call the body would refuse
    /// raises no card (interfaces §4.3). Reads the file; writes nothing.
    async fn confirmation_preview(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        let args = edit_args(arguments)?;
        let plan = self.plan(&args, ctx).await?;
        Ok(Some(edit_card(
            args.path,
            &plan.root,
            plan.first_line,
            plan.replacements,
            args.old_string,
            args.new_string,
        )))
    }
}

pub(super) fn file_edit_tool(
    db: Option<Database>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_edit".to_string(),
            description: "Change part of one file in the project: give the exact text to find \
                (old_string) and the text to put in its place (new_string), instead of writing \
                the whole file back. old_string must occur exactly once unless replace_all is \
                true. The rest of the file is kept byte for byte, line endings included. Its \
                current content is kept in the conversation's log first. In a project only — to \
                create a file, or with no project, use file_write."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The file, relative to the project folder (e.g. 'src/main.rs'). It must exist."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The exact text to find, whitespace and line breaks included. It must occur exactly once, unless replace_all is true."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The text to put in its place. It must differ from old_string."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace every occurrence instead of exactly one. Default false."
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            // Not strict: `replace_all` is optional with a default, and strict
            // mode requires every key (interfaces §7.4; Gotchas, R8).
            strict: None,
            input_examples: Some(vec![serde_json::json!({
                "path": "src/main.rs",
                "old_string": "let retries = 3;",
                "new_string": "let retries = 5;"
            })]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileEditTool { db, daemon_config })),
        // R48: it comes with the file_write capability — a template that
        // grants file_write holds it, and one that denies file_write denies
        // it (`resolve_capabilities` leaves a tool off a surface when any
        // capability it provides is denied).
        provides_capabilities: vec!["file_write".into(), "file_edit".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("file_edit"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
#[path = "file_edit_tests.rs"]
mod file_edit_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_log::{RecordType, SNAPSHOTS_DIR, SessionLogLimits, SessionLogService};
    use std::fs;
    use std::time::Duration;

    // ── §5.7: the pre-edit image `file_write` takes ─────────────────

    /// A guarded project, a sessions root and a live handle for one session —
    /// the three things every pre-edit test needs. `service` is `Arc`, via
    /// [`SessionLogService::into_arc`], so a handle has a way back to the
    /// service: a writer that idled out is reopened rather than wedging every
    /// later overwrite.
    struct Bench {
        gp: crate::test_util::GuardedProject,
        sessions: tempfile::TempDir,
        service: Arc<SessionLogService>,
    }

    /// A home store of its own: `working_folder` reads `projects/` under it,
    /// and no test may read the real one.
    struct SandboxedHome {
        dir: tempfile::TempDir,
        _env: crate::test_util::HomeStoreGuard,
    }

    impl SandboxedHome {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let env = crate::test_util::HomeStoreGuard::set(&dir.path().canonicalize().unwrap());
            Self { dir, _env: env }
        }

        fn root(&self) -> PathBuf {
            self.dir.path().canonicalize().unwrap()
        }
    }

    fn bench() -> Bench {
        bench_with_limits(SessionLogLimits::default())
    }

    fn bench_with_limits(limits: SessionLogLimits) -> Bench {
        let gp = crate::test_util::GuardedProject::new();
        let sessions = tempfile::tempdir().unwrap();
        let service =
            SessionLogService::new(sessions.path().to_path_buf(), None, limits, "test".to_string())
                .into_arc();
        Bench {
            gp,
            sessions,
            service,
        }
    }

    impl Bench {
        /// `file_write` with no database: a project write still writes the
        /// file, and its result says the app could not list it (R36).
        fn tool(&self, cap: Option<u64>) -> FileWriteTool {
            let daemon_config = cap.map(|bytes| {
                let mut config = DaemonConfig::default();
                config.orchestrator.sessions.snapshot_max_bytes = bytes;
                Arc::new(ArcSwap::from_pointee(config))
            });
            FileWriteTool {
                db: None,
                daemon_config,
            }
        }

        /// The project folder the writes land in.
        fn root(&self) -> &Path {
            &self.gp.root
        }

        fn ctx(&self, session: &str) -> ToolContext {
            ToolContext {
                session_id: Some(session.to_string()),
                session_log: Some(self.service.handle_for(session)),
                agent_instance_id: Some("lead_agent::a1".to_string()),
                ..self.project_ctx()
            }
        }

        /// The project turn, with no session log.
        fn project_ctx(&self) -> ToolContext {
            self.gp.ctx()
        }

        fn session_dir(&self, session: &str) -> PathBuf {
            self.sessions.path().join(session)
        }

        /// Every file this session imaged, newest name last.
        fn images(&self, session: &str) -> Vec<PathBuf> {
            let mut out: Vec<PathBuf> = fs::read_dir(self.session_dir(session).join(SNAPSHOTS_DIR))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            out.sort();
            out
        }
    }

    /// The core of §5.7: the bytes the write is about to destroy are in
    /// `snapshots/` when it lands, and a `file_snapshot` record names them.
    #[tokio::test]
    async fn a_write_over_an_existing_file_images_it_before_the_new_bytes_land() {
        let bench = bench();
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-a");
        fs::create_dir_all(bench.root().join("docs")).unwrap();
        fs::write(bench.root().join("docs/report.md"), "the old draft").unwrap();

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "docs/report.md", "content": "the new draft"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(ctx.session_log.as_ref().unwrap().flush().await);

        assert_eq!(
            fs::read_to_string(bench.root().join("docs/report.md")).unwrap(),
            "the new draft",
            "the write happened"
        );
        let images = bench.images("sess-a");
        assert_eq!(images.len(), 1, "{images:?}");
        assert_eq!(
            fs::read_to_string(&images[0]).unwrap(),
            "the old draft",
            "and the pre-edit bytes were saved before it did"
        );

        let records = crate::session_log::read_records(&bench.session_dir("sess-a")).unwrap();
        let snapshot = records
            .iter()
            .find(|r| r.kind == RecordType::FileSnapshot.as_str())
            .expect("the image is committed by a record");
        assert_eq!(snapshot.data["path"], "docs/report.md");
        assert_eq!(snapshot.data["size"].as_u64(), Some(13));
        assert_eq!(snapshot.agent.as_deref(), Some("lead_agent::a1"));
        let rel = snapshot.data["snapshot_ref"].as_str().unwrap();
        assert_eq!(
            bench.session_dir("sess-a").join(rel.trim_start_matches("file:")),
            images[0],
            "the record names the file that is there"
        );
    }

    /// A file that did not exist has no pre-edit state, so there is nothing to
    /// image and nothing to refuse.
    #[tokio::test]
    async fn a_new_file_is_not_imaged() {
        let bench = bench();
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-b");

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "fresh.txt", "content": "hello"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(ctx.session_log.as_ref().unwrap().flush().await);

        assert!(
            !bench.session_dir("sess-b").join(SNAPSHOTS_DIR).exists(),
            "a create is not an overwrite"
        );
    }

    /// R46: off a conversation log there is nowhere to keep a copy, so an
    /// existing file is not replaced — and a new one is still written.
    #[tokio::test]
    async fn without_a_session_log_an_existing_file_is_not_replaced() {
        let bench = bench();
        let tool = bench.tool(None);
        fs::write(bench.root().join("notes.txt"), "old").unwrap();

        let refused = tool
            .execute_with_context(
                &serde_json::json!({"path": "notes.txt", "content": "new"}),
                &bench.project_ctx(),
            )
            .await
            .expect_err("R46");
        assert_eq!(
            refused,
            "Refusing to overwrite 'notes.txt': it already exists, and no copy of its current \
             content can be kept here — a skill or a plugin agent keeps no conversation log. The \
             file is unchanged. Write the result to a new file, or ask for the change in the \
             conversation itself."
        );
        assert_eq!(
            fs::read_to_string(bench.root().join("notes.txt")).unwrap(),
            "old"
        );

        tool.execute_with_context(
            &serde_json::json!({"path": "fresh.txt", "content": "hello"}),
            &bench.project_ctx(),
        )
        .await
        .unwrap();
        assert_eq!(
            fs::read_to_string(bench.root().join("fresh.txt")).unwrap(),
            "hello"
        );
    }

    /// Above `snapshot_max_bytes` the write is refused, not performed
    /// unimaged — and the refusal names the limit it is asking about.
    #[tokio::test]
    async fn a_target_above_the_snapshot_cap_refuses_the_write() {
        let bench = bench();
        let tool = bench.tool(Some(8));
        let ctx = bench.ctx("sess-c");
        fs::write(bench.root().join("big.txt"), "far more than eight bytes").unwrap();

        let refused = tool
            .execute_with_context(
                &serde_json::json!({"path": "big.txt", "content": "clobber"}),
                &ctx,
            )
            .await
            .expect_err("the write is refused");
        assert!(refused.contains("snapshot_max_bytes"), "{refused}");
        assert!(refused.contains('8'), "{refused}");
        assert_eq!(
            fs::read_to_string(bench.root().join("big.txt")).unwrap(),
            "far more than eight bytes",
            "the file is unchanged"
        );
    }

    /// The default cap is `file_write`'s own 10 MB content bound: an
    /// overwrite this size always succeeded before S3 shipped, and it must
    /// keep succeeding now that S3 has a cap of its own (Task 56 fix round
    /// 1, Important #2 / R68).
    #[tokio::test]
    async fn a_five_megabyte_file_is_imaged_and_overwritten_at_the_default_cap() {
        let bench = bench();
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-5mb");
        let old = vec![b'a'; 5 * 1024 * 1024];
        fs::write(bench.root().join("big.bin"), &old).unwrap();

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "big.bin", "content": "new, much shorter"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok(), "a 5 MB overwrite at the default cap: {result:?}");
        assert!(ctx.session_log.as_ref().unwrap().flush().await);

        assert_eq!(
            fs::read_to_string(bench.root().join("big.bin")).unwrap(),
            "new, much shorter"
        );
        let images = bench.images("sess-5mb");
        assert_eq!(images.len(), 1, "{images:?}");
        assert_eq!(
            fs::metadata(&images[0]).unwrap().len(),
            5 * 1024 * 1024,
            "the 5 MB pre-edit image was kept"
        );
    }

    /// Above the default cap the write is refused, naming the key to raise —
    /// unchanged behaviour, just no longer reachable by a file `file_write`
    /// itself could have produced (Important #2 / R68).
    #[tokio::test]
    async fn an_eleven_megabyte_file_is_refused_at_the_default_cap() {
        let bench = bench();
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-11mb");
        let old = vec![b'a'; 11 * 1024 * 1024];
        fs::write(bench.root().join("huge.bin"), &old).unwrap();

        let refused = tool
            .execute_with_context(
                &serde_json::json!({"path": "huge.bin", "content": "clobber"}),
                &ctx,
            )
            .await
            .expect_err("above the default cap the write is refused");
        assert!(refused.contains("snapshot_max_bytes"), "{refused}");
        assert_eq!(
            fs::metadata(bench.root().join("huge.bin")).unwrap().len(),
            11 * 1024 * 1024,
            "the file is unchanged"
        );
    }

    /// Fail closed: a snapshot that cannot be written stops the write, rather
    /// than letting it through unimaged.
    #[tokio::test]
    async fn a_failed_image_refuses_the_write() {
        let bench = bench();
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-d");
        fs::write(bench.root().join("notes.txt"), "precious").unwrap();
        // `snapshots/` cannot be created: the name is taken by a file.
        fs::create_dir_all(bench.session_dir("sess-d")).unwrap();
        fs::write(bench.session_dir("sess-d").join(SNAPSHOTS_DIR), "in the way").unwrap();

        let refused = tool
            .execute_with_context(
                &serde_json::json!({"path": "notes.txt", "content": "clobber"}),
                &ctx,
            )
            .await
            .expect_err("the write is refused");
        assert!(refused.contains("pre-edit snapshot"), "{refused}");
        assert_eq!(
            fs::read_to_string(bench.root().join("notes.txt")).unwrap(),
            "precious",
            "the file is unchanged"
        );
    }

    /// A writer that idles out mid-run must not wedge every later overwrite:
    /// the next `file_write` over the same session gets a fresh snapshot
    /// through a writer reopened under the same id (Task 56 fix round 1,
    /// Important #1).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_write_after_the_writer_idles_out_gets_a_fresh_snapshot() {
        let bench = bench_with_limits(SessionLogLimits {
            idle_close: Duration::from_millis(50),
            ..SessionLogLimits::default()
        });
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-idle-write");
        fs::write(bench.root().join("notes.txt"), "first draft").unwrap();

        // Drive the writer up, then let it idle out before touching it again.
        assert!(ctx.session_log.as_ref().unwrap().flush().await);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            ctx.session_log.as_ref().unwrap().is_closed(),
            "the idle writer exited"
        );

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "notes.txt", "content": "second draft"}),
                &ctx,
            )
            .await;
        assert!(
            result.is_ok(),
            "an idle-closed writer must not wedge the overwrite: {result:?}"
        );

        assert_eq!(
            fs::read_to_string(bench.root().join("notes.txt")).unwrap(),
            "second draft",
            "the write happened"
        );
        let images = bench.images("sess-idle-write");
        assert_eq!(images.len(), 1, "{images:?}");
        assert_eq!(
            fs::read_to_string(&images[0]).unwrap(),
            "first draft",
            "the fresh writer still took the pre-edit image"
        );
    }

    /// R31 (storage design §11): the write resolver resolves the leaf like
    /// every other component, so a link at the leaf that leaves the root is
    /// refused. Before R31 the write followed it and replaced the outside
    /// file, with no pre-edit image kept; this test pinned that hole as
    /// `a_symlink_out_of_the_workspace_is_not_imaged`.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_through_a_link_out_of_the_root_is_refused() {
        let bench = bench();
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-e");
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("elsewhere.txt");
        fs::write(&elsewhere, "not the workspace's").unwrap();
        std::os::unix::fs::symlink(&elsewhere, bench.root().join("link.txt")).unwrap();

        let err = tool
            .execute_with_context(
                &serde_json::json!({"path": "link.txt", "content": "through the link"}),
                &ctx,
            )
            .await
            .expect_err("a write through a link out of the root is refused");
        assert_eq!(err, "Path 'link.txt' resolves outside the workspace boundary");
        assert_eq!(
            fs::read_to_string(&elsewhere).unwrap(),
            "not the workspace's",
            "the outside file is unchanged"
        );
        assert!(ctx.session_log.as_ref().unwrap().flush().await);
        assert!(
            !bench.session_dir("sess-e").join(SNAPSHOTS_DIR).exists(),
            "nothing was imaged"
        );
    }

    #[tokio::test]
    async fn test_file_write_creates_subdirs_within_workspace() {
        let gp = crate::test_util::GuardedProject::new();
        let tool = FileWriteTool {
            db: None,
            daemon_config: None,
        };
        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "subdir/nested/test.txt", "content": "hello"}),
                &gp.ctx(),
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(gp.root.join("subdir/nested/test.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_file_write_rejects_symlink_escape() {
        let gp = crate::test_util::GuardedProject::new();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), gp.root.join("escape_link")).unwrap();
        let tool = FileWriteTool {
            db: None,
            daemon_config: None,
        };
        let err = tool
            .execute_with_context(
                &serde_json::json!({"path": "escape_link/evil.txt", "content": "should not be written"}),
                &gp.ctx(),
            )
            .await
            .expect_err("a link out of the project is refused");
        assert!(err.contains("outside the workspace boundary"), "{err}");
        assert!(!outside.path().join("evil.txt").exists());
    }

    // ── S3-R1 (plan §7a): a link before a new directory ────────────────

    /// **S3-R1 (plan §7a).** The write resolver's absent-parent branch let a
    /// link to anywhere leave the workspace, as long as the directory after
    /// it was new: its boundary check compared the unresolved join with the
    /// root, which it always starts with.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_out_of_the_workspace_before_a_new_directory_is_refused() {
        let b = bench();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), b.root().join("link")).unwrap();

        let err = b
            .tool(None)
            .write(
                &serde_json::json!({"path": "link/newdir/x.json", "content": "escaped"}),
                &b.project_ctx(),
            )
            .await
            .expect_err("a link out of the workspace before a new directory is refused");
        assert!(err.contains("outside the workspace boundary"), "{err}");
        assert!(
            !outside.path().join("newdir").exists(),
            "a directory was created outside the workspace"
        );
    }

    // ── Storage design §10 (R26, R27, R32): file_read reads the working folder ──

    fn read_args(path: &str) -> serde_json::Value {
        serde_json::json!({ "path": path })
    }

    #[tokio::test]
    async fn file_read_reads_a_file_of_the_project_folder() {
        let gp = crate::test_util::GuardedProject::new();
        fs::create_dir_all(gp.root.join("src")).unwrap();
        fs::write(gp.root.join("src/main.rs"), "fn main() {}\n").unwrap();

        let out = FileReadTool
            .execute_with_context(&read_args("src/main.rs"), &gp.ctx())
            .await
            .unwrap();
        assert_eq!(out, "fn main() {}\n");
    }

    /// One conversation never sees another's files: each reads its own
    /// scratch folder, and `..` cannot reach a neighbour's.
    #[tokio::test]
    async fn file_read_reads_the_conversations_scratch_folder_and_no_other() {
        let gp = crate::test_util::GuardedProject::new();
        let scratch = gp.home_root().join("scratch");
        fs::create_dir_all(scratch.join("conv-test")).unwrap();
        fs::write(scratch.join("conv-test/notes.txt"), "mine").unwrap();
        fs::create_dir_all(scratch.join("conv-other")).unwrap();
        fs::write(scratch.join("conv-other/secret.txt"), "theirs").unwrap();
        let other = ToolContext {
            conversation_id: Some("conv-other".to_string()),
            ..Default::default()
        };

        let tool = FileReadTool;
        assert_eq!(
            tool.execute_with_context(&read_args("notes.txt"), &gp.scratch_ctx())
                .await
                .unwrap(),
            "mine"
        );
        assert_eq!(
            tool.execute_with_context(&read_args("secret.txt"), &other).await.unwrap(),
            "theirs"
        );
        let err = tool
            .execute_with_context(&read_args("secret.txt"), &gp.scratch_ctx())
            .await
            .expect_err("another conversation's file is not in this folder");
        // The resolver's own wording is S1's; what matters is that nothing of
        // the other conversation's file came back.
        assert!(!err.contains("theirs"), "{err}");
        let err = tool
            .execute_with_context(&read_args("../conv-other/secret.txt"), &gp.scratch_ctx())
            .await
            .expect_err("'..' is refused");
        assert!(err.contains(".."), "{err}");
        assert!(!err.contains("theirs"), "{err}");
    }

    /// R26: a call reached with no context names no folder.
    #[tokio::test]
    async fn file_read_refuses_a_call_with_no_context() {
        let err = FileReadTool
            .execute(&read_args("README.md"))
            .await
            .expect_err("no context");
        assert_eq!(
            err,
            "This tool acts in the conversation's working folder, and this call carries no \
             conversation to name one."
        );
    }

    /// R27: a project whose folder is gone is refused with §6's sentence
    /// before anything is read, and no folder is made in its place.
    #[tokio::test]
    async fn file_read_refuses_a_missing_project_before_reading() {
        let gp = crate::test_util::GuardedProject::new();
        fs::remove_dir_all(&gp.root).unwrap();

        let err = FileReadTool
            .execute_with_context(&read_args("README.md"), &gp.ctx())
            .await
            .expect_err("the project folder is missing");
        assert_eq!(
            err,
            WorkingFolderRefusal::ProjectMissing {
                root: gp.root.display().to_string()
            }
            .to_string()
        );
        assert!(
            err.starts_with(&format!("The project folder {} is missing", gp.root.display())),
            "{err}"
        );
        assert!(!gp.root.exists(), "nothing re-created the folder");
        assert!(!gp.home_root().join("scratch").exists());
    }

    /// R32's read rule, switched on: in a project nothing under a
    /// `.openalpaca` folder is read (ST-9); `.git` is (ST-16: reading never
    /// asks). A scratch folder has no such rule.
    #[tokio::test]
    async fn file_read_refuses_dot_openalpaca_in_a_project_and_reads_dot_git() {
        let gp = crate::test_util::GuardedProject::new();
        fs::create_dir_all(gp.root.join(".openalpaca/memory")).unwrap();
        fs::write(gp.root.join(".openalpaca/memory/note.md"), "old").unwrap();
        fs::create_dir_all(gp.root.join(".git")).unwrap();
        fs::write(gp.root.join(".git/config"), "[core]\n").unwrap();

        let err = FileReadTool
            .execute_with_context(&read_args(".openalpaca/memory/note.md"), &gp.ctx())
            .await
            .expect_err(".openalpaca is refused in a project");
        assert_eq!(
            err,
            "'.openalpaca/memory/note.md' is refused: a .openalpaca folder inside a project is an \
             older OpenAlpaca's, and nothing in it is read or written any more."
        );
        assert_eq!(
            FileReadTool
                .execute_with_context(&read_args(".git/config"), &gp.ctx())
                .await
                .unwrap(),
            "[core]\n"
        );

        let scratch = gp.home_root().join("scratch/conv-test/.openalpaca");
        fs::create_dir_all(&scratch).unwrap();
        fs::write(scratch.join("x.txt"), "plain").unwrap();
        assert_eq!(
            FileReadTool
                .execute_with_context(&read_args(".openalpaca/x.txt"), &gp.scratch_ctx())
                .await
                .unwrap(),
            "plain"
        );
    }

    /// R33 (storage design §11): the home store's five rules are gone from the
    /// file tools — from a project or a scratch folder nothing they protected
    /// is reachable, and a project of the user's may hold a `state/` folder,
    /// a `MEMORY.md` or a `SOUL.md` of its own: written with file_write, read
    /// back with file_read.
    #[tokio::test]
    async fn a_project_may_hold_a_state_folder_and_a_soul_md() {
        let gp = crate::test_util::GuardedProject::new();
        let files = [
            ("state/notes.json", "{}"),
            ("SOUL.md", "# Soul\n"),
            ("docs/USER.md", "# User\n"),
            ("IDENTITY.md", "# Identity\n"),
            ("MEMORY.md", "# Memory\n"),
            ("memory/.gitignore", "*\n"),
            ("notes/.history/a.md", "old\n"),
        ];
        let write = FileWriteTool {
            db: None,
            daemon_config: None,
        };
        for (path, content) in files {
            write
                .write(&serde_json::json!({ "path": path, "content": content }), &gp.ctx())
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        }

        for (path, content) in files {
            let out = FileReadTool
                .execute_with_context(&read_args(path), &gp.ctx())
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(out, content, "{path}");
        }
    }

    // ── G2 (FW-1): a file not written into a project folder is an artifact ──

    const OWNER: &str = "owner-g2";
    const PASTA: &str = "reports/pasta-night.md";

    /// A sandboxed home store, a database and a registered project — every
    /// placement `file_write` chooses between.
    struct Store {
        home: SandboxedHome,
        project: tempfile::TempDir,
        _db_dir: tempfile::TempDir,
        db: Database,
    }

    impl Store {
        fn new() -> Self {
            let home = SandboxedHome::new();
            // A project is a registered project's (design §3, R12) —
            // registered under this store's home, never the developer's.
            let project = tempfile::tempdir().unwrap();
            openalpaca_storage::store::projects::register(&project.path().canonicalize().unwrap())
                .unwrap();
            let db_dir = tempfile::tempdir().unwrap();
            let db = Database::open(&db_dir.path().join("test.db")).unwrap();
            Self {
                home,
                project,
                _db_dir: db_dir,
                db,
            }
        }

        /// `file_write` as the daemon builds it: its database, no startup root.
        fn tool(&self) -> FileWriteTool {
            FileWriteTool {
                db: Some(self.db.clone()),
                daemon_config: None,
            }
        }

        fn project_root(&self) -> PathBuf {
            self.project.path().canonicalize().unwrap()
        }

        /// A turn's context: its owner and agent, and a project or none.
        fn ctx(&self, project: Option<&Path>, task_id: Option<&str>) -> ToolContext {
            ToolContext {
                agent_id: Some("general_agent".to_string()),
                owner_id: Some(OWNER.to_string()),
                task_id: task_id.map(str::to_string),
                request_workspace_root: project.map(|p| p.to_str().unwrap().to_string()),
                ..Default::default()
            }
        }

        fn artifacts(&self) -> Vec<openalpaca_storage::ArtifactRecord> {
            openalpaca_storage::ArtifactStore::new(&self.db)
                .list(&openalpaca_storage::ArtifactQuery::new(OWNER))
                .unwrap()
                .0
        }

        /// A `task` row — `file_assets.task_id` is a foreign key.
        fn task(&self, id: &str, title: &str) {
            let now = chrono::Utc::now();
            openalpaca_storage::repository::TaskRepository::new(&self.db)
                .create(&openalpaca_storage::Task {
                    id: id.to_string(),
                    title: title.to_string(),
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
        }
    }

    fn pasta(content: &str) -> serde_json::Value {
        serde_json::json!({"path": PASTA, "content": content})
    }

    // ── R34: the card says where the write lands ──────────────────────

    /// What the card says about `args`, for a write the plan lets through —
    /// a write it refuses raises no card (interfaces §4.3), and fails here.
    async fn card(
        tool: &FileWriteTool,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Option<String> {
        tool.confirmation_preview(args, ctx)
            .await
            .expect("the write plan lets this call through")
    }

    fn by_product(store: &Path) -> String {
        format!(
            "Saves {PASTA} as a by-product in {}, OpenAlpaca's artifact store — not in a folder \
             of yours.",
            store.display()
        )
    }

    #[tokio::test]
    async fn a_write_card_with_no_project_names_the_home_artifact_store_and_creates_nothing() {
        let st = Store::new();
        let home = st.home.root();
        let tool = st.tool();

        let shown = card(&tool, &pasta("# Pasta night\n"), &st.ctx(None, None)).await;

        assert_eq!(shown, Some(by_product(&home.join("artifacts"))));
        assert!(!home.join("artifacts").exists(), "the card creates nothing");
        assert!(st.artifacts().is_empty(), "the card writes no row");
    }

    #[tokio::test]
    async fn a_write_card_shows_a_hidden_character_in_the_path_escaped_with_the_notice() {
        let st = Store::new();
        let tool = st.tool();
        let args = serde_json::json!({"path": "notes/\u{202e}dm\u{200b}.txt", "content": "x"});

        let shown = card(&tool, &args, &st.ctx(None, None)).await;

        assert_eq!(
            shown,
            Some(format!(
                "Saves notes/<U+202E>dm<U+200B>.txt as a by-product in {}, OpenAlpaca's artifact \
                 store — not in a folder of yours.\n\
                 Note: this request contains 2 hidden characters, shown as <U+…>.",
                st.home.root().join("artifacts").display()
            ))
        );
    }

    /// A project root no folder under `projects/` records: the card and the
    /// write give `NotAProject`'s sentence, no card is raised, and no
    /// by-product is saved instead (R27; interfaces §4.3).
    #[tokio::test]
    async fn a_write_card_whose_project_is_not_registered_is_refused_with_no_card() {
        let st = Store::new();
        let stranger = tempfile::tempdir().unwrap();
        let root = stranger.path().canonicalize().unwrap();
        let tool = st.tool();
        let ctx = st.ctx(Some(&root), None);
        let args = pasta("x");

        let expected = crate::tools::working_folder::WorkingFolderRefusal::NotAProject {
            root: root.to_str().unwrap().to_string(),
        }
        .to_string();
        assert_eq!(
            tool.confirmation_preview(&args, &ctx).await.unwrap_err(),
            expected
        );
        assert_eq!(tool.write(&args, &ctx).await.unwrap_err(), expected);
        assert!(st.artifacts().is_empty(), "no by-product instead");
    }

    /// A call the write refuses on its face raises no card: the card's
    /// refusal is the write's own sentence (interfaces §4.3).
    #[tokio::test]
    async fn a_write_card_for_a_path_the_write_refuses_on_its_face_is_refused_with_no_card() {
        let st = Store::new();
        let tool = st.tool();
        let ctx = st.ctx(None, None);

        for args in [
            serde_json::json!({"path": "/etc/hosts", "content": "x"}),
            serde_json::json!({"path": "../outside.md", "content": "x"}),
            serde_json::json!({"path": "reports/", "content": "x"}),
            serde_json::json!({"path": ".", "content": "x"}),
            serde_json::json!({"content": "x"}),
        ] {
            let refused = tool.confirmation_preview(&args, &ctx).await.unwrap_err();
            assert_eq!(
                refused,
                tool.write(&args, &ctx).await.unwrap_err(),
                "{args}"
            );
        }
        assert!(st.artifacts().is_empty());
    }

    /// The re-test's defect 1, now with no startup root at all: with no
    /// project on the turn the file is a by-product in the home store, and
    /// nothing lands at `<home>/<path>`.
    #[tokio::test]
    async fn without_a_project_the_file_is_saved_as_an_artifact_in_the_home_store() {
        let st = Store::new();
        let home = st.home.root();
        let tool = st.tool();

        for content in ["# Pasta night\n", "# Pasta night, again\n"] {
            tool.write(&pasta(content), &st.ctx(None, None))
                .await
                .unwrap_or_else(|e| panic!("{e}"));
        }

        assert!(
            !home.join(PASTA).exists(),
            "written into the home store's root"
        );
        let rows = st.artifacts();
        assert_eq!(rows.len(), 1, "one path, one artifact: {rows:?}");
        let row = &rows[0];
        assert!(!row.in_project);
        assert_eq!(row.project_root, None, "the home store");
        assert!(
            Path::new(&row.storage_path).starts_with(home.join("artifacts")),
            "{}",
            row.storage_path
        );
        assert_eq!(row.kind, Some(openalpaca_storage::ArtifactKind::Markdown));
        assert_eq!(row.name, "01-reports-pasta-night.md");
        assert_eq!(row.summary.as_deref(), Some(PASTA), "the path as written");
        assert_eq!(row.agent_id.as_deref(), Some("general_agent"));
        assert_eq!(
            fs::read_to_string(&row.storage_path).unwrap(),
            "# Pasta night, again\n"
        );
    }

    /// One path written twice in one run is one artifact at version 2, the
    /// first kept as version 1.
    #[tokio::test]
    async fn a_second_write_of_the_same_path_in_one_run_is_version_2() {
        let st = Store::new();
        let task = "7c87c6fc-0000-4000-8000-000000000000";
        st.task(task, "Pasta night shopping list");
        let ctx = st.ctx(None, Some(task));
        let tool = st.tool();

        let first = tool.write(&pasta("v1\n"), &ctx).await.unwrap();
        let second = tool.write(&pasta("v2\n"), &ctx).await.unwrap();

        assert!(first.contains("version 1"), "{first}");
        assert!(second.contains("version 2"), "{second}");
        let rows = st.artifacts();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].version, 2);
        assert_eq!(rows[0].task_id.as_deref(), Some(task));
        assert_eq!(fs::read_to_string(&rows[0].storage_path).unwrap(), "v2\n");
        let versions = openalpaca_storage::ArtifactStore::new(&st.db)
            .versions(&rows[0].id)
            .unwrap();
        assert_eq!(versions.len(), 2, "{versions:?}");
    }

    // ── Stage 2: a project turn writes the real file (ST-10, ST-12) ──────

    /// A project turn with a session log for `session` under `dir`, as the
    /// main loop's turn has. The service is returned so the test holds it for
    /// as long as the handle is used.
    fn logged(
        st: &Store,
        project: &Path,
        dir: &Path,
        session: &str,
    ) -> (Arc<SessionLogService>, ToolContext) {
        let service = SessionLogService::new(
            dir.to_path_buf(),
            None,
            SessionLogLimits::default(),
            "test".to_string(),
        )
        .into_arc();
        let ctx = ToolContext {
            session_id: Some(session.to_string()),
            session_log: Some(service.handle_for(session)),
            ..st.ctx(Some(project), None)
        };
        (service, ctx)
    }

    /// ST-10, ST-12, R35: a project turn writes the real file under the
    /// project folder and records its pointer row — no by-product, nothing
    /// under the project's folder in the home store — announces it, and says
    /// so whole.
    #[tokio::test]
    async fn a_project_turn_writes_the_real_file_and_its_pointer_row() {
        let st = Store::new();
        let project = st.project_root();
        let bus = crate::bus::EventBus::new(8);
        let mut rx = bus.subscribe();
        let ctx = ToolContext {
            event_bus: Some(bus.clone()),
            ..st.ctx(Some(&project), None)
        };

        let out = st
            .tool()
            .write(&pasta("# Pasta night\n"), &ctx)
            .await
            .unwrap();

        let file = project.join(PASTA);
        assert_eq!(fs::read_to_string(&file).unwrap(), "# Pasta night\n");
        assert_eq!(
            out,
            format!(
                "Wrote {PASTA} in the project (14 bytes), at {}. It is listed in the Library with \
                 the project's results. Tell the user the file is at that path. To change it, \
                 call file_write with \"{PASTA}\" again.",
                file.display()
            )
        );
        let rows = st.artifacts();
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert!(row.in_project);
        assert_eq!(row.project_root.as_deref(), project.to_str());
        assert_eq!(row.rel_path.as_deref(), Some(PASTA));
        assert_eq!(row.storage_path, file.to_string_lossy());
        assert_eq!(row.agent_id.as_deref(), Some("general_agent"));
        assert!(
            openalpaca_storage::ArtifactStore::new(&st.db)
                .versions(&row.id)
                .unwrap()
                .is_empty()
        );
        let name = openalpaca_storage::store::projects::folder_name(&project);
        assert!(
            !st.home
                .root()
                .join("projects")
                .join(&name)
                .join("artifacts")
                .exists(),
            "no by-product"
        );
        match rx.try_recv().expect("the write is announced") {
            crate::events::SystemEvent::ArtifactWritten {
                artifact_id,
                in_project,
                openable,
                path,
                ..
            } => {
                assert_eq!(artifact_id, row.id);
                assert!(in_project);
                assert!(openable, ".md opens in another app");
                assert_eq!(path, row.storage_path);
            }
            other => panic!("expected ArtifactWritten, got {other:?}"),
        }
    }

    /// ST-17, R35: the same path again replaces the file after keeping its
    /// old content in the conversation's log, and updates the same row.
    #[tokio::test]
    async fn a_second_write_of_a_project_file_replaces_it_and_keeps_one_row() {
        let st = Store::new();
        let project = st.project_root();
        let sessions = tempfile::tempdir().unwrap();
        let (_service, ctx) = logged(&st, &project, sessions.path(), "sess-r");
        let tool = st.tool();

        tool.write(&pasta("v1\n"), &ctx).await.unwrap();
        let out = tool.write(&pasta("v2, longer\n"), &ctx).await.unwrap();

        let file = project.join(PASTA);
        assert_eq!(
            out,
            format!(
                "Replaced {PASTA} in the project (11 bytes), at {}; its previous content is kept \
                 in this conversation's log. It is listed in the Library with the project's \
                 results. Tell the user the file is at that path. To change it, call file_write \
                 with \"{PASTA}\" again.",
                file.display()
            )
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "v2, longer\n");
        let rows = st.artifacts();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].size_bytes, 11);
        assert_eq!(rows[0].version, 1, "a result file has no versions");
        assert!(ctx.session_log.as_ref().unwrap().flush().await);
        let images: Vec<PathBuf> = fs::read_dir(sessions.path().join("sess-r").join(SNAPSHOTS_DIR))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(images.len(), 1, "{images:?}");
        assert_eq!(fs::read_to_string(&images[0]).unwrap(), "v1\n");
    }

    /// R36: the file is the result and the row a view of it — when the row
    /// cannot be written the write stands, and the result says the app could
    /// not list it, and why.
    #[tokio::test]
    async fn a_project_write_whose_row_cannot_be_written_still_reports_the_file() {
        let st = Store::new();
        let project = st.project_root();
        let root = project.to_string_lossy().into_owned();
        let (theirs, _) = openalpaca_storage::ArtifactStore::new(&st.db)
            .record_project_file(openalpaca_storage::NewProjectFile {
                owner_id: "someone-else",
                project_root: &root,
                rel_path: PASTA,
                content: b"theirs\n",
                task_id: None,
                agent_id: None,
            })
            .unwrap();

        let out = st
            .tool()
            .write(&pasta("# Pasta night\n"), &st.ctx(Some(&project), None))
            .await
            .unwrap();

        let file = project.join(PASTA);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "# Pasta night\n",
            "the file is the result"
        );
        assert_eq!(
            out,
            format!(
                "Wrote {PASTA} in the project (14 bytes), at {}. The file is written, but the app \
                 could not list it (the result file {} at {root}/{PASTA} belongs to another \
                 owner). Tell the user the file is at that path. To change it, call file_write \
                 with \"{PASTA}\" again.",
                file.display(),
                theirs.id
            )
        );

        // With no database at all, the same.
        let out = FileWriteTool {
            db: None,
            daemon_config: None,
        }
        .write(
            &serde_json::json!({"path": "notes/other.md", "content": "x\n"}),
            &st.ctx(Some(&project), None),
        )
        .await
        .unwrap();
        assert_eq!(
            fs::read_to_string(project.join("notes/other.md")).unwrap(),
            "x\n"
        );
        assert!(
            out.contains("The file is written, but the app could not list it (no database is wired to this tool)."),
            "{out}"
        );
    }

    /// R32 on for writes: in a project a write into the repository's own
    /// `.git` folder, or into an old in-project `.openalpaca` folder, is
    /// refused — any case — and no card is raised for it.
    #[tokio::test]
    async fn a_write_into_the_projects_git_or_openalpaca_folder_is_refused() {
        let st = Store::new();
        let project = st.project_root();
        let ctx = st.ctx(Some(&project), None);
        let tool = st.tool();
        let git = |p: &str| {
            format!(
                "Writing '{p}' is refused: it is inside the project's .git folder, which git \
                 keeps. Run git through shell_execute instead."
            )
        };
        let old_store = |p: &str| {
            format!(
                "'{p}' is refused: a .openalpaca folder inside a project is an older \
                 OpenAlpaca's, and nothing in it is read or written any more."
            )
        };

        for (path, refusal) in [
            (".git/config", git(".git/config")),
            (
                "sub/.GIT/hooks/pre-commit",
                git("sub/.GIT/hooks/pre-commit"),
            ),
            (
                ".openalpaca/artifacts/x.md",
                old_store(".openalpaca/artifacts/x.md"),
            ),
            (".OpenAlpaca/notes.md", old_store(".OpenAlpaca/notes.md")),
        ] {
            let args = serde_json::json!({"path": path, "content": "x"});
            assert_eq!(tool.write(&args, &ctx).await.unwrap_err(), refusal);
            assert_eq!(
                tool.confirmation_preview(&args, &ctx).await.unwrap_err(),
                refusal,
                "no card for a refused write"
            );
            assert!(!project.join(path).exists(), "{path}");
        }
        assert!(st.artifacts().is_empty());
    }

    /// R27: a project whose folder is gone refuses the write before anything
    /// runs, with §6's sentence, raises no card, and saves no by-product
    /// instead.
    #[tokio::test]
    async fn a_missing_project_refuses_the_write_and_raises_no_card() {
        let st = Store::new();
        let project = st.project_root();
        let ctx = st.ctx(Some(&project), None);
        let args = pasta("# Pasta night\n");
        let aside = project.with_extension("moved");
        fs::rename(&project, &aside).unwrap();

        let write = st.tool().write(&args, &ctx).await;
        let card = st.tool().confirmation_preview(&args, &ctx).await;
        fs::rename(&aside, &project).unwrap();

        let expected = crate::tools::working_folder::WorkingFolderRefusal::ProjectMissing {
            root: project.to_string_lossy().into_owned(),
        }
        .to_string();
        assert_eq!(write.unwrap_err(), expected);
        assert_eq!(card.unwrap_err(), expected);
        assert!(st.artifacts().is_empty(), "no by-product instead");
    }

    /// R34: the card says where the write lands — the project folder, and
    /// whether a file is there now — or the artifact store for a by-product;
    /// and R46 refuses a replacement no copy can be kept of, with no card.
    #[tokio::test]
    async fn the_card_names_the_project_folder_or_the_artifact_store() {
        let st = Store::new();
        let project = st.project_root();
        let sessions = tempfile::tempdir().unwrap();
        let (_service, ctx) = logged(&st, &project, sessions.path(), "sess-card");
        let tool = st.tool();

        let new = tool
            .confirmation_preview(&pasta("x"), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            new,
            format!(
                "Writes {PASTA} in the project folder {}.",
                project.display()
            )
        );

        fs::create_dir_all(project.join("reports")).unwrap();
        fs::write(project.join(PASTA), "old").unwrap();
        let replaces = tool
            .confirmation_preview(&pasta("x"), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            replaces,
            format!(
                "Replaces {PASTA} in the project folder {}; its current content is kept in this \
                 conversation's log.",
                project.display()
            )
        );
        let unlogged = tool
            .confirmation_preview(&pasta("x"), &st.ctx(Some(&project), None))
            .await
            .unwrap_err();
        assert!(
            unlogged.starts_with(&format!("Refusing to overwrite '{PASTA}'")),
            "{unlogged}"
        );

        let by_product = tool
            .confirmation_preview(&pasta("x"), &st.ctx(None, None))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            by_product,
            format!(
                "Saves {PASTA} as a by-product in {}, OpenAlpaca's artifact store — not in a \
                 folder of yours.",
                st.home.root().join("artifacts").display()
            )
        );
        assert_eq!(
            fs::read_to_string(project.join(PASTA)).unwrap(),
            "old",
            "a card writes nothing"
        );
    }

    /// ST-10 as this slice reads it: with no project the write is a
    /// by-product whether or not the call names a conversation — its place is
    /// the owner's artifact store, and no scratch folder is made for it.
    #[tokio::test]
    async fn with_no_project_a_write_is_a_by_product_with_or_without_a_conversation() {
        let st = Store::new();
        let tool = st.tool();
        let in_a_conversation = ToolContext {
            conversation_id: Some("conv-1".to_string()),
            ..st.ctx(None, None)
        };

        tool.write(
            &serde_json::json!({"path": "a.md", "content": "a\n"}),
            &in_a_conversation,
        )
        .await
        .unwrap();
        tool.write(
            &serde_json::json!({"path": "b.md", "content": "b\n"}),
            &st.ctx(None, None),
        )
        .await
        .unwrap();

        let rows = st.artifacts();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows.iter()
                .all(|r| !r.in_project && r.project_root.is_none())
        );
        assert!(
            !st.home.root().join("scratch").exists(),
            "no scratch folder for a by-product"
        );
    }

    /// ST-11 / §15: the description says where each file lands and drops the
    /// startup folder and the persona sentence.
    #[test]
    fn file_writes_description_says_where_each_file_lands() {
        let tool = file_write_tool(None, None);
        let description = &tool.definition.description;
        for phrase in [
            "real file in the project folder",
            ".git",
            ".openalpaca",
            "its current content in this conversation's log",
            "listed in the Library with the project's results",
            "With no project",
            "by-product",
            "10MB",
        ] {
            assert!(description.contains(phrase), "{phrase}: {description}");
        }
        assert!(
            !description.contains("daemon's workspace directory"),
            "{description}"
        );
        assert!(!description.contains("SOUL.md"), "{description}");
    }

    /// What the model reads back, and can repeat: the write is done, as which
    /// artifact and version, where it is, that the Library lists it, that it
    /// is the deliverable — not a step to route around — and how to change it.
    #[tokio::test]
    async fn the_artifact_result_names_the_path_and_the_library() {
        let st = Store::new();

        let out = st
            .tool()
            .write(&pasta("# Pasta night\n"), &st.ctx(None, None))
            .await
            .unwrap();

        let row = &st.artifacts()[0];
        assert!(out.contains(PASTA), "{out}");
        assert!(out.contains("is the artifact"), "{out}");
        assert!(out.contains("no project folder"), "{out}");
        assert!(out.contains(&row.name), "{out}");
        assert!(out.contains("version 1"), "{out}");
        assert!(Path::new(&row.storage_path).is_absolute());
        assert!(out.contains(&row.storage_path), "{out}");
        assert!(out.contains("Library"), "{out}");
        assert!(out.contains("deliverable"), "{out}");
        assert!(out.contains("do not write another copy"), "{out}");
        assert!(
            out.contains(&format!(
                "To change it, call file_write with \"{PASTA}\" again"
            )),
            "{out}"
        );
    }

    /// Fixture 18 step 10, after R33 (storage design §11): a model told to
    /// `file_write` the store's control snapshot reaches no file of the store.
    /// With no project the path names a by-product in the artifact store; in
    /// a project it is the project's own `state/` folder. The real snapshot is
    /// unchanged either way.
    #[tokio::test]
    async fn a_store_path_written_from_a_conversation_never_reaches_the_store() {
        const SNAPSHOT: &str = "state/memory-control/home-32acea7eb739b08afd959db91e46d450.json";
        let st = Store::new();
        let home = st.home.root();
        let project = st.project_root();
        fs::create_dir_all(home.join("state/memory-control")).unwrap();
        fs::write(home.join(SNAPSHOT), "{\"real\":true}").unwrap();
        let args = serde_json::json!({ "path": SNAPSHOT, "content": "{}" });

        st.tool()
            .write(&args, &st.ctx(None, None))
            .await
            .expect("no project: a by-product named after the path");
        st.tool()
            .write(&args, &st.ctx(Some(&project), None))
            .await
            .expect("a project: its own state/ folder");

        assert_eq!(
            fs::read_to_string(home.join(SNAPSHOT)).unwrap(),
            "{\"real\":true}",
            "the store's snapshot is untouched"
        );
        assert_eq!(fs::read_to_string(project.join(SNAPSHOT)).unwrap(), "{}");
        // From S4a the project write is the real file with a pointer row
        // (ST-12); the by-product is the one row not in the project.
        let rows: Vec<_> = st
            .artifacts()
            .into_iter()
            .filter(|r| !r.in_project)
            .collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].summary.as_deref(), Some(SNAPSHOT));
        assert!(
            !std::path::Path::new(&rows[0].storage_path).starts_with(home.join("state")),
            "{}",
            rows[0].storage_path
        );
    }

    /// Round G's five parts (FW-1), pinned whole. From Stage 2 (S3) on,
    /// `file_read` reads the working folder — the project or the
    /// conversation's scratch folder — and never the artifact store, so part
    /// 4 says there is nothing to read back (S5 names `artifact_read` there).
    #[tokio::test]
    async fn a_by_product_result_is_pinned_whole_in_five_parts() {
        let st = Store::new();
        let home = st.home.root();

        let out = st
            .tool()
            .write(&pasta("# Pasta night\n"), &st.ctx(None, None))
            .await
            .unwrap();

        let row = &st.artifacts()[0];
        assert_eq!(
            out,
            format!(
                "Saved: {PASTA} is the artifact \"{}\", version 1, at {}. This \
                 conversation has no project folder, so its files are kept in \
                 OpenAlpaca's artifact store. It is listed in the Library. This file \
                 is the deliverable: tell the user it is saved at that absolute \
                 path, and do not write another copy with the shell or any other \
                 tool. file_read cannot open it from this conversation; what you \
                 passed is what was saved, so there is nothing to check. To change \
                 it, call file_write with \"{PASTA}\" again; that saves its next \
                 version.",
                row.name, row.storage_path
            )
        );
        assert!(
            !home.join(PASTA).exists(),
            "written into the home store's root"
        );
    }

    /// The size cap comes before the routing: an oversized body is refused on
    /// a turn with no project exactly as on one with a project, and no
    /// artifact is saved. (Its store-owned half went with R33, storage design
    /// §11: no home-store rule binds `file_write` any more.)
    #[tokio::test]
    async fn the_size_cap_is_refused_before_routing() {
        let st = Store::new();
        let tool = st.tool();
        let ctx = st.ctx(None, None);

        let err = tool
            .write(
                &serde_json::json!({"path": "big.md", "content": "x".repeat(10 * 1024 * 1024 + 1)}),
                &ctx,
            )
            .await
            .expect_err("over the write limit");
        assert!(err.contains("byte write limit"), "{err}");
        assert!(
            st.artifacts().is_empty(),
            "a refused write saved an artifact"
        );
    }

    /// The row with `summary` = `path`, among a store's artifacts.
    fn row_for<'a>(
        rows: &'a [openalpaca_storage::ArtifactRecord],
        path: &str,
    ) -> &'a openalpaca_storage::ArtifactRecord {
        rows.iter()
            .find(|row| row.summary.as_deref() == Some(path))
            .unwrap_or_else(|| panic!("no artifact for {path}: {rows:?}"))
    }

    /// The eight hex digits `artifact_name` appended to the slug of `name`, a
    /// stored head whose extension is `ext` (`.md`, `.bin`).
    fn path_hash<'a>(name: &'a str, ext: &str) -> &'a str {
        let hash = name
            .strip_suffix(ext)
            .and_then(|stem| stem.rsplit_once('-'))
            .map(|(_, hash)| hash)
            .unwrap_or_else(|| panic!("{name}"));
        assert!(
            hash.len() == 8 && hash.chars().all(|c| c.is_ascii_hexdigit()),
            "{name}"
        );
        hash
    }

    /// F1: the slug keeps ASCII letters and digits only, so two paths outside
    /// ASCII would both be `01-artifact.md` — one artifact, the first file
    /// only in its history. Each gets a hash of itself instead: two paths, two
    /// artifacts, and the same path again is still its own next version.
    #[tokio::test]
    async fn two_paths_outside_ascii_in_one_run_are_two_artifacts() {
        let st = Store::new();
        let task = "5b0e9a3c-0000-4000-8000-000000000000";
        st.task(task, "Notes in Chinese");
        let ctx = st.ctx(None, Some(task));
        let tool = st.tool();
        let write = |path: &'static str, content: &'static str| {
            let tool = &tool;
            let ctx = &ctx;
            async move {
                tool.write(&serde_json::json!({"path": path, "content": content}), ctx)
                    .await
                    .unwrap_or_else(|e| panic!("{path}: {e}"))
            }
        };

        write("文档.md", "doc v1\n").await;
        write("笔记.md", "note v1\n").await;
        let again = write("文档.md", "doc v2\n").await;

        let rows = st.artifacts();
        assert_eq!(rows.len(), 2, "two paths, two artifacts: {rows:?}");
        let doc = row_for(&rows, "文档.md");
        let note = row_for(&rows, "笔记.md");
        assert_ne!(doc.name, note.name);
        for row in [doc, note] {
            path_hash(&row.name, ".md");
            assert_eq!(row.kind, Some(openalpaca_storage::ArtifactKind::Markdown));
        }
        assert_eq!(doc.version, 2, "the same path again is its next version");
        assert!(again.contains("version 2"), "{again}");
        assert_eq!(fs::read_to_string(&doc.storage_path).unwrap(), "doc v2\n");
        assert_eq!(note.version, 1);
        assert_eq!(fs::read_to_string(&note.storage_path).unwrap(), "note v1\n");
    }

    /// The hash survives the grammar: it is appended after the slug's budget
    /// was shortened for it, so a long path keeps it; a path the grammar
    /// carries whole is left as it is; and the hash is the whole path's, so
    /// two paths that differ only in an extension the name drops stay apart.
    #[test]
    fn artifact_name_keeps_its_hash_inside_the_slug_budget() {
        assert_eq!(artifact_name(PASTA), PASTA);
        assert_eq!(artifact_name("src/main.rs"), "src/main.rs");
        assert_eq!(artifact_name("文档.md"), artifact_name("文档.md"));
        assert_ne!(artifact_name("文档.md"), artifact_name("笔记.md"));
        assert!(artifact_name("文档.md").starts_with("artifact-"));
        assert!(artifact_name("报告/plan.md").starts_with("plan-"));
        // The store drops both extensions (`.bin`), so only the titles can
        // keep these two apart: the hash covers the extension too.
        assert_ne!(
            split_name(&artifact_name("文档.sh")).0,
            split_name(&artifact_name("文档.bat")).0,
        );

        for long in [
            format!("{}文档.md", "a".repeat(100)),
            format!("{}/notes.md", "a".repeat(100)),
        ] {
            let name = artifact_name(&long);
            let (title, _) = split_name(&name);
            assert!(name.ends_with(".md"), "{name}");
            assert_eq!(
                slugify(title, FILE_NAME_SLUG_BYTES),
                title,
                "the grammar would cut the hash"
            );
            let longer = format!("a{long}");
            assert_ne!(name, artifact_name(&longer), "the hash is the path's");
        }
    }

    /// R1-1 (a): the slug is cut at 60 bytes, so every file under a directory
    /// whose own slug fills them would be one artifact — 22 such files one row
    /// at version 22, the first two files' bytes deleted by the version cap.
    /// Each gets a hash of its path instead, keeps its extension and kind, and
    /// the first written again is its own version 2.
    #[tokio::test]
    async fn two_files_of_one_deep_directory_are_two_artifacts() {
        const DIR: &str = "packages/frontend/src/components/dashboard/analytics/charts";
        let bar = format!("{DIR}/BarChart.tsx");
        let line = format!("{DIR}/LineChart.tsx");
        assert_eq!(
            slugify(split_name(&bar).0, FILE_NAME_SLUG_BYTES),
            slugify(split_name(&line).0, FILE_NAME_SLUG_BYTES),
            "the grammar alone gives both one name"
        );
        let st = Store::new();
        let task = "9d41c2e7-0000-4000-8000-000000000000";
        st.task(task, "Dashboard charts");
        let ctx = st.ctx(None, Some(task));
        let tool = st.tool();

        for (path, content) in [(&bar, "bar v1\n"), (&line, "line v1\n"), (&bar, "bar v2\n")] {
            tool.write(&serde_json::json!({"path": path, "content": content}), &ctx)
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        }

        let rows = st.artifacts();
        assert_eq!(rows.len(), 2, "two files, two artifacts: {rows:?}");
        let bar_row = row_for(&rows, &bar);
        let line_row = row_for(&rows, &line);
        for row in [bar_row, line_row] {
            assert!(
                row.name.contains("-packages-frontend-src-"),
                "the slug is still read: {}",
                row.name
            );
            path_hash(&row.name, ".tsx");
            assert_eq!(row.kind, Some(openalpaca_storage::ArtifactKind::Code));
        }
        assert_eq!(
            bar_row.version, 2,
            "the same path again is its next version"
        );
        assert_eq!(
            fs::read_to_string(&bar_row.storage_path).unwrap(),
            "bar v2\n"
        );
        assert_eq!(line_row.version, 1);
        assert_eq!(
            fs::read_to_string(&line_row.storage_path).unwrap(),
            "line v1\n"
        );
    }

    /// R1-1 (b): an extension no kind admits — or none — is stored as `.bin`
    /// and is not part of the name, so `go.mod` and `go.sum` would be one
    /// artifact's two versions. Each pair below is two artifacts, and
    /// `go.mod` written again is its own version 2.
    #[tokio::test]
    async fn two_paths_whose_extension_no_kind_keeps_are_two_artifacts() {
        const PAIRS: [(&str, &str); 4] = [
            ("go.mod", "go.sum"),
            (".env", ".env.example"),
            ("Dockerfile", "Dockerfile.dev"),
            ("scripts/build.sh", "scripts/build.bat"),
        ];
        for (a, b) in PAIRS {
            assert_eq!(
                slugify(split_name(a).0, FILE_NAME_SLUG_BYTES),
                slugify(split_name(b).0, FILE_NAME_SLUG_BYTES),
                "{a} and {b}: one title"
            );
            assert!(kind_for_file_name(a).is_none() && kind_for_file_name(b).is_none());
        }
        let st = Store::new();
        let ctx = st.ctx(None, None);
        let tool = st.tool();
        let paths = PAIRS.iter().flat_map(|(a, b)| [*a, *b]);

        for path in paths.clone().chain(["go.mod"]) {
            tool.write(&serde_json::json!({"path": path, "content": path}), &ctx)
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        }

        let rows = st.artifacts();
        assert_eq!(rows.len(), 8, "eight paths, eight artifacts: {rows:?}");
        for path in paths {
            let row = row_for(&rows, path);
            path_hash(&row.name, ".bin");
            assert_eq!(fs::read_to_string(&row.storage_path).unwrap(), path);
            let expected = if path == "go.mod" { 2 } else { 1 };
            assert_eq!(row.version, expected, "{path}");
        }
    }

    /// F3, F7: the extension picks the kind through R21's sets, and text whose
    /// extension no kind admits is `code` — text the Library can show — still
    /// stored as `.bin`, R21's refusal of active content.
    #[tokio::test]
    async fn the_extension_picks_the_kind_and_text_no_kind_admits_is_code() {
        let st = Store::new();
        let tool = st.tool();
        let ctx = st.ctx(None, None);

        for (path, content) in [("src/main.rs", "fn main() {}\n"), ("build.sh", "echo hi\n")] {
            tool.write(&serde_json::json!({"path": path, "content": content}), &ctx)
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        }

        let rows = st.artifacts();
        let code = row_for(&rows, "src/main.rs");
        assert_eq!(code.kind, Some(openalpaca_storage::ArtifactKind::Code));
        assert!(code.name.ends_with("-src-main.rs"), "{}", code.name);
        let script = row_for(&rows, "build.sh");
        assert_eq!(script.kind, Some(openalpaca_storage::ArtifactKind::Code));
        assert_eq!(script.mime_type, "text/plain");
        assert!(script.name.contains("-build-"), "{}", script.name);
        path_hash(&script.name, ".bin");
    }

    /// F8: a path whose end names no file is refused before anything else —
    /// no by-product named for nothing, and no real file either.
    #[tokio::test]
    async fn a_path_that_names_no_file_is_refused() {
        let st = Store::new();
        let project = st.project_root();
        let tool = st.tool();

        for ctx in [st.ctx(None, None), st.ctx(Some(&project), None)] {
            for path in ["", ".", "reports/", "reports/.", " ", "reports/ "] {
                let err = tool
                    .write(&serde_json::json!({"path": path, "content": "x"}), &ctx)
                    .await
                    .expect_err(path);
                assert!(err.contains("names no file"), "{path:?}: {err}");
            }
        }
        assert!(!project.join("reports").exists());
        assert!(st.artifacts().is_empty(), "{:?}", st.artifacts());
    }

    /// F9: with no owner a by-product cannot be saved, and the turn is told
    /// why. In a project the file is the user's and needs no owner — only its
    /// row does, so the result says the app could not list it (R36).
    #[tokio::test]
    async fn a_turn_with_no_owner_cannot_save_an_artifact() {
        let st = Store::new();
        let home = st.home.root();
        let project = st.project_root();
        let ownerless = |project: Option<&Path>| ToolContext {
            request_workspace_root: project.map(|p| p.to_str().unwrap().to_string()),
            ..Default::default()
        };

        let err = st
            .tool()
            .write(&pasta("# Pasta night\n"), &ownerless(None))
            .await
            .expect_err("no owner");
        assert!(
            err.contains("file_write cannot save an artifact") && err.contains("no owner context"),
            "{err}"
        );
        assert!(!home.join("artifacts").exists(), "an artifact was saved");

        let out = st
            .tool()
            .write(&pasta("# Pasta night\n"), &ownerless(Some(&project)))
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(project.join(PASTA)).unwrap(),
            "# Pasta night\n"
        );
        assert!(
            out.contains(
                "The file is written, but the app could not list it (this turn has no owner \
                 context, and every listed file belongs to an owner)."
            ),
            "{out}"
        );
    }

    // ── R40: file_list ──────────────────────────────────────────────

    async fn list(ctx: &ToolContext, path: &str) -> Result<String, String> {
        FileListTool
            .execute_with_context(&serde_json::json!({ "path": path }), ctx)
            .await
    }

    /// One folder of the project, sorted by name in byte order, dotfiles
    /// included: a file with its size, a folder with a trailing `/`.
    #[tokio::test]
    async fn file_list_lists_a_project_folder_sorted_with_sizes_and_folders() {
        let gp = crate::test_util::GuardedProject::new();
        fs::write(gp.root.join("b.txt"), "hello").unwrap();
        fs::write(gp.root.join(".env"), "K=1").unwrap();
        fs::create_dir(gp.root.join("a")).unwrap();

        let out = list(&gp.ctx(), ".").await.unwrap();

        assert_eq!(
            out,
            format!(
                ". ({}): 3 entries\n.env  3 bytes\na/\nb.txt  5 bytes",
                gp.root.display()
            )
        );
    }

    /// A link is shown as a link (`@`), never followed in the listing it
    /// appears in; listing the link's own path follows it, and a link out of
    /// the project is refused like any path that leaves it (R31).
    #[cfg(unix)]
    #[tokio::test]
    async fn file_list_marks_a_link_and_refuses_one_out_of_the_project() {
        let gp = crate::test_util::GuardedProject::new();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), gp.root.join("out")).unwrap();
        fs::create_dir(gp.root.join("docs")).unwrap();
        std::os::unix::fs::symlink(gp.root.join("docs"), gp.root.join("in")).unwrap();

        let out = list(&gp.ctx(), ".").await.unwrap();
        assert_eq!(
            out,
            format!(". ({}): 3 entries\ndocs/\nin@\nout@", gp.root.display())
        );

        let err = list(&gp.ctx(), "out").await.unwrap_err();
        assert!(err.contains("resolves outside the workspace boundary"), "{err}");
        assert_eq!(
            list(&gp.ctx(), "in").await.unwrap(),
            format!("in ({}): 0 entries", gp.root.join("docs").display())
        );
    }

    /// A subfolder lists; a file, a `..`, an old in-project `.openalpaca`
    /// (R32, ST-9) and a folder that is not there are each refused, naming why.
    #[tokio::test]
    async fn file_list_refuses_a_file_a_parent_path_and_an_old_openalpaca_folder() {
        let gp = crate::test_util::GuardedProject::new();
        fs::create_dir_all(gp.root.join("src")).unwrap();
        fs::write(gp.root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::create_dir(gp.root.join(".openalpaca")).unwrap();

        assert_eq!(
            list(&gp.ctx(), "src").await.unwrap(),
            format!(
                "src ({}): 1 entries\nmain.rs  12 bytes",
                gp.root.join("src").display()
            )
        );
        assert_eq!(
            list(&gp.ctx(), "src/main.rs").await.unwrap_err(),
            "'src/main.rs' is a file, not a folder; read it with file_read."
        );
        let err = list(&gp.ctx(), "../").await.unwrap_err();
        assert!(err.contains("Path traversal ('..') is not allowed."), "{err}");
        let err = list(&gp.ctx(), ".openalpaca").await.unwrap_err();
        assert!(
            err.contains("a .openalpaca folder inside a project is an older OpenAlpaca's"),
            "{err}"
        );
        let err = list(&gp.ctx(), "nowhere").await.unwrap_err();
        assert!(err.contains("Path resolution failed for 'nowhere'"), "{err}");
    }

    /// With no project the working folder is the conversation's scratch
    /// folder; listing it is the first use that makes it (`ensure`).
    #[tokio::test]
    async fn file_list_in_a_conversation_with_no_project_lists_its_scratch_folder() {
        let gp = crate::test_util::GuardedProject::new();
        let dir = gp
            .home_root()
            .join("scratch")
            .join(crate::session_log::session_dir_name("conv-test"));
        assert!(!dir.exists(), "nothing has made the scratch folder yet");

        let out = list(&gp.scratch_ctx(), ".").await.unwrap();

        assert_eq!(out, format!(". ({}): 0 entries", dir.display()));
        assert!(dir.is_dir(), "listing made the folder, as every acting tool does");
    }

    /// R26: reached with no context there is no conversation to name a
    /// folder, so the call is refused with the working folder's own sentence.
    #[tokio::test]
    async fn file_list_with_no_context_is_refused() {
        let err = FileListTool
            .execute(&serde_json::json!({"path": "."}))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            crate::tools::working_folder::WorkingFolderRefusal::NoContext.to_string()
        );
    }

    /// At most 500 entries, then one line saying how many more there are.
    #[tokio::test]
    async fn file_list_shows_500_entries_and_says_how_many_more() {
        let gp = crate::test_util::GuardedProject::new();
        for i in 0..502 {
            fs::write(gp.root.join(format!("f{i:03}.txt")), "").unwrap();
        }

        let out = list(&gp.ctx(), ".").await.unwrap();

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 502, "the header, 500 entries and the closing line");
        assert_eq!(lines[0], format!(". ({}): 502 entries", gp.root.display()));
        assert_eq!(lines[1], "f000.txt  0 bytes");
        assert_eq!(lines[500], "f499.txt  0 bytes");
        assert_eq!(
            lines[501],
            "... and 2 more entries not shown — list a subfolder"
        );
    }
}
