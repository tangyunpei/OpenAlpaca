//! `artifact_write` — the producer of durable agent deliverables (plan §4.6).
//!
//! The first **per-request** file writer in the codebase. `file_write` writes
//! under a `workspace_root` captured once at daemon startup
//! (`services/tools.rs` → `builtins::builtin_tools`); `artifact_write` resolves
//! its store from [`ToolContext::workspace_id`] on every call, so an artifact
//! produced for a request that arrived with a workspace lands in *that*
//! project's store and never in the daemon's current directory.

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::{ArtifactKind, ArtifactStore, Database, NewArtifact};

use super::annotations_for_builtin;
use crate::daemon_config::{ArtifactsConfig, DaemonConfig};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};

/// The kinds the tool accepts, in the order the description lists them.
const KINDS: &[&str] = &[
    "markdown",
    "code",
    "terminal",
    "table",
    "plan",
    "image",
    "html",
    "binary",
];

/// The store an invocation writes to (plan §4.6, "Critical").
///
/// `workspace_id` is the canonical absolute path of the request's workspace
/// root (`memory::workspace::workspace_id_from_root`). Absent — a chat turn
/// with no project, a detached loop — the artifact belongs to the home store.
/// A non-absolute value is not a project root: `store_root` would reject it,
/// so it degrades to the home store with a warning rather than failing a write.
fn scope_from_context(ctx: &ToolContext) -> StoreScope {
    match ctx.workspace_id.as_deref() {
        Some(id) if !id.trim().is_empty() => {
            let root = PathBuf::from(id);
            if root.is_absolute() {
                StoreScope::Project(root)
            } else {
                tracing::warn!(
                    workspace_id = id,
                    "artifact_write: workspace_id is not an absolute path — using the home store"
                );
                StoreScope::Home
            }
        }
        _ => StoreScope::Home,
    }
}

/// Split a model-supplied `name` into the artifact title and the extension
/// hint the grammar reads.
///
/// `"quarterly-report.md"` → title `"quarterly-report"`, hint
/// `Some("quarterly-report.md")`: passing the whole name as the title would
/// slugify the dot into a hyphen and produce `01-quarterly-report-md.md`.
/// A trailing segment that is not extension-shaped (`ext := [a-z0-9]{1,8}`)
/// is part of the title — `"v1.2 plan"` keeps its dot.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty()
                && (1..=8).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            (stem, Some(name))
        }
        _ => (name, Some(name)),
    }
}

/// `metadata` accepts either a JSON object or a JSON string carrying one; both
/// end up as the `metadata_json` column. Anything else is a caller error, not
/// a silently dropped field.
fn metadata_json(value: Option<&serde_json::Value>) -> Result<Option<String>, String> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => {
            let parsed: serde_json::Value = serde_json::from_str(s)
                .map_err(|e| format!("metadata is not valid JSON: {e}"))?;
            if !parsed.is_object() {
                return Err("metadata must be a JSON object (or a string holding one)".to_string());
            }
            Ok(Some(s.clone()))
        }
        Some(v) if v.is_object() => Ok(Some(v.to_string())),
        Some(_) => Err("metadata must be a JSON object (or a string holding one)".to_string()),
    }
}

struct ArtifactWriteTool {
    db: Option<Database>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
}

impl ArtifactWriteTool {
    fn caps(&self) -> ArtifactsConfig {
        artifact_caps(self.daemon_config.as_ref())
    }
}

#[async_trait]
impl BuiltInTool for ArtifactWriteTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("artifact_write requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "artifact_write requires database context".to_string())?;

        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Missing required parameter: name".to_string())?;
        let kind_str = arguments
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: kind".to_string())?;
        let kind = ArtifactKind::parse(&kind_str.to_lowercase()).ok_or_else(|| {
            format!(
                "Unknown artifact kind '{kind_str}'. Valid kinds: {}",
                KINDS.join(", ")
            )
        })?;
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        let caps = self.caps();
        if content.len() as u64 > caps.max_artifact_bytes {
            return Err(format!(
                "Artifact '{name}' is {} bytes, exceeding the {} byte limit. \
                 Write a shorter artifact, or split it into parts.",
                content.len(),
                caps.max_artifact_bytes
            ));
        }

        let note = arguments.get("note").and_then(|v| v.as_str());
        let summary = arguments.get("summary").and_then(|v| v.as_str());
        let metadata = metadata_json(arguments.get("metadata"))?;

        // Attribution. The task row also carries the title `run_dir` is named
        // for, and the owner when the context did not thread one.
        let task = match ctx.task_id.as_deref() {
            Some(task_id) => openalpaca_storage::repository::TaskRepository::new(db)
                .get(task_id)
                .map_err(|e| format!("Failed to load task: {e}"))?,
            None => None,
        };
        let owner_id = ctx
            .owner_id
            .as_deref()
            .or(task.as_ref().map(|t| t.created_by.as_str()))
            .ok_or_else(|| "artifact_write requires an owner context".to_string())?;

        let scope = scope_from_context(ctx);
        let (title, name_hint) = split_name(name);

        let mut new = NewArtifact::new(owner_id, &scope, kind, title, content.as_bytes());
        new.name_hint = name_hint;
        new.note = note;
        new.summary = summary;
        new.metadata_json = metadata.as_deref();
        new.task_id = ctx.task_id.as_deref();
        new.task_title = task.as_ref().map(|t| t.title.as_str());
        new.agent_id = ctx.agent_id.as_deref();
        new.created = Utc::now();
        new.max_versions = Some(caps.max_versions_per_artifact);

        let (record, _created) = ArtifactStore::new(db)
            .put(new)
            .map_err(|e| format!("Failed to write artifact '{name}': {e}"))?;

        serde_json::to_string(&serde_json::json!({
            "artifact_id": record.id,
            "path": record.storage_path,
            "version": record.version,
        }))
        .map_err(|e| format!("Serialization error: {e}"))
    }
}

// ---------------------------------------------------------------------------
// The `workspace_write(entry_type = "artifact")` spill bridge (plan §4.6)
// ---------------------------------------------------------------------------

/// How much of a spilled artifact the workspace entry keeps for prompt
/// assembly. `format_for_prompt` truncates at 2000 anyway
/// (`task_state/workspace.rs`), so the entry only has to say enough for an
/// agent to recognize what it wrote.
pub(super) const SPILL_PREVIEW_CHARS: usize = 512;

/// The `SPILL_PREVIEW_CHARS`-bounded head of `content`, ellipsis included.
pub(super) fn spill_preview(content: &str) -> String {
    if content.chars().count() <= SPILL_PREVIEW_CHARS {
        return content.to_string();
    }
    let mut preview: String = content.chars().take(SPILL_PREVIEW_CHARS - 1).collect();
    preview.push('…');
    preview
}

/// The caps in effect, or their defaults where no config was wired.
pub(super) fn artifact_caps(daemon_config: Option<&Arc<ArcSwap<DaemonConfig>>>) -> ArtifactsConfig {
    daemon_config
        .map(|c| c.load().execution.artifacts.clone())
        .unwrap_or_default()
}

/// Write a workspace entry's body to the artifact store and return the
/// `file_asset_id` the entry should carry.
///
/// Same scope rule as the tool: the request's workspace, else the home store.
/// The entry's key is the artifact title, so rewriting a key supersedes rather
/// than duplicating. Errors are returned, never swallowed — the caller keeps
/// the entry inline and says so.
pub(super) fn spill_workspace_entry(
    db: &Database,
    ctx: &ToolContext,
    task_id: &str,
    key: &str,
    content: &str,
    daemon_config: Option<&Arc<ArcSwap<DaemonConfig>>>,
) -> Result<String, String> {
    let caps = artifact_caps(daemon_config);
    if content.len() as u64 > caps.max_artifact_bytes {
        return Err(format!(
            "{} bytes exceeds the {} byte artifact limit",
            content.len(),
            caps.max_artifact_bytes
        ));
    }

    let task = openalpaca_storage::repository::TaskRepository::new(db)
        .get(task_id)
        .map_err(|e| format!("failed to load task: {e}"))?
        .ok_or_else(|| format!("task '{task_id}' not found"))?;
    let owner_id = ctx.owner_id.as_deref().unwrap_or(task.created_by.as_str());

    let scope = scope_from_context(ctx);
    let (title, name_hint) = split_name(key);
    let mut new = NewArtifact::new(
        owner_id,
        &scope,
        ArtifactKind::Markdown,
        title,
        content.as_bytes(),
    );
    new.name_hint = name_hint;
    new.task_id = Some(task_id);
    new.task_title = Some(task.title.as_str());
    new.agent_id = ctx.agent_id.as_deref();
    new.created = Utc::now();
    new.max_versions = Some(caps.max_versions_per_artifact);

    let (record, _created) = ArtifactStore::new(db).put(new).map_err(|e| e.to_string())?;
    Ok(record.id)
}

/// The tool definition — the model-facing contract of plan §4.6.
pub(super) fn artifact_write_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "artifact_write".to_string(),
        description: "Save a deliverable — a document, plan, table, snippet or report — \
            to the artifact store, where it is versioned, listed in the Library and \
            attached to the task that produced it. Prefer this over file_write for \
            anything the user is meant to keep or receive: file_write drops bytes in \
            the daemon's workspace, artifact_write records an owned, versioned \
            artifact under the project's .openalpaca/artifacts directory. Writing the \
            same name again supersedes the previous version instead of overwriting it \
            (the old version stays retrievable). Content is limited to 10MB. Returns \
            the artifact id, its path on disk, and the version number just written."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "A short descriptive file name for the artifact, e.g. 'competitive-analysis' or 'summary.md'. A recognized extension is honoured when it suits the kind; otherwise the kind picks one. Reusing a name within the same task writes a new version."
                },
                "kind": {
                    "type": "string",
                    "enum": ["markdown", "code", "terminal", "table", "plan", "image", "html", "binary"],
                    "description": "What the content is: 'markdown' (prose/report), 'plan', 'code', 'table' (CSV/TSV/JSON), 'terminal' (captured output), 'html', 'image', or 'binary'."
                },
                "content": {
                    "type": "string",
                    "description": "The artifact body as a UTF-8 string (max 10MB)"
                },
                "note": {
                    "type": "string",
                    "description": "Optional one-line note describing this version, e.g. 'added the risks section'. Shown in the version history."
                },
                "summary": {
                    "type": "string",
                    "description": "Optional short summary of the artifact as a whole, shown in listings."
                },
                "metadata": {
                    "type": "string",
                    "description": "Optional JSON object (as a string) with extra structured fields to store alongside the artifact."
                }
            },
            "required": ["name", "kind", "content"]
        }),
        strict: Some(true),
        input_examples: Some(vec![
            serde_json::json!({
                "name": "competitive-analysis",
                "kind": "markdown",
                "content": "# Competitive analysis\n\n## Summary\n...",
                "summary": "Three competitors compared on price and coverage"
            }),
            serde_json::json!({
                "name": "revenue-by-region.csv",
                "kind": "table",
                "content": "region,revenue\nEMEA,120000\nAPAC,98000\n"
            }),
        ]),
    }
}

pub(super) fn artifact_write_tool(
    db: Option<Database>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: artifact_write_tool_definition(),
        backend: ToolBackend::BuiltIn(Arc::new(ArtifactWriteTool { db, daemon_config })),
        provides_capabilities: vec!["artifact_write".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("artifact_write"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::HomeStoreGuard;
    use tempfile::TempDir;

    const OWNER: &str = "owner-1";

    /// A temp project root, a temp home root and a temp database — no test
    /// here ever touches a real store or the daemon's current directory.
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

        fn home_root(&self) -> PathBuf {
            self._home.path().canonicalize().unwrap()
        }

        fn tool(&self) -> ArtifactWriteTool {
            ArtifactWriteTool {
                db: Some(self.db.clone()),
                daemon_config: None,
            }
        }

        fn tool_with_caps(&self, caps: ArtifactsConfig) -> ArtifactWriteTool {
            let mut cfg = DaemonConfig::default();
            cfg.execution.artifacts = caps;
            ArtifactWriteTool {
                db: Some(self.db.clone()),
                daemon_config: Some(Arc::new(ArcSwap::from_pointee(cfg))),
            }
        }

        /// A `task` row — `file_assets.task_id` is a foreign key.
        fn task(&self, id: &str, title: &str) {
            let now = Utc::now();
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
                    source_lane: "cli".to_string(),
                    created_at: now,
                    updated_at: now,
                    completed_at: None,
                    state_json: None,
                    state_version: 0,
                    outcome_json: None,
                    outcome_kind: None,
                    artifact_count: 0,
                })
                .unwrap();
        }
    }

    fn ctx(workspace_id: Option<&str>, task_id: Option<&str>) -> ToolContext {
        ToolContext {
            agent_id: Some("writing_agent".to_string()),
            task_id: task_id.map(str::to_string),
            owner_id: Some(OWNER.to_string()),
            workspace_id: workspace_id.map(str::to_string),
            ..Default::default()
        }
    }

    fn parse(out: &str) -> serde_json::Value {
        serde_json::from_str(out).unwrap()
    }

    #[tokio::test]
    async fn writes_under_the_project_store_when_the_context_has_a_workspace() {
        let fx = Fixture::new();
        fx.task("t-1111aaaa-0000-0000-0000-000000000000", "Quarterly review");
        let root = fx.project_root();

        let out = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({
                    "name": "quarterly-report.md",
                    "kind": "markdown",
                    "content": "# Q3\n\nAll good.\n",
                    "summary": "Q3 summary",
                    "note": "first cut"
                }),
                &ctx(
                    Some(root.to_str().unwrap()),
                    Some("t-1111aaaa-0000-0000-0000-000000000000"),
                ),
            )
            .await
            .unwrap();

        let v = parse(&out);
        let path = PathBuf::from(v["path"].as_str().unwrap());
        assert_eq!(v["version"], 1);
        assert!(!v["artifact_id"].as_str().unwrap().is_empty());
        assert!(
            path.starts_with(root.join(".openalpaca").join("artifacts")),
            "artifact must land in the project store, got {}",
            path.display()
        );
        // `<run_dir>/NN-<slug>.<ext>` — the dot in the name is an extension
        // hint, never part of the slug.
        assert_eq!(path.file_name().unwrap(), "01-quarterly-report.md");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Q3\n\nAll good.\n");
        let run_dir = path.parent().unwrap().file_name().unwrap().to_str().unwrap();
        assert!(
            run_dir.ends_with("-quarterly-review-t-1111a") || run_dir.contains("quarterly-review"),
            "run dir should be named for the task: {run_dir}"
        );
    }

    /// The whole point of the tool: the scope comes from the *request*, never
    /// from the daemon's current directory or the startup `workspace_root`.
    #[tokio::test]
    async fn never_writes_under_the_daemon_working_directory() {
        let fx = Fixture::new();
        let root = fx.project_root();
        let cwd = std::env::current_dir().unwrap();

        let out = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({"name": "notes", "kind": "markdown", "content": "hi"}),
                &ctx(Some(root.to_str().unwrap()), None),
            )
            .await
            .unwrap();

        let path = PathBuf::from(parse(&out)["path"].as_str().unwrap());
        assert!(path.starts_with(&root));
        assert!(
            !path.starts_with(&cwd),
            "artifact escaped into the daemon CWD: {}",
            path.display()
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_home_store_without_a_workspace() {
        let fx = Fixture::new();

        let out = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({"name": "chat-notes", "kind": "markdown", "content": "hi"}),
                &ctx(None, None),
            )
            .await
            .unwrap();

        let path = PathBuf::from(parse(&out)["path"].as_str().unwrap());
        assert!(
            path.starts_with(fx.home_root().join("artifacts")),
            "expected the home store, got {}",
            path.display()
        );
        // No task → the loose bucket, not a run directory.
        assert!(
            path.to_string_lossy().contains("/loose/"),
            "expected a loose-dir placement, got {}",
            path.display()
        );
    }

    #[tokio::test]
    async fn a_relative_workspace_id_degrades_to_the_home_store() {
        let fx = Fixture::new();

        let out = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({"name": "notes", "kind": "markdown", "content": "hi"}),
                &ctx(Some("relative/path"), None),
            )
            .await
            .unwrap();

        let path = PathBuf::from(parse(&out)["path"].as_str().unwrap());
        assert!(path.starts_with(fx.home_root().join("artifacts")));
    }

    #[tokio::test]
    async fn refuses_content_over_the_cap() {
        let fx = Fixture::new();
        let root = fx.project_root();
        let tool = fx.tool_with_caps(ArtifactsConfig {
            max_artifact_bytes: 16,
            max_versions_per_artifact: 20,
        });

        let err = tool
            .execute_with_context(
                &serde_json::json!({
                    "name": "too-big",
                    "kind": "markdown",
                    "content": "x".repeat(17),
                }),
                &ctx(Some(root.to_str().unwrap()), None),
            )
            .await
            .unwrap_err();

        assert!(err.contains("17 bytes"), "{err}");
        assert!(err.contains("16 byte limit"), "{err}");
        assert!(
            !root.join(".openalpaca").join("artifacts").exists(),
            "a refused write must not create the store"
        );
    }

    #[tokio::test]
    async fn the_default_cap_is_ten_megabytes() {
        assert_eq!(
            ArtifactsConfig::default().max_artifact_bytes,
            10 * 1024 * 1024
        );
        let fx = Fixture::new();
        let root = fx.project_root();
        // Just over 10 MB is refused with the default (no config wired).
        let err = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({
                    "name": "huge",
                    "kind": "markdown",
                    "content": "x".repeat(10 * 1024 * 1024 + 1),
                }),
                &ctx(Some(root.to_str().unwrap()), None),
            )
            .await
            .unwrap_err();
        assert!(err.contains("10485760 byte limit"), "{err}");
    }

    #[tokio::test]
    async fn a_second_write_of_the_same_name_supersedes() {
        let fx = Fixture::new();
        fx.task("t-2222bbbb-0000-0000-0000-000000000000", "Report run");
        let root = fx.project_root();
        let c = ctx(
            Some(root.to_str().unwrap()),
            Some("t-2222bbbb-0000-0000-0000-000000000000"),
        );
        let tool = fx.tool();

        let first = parse(
            &tool
                .execute_with_context(
                    &serde_json::json!({"name": "report", "kind": "markdown", "content": "v1\n"}),
                    &c,
                )
                .await
                .unwrap(),
        );
        let second = parse(
            &tool
                .execute_with_context(
                    &serde_json::json!({
                        "name": "report", "kind": "markdown",
                        "content": "v2\n", "note": "revised"
                    }),
                    &c,
                )
                .await
                .unwrap(),
        );

        assert_eq!(first["version"], 1);
        assert_eq!(second["version"], 2);
        assert_eq!(first["artifact_id"], second["artifact_id"]);

        let head = PathBuf::from(second["path"].as_str().unwrap());
        assert_eq!(std::fs::read_to_string(&head).unwrap(), "v2\n");
        let v1 = head
            .parent()
            .unwrap()
            .join(".versions")
            .join("01-report")
            .join("v1.md");
        assert_eq!(std::fs::read_to_string(&v1).unwrap(), "v1\n");
    }

    #[tokio::test]
    async fn prune_bound_comes_from_config() {
        let fx = Fixture::new();
        let root = fx.project_root();
        let tool = fx.tool_with_caps(ArtifactsConfig {
            max_artifact_bytes: 10 * 1024 * 1024,
            max_versions_per_artifact: 2,
        });
        let c = ctx(Some(root.to_str().unwrap()), None);

        let mut path = PathBuf::new();
        for i in 1..=3 {
            let out = tool
                .execute_with_context(
                    &serde_json::json!({
                        "name": "log", "kind": "terminal", "content": format!("run {i}\n")
                    }),
                    &c,
                )
                .await
                .unwrap();
            path = PathBuf::from(parse(&out)["path"].as_str().unwrap());
        }

        let versions = path.parent().unwrap().join(".versions").join("01-log");
        assert!(!versions.join("v1.log").exists(), "v1 should be pruned");
        assert!(versions.join("v2.log").exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "run 3\n");
    }

    #[tokio::test]
    async fn rejects_an_unknown_kind_and_missing_parameters() {
        let fx = Fixture::new();
        let root = fx.project_root();
        let tool = fx.tool();
        let c = ctx(Some(root.to_str().unwrap()), None);

        let err = tool
            .execute_with_context(
                &serde_json::json!({"name": "x", "kind": "spreadsheet", "content": "a"}),
                &c,
            )
            .await
            .unwrap_err();
        assert!(err.contains("Unknown artifact kind"), "{err}");
        assert!(err.contains("markdown"), "{err}");

        for missing in ["name", "kind", "content"] {
            let mut args = serde_json::json!({"name": "x", "kind": "markdown", "content": "a"});
            args.as_object_mut().unwrap().remove(missing);
            let err = tool.execute_with_context(&args, &c).await.unwrap_err();
            assert!(err.contains(missing), "{err}");
        }
    }

    #[tokio::test]
    async fn metadata_accepts_an_object_or_a_json_string() {
        let fx = Fixture::new();
        let root = fx.project_root();
        let c = ctx(Some(root.to_str().unwrap()), None);
        let tool = fx.tool();

        for metadata in [
            serde_json::json!({"source": "web"}),
            serde_json::json!(r#"{"source":"web"}"#),
        ] {
            let out = tool
                .execute_with_context(
                    &serde_json::json!({
                        "name": "m", "kind": "markdown", "content": "x", "metadata": metadata
                    }),
                    &c,
                )
                .await
                .unwrap();
            let id = parse(&out)["artifact_id"].as_str().unwrap().to_string();
            let record = ArtifactStore::new(&fx.db).get(&id, OWNER).unwrap().unwrap();
            let stored: serde_json::Value =
                serde_json::from_str(record.metadata_json.as_deref().unwrap()).unwrap();
            assert_eq!(stored["source"], "web");
        }

        let err = tool
            .execute_with_context(
                &serde_json::json!({
                    "name": "m2", "kind": "markdown", "content": "x", "metadata": "not json"
                }),
                &c,
            )
            .await
            .unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
    }

    #[tokio::test]
    async fn attributes_the_row_to_the_context() {
        let fx = Fixture::new();
        fx.task("t-3333cccc-0000-0000-0000-000000000000", "Attribution run");
        let root = fx.project_root();

        let out = fx
            .tool()
            .execute_with_context(
                &serde_json::json!({"name": "a", "kind": "plan", "content": "steps"}),
                &ctx(
                    Some(root.to_str().unwrap()),
                    Some("t-3333cccc-0000-0000-0000-000000000000"),
                ),
            )
            .await
            .unwrap();

        let id = parse(&out)["artifact_id"].as_str().unwrap().to_string();
        let record = ArtifactStore::new(&fx.db).get(&id, OWNER).unwrap().unwrap();
        assert_eq!(
            record.task_id.as_deref(),
            Some("t-3333cccc-0000-0000-0000-000000000000")
        );
        assert_eq!(record.agent_id.as_deref(), Some("writing_agent"));
        assert_eq!(record.owner_id, OWNER);
        assert_eq!(record.project_root.as_deref(), root.to_str());
    }

    #[tokio::test]
    async fn requires_a_database_and_an_owner() {
        let fx = Fixture::new();
        let root = fx.project_root();
        let args = serde_json::json!({"name": "a", "kind": "markdown", "content": "x"});

        let no_db = ArtifactWriteTool {
            db: None,
            daemon_config: None,
        };
        let err = no_db
            .execute_with_context(&args, &ctx(Some(root.to_str().unwrap()), None))
            .await
            .unwrap_err();
        assert!(err.contains("database context"), "{err}");

        let ownerless = ToolContext {
            workspace_id: Some(root.to_str().unwrap().to_string()),
            ..Default::default()
        };
        let err = fx
            .tool()
            .execute_with_context(&args, &ownerless)
            .await
            .unwrap_err();
        assert!(err.contains("owner context"), "{err}");
    }

    /// The Phase 2 verify item, as far as a test without a live model can take
    /// it: a subagent-shaped call — capability-resolved from a template,
    /// dispatched through the sandboxed execute path an agentic round uses —
    /// lands its artifact under the *request's* project store, while the
    /// registry's own `workspace_root` (what `file_write` writes under) points
    /// somewhere else entirely.
    #[tokio::test]
    async fn a_sandboxed_agent_call_lands_in_the_requests_project_store() {
        use crate::agent::subagent::AgentConstraints;
        use crate::bus::EventBus;
        use crate::daemon_config::CircuitBreakerConfig;
        use crate::security::sandbox::{SandboxManager, SandboxPolicy};
        use crate::test_util::make_agent;
        use crate::tools::ToolRegistry;
        use openalpaca_llm::ToolCall;

        let fx = Fixture::new();
        fx.task("t-5555ffff-0000-0000-0000-000000000000", "Sandbox run");
        let project = fx.project_root();
        let startup_root = TempDir::new().unwrap();

        // The registry the daemon builds at boot: a startup workspace_root that
        // is NOT this request's project.
        let registry = ToolRegistry::default();
        for tool in super::super::builtin_tools(
            Some(fx.db.clone()),
            None,
            Some(Arc::new(ArcSwap::from_pointee(DaemonConfig::default()))),
            None,
            Some(startup_root.path().to_path_buf()),
        ) {
            registry.register(tool).unwrap();
        }
        let registry = Arc::new(registry);

        // The template grants the capability; capability resolution finds the
        // tool by it.
        let agent = make_agent("writing_agent", vec!["artifact_write"]);
        let defs = crate::tools::resolve_agent_tools(&agent, &registry, None);
        assert!(
            defs.iter().any(|d| d.name == "artifact_write"),
            "the capability must resolve to the tool"
        );

        let sandbox =
            SandboxManager::new(registry, EventBus::default(), &CircuitBreakerConfig::default());
        let constraints = AgentConstraints {
            allowed_capabilities: vec!["artifact_write".to_string()],
            ..Default::default()
        };
        let mut policy = SandboxPolicy::from_constraints("writing_agent", &constraints);
        let call = ToolCall {
            id: "call-1".to_string(),
            name: "artifact_write".to_string(),
            arguments: serde_json::json!({
                "name": "deliverable",
                "kind": "markdown",
                "content": "# Deliverable\n"
            }),
        };
        let call_ctx = ctx(
            Some(project.to_str().unwrap()),
            Some("t-5555ffff-0000-0000-0000-000000000000"),
        );

        // artifact_write carries the same destructive annotation as its sibling
        // writers, so an agent that declares no confirmation list gates it
        // exactly like `file_write`/`workspace_write` — fail-closed with no
        // broker wired. A real run has a broker (or the daemon's auto-approve).
        let refusal = sandbox
            .execute_tool(&call, &policy, &call_ctx)
            .await
            .unwrap_err();
        assert!(refusal.contains("requires human confirmation"), "{refusal}");
        policy.auto_approve = true;

        let out = sandbox
            .execute_tool(&call, &policy, &call_ctx)
            .await
            .unwrap();

        let path = PathBuf::from(parse(&out)["path"].as_str().unwrap());
        assert!(
            path.starts_with(project.join(".openalpaca").join("artifacts")),
            "expected the request's project store, got {}",
            path.display()
        );
        assert!(
            !path.starts_with(startup_root.path()),
            "the startup workspace_root must not attract artifacts"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Deliverable\n");
    }

    #[test]
    fn split_name_only_honours_extension_shaped_suffixes() {
        assert_eq!(split_name("report.md"), ("report", Some("report.md")));
        assert_eq!(split_name("data.csv"), ("data", Some("data.csv")));
        assert_eq!(
            split_name("v1.2 plan"),
            ("v1.2 plan", Some("v1.2 plan")),
            "a non-extension suffix stays part of the title"
        );
        assert_eq!(split_name("notes"), ("notes", Some("notes")));
        assert_eq!(split_name(".hidden"), (".hidden", Some(".hidden")));
    }
}
