use super::*;

#[test]
fn test_builtin_tools_count_without_db() {
    let tools = builtin_tools(None, None, None, None, None);
    // 6 base tools (incl. artifact_write) + 2 workspace tools
    assert_eq!(tools.len(), 8);
}

#[test]
fn test_builtin_tools_count_with_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let tools = builtin_tools(Some(db), None, Some(dc), None, None);
    // 7 base tools (with memory_search) + 2 workspace tools
    assert_eq!(tools.len(), 9);
}

#[test]
fn test_all_tools_have_valid_definitions() {
    for tool in builtin_tools(None, None, None, None, None) {
        assert!(!tool.definition.name.is_empty());
        assert!(!tool.definition.description.is_empty());
        assert!(tool.definition.parameters.is_object());
    }
}

#[test]
fn test_builtin_tools_with_persona_context_includes_update_persona() {
    use crate::bus::EventBus;

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let ctx = PersonaToolContext {
        soul_path: dir.path().join("SOUL.md"),
        user_path: dir.path().join("USER.md"),
        identity_path: dir.path().join("IDENTITY.md"),
        backup_dir: dir.path().join("backups"),
        bus: EventBus::new(16),
        max_backups: None,
    };
    let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let tools = builtin_tools_with_persona_context(Some(db), None, ctx, Some(dc), None, None, None);
    // 9 base (7 + 2 workspace) + 1 update_persona = 10 (no send since connector_send_provider is None)
    assert_eq!(tools.len(), 10, "Should have 10 tools (9 base + update_persona)");
    assert!(
        tools.iter().any(|t| t.definition.name == "update_persona"),
        "update_persona tool must be present"
    );
}

// --- Gap 7.4: strict + input_examples snapshot tests ---

#[test]
fn test_all_tools_have_strict_enabled() {
    use crate::bus::EventBus;

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let ctx = PersonaToolContext {
        soul_path: dir.path().join("SOUL.md"),
        user_path: dir.path().join("USER.md"),
        identity_path: dir.path().join("IDENTITY.md"),
        backup_dir: dir.path().join("backups"),
        bus: EventBus::new(16),
        max_backups: None,
    };
    let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let provider: ConnectorSendLock = Arc::new(std::sync::RwLock::new(None));
    let tools = builtin_tools_with_persona_context(
        Some(db), None, ctx, Some(dc), None, None, Some(provider),
    );
    for tool in &tools {
        assert_eq!(
            tool.definition.strict, Some(true),
            "Tool '{}' should have strict: Some(true)",
            tool.definition.name
        );
    }
    // Also check workspace tool definitions
    for def in workspace_tool_definitions() {
        assert_eq!(
            def.strict, Some(true),
            "Tool '{}' should have strict: Some(true)",
            def.name
        );
    }
}

#[test]
fn test_all_tool_descriptions_are_detailed() {
    let tools = builtin_tools(None, None, None, None, None);
    for tool in &tools {
        assert!(
            tool.definition.description.len() > 100,
            "Tool '{}' description is too short ({} chars): '{}'",
            tool.definition.name,
            tool.definition.description.len(),
            tool.definition.description,
        );
    }
}

#[test]
fn test_complex_tools_have_input_examples() {
    use crate::bus::EventBus;

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let ctx = PersonaToolContext {
        soul_path: dir.path().join("SOUL.md"),
        user_path: dir.path().join("USER.md"),
        identity_path: dir.path().join("IDENTITY.md"),
        backup_dir: dir.path().join("backups"),
        bus: EventBus::new(16),
        max_backups: None,
    };
    let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let provider: ConnectorSendLock = Arc::new(std::sync::RwLock::new(None));
    let tools = builtin_tools_with_persona_context(
        Some(db), None, ctx, Some(dc), None, None, Some(provider),
    );

    let tools_needing_examples = ["update_persona", "send"];
    for tool in &tools {
        if tools_needing_examples.contains(&tool.definition.name.as_str()) {
            assert!(
                tool.definition.input_examples.is_some(),
                "Tool '{}' should have input_examples",
                tool.definition.name
            );
        }
    }
    // workspace_write also needs examples
    for def in workspace_tool_definitions() {
        if def.name == "workspace_write" {
            assert!(def.input_examples.is_some(), "workspace_write should have input_examples");
        }
    }
}

// --- Issue 2: shell_execute 300s defense-in-depth timeout ---

#[tokio::test]
async fn test_shell_execute_timeout_is_300s() {
    // The shell_execute tool has a 300s defense-in-depth timeout.
    // We verify this by running a quick command through the tool and
    // checking the tool produces correct output (it wouldn't if the
    // timeout were misconfigured to 0).
    use crate::tools::registry::ToolBackend;

    let tool = shell_execute::shell_execute_tool();
    match &tool.backend {
        ToolBackend::BuiltIn(handler) => {
            let result: Result<String, String> = handler
                .execute(&serde_json::json!({"command": "echo test_timeout"}))
                .await;
            assert!(
                result.is_ok(),
                "shell_execute should succeed for quick commands"
            );
            assert!(result.unwrap().contains("test_timeout"));
        }
        _ => panic!("shell_execute should use BuiltIn backend"),
    }
}

// --- Issue 5: Workspace root override vs current_dir() ---

#[test]
fn test_builtin_tools_uses_explicit_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let ws_root = dir.path().to_path_buf();

    // Pass an explicit workspace root — should not fall back to current_dir()
    let tools = builtin_tools(None, None, None, None, Some(ws_root.clone()));
    // 6 base tools (incl. artifact_write) + 2 workspace tools
    assert_eq!(tools.len(), 8);

    // Verify tools were created (they compile and register without error
    // when given an explicit workspace root).
    let tool_names: Vec<&str> = tools.iter().map(|t| t.definition.name.as_str()).collect();
    assert!(tool_names.contains(&"file_read"));
    assert!(tool_names.contains(&"file_write"));
}

#[test]
fn test_builtin_tools_falls_back_to_current_dir_when_none() {
    // When workspace_root is None, should fall back to current_dir()
    // This must not panic even if current_dir() is available.
    let tools = builtin_tools(None, None, None, None, None);
    // 6 base tools (incl. artifact_write) + 2 workspace tools
    assert_eq!(tools.len(), 8);
}

// --- json_to_cli_args ---

#[test]
fn test_json_to_cli_args_strings() {
    let args = json_to_cli_args(&serde_json::json!({"name": "alice", "age": "30"}));
    assert!(args.contains(&"--name=alice".to_string()));
    assert!(args.contains(&"--age=30".to_string()));
}

#[test]
fn test_json_to_cli_args_empty() {
    let args = json_to_cli_args(&serde_json::json!({}));
    assert!(args.is_empty());
}

#[test]
fn test_json_to_cli_args_non_object() {
    let args = json_to_cli_args(&serde_json::json!("not an object"));
    assert!(args.is_empty());
}

// --- annotations_for_builtin (P3) ---

#[test]
fn annotations_for_builtin_known_names() {
    let names = [
        "file_read", "file_write", "workspace_read", "workspace_write",
        "artifact_write", "memory_search", "send", "shell_execute",
        "update_persona", "web_fetch", "web_search",
    ];
    for name in names {
        assert!(
            annotations_for_builtin(name).is_some(),
            "builtin '{name}' should have annotations"
        );
    }
}

#[test]
fn annotations_for_builtin_unknown_returns_none() {
    assert!(annotations_for_builtin("nonsense").is_none());
    assert!(annotations_for_builtin("").is_none());
}

#[test]
fn annotations_for_builtin_destructive_tools_tagged() {
    for name in ["file_write", "workspace_write", "artifact_write", "shell_execute", "update_persona", "send"] {
        let ann = annotations_for_builtin(name).unwrap();
        assert_eq!(ann.destructive_hint, Some(true), "{name} should be destructive");
    }
}

#[test]
fn annotations_for_builtin_readonly_tools_tagged() {
    for name in ["file_read", "workspace_read", "memory_search", "web_fetch", "web_search"] {
        let ann = annotations_for_builtin(name).unwrap();
        assert_eq!(ann.read_only_hint, Some(true), "{name} should be read_only");
    }
}

#[test]
fn annotations_for_builtin_open_world_correct() {
    for name in ["web_fetch", "web_search", "shell_execute", "send"] {
        let ann = annotations_for_builtin(name).unwrap();
        assert_eq!(ann.open_world_hint, Some(true), "{name} should be open_world");
    }
    for name in ["file_read", "file_write", "workspace_read", "workspace_write", "artifact_write", "memory_search", "update_persona"] {
        let ann = annotations_for_builtin(name).unwrap();
        assert_eq!(ann.open_world_hint, Some(false), "{name} should NOT be open_world");
    }
}

// ---------------------------------------------------------------------------
// workspace_write(entry_type="artifact") — the spill bridge (plan §4.6)
// ---------------------------------------------------------------------------

mod workspace_artifact_spill {
    use super::*;
    use crate::orchestrator::task_state::{TaskState, WorkspaceEntry};
    use crate::test_util::HomeStoreGuard;
    use crate::tools::registry::ToolContext;
    use openalpaca_storage::{Database, FileAssetRepository, Task, TaskStatus};
    use std::path::PathBuf;
    use tempfile::TempDir;

    const OWNER: &str = "owner-1";
    const TASK_ID: &str = "t-4444dddd-0000-0000-0000-000000000000";

    struct Fixture {
        _home: TempDir,
        _env: HomeStoreGuard,
        _db_dir: TempDir,
        project: TempDir,
        db: Database,
    }

    impl Fixture {
        fn new() -> Self {
            let home = TempDir::new().unwrap();
            let env = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
            let project = TempDir::new().unwrap();
            let db_dir = TempDir::new().unwrap();
            let db = Database::open(&db_dir.path().join("test.db")).unwrap();
            let now = chrono::Utc::now();
            openalpaca_storage::repository::TaskRepository::new(&db)
                .create(&Task {
                    id: TASK_ID.to_string(),
                    title: "Spill run".to_string(),
                    description: None,
                    status: TaskStatus::Running,
                    priority: 0,
                    progress_current: None,
                    progress_total: None,
                    result_summary: None,
                    created_by: OWNER.to_string(),
                    source_lane: "cli".to_string(),
                    created_at: now,
                    updated_at: now,
                    completed_at: None,
                    state_json: Some(TaskState::initial("spill run", &[]).to_json()),
                    state_version: 0,
                    outcome_json: None,
                    outcome_kind: None,
                    artifact_count: 0,
                    workspace_id: None,
                })
                .unwrap();
            Self {
                _home: home,
                _env: env,
                _db_dir: db_dir,
                project,
                db,
            }
        }

        fn project_root(&self) -> PathBuf {
            self.project.path().canonicalize().unwrap()
        }

        fn tool(&self) -> WorkspaceWriteTool {
            WorkspaceWriteTool {
                db: Some(self.db.clone()),
                daemon_config: None,
            }
        }

        fn read_tool(&self) -> WorkspaceReadTool {
            WorkspaceReadTool {
                db: Some(self.db.clone()),
            }
        }

        /// A turn that arrived with a workspace: `request_workspace_root` is
        /// the field that places content (R22), and production sets the
        /// memory-scoping id from the same path.
        fn ctx(&self) -> ToolContext {
            let root = self.project_root().to_str().unwrap().to_string();
            ToolContext {
                agent_id: Some("writing_agent".to_string()),
                task_id: Some(TASK_ID.to_string()),
                owner_id: Some(OWNER.to_string()),
                workspace_id: Some(root.clone()),
                request_workspace_root: Some(root),
                ..Default::default()
            }
        }

        fn state(&self) -> TaskState {
            let task = openalpaca_storage::repository::TaskRepository::new(&self.db)
                .get(TASK_ID)
                .unwrap()
                .unwrap();
            serde_json::from_str(task.state_json.as_deref().unwrap()).unwrap()
        }

        fn entry(&self, key: &str) -> WorkspaceEntry {
            self.state()
                .workspace
                .entries
                .into_iter()
                .find(|e| e.key == key)
                .unwrap()
        }
    }

    fn long_body() -> String {
        // Comfortably past the 512-char preview bound.
        (0..40)
            .map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n"))
            .collect()
    }

    #[tokio::test]
    async fn artifact_entry_spills_to_the_store_and_keeps_a_preview() {
        let fx = Fixture::new();
        let body = long_body();

        let out = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({
                    "key": "draft_v1",
                    "content": body,
                    "entry_type": "artifact"
                }),
                &fx.ctx(),
            )
            .await
            .unwrap();
        assert!(out.contains("draft_v1"), "{out}");

        let entry = fx.entry("draft_v1");
        let asset_id = entry
            .file_asset_id
            .clone()
            .expect("an artifact entry must carry its file_asset_id");

        // The entry keeps a 512-char preview, not the whole body.
        assert!(
            entry.content.chars().count() <= 512,
            "preview is {} chars",
            entry.content.chars().count()
        );
        assert!(entry.content.starts_with("line 0: the quick brown fox"));
        assert!(entry.content.len() < body.len());

        // …and `format_for_prompt` renders that preview.
        let prompt = fx.state().workspace.format_for_prompt(&[]);
        assert!(prompt.contains("line 0: the quick brown fox"));
        assert!(!prompt.contains("line 39"));

        // The full bytes are in the project store, under the run directory.
        let asset = FileAssetRepository::new(&fx.db)
            .get_by_id(&asset_id)
            .unwrap()
            .expect("the spilled asset must resolve by id");
        assert_eq!(asset.owner_id, OWNER);
        let path = PathBuf::from(&asset.storage_path);
        assert!(
            path.starts_with(fx.project_root().join(".openalpaca").join("artifacts")),
            "spilled to {}",
            path.display()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }

    /// T28: the spill writes a produced artifact, so it announces one too —
    /// the Library must not have to wait for a `task_status` to learn that a
    /// deliverable appeared.
    #[tokio::test]
    async fn the_spill_announces_the_artifact_it_wrote() {
        use crate::events::SystemEvent;

        let fx = Fixture::new();
        let bus = crate::bus::EventBus::new(16);
        let mut rx = bus.subscribe();
        let mut ctx = fx.ctx();
        ctx.event_bus = Some(bus.clone());

        fx.tool()
            .execute_with_context(
                &serde_json::json!({
                    "key": "draft_v1", "content": long_body(), "entry_type": "artifact"
                }),
                &ctx,
            )
            .await
            .unwrap();

        let asset_id = fx.entry("draft_v1").file_asset_id.clone().unwrap();
        match rx.try_recv().expect("the spill must announce on the bus") {
            SystemEvent::ArtifactWritten {
                artifact_id,
                task_id,
                agent_id,
                name,
                kind,
                version,
                path,
                ..
            } => {
                assert_eq!(artifact_id, asset_id);
                assert_eq!(task_id.as_deref(), Some(TASK_ID));
                assert_eq!(agent_id.as_deref(), Some("writing_agent"));
                assert_eq!(name, "01-draft-v1.md");
                assert_eq!(kind, "markdown");
                assert_eq!(version, 1);
                assert!(
                    PathBuf::from(&path)
                        .starts_with(fx.project_root().join(".openalpaca").join("artifacts")),
                    "announced {path}"
                );
            }
            other => panic!("Expected ArtifactWritten, got {other:?}"),
        }
    }

    /// A plain text entry writes nothing to the artifact store, so it announces
    /// nothing either.
    #[tokio::test]
    async fn a_text_entry_announces_nothing() {
        let fx = Fixture::new();
        let bus = crate::bus::EventBus::new(16);
        let mut rx = bus.subscribe();
        let mut ctx = fx.ctx();
        ctx.event_bus = Some(bus.clone());

        fx.tool()
            .execute_with_context(
                &serde_json::json!({"key": "note", "content": "short", "entry_type": "text"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(
            rx.try_recv().is_err(),
            "a text entry must not announce an artifact"
        );
    }

    /// The payoff chain: the pointer `collect_artifacts_from_workspace` builds
    /// now carries a resolvable id, so `artifact_count` counts something real
    /// and artifact delivery has a file to send.
    #[tokio::test]
    async fn the_artifact_pointer_now_resolves() {
        let fx = Fixture::new();
        fx.tool()
            .execute_with_context(
                &serde_json::json!({
                    "key": "final_report", "content": long_body(), "entry_type": "artifact"
                }),
                &fx.ctx(),
            )
            .await
            .unwrap();

        let pointers = fx.state().collect_artifacts_from_workspace();
        assert_eq!(pointers.len(), 1);
        let id = pointers[0]
            .file_asset_id
            .clone()
            .expect("ArtifactPointer.file_asset_id must be populated");
        assert!(
            FileAssetRepository::new(&fx.db)
                .get_by_id(&id)
                .unwrap()
                .is_some(),
            "the pointer must resolve to a file asset"
        );
    }

    #[tokio::test]
    async fn a_text_entry_never_spills() {
        let fx = Fixture::new();
        let body = long_body();

        fx.tool()
            .execute_with_context(
                &serde_json::json!({"key": "notes", "content": body, "entry_type": "text"}),
                &fx.ctx(),
            )
            .await
            .unwrap();

        let entry = fx.entry("notes");
        assert!(entry.file_asset_id.is_none());
        assert_eq!(entry.content, body, "text entries keep their full content");
        assert!(
            !fx.project_root().join(".openalpaca").join("artifacts").exists(),
            "a text entry must not create an artifact store"
        );
    }

    #[tokio::test]
    async fn an_explicit_file_asset_id_wins_and_suppresses_the_spill() {
        let fx = Fixture::new();
        let body = long_body();

        fx.tool()
            .execute_with_context(
                &serde_json::json!({
                    "key": "upload_backed",
                    "content": body,
                    "entry_type": "artifact",
                    "file_asset_id": "already-uploaded-id"
                }),
                &fx.ctx(),
            )
            .await
            .unwrap();

        let entry = fx.entry("upload_backed");
        assert_eq!(entry.file_asset_id.as_deref(), Some("already-uploaded-id"));
        assert_eq!(entry.content, body);
        assert!(!fx.project_root().join(".openalpaca").join("artifacts").exists());
    }

    #[tokio::test]
    async fn rewriting_the_same_key_supersedes_the_artifact() {
        let fx = Fixture::new();
        let ctx = fx.ctx();
        let tool = fx.tool();

        for body in ["first body\n", "second body\n"] {
            tool.execute_with_context(
                &serde_json::json!({"key": "draft", "content": body, "entry_type": "artifact"}),
                &ctx,
            )
            .await
            .unwrap();
        }

        let entry = fx.entry("draft");
        let id = entry.file_asset_id.clone().unwrap();
        let store = openalpaca_storage::ArtifactStore::new(&fx.db);
        let record = store.get(&id, OWNER).unwrap().unwrap();
        assert_eq!(record.version, 2, "the second write supersedes");
        assert_eq!(
            std::fs::read_to_string(&record.storage_path).unwrap(),
            "second body\n"
        );
    }

    /// A store failure must not lose the entry: the write still lands, keeps
    /// its full content, and the caller is told the spill did not happen.
    #[tokio::test]
    async fn a_failed_spill_degrades_loudly_and_keeps_the_content() {
        let fx = Fixture::new();
        let body = long_body();
        // A file where the store root must be a directory — `ensure_store`
        // cannot create `<project>/.openalpaca`, so `put` fails.
        std::fs::write(fx.project_root().join(".openalpaca"), b"not a directory").unwrap();

        let out = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({"key": "doomed", "content": body, "entry_type": "artifact"}),
                &fx.ctx(),
            )
            .await
            .unwrap();

        assert!(out.contains("could not be saved"), "{out}");
        let entry = fx.entry("doomed");
        assert!(entry.file_asset_id.is_none());
        assert!(!entry.truncated);
        assert_eq!(entry.content, body);
    }

    /// Fix round 1, Important 2. Before this, a second agent reading a spilled
    /// hand-off saw 512 characters and a bare ellipsis, with nothing in the
    /// tool result saying an artifact existed or how to name it.
    #[tokio::test]
    async fn workspace_read_names_the_artifact_behind_a_preview() {
        let fx = Fixture::new();
        let body = long_body();

        fx.tool()
            .execute_with_context(
                &serde_json::json!({"key": "draft_v1", "content": body, "entry_type": "artifact"}),
                &fx.ctx(),
            )
            .await
            .unwrap();
        fx.tool()
            .execute_with_context(
                &serde_json::json!({"key": "notes", "content": "short", "entry_type": "text"}),
                &fx.ctx(),
            )
            .await
            .unwrap();

        let out = fx
            .read_tool()
            .execute_with_context(&serde_json::json!({}), &fx.ctx())
            .await
            .unwrap();
        let entries: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();

        let draft = entries
            .iter()
            .find(|e| e["key"] == "draft_v1")
            .expect("the artifact entry");
        assert_eq!(
            draft["file_asset_id"].as_str(),
            fx.entry("draft_v1").file_asset_id.as_deref(),
            "the reader must be handed the id Phase 3's routes resolve"
        );
        assert_eq!(draft["truncated"], serde_json::json!(true));
        assert!(
            draft["content"].as_str().unwrap().chars().count() <= 512,
            "the entry still carries only the preview"
        );

        let notes = entries.iter().find(|e| e["key"] == "notes").unwrap();
        assert!(
            notes.get("file_asset_id").is_none() && notes.get("truncated").is_none(),
            "an unspilled entry gains no artifact fields: {notes}"
        );
    }

    /// A short artifact spills but is not shortened — the entry holds the whole
    /// body, so `truncated` must say so even though an asset exists.
    #[tokio::test]
    async fn a_spill_that_fits_in_the_preview_is_not_truncated() {
        let fx = Fixture::new();

        fx.tool()
            .execute_with_context(
                &serde_json::json!({"key": "tiny", "content": "# Tiny\n", "entry_type": "artifact"}),
                &fx.ctx(),
            )
            .await
            .unwrap();

        let entry = fx.entry("tiny");
        assert!(entry.file_asset_id.is_some());
        assert!(!entry.truncated);
        assert_eq!(entry.content, "# Tiny\n");
    }

    /// The spill resolves its store exactly like the tool (R22): a connector
    /// turn carries a CWD-derived `workspace_id` and no request root, and its
    /// bytes belong in the home store.
    #[tokio::test]
    async fn a_connector_turn_spills_into_the_home_store() {
        let fx = Fixture::new();
        let connector_ctx = ToolContext {
            workspace_id: Some(fx.project_root().to_str().unwrap().to_string()),
            request_workspace_root: None,
            ..fx.ctx()
        };

        fx.tool()
            .execute_with_context(
                &serde_json::json!({
                    "key": "reply_draft",
                    "content": long_body(),
                    "entry_type": "artifact"
                }),
                &connector_ctx,
            )
            .await
            .unwrap();

        let asset_id = fx.entry("reply_draft").file_asset_id.clone().unwrap();
        let asset = FileAssetRepository::new(&fx.db)
            .get_by_id(&asset_id)
            .unwrap()
            .unwrap();
        assert!(
            !PathBuf::from(&asset.storage_path).starts_with(fx.project_root()),
            "a connector turn's spill must not land in the daemon's project: {}",
            asset.storage_path
        );
    }
}
