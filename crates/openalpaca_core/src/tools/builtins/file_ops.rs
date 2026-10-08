use crate::daemon_config::{DaemonConfig, SessionsConfig};
use crate::session_log::SnapshotSpec;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::store::{
    FILE_NAME_SLUG_BYTES, StoreScope, is_the_home_root, kind_for_file_name, slugify,
};
use openalpaca_storage::{ArtifactKind, Database};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use super::annotations_for_builtin;
use super::artifact_write::{ToolArtifact, put_tool_artifact, scope_from_context, split_name};
use super::helpers::{
    MAX_FILE_READ_SIZE, StoreAccess, is_identity_path, is_soul_path, is_store_owned_path,
    is_store_owned_resolved, is_user_path, resolve_workspace_path,
    resolve_workspace_path_for_write, validate_workspace_path,
};

// --- file_read ---

struct FileReadTool {
    workspace_root: PathBuf,
}

#[async_trait]
impl BuiltInTool for FileReadTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        // The store's `state/` is the daemon's, never the model's to read
        // (design §2): the control snapshots there map owner keys to ids.
        let refuse = |what: &str| {
            format!(
                "Reading '{path}' is refused: it is inside {what}, which belongs to the daemon's \
                 own state/ and is not workspace content."
            )
        };
        if let Some(what) = is_store_owned_path(path, StoreAccess::Read) {
            return Err(refuse(what));
        }

        // Security: resolve path, reject traversal and symlink escapes
        let full_path = resolve_workspace_path(path, &self.workspace_root)?;
        // Again on the resolved path: a symlink inside the workspace that
        // points at `state/` passes the string check above.
        if let Some(what) =
            is_store_owned_resolved(&full_path, &self.workspace_root, StoreAccess::Read)
        {
            return Err(refuse(what));
        }

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

pub(super) fn file_read_tool(workspace_root: PathBuf) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file's full contents from the workspace directory. \
                Returns the file content as a UTF-8 string. Files larger than 10MB are \
                rejected. Use this to inspect configuration files, source code, or data \
                before processing. For binary files, use shell_execute with appropriate \
                commands instead."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace (e.g., 'src/main.rs', 'config/settings.toml'). Absolute paths and '..' traversal are rejected."
                    }
                },
                "required": ["path"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"path": "src/main.rs"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileReadTool { workspace_root })),
        provides_capabilities: vec!["file_read".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("file_read"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

// --- file_write ---

struct FileWriteTool {
    workspace_root: PathBuf,
    /// The artifact store a file not written into a project folder is saved
    /// to (G2). Absent (tests, the CLI) refuses such a write at call time,
    /// as `artifact_write` does.
    db: Option<Database>,
    /// `[orchestrator.sessions] snapshot_max_bytes` — the bound on a pre-edit
    /// image (§5.7) — and `[execution.artifacts]`, the caps on a file saved as
    /// an artifact. Absent (tests, the CLI) means the documented defaults.
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
}

impl FileWriteTool {
    fn snapshot_max_bytes(&self) -> u64 {
        match self.daemon_config {
            Some(ref config) => config.load().orchestrator.sessions.snapshot_max_bytes,
            None => SessionsConfig::default().snapshot_max_bytes,
        }
    }

    /// Take §5.7's pre-edit image of `full_path`, or explain why the write
    /// must not happen.
    ///
    /// **Fail closed.** `Ok(())` means either that an image now exists in the
    /// session's `snapshots/`, committed by a `file_snapshot` record, or that
    /// this write has nothing to image — there is no session log, or the
    /// target does not exist, or it is not a regular file inside the
    /// workspace. `Err` means an image was owed and could not be taken, and
    /// the caller must not write: overwriting a file whose only copy this was
    /// is exactly what the tier exists to prevent, and doing it quietly is
    /// worse than refusing loudly.
    async fn snapshot_before_overwrite(
        &self,
        path: &str,
        full_path: &Path,
        ctx: &ToolContext,
    ) -> Result<(), String> {
        let Some(log) = ctx.session_log.as_ref() else {
            tracing::debug!(path, "file_write: no session log, so no pre-edit image");
            return Ok(());
        };
        // Only an existing regular file has a pre-edit state. A new file, a
        // directory, a device — nothing to image, and nothing to refuse.
        let Ok(metadata) = tokio::fs::metadata(full_path).await else {
            return Ok(());
        };
        if !metadata.is_file() {
            return Ok(());
        }
        // Through any symlink, and only then judged: `file_write` resolves the
        // *parent* against the workspace, so the last component can still be a
        // link out of it.
        let Ok(target) = tokio::fs::canonicalize(full_path).await else {
            return Ok(());
        };
        if !is_snapshotable(&target, &self.workspace_root, store_root().as_deref()) {
            tracing::debug!(
                path,
                target = %target.display(),
                "file_write: the target is not a workspace file, so no pre-edit image"
            );
            return Ok(());
        }

        let cap = self.snapshot_max_bytes();
        if metadata.len() > cap {
            return Err(format!(
                "Refusing to overwrite '{}': it is {} bytes, above the {} byte \
                 [orchestrator.sessions] snapshot_max_bytes limit, so no pre-edit \
                 snapshot can be kept for it. The file is unchanged. Raise \
                 snapshot_max_bytes, or copy the file aside yourself first.",
                path,
                metadata.len(),
                cap
            ));
        }

        let spec = SnapshotSpec {
            source: target,
            path: path.to_string(),
            task_id: ctx.task_id.clone(),
            span_id: None,
            agent: ctx
                .agent_instance_id
                .clone()
                .or_else(|| ctx.agent_id.clone()),
        };
        match log.snapshot(spec).await {
            Ok(_) => Ok(()),
            Err(e) => Err(format!(
                "Refusing to overwrite '{}': its pre-edit snapshot could not be \
                 taken ({}). The file is unchanged.",
                path, e
            )),
        }
    }
}

/// The home store, so a file inside it is never imaged.
///
/// `snapshots/` and `results/` live there; a session that imaged its own log
/// would grow the very bytes the caps are about. Unresolvable means there is
/// no store, and therefore no session log to have got here with.
fn store_root() -> Option<PathBuf> {
    openalpaca_storage::store::home_root()
        .ok()
        .map(|root| root.canonicalize().unwrap_or(root))
}

/// Whether a resolved write target is a workspace file worth imaging.
///
/// Two conditions, both after symlink resolution: inside the workspace root —
/// a link out of it points at a file this tool has no claim to copy — and
/// outside the home store, which is where the images themselves live.
fn is_snapshotable(target: &Path, workspace_root: &Path, store: Option<&Path>) -> bool {
    let Ok(root) = workspace_root.canonicalize() else {
        return false;
    };
    if !target.starts_with(&root) {
        return false;
    }
    !store.is_some_and(|store| target.starts_with(store))
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
}

impl FileWriteTool {
    async fn write(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        // Guard against disk exhaustion: limit write size to 10 MB
        const MAX_FILE_WRITE_SIZE: u64 = 10 * 1024 * 1024;
        if content.len() as u64 > MAX_FILE_WRITE_SIZE {
            return Err(format!(
                "Content size {} bytes exceeds the {} byte write limit",
                content.len(),
                MAX_FILE_WRITE_SIZE
            ));
        }

        // Security: reject absolute paths and .. components
        validate_workspace_path(path)?;
        // A path with no file name at its end ('', '.', 'reports/', a blank
        // one) names a directory, not a file: the plain write could not write
        // it, and an artifact would be named for nothing (`artifact.bin`).
        let last = path.rsplit(['/', '\\']).next().unwrap_or_default().trim();
        if last.is_empty() || last == "." {
            return Err(format!(
                "Path '{path}' names no file. Give a file name, e.g. 'reports/summary.md'."
            ));
        }

        // Safety: block writes to SOUL.md — use update_persona tool instead
        if is_soul_path(path) {
            return Err("Writing to SOUL.md via file_write is blocked. \
                 Use the update_persona tool with target=\"soul\" instead, \
                 which provides validation, backup, and safe atomic writes."
                .to_string());
        }

        // Safety: block writes to USER.md — use update_persona tool instead
        if is_user_path(path) {
            return Err("Writing to USER.md via file_write is blocked. \
                 Use the update_persona tool with target=\"user\" instead, \
                 which provides validation, backup, and safe atomic writes."
                .to_string());
        }

        // Safety: block writes to IDENTITY.md — use update_persona tool instead
        if is_identity_path(path) {
            return Err("Writing to IDENTITY.md via file_write is blocked. \
                 Use the update_persona tool with target=\"identity\" instead, \
                 which provides validation, backup, and safe atomic writes."
                .to_string());
        }

        // The store's own files (design §2, plan 3.2): `state/` and a memory
        // partition's control files are the service's, never file_write's.
        let refuse = |what: &str| {
            format!(
                "Writing '{path}' is refused: it is {what}, which the daemon owns. Memory notes \
                 are written with memory_store and memory_update; nothing under state/ is \
                 workspace content."
            )
        };
        if let Some(what) = is_store_owned_path(path, StoreAccess::Write) {
            return Err(refuse(what));
        }

        // G2 (FW-1): the workspace root is the daemon's starting directory,
        // never the turn's, so the plain write below is taken only on a turn
        // that carries a project and from a root that is not the home store —
        // where both launchers start the daemon, and where a relative path
        // would land owned by no content kind. Every other write is an
        // artifact, on `artifact_write`'s own store path.
        let scope = scope_from_context(ctx);
        if !matches!(scope, StoreScope::Project(_)) || is_the_home_root(&self.workspace_root) {
            return self.save_as_artifact(path, content, ctx, &scope);
        }

        // Security: resolve path, reject symlink escapes BEFORE creating directories.
        // This prevents out-of-workspace directory creation via symlinked intermediates.
        let full_path = resolve_workspace_path_for_write(path, &self.workspace_root)?;
        // Again on the resolved path: a symlink inside the workspace that
        // points at `state/` or a partition's control files passes the
        // string check above.
        if let Some(what) =
            is_store_owned_resolved(&full_path, &self.workspace_root, StoreAccess::Write)
        {
            return Err(refuse(what));
        }

        // §5.7: the bytes this write is about to destroy are imaged first, and
        // the write does not happen unless that succeeded. Everything above is
        // a refusal that never touched the file; from here on the file changes,
        // so this is the last point at which nothing has been lost yet.
        self.snapshot_before_overwrite(path, &full_path, ctx).await?;

        // Create parent directories AFTER boundary validation
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        tokio::fs::write(&full_path, content)
            .await
            .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            path
        ))
    }

    /// G2: the write is an artifact — in the project's store when the turn
    /// carries one, else in the home store — named for `path`
    /// ([`artifact_name`]), with the path as written for its summary, and
    /// the kind its extension picks (R21's sets, `kind_for_file_name`). The
    /// content is a UTF-8 string by construction, so an extension no kind
    /// admits — `build.sh`, `Makefile`, `logo.svg` — is `code`, text the
    /// Library can show; R21 still stores it as `.bin`. The same path written
    /// again in the same run is the same artifact's next version.
    fn save_as_artifact(
        &self,
        path: &str,
        content: &str,
        ctx: &ToolContext,
        scope: &StoreScope,
    ) -> Result<String, String> {
        let db = self.db.as_ref().ok_or_else(|| {
            "file_write requires database context to save an artifact".to_string()
        })?;
        let record = put_tool_artifact(
            db,
            ctx,
            scope,
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
        let kept = match scope {
            StoreScope::Project(_) => {
                "This conversation's files are kept in the project's artifact store."
            }
            StoreScope::Home => {
                "This conversation has no project folder, so its files are kept in \
                 OpenAlpaca's artifact store."
            }
        };
        let read_back = match read_back_path(&record.storage_path, &self.workspace_root) {
            Some(rel) => format!("To read it back, call file_read with \"{rel}\"."),
            None => "file_read cannot open it from this conversation; what you passed is what \
                     was saved, so there is nothing to check."
                .to_string(),
        };
        Ok(format!(
            "Saved: {path} is the artifact \"{}\", version {}, at {}. {kept} It is listed in the \
             Library. This file is the deliverable: tell the user it is saved at that absolute \
             path, and do not write another copy with the shell or any other tool. {read_back} \
             To change it, call file_write with \"{path}\" again; that saves its next version.",
            record.name, record.version, record.storage_path
        ))
    }
}

/// The path `file_read` opens a saved artifact by: its file relative to the
/// workspace root, `/`-separated, both resolved first. `None` when the file
/// is not under the root — a store seen from a root that does not contain it
/// (a daemon started outside the home store). A project's by-products are in
/// its folder under the home store's `projects/`, so from the home store they
/// are under the root. `file_read` joins the same root, so the path named is
/// one it opens.
fn read_back_path(storage_path: &str, workspace_root: &Path) -> Option<String> {
    let file = Path::new(storage_path).canonicalize().ok()?;
    let root = workspace_root.canonicalize().ok()?;
    let parts = file
        .strip_prefix(&root)
        .ok()?
        .components()
        .map(|part| match part {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
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
    workspace_root: PathBuf,
    db: Option<Database>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_write".to_string(),
            description: "Writes a file in the project when the conversation has one; \
                otherwise saves it as an artifact (it appears in the Library). With a \
                project, the path is relative to the daemon's workspace directory — \
                unless that directory is OpenAlpaca's own store, in which case the file \
                is saved as an artifact of the project instead. An artifact is named \
                after the path (plus a short hash of the path when the name cannot carry \
                it whole: a character outside ASCII, a long path, or an extension no kind \
                keeps, or none), its extension picks its kind, and writing the same path \
                again in one run saves its next version. The result says which happened. \
                An artifact is the saved file: its result gives the absolute path to \
                report to the user and, where there is one, the path file_read opens it \
                by; never copy it elsewhere with the shell. Creates parent directories \
                automatically if they don't exist. Content is limited to 10MB. Writing to \
                persona files (SOUL.md, USER.md, IDENTITY.md) is blocked — use \
                update_persona instead."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace (e.g., 'src/main.rs', 'output/report.txt'); for an artifact, its name. Absolute paths and '..' traversal are rejected."
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
        backend: ToolBackend::BuiltIn(Arc::new(FileWriteTool {
            workspace_root,
            db,
            daemon_config,
        })),
        provides_capabilities: vec!["file_write".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("file_write"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_log::{RecordType, SNAPSHOTS_DIR, SessionLogLimits, SessionLogService};
    use std::fs;
    use std::time::Duration;

    // ── §5.7: the pre-edit image `file_write` takes ─────────────────

    /// A workspace, a sessions root and a live handle for one session — the
    /// three things every snapshot test needs.
    ///
    /// `service` is `Arc`, via [`SessionLogService::into_arc`] rather than a
    /// plain `Arc::new`: that is what gives a handle a way back to the
    /// service, which is what a fix-round-1 test needs to prove a closed
    /// writer gets reopened rather than wedging every later overwrite.
    struct Bench {
        work: tempfile::TempDir,
        sessions: tempfile::TempDir,
        service: Arc<SessionLogService>,
        _home: SandboxedHome,
    }

    /// A home store of its own. G2's routing asks whether the workspace root
    /// *is* the home store, and no test may resolve the real one to answer.
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

    /// A turn that carries a project — the only kind on which `file_write`
    /// writes into its workspace root (G2). Every pre-G2 test of the plain
    /// write runs on one.
    fn project_ctx(project: &Path) -> ToolContext {
        ToolContext {
            request_workspace_root: Some(project.to_str().unwrap().to_string()),
            ..Default::default()
        }
    }

    fn bench() -> Bench {
        bench_with_limits(SessionLogLimits::default())
    }

    fn bench_with_limits(limits: SessionLogLimits) -> Bench {
        let work = tempfile::tempdir().unwrap();
        let sessions = tempfile::tempdir().unwrap();
        let service =
            SessionLogService::new(sessions.path().to_path_buf(), None, limits, "test".to_string())
                .into_arc();
        Bench {
            work,
            sessions,
            service,
            _home: SandboxedHome::new(),
        }
    }

    impl Bench {
        fn tool(&self, cap: Option<u64>) -> FileWriteTool {
            let daemon_config = cap.map(|bytes| {
                let mut config = DaemonConfig::default();
                config.orchestrator.sessions.snapshot_max_bytes = bytes;
                Arc::new(ArcSwap::from_pointee(config))
            });
            FileWriteTool {
                workspace_root: self.work.path().to_path_buf(),
                db: None,
                daemon_config,
            }
        }

        fn ctx(&self, session: &str) -> ToolContext {
            ToolContext {
                session_id: Some(session.to_string()),
                session_log: Some(self.service.handle_for(session)),
                agent_instance_id: Some("lead_agent::a1".to_string()),
                ..self.project_ctx()
            }
        }

        /// A turn with this workspace as its project and no session.
        fn project_ctx(&self) -> ToolContext {
            project_ctx(self.work.path())
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
        fs::create_dir_all(bench.work.path().join("docs")).unwrap();
        fs::write(bench.work.path().join("docs/report.md"), "the old draft").unwrap();

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "docs/report.md", "content": "the new draft"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(ctx.session_log.as_ref().unwrap().flush().await);

        assert_eq!(
            fs::read_to_string(bench.work.path().join("docs/report.md")).unwrap(),
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

    /// Off a session there is nowhere to put an image, so the write proceeds
    /// as it always did — the tier is a session facility, not a gate.
    #[tokio::test]
    async fn without_a_session_log_no_image_is_taken() {
        let bench = bench();
        let tool = bench.tool(None);
        fs::write(bench.work.path().join("notes.txt"), "old").unwrap();

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "notes.txt", "content": "new"}),
                &bench.project_ctx(),
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            fs::read_to_string(bench.work.path().join("notes.txt")).unwrap(),
            "new"
        );
        assert!(
            !bench.sessions.path().join("sess-none").exists(),
            "no session, no session directory"
        );
    }

    /// Above `snapshot_max_bytes` the write is refused, not performed
    /// unimaged — and the refusal names the limit it is asking about.
    #[tokio::test]
    async fn a_target_above_the_snapshot_cap_refuses_the_write() {
        let bench = bench();
        let tool = bench.tool(Some(8));
        let ctx = bench.ctx("sess-c");
        fs::write(bench.work.path().join("big.txt"), "far more than eight bytes").unwrap();

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
            fs::read_to_string(bench.work.path().join("big.txt")).unwrap(),
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
        fs::write(bench.work.path().join("big.bin"), &old).unwrap();

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "big.bin", "content": "new, much shorter"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok(), "a 5 MB overwrite at the default cap: {result:?}");
        assert!(ctx.session_log.as_ref().unwrap().flush().await);

        assert_eq!(
            fs::read_to_string(bench.work.path().join("big.bin")).unwrap(),
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
        fs::write(bench.work.path().join("huge.bin"), &old).unwrap();

        let refused = tool
            .execute_with_context(
                &serde_json::json!({"path": "huge.bin", "content": "clobber"}),
                &ctx,
            )
            .await
            .expect_err("above the default cap the write is refused");
        assert!(refused.contains("snapshot_max_bytes"), "{refused}");
        assert_eq!(
            fs::metadata(bench.work.path().join("huge.bin"))
                .unwrap()
                .len(),
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
        fs::write(bench.work.path().join("notes.txt"), "precious").unwrap();
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
            fs::read_to_string(bench.work.path().join("notes.txt")).unwrap(),
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
        fs::write(bench.work.path().join("notes.txt"), "first draft").unwrap();

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
            fs::read_to_string(bench.work.path().join("notes.txt")).unwrap(),
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

    /// A **known confinement hole, not a decision**: the final component of a
    /// write path is followed even when it is a symlink out of the workspace, so
    /// the write below lands outside and succeeds. `resolve_workspace_path_for_write`
    /// (`helpers/mod.rs`) canonicalizes the *parent* and re-appends the leaf
    /// verbatim, so nothing resolves the leaf — unlike the store writers'
    /// `confine_to_root` (`openalpaca_storage::store::artifact`), which
    /// canonicalizes the nearest *existing* ancestor and therefore catches an
    /// escaping symlink leaf. The plan's symlink claim (Phase 2 *Verify:*,
    /// "`confine_to_root` rejects symlinked-parent escapes") is about those
    /// writers; this helper predates it and no phase changed it. Carried as a
    /// follow-up from §5.7's own review (T56: "a symlinked target outside the
    /// workspace is still overwritten unimaged (pre-existing)").
    ///
    /// What §5.7 adds is only the second assertion: such a file is not this
    /// session's to copy, so no pre-edit image is taken. This test therefore
    /// pins today's behaviour rather than blessing it — closing the hole must
    /// flip the `is_ok` below deliberately.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_out_of_the_workspace_is_not_imaged() {
        let bench = bench();
        let tool = bench.tool(None);
        let ctx = bench.ctx("sess-e");
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("elsewhere.txt");
        fs::write(&elsewhere, "not the workspace's").unwrap();
        std::os::unix::fs::symlink(&elsewhere, bench.work.path().join("link.txt")).unwrap();

        let result = tool
            .execute_with_context(
                &serde_json::json!({"path": "link.txt", "content": "through the link"}),
                &ctx,
            )
            .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(ctx.session_log.as_ref().unwrap().flush().await);
        assert!(
            !bench.session_dir("sess-e").join(SNAPSHOTS_DIR).exists(),
            "a file outside the workspace is not imaged"
        );
    }

    /// The store holds the images themselves. A file inside it is never one,
    /// whatever the workspace root happens to be.
    #[test]
    fn a_file_inside_the_store_is_never_imaged() {
        let work = tempfile::tempdir().unwrap();
        let store = work.path().join(".openalpaca");
        fs::create_dir_all(store.join("sessions/s/results")).unwrap();
        let spill = store.join("sessions/s/results/000001-t-dump.txt");
        fs::write(&spill, "a spilled result").unwrap();
        let plain = work.path().join("ordinary.txt");
        fs::write(&plain, "a workspace file").unwrap();
        // Both roots come to the check resolved, as `store_root` hands them
        // over: on macOS `/var` is itself a link to `/private/var`.
        let store = store.canonicalize().unwrap();

        assert!(
            is_snapshotable(&plain.canonicalize().unwrap(), work.path(), Some(&store)),
            "an ordinary workspace file is imaged"
        );
        assert!(
            !is_snapshotable(&spill.canonicalize().unwrap(), work.path(), Some(&store)),
            "the store's own bytes are never imaged"
        );
        assert!(
            !is_snapshotable(Path::new("/etc/hosts"), work.path(), Some(&store)),
            "and nothing outside the workspace is"
        );
    }

    #[tokio::test]
    async fn test_file_write_blocks_soul_md() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
            db: None,
            daemon_config: None,
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "SOUL.md",
                "content": "malicious content"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("update_persona"));
    }

    #[tokio::test]
    async fn test_file_write_creates_subdirs_within_workspace() {
        let _home = SandboxedHome::new();
        let dir = tempfile::tempdir().unwrap();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
            db: None,
            daemon_config: None,
        };
        let result = tool
            .execute_with_context(
                &serde_json::json!({
                    "path": "subdir/nested/test.txt",
                    "content": "hello"
                }),
                &project_ctx(dir.path()),
            )
            .await;
        assert!(result.is_ok());
        assert!(dir.path().join("subdir/nested/test.txt").exists());
    }

    #[tokio::test]
    async fn test_file_write_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        // Create a symlink inside workspace pointing outside
        let link_path = dir.path().join("escape_link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link_path).unwrap();
        #[cfg(not(unix))]
        {
            // On non-unix, skip this test
            return;
        }

        let _home = SandboxedHome::new();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
            db: None,
            daemon_config: None,
        };
        let result = tool
            .execute_with_context(
                &serde_json::json!({
                    "path": "escape_link/evil.txt",
                    "content": "should not be written"
                }),
                &project_ctx(dir.path()),
            )
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("outside the workspace") || err_msg.contains("boundary"),
            "Should reject symlink escape, got: {}",
            err_msg
        );
        // Verify nothing was written outside
        assert!(!outside.path().join("evil.txt").exists());
    }

    #[tokio::test]
    async fn test_file_write_blocks_soul_md_in_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
            db: None,
            daemon_config: None,
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "config/SOUL.md",
                "content": "sneaky content"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("update_persona"));
    }

    // ── Memory redesign, plan 3.2: the store's own files are refused ────

    /// Design §2: the builtin file tools refuse any path whose first
    /// component is `state` (the control snapshots and the database live
    /// there, and the daemon runs in the home store), and the writers also
    /// refuse a partition's control files — `.history/`, `MEMORY.md` and
    /// `memory/.gitignore`. Fixture 18 step 10.
    #[tokio::test]
    async fn file_write_refuses_the_store_state_and_the_partition_control_files() {
        let b = bench();
        let tool = b.tool(None);
        let ctx = b.project_ctx();
        for path in [
            "state/memory-control/home-32acea7eb739b08afd959db91e46d450.json",
            "state/openalpaca.db",
            "State/anything.txt",
        ] {
            let err = tool
                .write(&serde_json::json!({"path": path, "content": "{}"}), &ctx)
                .await
                .expect_err(path);
            assert!(err.contains("state/"), "{path}: {err}");
            assert!(!b.work.path().join(path).exists(), "{path} was written");
        }
        for path in [
            ".openalpaca/memory/owners/32acea7eb739b08afd959db91e46d450/.history/x/y.md",
            "memory/owners/32acea7eb739b08afd959db91e46d450/MEMORY.md",
            "MEMORY.md",
            ".openalpaca/memory/.gitignore",
            "memory/.gitignore",
        ] {
            let err = tool
                .write(&serde_json::json!({"path": path, "content": "*\n"}), &ctx)
                .await
                .expect_err(path);
            assert!(
                err.contains("memory") || err.contains(".history") || err.contains("MEMORY.md"),
                "{path}: {err}"
            );
            assert!(!b.work.path().join(path).exists(), "{path} was written");
        }
        // Not store-owned: a `state` directory below the first component, an
        // ordinary note file, a `.gitignore` anywhere but under `memory/`.
        for path in ["src/state/notes.md", "memory/owners/x/notes/a.md", ".gitignore"] {
            tool.write(&serde_json::json!({"path": path, "content": "ok\n"}), &ctx)
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(b.work.path().join(path).exists());
        }
    }

    #[tokio::test]
    async fn file_read_refuses_the_store_state_but_not_a_partition_file() {
        let b = bench();
        let root = b.work.path().to_path_buf();
        fs::create_dir_all(root.join("state/memory-control")).unwrap();
        fs::write(root.join("state/memory-control/home.json"), "{\"owner_id\":\"secret\"}").unwrap();
        let tool = FileReadTool {
            workspace_root: root.clone(),
        };
        let err = tool
            .execute(&serde_json::json!({"path": "state/memory-control/home.json"}))
            .await
            .expect_err("state/ is refused");
        assert!(err.contains("state/"), "{err}");
        assert!(!err.contains("secret"), "{err}");
        fs::create_dir_all(root.join("memory/owners/k/.history/id")).unwrap();
        fs::write(root.join("memory/owners/k/.history/id/rev.md"), "old").unwrap();
        fs::write(root.join("memory/owners/k/MEMORY.md"), "# Memory").unwrap();
        for path in ["memory/owners/k/.history/id/rev.md", "memory/owners/k/MEMORY.md"] {
            tool.execute(&serde_json::json!({"path": path}))
                .await
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        }
    }

    /// Defence in depth: the argument check reads the string, and a symlink
    /// inside the workspace that points at `state/` canonicalizes under the
    /// root, so both tools run the check again on the path the resolution
    /// landed on. Unix only: the test needs a symlink.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_into_the_store_state_is_refused_after_resolution() {
        let b = bench();
        let root = b.work.path().to_path_buf();
        fs::create_dir_all(root.join("state/memory-control")).unwrap();
        fs::write(
            root.join("state/memory-control/home.json"),
            "{\"owner_id\":\"secret\"}",
        )
        .unwrap();
        std::os::unix::fs::symlink(root.join("state"), root.join("link")).unwrap();

        let read = FileReadTool {
            workspace_root: root.clone(),
        };
        let err = read
            .execute(&serde_json::json!({"path": "link/memory-control/home.json"}))
            .await
            .expect_err("a symlink into state/ is refused on read");
        assert!(err.contains("state/"), "{err}");
        assert!(!err.contains("secret"), "{err}");

        let write = b.tool(None);
        let err = write
            .write(
                &serde_json::json!({"path": "link/memory-control/x.json", "content": "{}"}),
                &b.project_ctx(),
            )
            .await
            .expect_err("a symlink into state/ is refused on write");
        assert!(err.contains("state/"), "{err}");
        assert!(
            !root.join("state/memory-control/x.json").exists(),
            "written through the link"
        );
    }

    /// **S3-R1 (plan §7a), the probe.** A link into `state/` followed by a
    /// directory that does not exist yet: the write resolver's absent-parent
    /// branch used to return the joined path unresolved, so the string check
    /// saw `link/…`, the resolved-path check saw `link/…` too, and
    /// `create_dir_all` followed the link — the write landed in
    /// `state/newdir/x.json`. The nearest existing ancestor is resolved now,
    /// and the store check runs on what it resolved to.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_into_state_before_a_new_directory_is_refused() {
        let b = bench();
        let root = b.work.path().to_path_buf();
        fs::create_dir_all(root.join("state")).unwrap();
        std::os::unix::fs::symlink("state", root.join("link")).unwrap();

        let err = b
            .tool(None)
            .write(
                &serde_json::json!({"path": "link/newdir/x.json", "content": "{}"}),
                &b.project_ctx(),
            )
            .await
            .expect_err("a link into state/ before a new directory is refused");
        assert!(err.contains("state/"), "{err}");
        assert!(
            !root.join("state/newdir").exists(),
            "a directory was created through the link"
        );
    }

    /// **S3-R1, the other half.** The same branch let a link to anywhere
    /// leave the workspace, as long as the directory after it was new: its
    /// boundary check compared the unresolved join with the root, which it
    /// always starts with.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_out_of_the_workspace_before_a_new_directory_is_refused() {
        let b = bench();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), b.work.path().join("link")).unwrap();

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

    // ── G2 (FW-1): a file not written into a project folder is an artifact ──

    const OWNER: &str = "owner-g2";
    const PASTA: &str = "reports/pasta-night.md";

    /// A sandboxed home store, a database, a workspace that is not the home
    /// store and a project — every placement G2 chooses between.
    struct Store {
        home: SandboxedHome,
        work: tempfile::TempDir,
        project: tempfile::TempDir,
        _db_dir: tempfile::TempDir,
        db: Database,
    }

    impl Store {
        fn new() -> Self {
            let home = SandboxedHome::new();
            // A project scope is a registered project's (design §3, R12) —
            // registered under this store's home, never the developer's.
            let project = tempfile::tempdir().unwrap();
            openalpaca_storage::store::projects::register(&project.path().canonicalize().unwrap())
                .unwrap();
            let db_dir = tempfile::tempdir().unwrap();
            let db = Database::open(&db_dir.path().join("test.db")).unwrap();
            Self {
                home,
                work: tempfile::tempdir().unwrap(),
                project,
                _db_dir: db_dir,
                db,
            }
        }

        /// `file_write` with `root` as its workspace root, as the daemon
        /// builds it from its starting directory.
        fn tool_at(&self, root: &Path) -> FileWriteTool {
            FileWriteTool {
                workspace_root: root.to_path_buf(),
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

    /// The re-test's defect 1: with no project on the turn the file is an
    /// artifact in the home store — whatever directory the daemon started in,
    /// the home store itself included — and nothing lands at `<root>/<path>`.
    #[tokio::test]
    async fn without_a_project_the_file_is_saved_as_an_artifact_in_the_home_store() {
        let st = Store::new();
        let home = st.home.root();
        let work = st.work.path().to_path_buf();

        for (root, content) in [
            (&work, "# Pasta night\n"),
            (&home, "# Pasta night, again\n"),
        ] {
            st.tool_at(root)
                .write(&pasta(content), &st.ctx(None, None))
                .await
                .unwrap_or_else(|e| panic!("{}: {e}", root.display()));
            assert!(
                !root.join(PASTA).exists(),
                "the file landed at {}",
                root.join(PASTA).display()
            );
        }

        let rows = st.artifacts();
        assert_eq!(rows.len(), 1, "one path, one artifact: {rows:?}");
        let row = &rows[0];
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

    /// The packaged app: the daemon runs in the home store, so a turn that
    /// carries a project still cannot write into its workspace root — the
    /// file goes to the project's own store instead.
    #[tokio::test]
    async fn a_project_turn_whose_root_is_the_home_store_saves_into_the_projects_store() {
        let st = Store::new();
        let home = st.home.root();
        let project = st.project_root();

        let out = st
            .tool_at(&home)
            .write(&pasta("# Pasta night\n"), &st.ctx(Some(&project), None))
            .await
            .unwrap();

        assert!(
            !home.join(PASTA).exists(),
            "written into the home store's root"
        );
        assert!(
            !project.join(PASTA).exists(),
            "written into the project folder"
        );
        let rows = st.artifacts();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].project_root.as_deref(), project.to_str());
        assert!(
            Path::new(&rows[0].storage_path).starts_with(
                openalpaca_storage::store::store_root(&StoreScope::Project(project.clone()))
                    .unwrap()
                    .join("artifacts")
            ),
            "{}",
            rows[0].storage_path
        );
        assert!(out.contains(&rows[0].storage_path), "{out}");
        assert!(out.contains("Library"), "{out}");
        assert!(out.contains("the project's artifact store"), "{out}");
        assert!(
            !out.contains("no project folder"),
            "the turn carries a project: {out}"
        );
        // The project's folder is under the home store `file_read` reads from.
        let name = openalpaca_storage::store::projects::folder_name(&project);
        assert!(
            out.contains(&format!(
                "To read it back, call file_read with \"projects/{name}/artifacts/"
            )),
            "{out}"
        );
        assert!(!out.contains("file_read cannot open it"), "{out}");
    }

    /// The root is compared resolved: a workspace root that is a link to the
    /// home store is the home store, and the file is still an artifact.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_workspace_root_that_links_to_the_home_store_counts_as_the_home_store() {
        let st = Store::new();
        let home = st.home.root();
        let links = tempfile::tempdir().unwrap();
        let link = links.path().join("home-link");
        std::os::unix::fs::symlink(&home, &link).unwrap();
        let project = st.project_root();

        st.tool_at(&link)
            .write(&pasta("# Pasta night\n"), &st.ctx(Some(&project), None))
            .await
            .unwrap();

        assert!(!home.join(PASTA).exists(), "written through the link");
        let rows = st.artifacts();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].project_root.as_deref(), project.to_str());
    }

    /// The case G2 leaves as it was: a project on the turn and a workspace
    /// root that is not the home store is the plain write, and no artifact.
    #[tokio::test]
    async fn a_project_turn_with_a_workspace_root_writes_the_plain_file() {
        let st = Store::new();
        let project = st.project_root();

        let out = st
            .tool_at(&project)
            .write(&pasta("# Pasta night\n"), &st.ctx(Some(&project), None))
            .await
            .unwrap();

        assert_eq!(
            fs::read_to_string(project.join(PASTA)).unwrap(),
            "# Pasta night\n"
        );
        assert!(out.starts_with("Successfully wrote"), "{out}");
        assert!(
            st.artifacts().is_empty(),
            "the plain write records no artifact"
        );
        assert!(
            !project.join(".openalpaca").exists(),
            "the plain write seeds no project store"
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
        let tool = st.tool_at(st.work.path());

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

    /// What the model reads back, and can repeat: the write is done, as which
    /// artifact and version, where it is, that the Library lists it, that it
    /// is the deliverable — not a step to route around — and how to change it.
    #[tokio::test]
    async fn the_artifact_result_names_the_path_and_the_library() {
        let st = Store::new();

        let out = st
            .tool_at(st.work.path())
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

    /// The path a result tells the model to read the artifact back by.
    fn read_back_in(out: &str) -> &str {
        let (_, rest) = out
            .split_once("To read it back, call file_read with \"")
            .unwrap_or_else(|| panic!("no read-back path: {out}"));
        rest.split_once('"')
            .unwrap_or_else(|| panic!("an unclosed read-back path: {out}"))
            .0
    }

    /// Round G's live check, defect 2. From the home store — where both
    /// launchers start the daemon — the result names the path `file_read`
    /// opens the artifact by, and `file_read` on that path, from the same
    /// root and turn, returns what was written; written again, the path its
    /// result names returns the second content. The first result is pinned
    /// whole: the five parts, in order.
    #[tokio::test]
    async fn from_the_home_store_the_result_names_a_path_file_read_opens() {
        let st = Store::new();
        let home = st.home.root();
        let ctx = st.ctx(None, None);
        let write = st.tool_at(&home);
        let read = FileReadTool {
            workspace_root: home.clone(),
        };

        for (i, content) in ["# Pasta night\n", "# Pasta night, again\n"]
            .into_iter()
            .enumerate()
        {
            let out = write.write(&pasta(content), &ctx).await.unwrap();
            assert!(!out.contains("file_read cannot open"), "{out}");
            let rel = read_back_in(&out);
            let read_back = read
                .execute_with_context(&serde_json::json!({ "path": rel }), &ctx)
                .await
                .unwrap_or_else(|e| panic!("file_read({rel}): {e}"));
            assert_eq!(read_back, content, "{out}");

            if i == 0 {
                let row = &st.artifacts()[0];
                assert_eq!(
                    out,
                    format!(
                        "Saved: {PASTA} is the artifact \"{}\", version 1, at {}. This \
                         conversation has no project folder, so its files are kept in \
                         OpenAlpaca's artifact store. It is listed in the Library. This file \
                         is the deliverable: tell the user it is saved at that absolute \
                         path, and do not write another copy with the shell or any other \
                         tool. To read it back, call file_read with \"{rel}\". To change it, \
                         call file_write with \"{PASTA}\" again; that saves its next version.",
                        row.name, row.storage_path
                    )
                );
            }
        }
        assert!(
            !home.join(PASTA).exists(),
            "written into the home store's root"
        );
    }

    /// From a workspace root that is not the home store the artifact is out
    /// of `file_read`'s reach, and the result says so rather than name a path.
    #[tokio::test]
    async fn from_a_root_that_is_not_the_home_store_the_result_says_file_read_cannot_open_it() {
        let st = Store::new();

        let out = st
            .tool_at(st.work.path())
            .write(&pasta("# Pasta night\n"), &st.ctx(None, None))
            .await
            .unwrap();

        assert!(
            out.contains("file_read cannot open it from this conversation"),
            "{out}"
        );
        assert!(!out.contains("To read it back"), "{out}");
    }

    /// The refusals come before the routing: a store-owned path and an
    /// oversized body are refused on a turn with no project exactly as on one
    /// with a project, and no artifact is saved.
    #[tokio::test]
    async fn a_store_owned_path_and_the_size_cap_are_refused_before_routing() {
        let st = Store::new();
        let tool = st.tool_at(st.work.path());
        let ctx = st.ctx(None, None);

        for path in [
            "state/memory-control/home.json",
            "memory/.gitignore",
            "notes/MEMORY.md",
        ] {
            let err = tool
                .write(&serde_json::json!({"path": path, "content": "{}"}), &ctx)
                .await
                .expect_err(path);
            assert!(err.contains("which the daemon owns"), "{path}: {err}");
        }
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
        let tool = st.tool_at(st.work.path());
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
        let tool = st.tool_at(st.work.path());

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
        let tool = st.tool_at(st.work.path());
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
        let tool = st.tool_at(st.work.path());
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

    /// F8: a path whose end names no file is refused before the routing — no
    /// artifact named for nothing, and no plain write either.
    #[tokio::test]
    async fn a_path_that_names_no_file_is_refused() {
        let st = Store::new();
        let project = st.project_root();

        for (root, ctx) in [
            (st.work.path().to_path_buf(), st.ctx(None, None)),
            (project.clone(), st.ctx(Some(&project), None)),
        ] {
            for path in ["", ".", "reports/", "reports/.", " ", "reports/ "] {
                let err = st
                    .tool_at(&root)
                    .write(&serde_json::json!({"path": path, "content": "x"}), &ctx)
                    .await
                    .expect_err(path);
                assert!(err.contains("names no file"), "{path:?}: {err}");
            }
            assert!(!root.join("reports").exists(), "{}", root.display());
        }
        assert!(st.artifacts().is_empty(), "{:?}", st.artifacts());
    }

    /// F9: a turn with no owner — `POST /v1/command {"command":"process"}`,
    /// which runs as the system — cannot save an artifact, and is told why;
    /// its plain write, which needs no owner, is unchanged.
    #[tokio::test]
    async fn a_turn_with_no_owner_cannot_save_an_artifact() {
        let st = Store::new();
        let home = st.home.root();
        let work = st.work.path().to_path_buf();
        let project = st.project_root();
        let ownerless = |project: Option<&Path>| ToolContext {
            request_workspace_root: project.map(|p| p.to_str().unwrap().to_string()),
            ..Default::default()
        };

        for (root, ctx) in [(&work, ownerless(None)), (&home, ownerless(Some(&project)))] {
            let err = st
                .tool_at(root)
                .write(&pasta("# Pasta night\n"), &ctx)
                .await
                .expect_err("no owner");
            assert!(
                err.contains("file_write cannot save an artifact")
                    && err.contains("no owner context"),
                "{err}"
            );
            assert!(!root.join(PASTA).exists());
        }
        assert!(!home.join("artifacts").exists(), "an artifact was saved");
        assert!(
            !project.join(".openalpaca").exists(),
            "an artifact was saved"
        );

        let out = st
            .tool_at(&project)
            .write(&pasta("# Pasta night\n"), &ownerless(Some(&project)))
            .await
            .unwrap();
        assert!(out.starts_with("Successfully wrote"), "{out}");
    }
}
