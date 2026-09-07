use crate::daemon_config::{DaemonConfig, SessionsConfig};
use crate::session_log::SnapshotSpec;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::annotations_for_builtin;
use super::helpers::{
    MAX_FILE_READ_SIZE, is_identity_path, is_soul_path, is_user_path, resolve_workspace_path,
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

        // Security: resolve path, reject traversal and symlink escapes
        let full_path = resolve_workspace_path(path, &self.workspace_root)?;

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
    /// Only `[orchestrator.sessions] snapshot_max_bytes` is read from it — the
    /// bound on a pre-edit image (§5.7). Absent (tests, the CLI) means the
    /// documented default.
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

        // Security: resolve path, reject symlink escapes BEFORE creating directories.
        // This prevents out-of-workspace directory creation via symlinked intermediates.
        let full_path = resolve_workspace_path_for_write(path, &self.workspace_root)?;

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
}

pub(super) fn file_write_tool(
    workspace_root: PathBuf,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_write".to_string(),
            description: "Write or overwrite a file in the workspace directory. Creates \
                parent directories automatically if they don't exist. Content is limited \
                to 10MB. Writing to persona files (SOUL.md, USER.md, IDENTITY.md) is \
                blocked — use update_persona instead. Returns a confirmation with the \
                byte count written."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace (e.g., 'src/main.rs', 'output/report.txt'). Absolute paths and '..' traversal are rejected."
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
                daemon_config,
            }
        }

        fn ctx(&self, session: &str) -> ToolContext {
            ToolContext {
                session_id: Some(session.to_string()),
                session_log: Some(self.service.handle_for(session)),
                agent_instance_id: Some("lead_agent::a1".to_string()),
                ..Default::default()
            }
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
            .execute(&serde_json::json!({"path": "notes.txt", "content": "new"}))
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

    /// The last component of a write path may still be a symlink out of the
    /// workspace. That file is not this session's to copy, so it is not imaged
    /// — the write behaves exactly as it did before §5.7.
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
        let dir = tempfile::tempdir().unwrap();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
            daemon_config: None,
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "subdir/nested/test.txt",
                "content": "hello"
            }))
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

        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
            daemon_config: None,
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "escape_link/evil.txt",
                "content": "should not be written"
            }))
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
}
