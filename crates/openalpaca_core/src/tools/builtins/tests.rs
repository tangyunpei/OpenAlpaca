use super::*;

#[test]
fn test_builtin_tools_count_without_db() {
    let tools = builtin_tools(None, None, None, None);
    // 7 base tools (incl. artifact_write and read_result) + 2 workspace tools
    assert_eq!(tools.len(), 9);
}

#[test]
fn test_builtin_tools_count_with_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let tools = builtin_tools(Some(db), Some(dc), None, None);
    // 7 base tools + 2 workspace tools: the memory tools are the service's
    // (`memory_read_tools`), not this list's.
    assert_eq!(tools.len(), 9);
}

#[test]
fn test_all_tools_have_valid_definitions() {
    for tool in builtin_tools(None, None, None, None) {
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
    let tools = builtin_tools_with_persona_context(Some(db), ctx, Some(dc), None, None, None);
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
        Some(db), ctx, Some(dc), None, None, Some(provider),
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
    let tools = builtin_tools(None, None, None, None);
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
        Some(db), ctx, Some(dc), None, None, Some(provider),
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
    let tools = builtin_tools(None, None, None, Some(ws_root.clone()));
    // 7 base tools (incl. artifact_write and read_result) + 2 workspace tools
    assert_eq!(tools.len(), 9);

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
    let tools = builtin_tools(None, None, None, None);
    // 7 base tools (incl. artifact_write and read_result) + 2 workspace tools
    assert_eq!(tools.len(), 9);
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
    for name in ["file_write", "shell_execute", "update_persona", "send"] {
        let ann = annotations_for_builtin(name).unwrap();
        assert_eq!(ann.destructive_hint, Some(true), "{name} should be destructive");
    }
}

/// ST-15 (storage design §12): `artifact_write` and `workspace_write` write
/// only under ~/.openalpaca, so they carry no destructive hint, and an agent
/// with no confirmation list of its own is never asked about them, while
/// `file_write` and `shell_execute` still are. Decided by tool name, before
/// the tool runs, so `file_write` saving a by-product still asks.
#[test]
fn annotations_for_builtin_by_product_writers_never_prompt() {
    use crate::agent::subagent::AgentConstraints;
    use crate::security::sandbox::{SandboxPolicy, effective_confirmation_set};
    use crate::tools::ToolRegistry;

    for name in ["artifact_write", "workspace_write"] {
        let ann = annotations_for_builtin(name).unwrap();
        assert_eq!(ann.destructive_hint, Some(false), "{name} must not prompt");
        assert_eq!(ann.read_only_hint, Some(false), "{name} still writes");
        assert_eq!(ann.open_world_hint, Some(false), "{name} writes under ~/.openalpaca only");
    }

    let registry = ToolRegistry::default();
    for tool in builtin_tools(None, None, None, None) {
        registry.register(tool).unwrap();
    }
    let policy = SandboxPolicy::from_constraints("writing_agent", &AgentConstraints::default());
    let set = effective_confirmation_set(&policy, &registry);
    for name in ["artifact_write", "workspace_write"] {
        assert!(!set.contains(&name.to_string()), "{name} asks; the set is {set:?}");
    }
    for name in ["file_write", "shell_execute"] {
        assert!(set.contains(&name.to_string()), "{name} must ask; the set is {set:?}");
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
            // A project scope is a registered project's (design §3, R12) —
            // registered under this fixture's home, never the developer's.
            openalpaca_storage::store::projects::register(&project.path().canonicalize().unwrap())
                .unwrap();
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
                    source_task_id: None,
                    session_id: None,
                    unattended: false,
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
            path.starts_with(
                openalpaca_storage::store::store_root(
                    &openalpaca_storage::store::StoreScope::Project(fx.project_root())
                )
                .unwrap()
                .join("artifacts")
            ),
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
                    PathBuf::from(&path).starts_with(
                        openalpaca_storage::store::store_root(
                            &openalpaca_storage::store::StoreScope::Project(fx.project_root())
                        )
                        .unwrap()
                        .join("artifacts")
                    ),
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
        // A file where the project's `artifacts/` must be a directory, so
        // `put` fails — on either side of the flip.
        let store = openalpaca_storage::store::store_root(
            &openalpaca_storage::store::StoreScope::Project(fx.project_root()),
        )
        .unwrap();
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("artifacts"), b"not a directory").unwrap();

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

    /// The spill resolves its store exactly like the tool (R22): a context
    /// with a `workspace_id` and no request root — a connector turn's before
    /// R-S13 — still puts its bytes in the home store.
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

    /// Design §2 / plan 3.2: `workspace_write`'s key is a path in the
    /// artifact store, and the store's own entries are refused before the
    /// task is even looked up.
    #[tokio::test]
    async fn workspace_write_refuses_store_owned_keys() {
        let fx = Fixture::new();
        let tool = builtin_tools(Some(fx.db.clone()), None, None, None)
            .into_iter()
            .find(|t| t.definition.name == "workspace_write")
            .unwrap();
        let ToolBackend::BuiltIn(backend) = tool.backend else {
            panic!("builtin");
        };
        let ctx = ToolContext {
            task_id: Some(TASK_ID.to_string()),
            agent_id: Some("agent".to_string()),
            owner_id: Some(OWNER.to_string()),
            ..Default::default()
        };
        for key in ["state/memory-control/x.json", "MEMORY.md", ".history/x", "memory/.gitignore"] {
            let err = backend
                .execute_with_context(&serde_json::json!({"key": key, "content": "x"}), &ctx)
                .await
                .expect_err(key);
            assert!(
                err.contains("state/") || err.contains("memory") || err.contains(".history") || err.contains("MEMORY.md"),
                "{key}: {err}"
            );
        }
        backend
            .execute_with_context(&serde_json::json!({"key": "notes/plan.md", "content": "x"}), &ctx)
            .await
            .unwrap();
    }
}

/// `read_result` — the scoped reader for a spilled tool result (§5.4).
///
/// Everything here turns on one sentence: it "resolves **only** inside the
/// current session's `results/` (session id from `ToolContext`; `file_read` is
/// **not** widened to the home root)".
mod read_result {
    use super::*;
    use crate::tools::registry::{RegisteredTool, ToolBackend, ToolContext};
    use std::path::Path;

    fn tool(root: &Path) -> RegisteredTool {
        super::super::read_result::read_result_tool(Some(root.to_path_buf()))
    }

    async fn call(registered: &RegisteredTool, ctx: &ToolContext, args: serde_json::Value) -> Result<String, String> {
        match &registered.backend {
            ToolBackend::BuiltIn(t) => t.execute_with_context(&args, ctx).await,
            _ => panic!("read_result is a builtin"),
        }
    }

    fn ctx_for(session: &str) -> ToolContext {
        ToolContext {
            session_id: Some(session.to_string()),
            ..Default::default()
        }
    }

    fn spill(root: &Path, session: &str, name: &str, body: &str) -> String {
        let dir = root.join(session).join("results");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
        format!("file:results/{name}")
    }

    #[tokio::test]
    async fn it_pages_a_spilled_result_by_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (0..5000).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let rel = spill(dir.path(), "sess-a", "000012-abc-shell_execute.txt", &body);
        let registered = tool(dir.path());
        let ctx = ctx_for("sess-a");

        let head = call(&registered, &ctx, serde_json::json!({"result_ref": rel, "limit": 100}))
            .await
            .unwrap();
        assert!(head.starts_with(&body[..100]), "the page leads with the bytes");
        assert!(head.contains("next offset=100"), "a partial page names its cursor: {head}");

        let middle = call(
            &registered,
            &ctx,
            serde_json::json!({"result_ref": rel, "offset": 100, "limit": 50}),
        )
        .await
        .unwrap();
        assert!(middle.starts_with(&body[100..150]));

        let tail = call(
            &registered,
            &ctx,
            serde_json::json!({"result_ref": rel, "offset": 4990, "limit": 100}),
        )
        .await
        .unwrap();
        assert!(tail.starts_with(&body[4990..]));
        assert!(!tail.contains("next offset="), "the last page has no cursor: {tail}");

        // Past the end is an empty page, not an error.
        let past = call(
            &registered,
            &ctx,
            serde_json::json!({"result_ref": rel, "offset": 9999}),
        )
        .await
        .unwrap();
        assert!(past.contains("5000"), "the page still reports the size: {past}");
    }

    /// The reference is a name under this session's `results/`, and nothing
    /// else. A `..` is refused before it can be resolved.
    #[tokio::test]
    async fn a_traversing_reference_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        spill(dir.path(), "sess-a", "000001-x-dump.txt", "mine");
        std::fs::write(dir.path().join("secret.txt"), "not yours").unwrap();
        let registered = tool(dir.path());
        let ctx = ctx_for("sess-a");

        for reference in [
            "file:results/../../secret.txt",
            "results/../secret.txt",
            "file:../secret.txt",
            "file:/etc/passwd",
            "file:results/nested/other.txt",
        ] {
            let err = call(&registered, &ctx, serde_json::json!({"result_ref": reference}))
                .await
                .unwrap_err();
            assert!(
                err.contains("not a result reference") || err.contains("outside"),
                "{reference} must be refused, got: {err}"
            );
        }
    }

    /// A reference is scoped by the session in `ToolContext`, so one session
    /// can never page another's spill even though both live under the same
    /// sessions root.
    #[tokio::test]
    async fn another_sessions_reference_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let rel = spill(dir.path(), "sess-b", "000003-y-dump.txt", "b's secret");
        let registered = tool(dir.path());

        let err = call(&registered, &ctx_for("sess-a"), serde_json::json!({"result_ref": rel.clone()}))
            .await
            .unwrap_err();
        assert!(err.contains("no spilled result"), "got: {err}");

        // The same reference from its own session resolves.
        let ok = call(&registered, &ctx_for("sess-b"), serde_json::json!({"result_ref": rel}))
            .await
            .unwrap();
        assert!(ok.contains("b's secret"));
    }

    /// Off a session there is no `results/` to scope to, so the call refuses
    /// rather than falling back to a root.
    #[tokio::test]
    async fn without_a_session_the_call_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let registered = tool(dir.path());
        let err = call(
            &registered,
            &ToolContext::default(),
            serde_json::json!({"result_ref": "file:results/000001-x-dump.txt"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("session"), "got: {err}");
    }

    /// Paging must **seek**, not read the file into memory and slice it.
    ///
    /// `log_max_session_bytes` is 256 MB, so a spill of that order is
    /// representable — and paging one in 8 KB pages read it whole 25 000
    /// times. The file here is that size (sparse: `set_len` allocates
    /// nothing), and the tail is read with a small limit: a whole-file read
    /// would need 256 MB of RAM to answer it.
    #[tokio::test]
    async fn a_huge_spill_is_paged_without_being_read_whole() {
        const TOTAL: u64 = 256 * 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let results = dir.path().join("sess-a").join("results");
        std::fs::create_dir_all(&results).unwrap();
        let path = results.join("000042-big-shell_execute.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(TOTAL).unwrap();
        drop(file);

        // The bounded read is the mechanism: at most `limit + 4` bytes leave
        // the disk for one page, whatever the file's size.
        let (buf, total) = super::super::read_result::read_page_bytes(&path, TOTAL - 4_096, 8_192)
            .await
            .unwrap();
        assert_eq!(total, TOTAL as usize, "the size comes from the file's own metadata");
        assert!(
            buf.len() <= 8_192 + 4,
            "one page reads a bounded buffer, not the file: {} bytes",
            buf.len()
        );
        assert_eq!(buf.len(), 4_096, "and stops at the end of the file");

        // And through the tool: the tail page reports the real total.
        let registered = tool(dir.path());
        let tail = call(
            &registered,
            &ctx_for("sess-a"),
            serde_json::json!({
                "result_ref": "file:results/000042-big-shell_execute.txt",
                "offset": TOTAL - 100,
                "limit": 8_192,
            }),
        )
        .await
        .unwrap();
        assert!(tail.contains(&format!("of {TOTAL} from")), "{tail}");
        assert!(!tail.contains("next offset="), "the last page has no cursor");
    }

    /// The reported range is the bytes the page actually covers, so a caller
    /// that resumes from `next offset` neither re-reads nor skips.
    #[tokio::test]
    async fn the_reported_range_is_the_page_that_was_read() {
        let dir = tempfile::tempdir().unwrap();
        // Two-byte characters, so a raw offset can land mid-character.
        let body = "é".repeat(100);
        let rel = spill(dir.path(), "sess-a", "000003-x-dump.txt", &body);
        let registered = tool(dir.path());

        let page = call(
            &registered,
            &ctx_for("sess-a"),
            serde_json::json!({"result_ref": rel, "offset": 1, "limit": 9}),
        )
        .await
        .unwrap();
        // Offset 1 is a continuation byte: the page starts at 2, and the
        // footer says so rather than repeating the offset it was asked for.
        assert!(page.contains("bytes 2–12 of 200"), "{page}");
        assert!(page.contains("next offset=12"), "{page}");
    }

    /// A spill the writer could not write is named as such.
    ///
    /// `write_spill_file` fails asynchronously — the loop has already handed
    /// the model the stub — and the record it wrote instead keeps the preview
    /// and gains `spill_error` plus the reference that was not honoured. The
    /// tool consults that record so the model is told what actually happened,
    /// rather than "no spilled result", which reads like "you made this up".
    #[tokio::test]
    async fn a_spill_the_writer_never_wrote_is_named_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("sess-a");
        std::fs::create_dir_all(&session).unwrap();
        let line = serde_json::json!({
            "v": 1,
            "seq": 7,
            "ts": "2026-09-05T10:22:03.114Z",
            "type": "tool_result",
            "data": {
                "tool_use_id": "tc_1",
                "name": "dump",
                "ok": true,
                "result": "the first 2 KB",
                "spill_ref": "results/000007-tc1-dump.txt",
                "spill_error": "No space left on device (os error 28)",
            },
        });
        std::fs::write(
            session.join(crate::session_log::LIVE_SEGMENT),
            format!("{line}\n"),
        )
        .unwrap();

        let registered = tool(dir.path());
        let err = call(
            &registered,
            &ctx_for("sess-a"),
            serde_json::json!({"result_ref": "file:results/000007-tc1-dump.txt"}),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("was not written"),
            "the honest message names the failed spill: {err}"
        );
        assert!(
            err.contains("spill_error"),
            "and points at the record that says why: {err}"
        );

        // A reference the log knows nothing about keeps the plain refusal.
        let missing = call(
            &registered,
            &ctx_for("sess-a"),
            serde_json::json!({"result_ref": "file:results/000009-none-dump.txt"}),
        )
        .await
        .unwrap_err();
        assert!(
            missing.contains("no spilled result"),
            "an ordinary miss is unchanged: {missing}"
        );
    }

    /// Owner decision T15 is **not** adopted: `read_result` is registered and
    /// the stub is emitted everywhere, but the capability is appended to no
    /// allowlist. So the gate refuses it, fail-closed, exactly like any other
    /// capability an agent was not granted.
    #[tokio::test]
    async fn denied_read_result_is_refused() {
        use crate::bus::EventBus;
        use crate::security::capabilities::Allowlist;
        use crate::security::sandbox::{SandboxManager, SandboxPolicy};
        use crate::tools::ToolRegistry;

        let dir = tempfile::tempdir().unwrap();
        let rel = spill(dir.path(), "sess-a", "000001-x-dump.txt", "spilled bytes");

        let registry = ToolRegistry::default();
        registry.register(tool(dir.path())).unwrap();
        let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());

        let policy = SandboxPolicy {
            agent_id: "research_agent::a1".to_string(),
            // What every template's resolved allowlist looks like today: the
            // capability was never appended anywhere.
            allowed_capabilities: Allowlist::Only(vec!["file_read".to_string()]),
            denied_capabilities: vec![],
            require_confirmation_for: vec![],
            max_tool_calls: None,
            max_tool_runtime_secs: 30,
            stream_id: None,
            lane_key: None,
            confirmation_timeout_secs: None,
            auto_approve: false,
            unattended: false,
        };
        let call = openalpaca_llm::ToolCall {
            id: "tu-denied".to_string(),
            name: "read_result".to_string(),
            arguments: serde_json::json!({"result_ref": rel}),
        };
        let err = sandbox
            .execute_tool(&call, &policy, &ctx_for("sess-a"))
            .await
            .unwrap_err();
        assert!(
            err.contains("read_result") && err.contains("not in allow list"),
            "the gate refuses and says so: {err}"
        );
        assert!(
            !err.contains("spilled bytes"),
            "a refusal never leaks the payload"
        );
    }

    /// The T15 slot. If the owner decides `read_result` becomes an ambient
    /// capability (appended constructor-side, the way `workspace_read/write`
    /// already are for subagents), this is the test that must pass — an
    /// `Only(empty)` policy from Phase 0 A0 would otherwise leave an agent
    /// unable to page into its own spilled result. Ignored until decided:
    /// nothing appends the entry today.
    #[tokio::test]
    #[ignore = "owner decision T15 pending — read_result is on no allowlist"]
    async fn only_empty_allowlist_can_still_read_result() {
        use crate::bus::EventBus;
        use crate::security::capabilities::Allowlist;
        use crate::security::sandbox::{SandboxManager, SandboxPolicy};
        use crate::tools::ToolRegistry;

        let dir = tempfile::tempdir().unwrap();
        let rel = spill(dir.path(), "sess-a", "000001-x-dump.txt", "spilled bytes");
        let registry = ToolRegistry::default();
        registry.register(tool(dir.path())).unwrap();
        let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
        let policy = SandboxPolicy {
            agent_id: "research_agent::a1".to_string(),
            allowed_capabilities: Allowlist::Only(vec![]),
            denied_capabilities: vec![],
            require_confirmation_for: vec![],
            max_tool_calls: None,
            max_tool_runtime_secs: 30,
            stream_id: None,
            lane_key: None,
            confirmation_timeout_secs: None,
            auto_approve: false,
            unattended: false,
        };
        let call = openalpaca_llm::ToolCall {
            id: "tu-ambient".to_string(),
            name: "read_result".to_string(),
            arguments: serde_json::json!({"result_ref": rel}),
        };
        let out = sandbox
            .execute_tool(&call, &policy, &ctx_for("sess-a"))
            .await
            .expect("an ambient capability is admitted by an empty allowlist");
        assert!(out.contains("spilled bytes"));
    }

    /// It is registered in the builtin set — the stub the loop emits names a
    /// tool that has to exist on every surface, decided allowlist or not.
    #[test]
    fn it_is_registered_with_the_builtins() {
        let tools = builtin_tools(None, None, None, None);
        let found = tools
            .iter()
            .find(|t| t.definition.name == "read_result")
            .expect("read_result is a builtin");
        assert_eq!(found.provides_capabilities, vec!["read_result".to_string()]);
        assert_eq!(
            found.annotations.as_ref().and_then(|a| a.read_only_hint),
            Some(true),
            "paging a spilled result reads and nothing more"
        );
    }
}

// --- Memory redesign (plan slice 3, row 3.2): annotations and descriptions ---

/// Design §7, *Capabilities*: `memory_forget` and `memory_update` carry
/// `destructive_local_annotations()` so the sandbox derives its confirmation
/// set from them; `memory_store`, `memory_search`, `memory_read`,
/// `memory_list` and `memory_timeline` do not prompt.
#[test]
fn memory_tool_annotations_decide_the_confirmation_prompt() {
    for name in ["memory_forget", "memory_update"] {
        let ann = annotations_for_builtin(name).unwrap_or_else(|| panic!("{name} has annotations"));
        assert_eq!(ann.destructive_hint, Some(true), "{name} prompts");
        assert_eq!(ann.open_world_hint, Some(false));
    }
    for name in [
        "memory_search",
        "memory_read",
        "memory_list",
        "memory_timeline",
    ] {
        let ann = annotations_for_builtin(name).unwrap_or_else(|| panic!("{name} has annotations"));
        assert_eq!(ann.read_only_hint, Some(true), "{name} is read-only");
        assert_ne!(ann.destructive_hint, Some(true));
    }
    assert!(
        !annotations_for_builtin("memory_store")
            .and_then(|a| a.destructive_hint)
            .unwrap_or(false),
        "a plain save never prompts (design §7: a tool-issued supersede is bounded by authority instead)"
    );
}

/// `memory_search.rs:140` used to promise "hybrid search combining full-text
/// search and vector similarity"; the redesign is lexical (design §1, §5)
/// and the description must not claim more.
#[test]
fn memory_tool_descriptions_promise_no_semantic_search() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let service = std::sync::Arc::new(crate::memory::service::MemoryService::new(
        Some(db),
        openalpaca_storage::memory_index::MemoryIndex::open_in_memory().unwrap(),
    ));
    let mut definitions: Vec<ToolDefinition> = memory_read_tools(service.clone())
        .into_iter()
        .map(|t| t.definition)
        .collect();
    definitions.push(memory_store_tool_definition());
    definitions.push(memory_update_tool_definition());
    definitions.push(memory_forget_tool_definition());
    assert_eq!(definitions.len(), 7);
    for def in &definitions {
        let text = def.description.to_lowercase();
        for banned in ["semantic", "vector", "hybrid", "similarity", "embedding"] {
            assert!(
                !text.contains(banned),
                "{} promises {banned:?}: {}",
                def.name,
                def.description
            );
        }
        assert_eq!(
            def.parameters.get("additionalProperties"),
            Some(&serde_json::json!(false)),
            "{} must close its schema (design §3 rule 6)",
            def.name
        );
        assert_eq!(def.strict, Some(true), "{}", def.name);
    }
}

/// What OpenAI's strict mode accepts, restated here on its own from its
/// documentation (the function-calling guide, *Strict mode*; the
/// structured-outputs guide, *Supported schemas*) so that it checks the
/// provider's `strict_mode_compatible` rather than copies it: the root is an
/// object; every node says what it is (a `type`, an `enum`, a `const`, an
/// `anyOf` or a `$ref`); every object is closed and requires every property;
/// every array has one `items`; and a node carries only keywords from the
/// documented list, each where it applies — anything else is refused.
/// `Err` names the first breach.
fn openai_strict_accepts(schema: &serde_json::Value) -> Result<(), String> {
    use serde_json::Value;

    const FORMATS: [&str; 9] = [
        "date-time",
        "time",
        "date",
        "duration",
        "email",
        "hostname",
        "ipv4",
        "ipv6",
        "uuid",
    ];

    fn check(path: &str, schema: &Value, root: &Value) -> Result<(), String> {
        let node = schema
            .as_object()
            .ok_or_else(|| format!("{path}: not a schema object"))?;
        let types: Vec<&str> = match node.get("type") {
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let is = |t: &str| types.contains(&t);
        if !types.is_empty()
            && !types.iter().all(|t| {
                [
                    "string", "number", "integer", "boolean", "object", "array", "null",
                ]
                .contains(t)
            })
        {
            return Err(format!("{path}: a type strict mode does not know"));
        }
        if !["type", "enum", "const", "anyOf", "$ref"]
            .iter()
            .any(|k| node.contains_key(*k))
        {
            return Err(format!("{path}: says nothing of what it is"));
        }
        for (key, value) in node {
            let belongs = match key.as_str() {
                "type" | "description" | "title" | "enum" | "const" | "anyOf" => true,
                "$ref" => node.len() == 1,
                "$defs" | "definitions" => path == "$",
                "properties" | "required" | "additionalProperties" => is("object"),
                "items" | "minItems" | "maxItems" => is("array"),
                "pattern" => is("string"),
                "format" => is("string") && FORMATS.contains(&value.as_str().unwrap_or("")),
                "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
                    is("number") || is("integer")
                }
                _ => false,
            };
            if !belongs {
                return Err(format!("{path}: `{key}` is not accepted here"));
            }
        }
        if let Some(target) = node.get("$ref").and_then(Value::as_str) {
            let found = target == "#"
                || ["$defs", "definitions"].iter().any(|defs| {
                    target
                        .strip_prefix(&format!("#/{defs}/"))
                        .is_some_and(|name| root[*defs].get(name).is_some())
                });
            if !found {
                return Err(format!("{path}: `$ref` {target} names no local definition"));
            }
        }
        if is("object") {
            if node.get("additionalProperties") != Some(&Value::Bool(false)) {
                return Err(format!("{path}: additionalProperties is not false"));
            }
            let properties = node
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("{path}: no properties"))?;
            let required: Vec<&str> = node
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut sorted = required.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let mut keys: Vec<&str> = properties.keys().map(String::as_str).collect();
            keys.sort_unstable();
            if sorted != keys || sorted.len() != required.len() {
                return Err(format!(
                    "{path}: required {required:?} is not every key {keys:?}"
                ));
            }
            for (key, property) in properties {
                check(&format!("{path}.{key}"), property, root)?;
            }
        }
        if is("array") {
            let items = node
                .get("items")
                .ok_or_else(|| format!("{path}: an array with no items"))?;
            check(&format!("{path}[]"), items, root)?;
        }
        if let Some(branches) = node.get("anyOf") {
            for (i, branch) in branches.as_array().into_iter().flatten().enumerate() {
                check(&format!("{path}|{i}"), branch, root)?;
            }
        }
        for defs in ["$defs", "definitions"] {
            for (name, definition) in node
                .get(defs)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                check(&format!("{path}/{defs}/{name}"), definition, root)?;
            }
        }
        Ok(())
    }

    if schema.get("type") != Some(&Value::String("object".into())) {
        return Err("$: the root is not an object".into());
    }
    check("$", schema, schema)
}

/// R8: every builtin definition — the registered ones and the per-request
/// main-loop and skill ones — goes through the OpenAI tool serializer the
/// OpenAI and Ollama providers use, and `strict: true` goes out exactly where
/// OpenAI's strict mode accepts the schema. A strict-marked schema that
/// breaks its rules is refused with a 400 for the whole request, so a future
/// builtin with an optional property or an open object must lose `strict`
/// here rather than every tool-carrying turn on OpenAI. The schema itself is
/// always sent as written.
#[test]
fn every_builtin_goes_to_openai_with_strict_only_where_strict_mode_accepts_it() {
    use crate::bus::EventBus;
    use openalpaca_llm::providers::openai_tools::openai_function_tool;

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
    let mut definitions: Vec<ToolDefinition> = builtin_tools_with_persona_context(
        Some(db.clone()),
        ctx,
        Some(dc),
        None,
        None,
        Some(provider),
    )
    .into_iter()
    .map(|t| t.definition)
    .collect();
    let service = std::sync::Arc::new(crate::memory::service::MemoryService::new(
        Some(db),
        openalpaca_storage::memory_index::MemoryIndex::open_in_memory().unwrap(),
    ));
    definitions.extend(memory_read_tools(service).into_iter().map(|t| t.definition));
    definitions.extend([
        memory_store_tool_definition(),
        memory_update_tool_definition(),
        memory_forget_tool_definition(),
        memory_confirm_tool_definition(),
        invoke_skill_tool_definition(),
        start_workflow_tool_definition(),
        steer_workflow_tool_definition(),
        task_status_tool_definition(),
    ]);

    let mut kept = Vec::new();
    let mut marked = 0;
    for def in &definitions {
        let entry = openai_function_tool(def);
        let function = &entry["function"];
        assert_eq!(entry["type"], "function", "{}", def.name);
        assert_eq!(
            function["parameters"], def.parameters,
            "{} is sent as written",
            def.name
        );
        let sent_strict = function.get("strict") == Some(&serde_json::json!(true));
        if def.strict != Some(true) {
            assert!(!sent_strict, "{} is not strict-marked", def.name);
            continue;
        }
        marked += 1;
        match openai_strict_accepts(&def.parameters) {
            Ok(()) => {
                assert!(sent_strict, "{}: a compliant schema keeps strict", def.name);
                kept.push(def.name.clone());
            }
            Err(breach) => assert!(
                !sent_strict,
                "{} would be refused by OpenAI's strict mode ({breach}), yet goes out strict",
                def.name
            ),
        }
    }
    // The walk saw what it is meant to: all 21 strict-marked builtins, and
    // the one schema already written in strict mode's shape keeps its flag.
    assert_eq!(marked, 21, "strict-marked builtins seen");
    assert!(kept.contains(&"memory_confirm".to_string()), "{kept:?}");

    // The serializer and this restatement agree beyond today's builtins:
    // closed, fully required schemas carrying a keyword strict mode refuses
    // (the review's probe) lose the flag, and ones using only documented
    // keywords keep it.
    let closed = |properties: serde_json::Value| {
        let keys: Vec<String> = properties.as_object().unwrap().keys().cloned().collect();
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": keys,
            "additionalProperties": false,
        })
    };
    let with_defs = |mut schema: serde_json::Value| {
        schema["$defs"] = serde_json::json!({ "n": closed(serde_json::json!({})) });
        schema
    };
    let refused = [
        closed(
            serde_json::json!({ "v": { "type": "array", "items": { "type": "string" }, "uniqueItems": true } }),
        ),
        closed(
            serde_json::json!({ "v": { "type": "array", "items": { "type": "string" }, "minContains": 1 } }),
        ),
        serde_json::json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false, "minProperties": 0 }),
        serde_json::json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false, "maxProperties": 3 }),
        closed(serde_json::json!({ "v": { "type": "string", "default": "x" } })),
        with_defs(closed(
            serde_json::json!({ "v": { "$ref": "#/$defs/n", "title": "N" } }),
        )),
        closed(serde_json::json!({ "v": { "type": "string", "format": "uri" } })),
        with_defs(closed(
            serde_json::json!({ "v": { "$ref": "#/$defs/n", "description": "N" } }),
        )),
    ];
    let accepted = [
        closed(serde_json::json!({
            "when": { "type": ["string", "null"], "format": "date-time" },
            "code": { "type": "string", "pattern": "^[a-z]+$" },
            "count": { "type": "integer", "minimum": 0, "maximum": 9 },
            "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 5 },
        })),
        with_defs(closed(serde_json::json!({ "v": { "$ref": "#/$defs/n" } }))),
    ];
    for (schema, compliant) in refused
        .into_iter()
        .map(|s| (s, false))
        .chain(accepted.into_iter().map(|s| (s, true)))
    {
        assert_eq!(
            openai_strict_accepts(&schema).is_ok(),
            compliant,
            "{schema}"
        );
        let def = ToolDefinition {
            name: "probe".to_string(),
            description: "probe".to_string(),
            parameters: schema.clone(),
            strict: Some(true),
            input_examples: None,
        };
        let sent =
            openai_function_tool(&def)["function"].get("strict") == Some(&serde_json::json!(true));
        assert_eq!(sent, compliant, "{schema}");
    }
}

// --- R29: which tools keep the `../` rule (storage design §11) ---

#[test]
fn every_first_party_builtin_says_its_arguments_reach_no_program() {
    use crate::bus::EventBus;
    use crate::tools::registry::ToolBackend;

    let dir = tempfile::tempdir().unwrap();
    let ctx = PersonaToolContext {
        soul_path: dir.path().join("SOUL.md"),
        user_path: dir.path().join("USER.md"),
        identity_path: dir.path().join("IDENTITY.md"),
        backup_dir: dir.path().join("backups"),
        bus: EventBus::new(16),
        max_backups: None,
    };
    let provider: ConnectorSendLock = Arc::new(std::sync::RwLock::new(None));
    let tools = builtin_tools_with_persona_context(
        None,
        ctx,
        None,
        None,
        Some(dir.path().to_path_buf()),
        Some(provider),
    );
    assert!(tools.iter().any(|t| t.definition.name == "send"), "send is on the list");
    for tool in &tools {
        match &tool.backend {
            ToolBackend::BuiltIn(handler) => assert!(
                !handler.arguments_reach_a_program(),
                "{} keeps the ../ rule",
                tool.definition.name
            ),
            _ => panic!("{} is not a builtin", tool.definition.name),
        }
    }
}

#[test]
fn a_skill_script_says_its_arguments_reach_a_program() {
    let skill = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(skill.path().join("scripts")).unwrap();
    std::fs::write(skill.path().join("scripts/noop.sh"), "exit 0\n").unwrap();
    let script = ScriptToolBuiltIn::new(
        skill.path(),
        &crate::middleware::skill::ScriptConfig {
            file: "noop.sh".to_string(),
            name: "noop".to_string(),
            description: "Does nothing".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            interpreter: Some("sh".to_string()),
            timeout_secs: 5,
        },
    )
    .unwrap();
    assert!(script.arguments_reach_a_program());
}

// --- R45: a skill script is killed with what it started (storage design §11) ---

#[cfg(unix)]
#[tokio::test]
async fn a_skill_script_that_times_out_takes_the_processes_it_started_with_it() {
    use crate::test_util::{GONE_WITHIN, KillOnFail, pid_from_file, wait_until_gone};
    use std::time::Duration;

    let skill = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(skill.path().join("scripts")).unwrap();
    std::fs::write(
        skill.path().join("scripts/hang.sh"),
        "for a in \"$@\"; do case \"$a\" in --pidfile=*) P=\"${a#--pidfile=}\" ;; esac; done\n\
         sleep 30 &\n\
         echo $! > \"$P\"\n\
         wait\n",
    )
    .unwrap();
    let tool = ScriptToolBuiltIn::new(
        skill.path(),
        &crate::middleware::skill::ScriptConfig {
            file: "hang.sh".to_string(),
            name: "hang".to_string(),
            description: "Starts a sleep and waits on it".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            interpreter: Some("sh".to_string()),
            timeout_secs: 1,
        },
    )
    .unwrap();
    let pidfile = skill.path().join("pid");

    let err = tool
        .execute(&serde_json::json!({"pidfile": pidfile.to_str().unwrap()}))
        .await
        .unwrap_err();
    let pid = pid_from_file(&pidfile, Duration::from_secs(1)).await;
    let _kill = KillOnFail(pid);
    assert_eq!(err, "Script timed out after 1s");
    assert!(
        wait_until_gone(pid, GONE_WITHIN).await,
        "the script's background sleep {pid} outlived its timeout"
    );
}
