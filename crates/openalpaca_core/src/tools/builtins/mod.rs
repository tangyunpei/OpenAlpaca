mod file_ops;
mod helpers;
mod memory_search;
mod send;
mod shell_execute;
mod update_persona;
mod web_fetch;
mod web_search;

use crate::daemon_config::DaemonConfig;
use crate::orchestrator::task_state::{TaskState, WorkspaceEntryType};
use crate::orchestrator::ConnectorSendProvider;
use crate::tools::registry::{BuiltInTool, ToolContext};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_llm::{ToolDefinition, WebSearchConfig};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::process::Command;

/// Shared lock type for the connector send provider, set post-construction.
pub type ConnectorSendLock = Arc<RwLock<Option<Arc<dyn ConnectorSendProvider>>>>;

use self::file_ops::{file_read_tool, file_write_tool};
use self::memory_search::memory_search_tool;
use self::shell_execute::shell_execute_tool;
use self::update_persona::update_persona_tool;
use self::web_fetch::web_fetch_tool;
use self::web_search::web_search_tool;

use super::registry::{RegisteredTool, ToolBackend};

/// Context required by the `update_persona` tool at runtime.
/// Re-export from the update_persona module.
pub use update_persona::PersonaToolContext;

// ---------------------------------------------------------------------------
// WorkspaceReadTool
// ---------------------------------------------------------------------------

struct WorkspaceReadTool {
    db: Option<openalpaca_storage::Database>,
}

#[async_trait]
impl BuiltInTool for WorkspaceReadTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("workspace_read requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "workspace_read requires database context".to_string())?;
        let task_id = ctx
            .task_id
            .as_deref()
            .ok_or_else(|| "workspace_read requires a task context".to_string())?;

        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let task = repo
            .get(task_id)
            .map_err(|e| format!("Failed to load task: {e}"))?
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        let state: TaskState = match task.state_json.as_deref() {
            Some(json) => serde_json::from_str(json)
                .map_err(|e| format!("Failed to parse task state: {e}"))?,
            None => return Ok("[]".to_string()),
        };

        let key = arguments
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let entries = state.workspace.read(key);
        let result: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "key": e.key,
                    "content": e.content,
                    "author": e.author_agent_id,
                    "type": e.entry_type,
                })
            })
            .collect();

        serde_json::to_string(&result).map_err(|e| format!("Serialization error: {e}"))
    }
}

// ---------------------------------------------------------------------------
// WorkspaceWriteTool
// ---------------------------------------------------------------------------

struct WorkspaceWriteTool {
    db: Option<openalpaca_storage::Database>,
}

#[async_trait]
impl BuiltInTool for WorkspaceWriteTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("workspace_write requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "workspace_write requires database context".to_string())?;
        let task_id = ctx
            .task_id
            .as_deref()
            .ok_or_else(|| "workspace_write requires a task context".to_string())?;
        let agent_id = ctx.agent_id.as_deref().unwrap_or("unknown");

        let key = arguments
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: key".to_string())?;
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        // Enforce the documented 32KB content size limit
        const MAX_WORKSPACE_CONTENT_SIZE: usize = 32768;
        if content.len() > MAX_WORKSPACE_CONTENT_SIZE {
            return Err(format!(
                "Content size {} bytes exceeds the {} byte limit. \
                 Condense or summarize your content to fit within the limit, then retry.",
                content.len(),
                MAX_WORKSPACE_CONTENT_SIZE
            ));
        }

        let entry_type_str = arguments
            .get("entry_type")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let entry_type = match entry_type_str {
            "artifact" => WorkspaceEntryType::Artifact,
            "summary" => WorkspaceEntryType::Summary,
            "context" => WorkspaceEntryType::Context,
            _ => WorkspaceEntryType::Text,
        };

        const MAX_RETRIES: usize = 5;
        for attempt in 0..MAX_RETRIES {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            let task = repo
                .get(task_id)
                .map_err(|e| format!("Failed to load task: {e}"))?
                .ok_or_else(|| format!("Task '{}' not found", task_id))?;

            let mut state: TaskState = match task.state_json.as_deref() {
                Some(json) => serde_json::from_str(json)
                    .map_err(|e| format!("Failed to parse task state: {e}"))?,
                None => return Err("Task has no state".to_string()),
            };

            state
                .workspace
                .write(key, content, agent_id, entry_type.clone(), &[])?;

            // Set file_asset_id in the same state mutation (before persisting)
            if let Some(fid) = arguments.get("file_asset_id").and_then(|v| v.as_str()) {
                state.workspace.set_file_asset_id(key, fid);
            }

            let new_json = state.to_json();
            let updated = repo
                .update_state(task_id, &new_json, task.state_version)
                .map_err(|e| format!("Failed to persist workspace: {e}"))?;

            if updated {
                return Ok(format!("Workspace entry '{}' written successfully", key));
            }

            // Version conflict — retry with fresh state
            if attempt < MAX_RETRIES - 1 {
                tracing::debug!(
                    "Workspace write version conflict for key '{}' (attempt {}/{}), retrying",
                    key,
                    attempt + 1,
                    MAX_RETRIES
                );
                // Jittered backoff to reduce collision probability.
                // Simple deterministic jitter using attempt number as seed (no rand crate needed).
                let base_ms = 10u64 * (1u64 << attempt.min(4)); // 10, 20, 40, 80, 160
                let jitter = (attempt as u64 * 7 + 3) % (base_ms / 4).max(1); // deterministic jitter
                tokio::time::sleep(std::time::Duration::from_millis(base_ms + jitter)).await;
            }
        }

        Err(format!(
            "Workspace write for key '{}' failed after {} retries due to concurrent modifications",
            key, MAX_RETRIES
        ))
    }
}

/// Return all built-in tool definitions and implementations.
/// When `db` is provided, memory-backed tools (memory_search) are included.
/// When `embedder` is provided, memory_search uses hybrid (FTS + vector) search.
///
/// `workspace_root` is the explicit workspace directory for file tools.
/// If `None`, falls back to `std::env::current_dir()` (for backward
/// compatibility in tests and CLI).
pub fn builtin_tools(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
) -> Vec<RegisteredTool> {
    let ws_cfg = web_search_config
        .unwrap_or_else(|| Arc::new(ArcSwap::from_pointee(WebSearchConfig::default())));

    let ws_root = workspace_root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Clone db before memory_search consumes it — workspace tools need it too.
    let ws_db = db.clone();

    let mut tools = vec![
        web_search_tool(ws_cfg),
        web_fetch_tool(),
        file_read_tool(ws_root.clone()),
        file_write_tool(ws_root),
        shell_execute_tool(),
    ];
    if let (Some(db), Some(dc)) = (db, daemon_config) {
        tools.push(memory_search_tool(db, embedder, dc));
    }

    // Always register workspace tools — definitions needed for capability resolution.
    for def in workspace_tool_definitions() {
        let cap = if def.name == "workspace_read" {
            vec!["workspace_read".to_string()]
        } else {
            vec!["workspace_write".to_string()]
        };
        let backend = if def.name == "workspace_read" {
            ToolBackend::BuiltIn(Arc::new(WorkspaceReadTool { db: ws_db.clone() }))
        } else {
            ToolBackend::BuiltIn(Arc::new(WorkspaceWriteTool { db: ws_db.clone() }))
        };
        tools.push(RegisteredTool {
            definition: def,
            backend,
            provides_capabilities: cap,
            exempt_from_timeout: false,
        });
    }

    tools
}

/// Return all built-in tools, including `update_persona`,
/// and optionally `send`.
pub fn builtin_tools_with_persona_context(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    persona_ctx: PersonaToolContext,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
    connector_send_provider: Option<ConnectorSendLock>,
) -> Vec<RegisteredTool> {
    let mut tools = builtin_tools(db, embedder, daemon_config, web_search_config, workspace_root);
    tools.push(update_persona_tool(persona_ctx));
    if let Some(provider) = connector_send_provider {
        tools.push(send::send_tool(provider));
    }
    tools
}

/// Return ToolDefinition entries for workspace tools.
/// Used by `builtin_tools()` to register workspace tools with BuiltIn backends,
/// and available externally for tests and capability checks.
pub fn workspace_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "workspace_read".to_string(),
            description: "Read entries from the shared task workspace used for inter-agent \
                collaboration. Provide a specific key to retrieve one entry, or omit key \
                to list all entries. Returns a JSON array of entries with key, content, \
                author agent ID, and entry type. Use this to read work products from \
                other agents in a multi-agent task."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "The workspace entry key to read (e.g., 'research_results', 'draft_v1'). Omit to list all entries."
                    }
                },
                "required": []
            }),
            strict: Some(true),
            input_examples: None,
        },
        ToolDefinition {
            name: "workspace_write".to_string(),
            description: "Write an entry to the shared task workspace for inter-agent \
                collaboration. Content is limited to 32KB. Entry types: 'text' (default), \
                'artifact' (code/document output), 'summary' (condensed results), or \
                'context' (background information). Supports optimistic concurrency — \
                retries automatically on conflict from parallel agent writes. Other \
                agents can read your entries via workspace_read."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "string",
                        "description": "A descriptive key for this entry (e.g., 'research_results', 'outline', 'draft_v1'). Must be unique per task."
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to store as a UTF-8 string (max 32KB)",
                        "maxLength": 32768
                    },
                    "entry_type": {
                        "type": "string",
                        "enum": ["text", "artifact", "summary", "context"],
                        "description": "Entry type: 'text' (general), 'artifact' (code/document output), 'summary' (condensed results), 'context' (background info). Default: 'text'"
                    },
                    "file_asset_id": {
                        "type": "string",
                        "description": "Optional file asset ID to associate with this entry. When set, enables file delivery to external channels (e.g. Telegram)."
                    }
                },
                "required": ["key", "content"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({
                    "key": "research_results",
                    "content": "Found 3 relevant papers on the topic...",
                    "entry_type": "summary"
                }),
                serde_json::json!({
                    "key": "draft_v1",
                    "content": "# Introduction\n\nThis document outlines...",
                    "entry_type": "artifact"
                }),
            ]),
        },
    ]
}

// ---------------------------------------------------------------------------
// ScriptToolBuiltIn
// ---------------------------------------------------------------------------

/// A skill-bundled script tool wrapped as a BuiltInTool.
pub struct ScriptToolBuiltIn {
    script_path: PathBuf,
    interpreter: Option<String>,
    timeout_secs: u64,
    skill_dir: PathBuf,
}

impl ScriptToolBuiltIn {
    /// Create with path-traversal validation.
    pub fn new(
        skill_dir: &std::path::Path,
        cfg: &crate::middleware::skill::ScriptConfig,
    ) -> Result<Self, String> {
        let script_path = skill_dir.join("scripts").join(&cfg.file);
        let canonical = script_path.canonicalize().map_err(|e| {
            format!("Script '{}' not found: {}", cfg.file, e)
        })?;
        let scripts_dir = skill_dir.join("scripts").canonicalize().map_err(|e| {
            format!("Scripts directory not found: {}", e)
        })?;
        if !canonical.starts_with(&scripts_dir) {
            return Err(format!(
                "Script '{}' resolves outside scripts/ directory (path traversal blocked)",
                cfg.file
            ));
        }
        Ok(Self {
            script_path: canonical,
            interpreter: cfg.interpreter.clone(),
            timeout_secs: cfg.timeout_secs,
            skill_dir: skill_dir.to_path_buf(),
        })
    }

    /// Generate a ToolDefinition for a script tool.
    pub fn tool_definition(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: format!("skill_script:{}", name),
            description: format!("Skill script: {}", name),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            strict: None,
            input_examples: None,
        }
    }
}

#[async_trait]
impl BuiltInTool for ScriptToolBuiltIn {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let args = json_to_cli_args(arguments);
        let mut cmd = if let Some(ref interp) = self.interpreter {
            let mut c = Command::new(interp);
            c.arg(&self.script_path);
            c
        } else {
            Command::new(&self.script_path)
        };
        cmd.args(&args);
        cmd.current_dir(&self.skill_dir);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| format!("Script timed out after {}s", self.timeout_secs))?
        .map_err(|e| format!("Failed to execute script: {}", e))?;

        // Cap output size to prevent memory pressure from scripts producing
        // unbounded stdout/stderr, matching the command backend limit.
        const MAX_SCRIPT_OUTPUT_BYTES: usize = 512 * 1024; // 512 KB
        if output.status.success() {
            let stdout = String::from_utf8_lossy(
                &output.stdout[..output.stdout.len().min(MAX_SCRIPT_OUTPUT_BYTES)],
            );
            Ok(stdout.to_string())
        } else {
            let stderr = String::from_utf8_lossy(
                &output.stderr[..output.stderr.len().min(MAX_SCRIPT_OUTPUT_BYTES)],
            );
            Err(format!(
                "Script failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.chars().take(500).collect::<String>()
            ))
        }
    }
}

/// Convert JSON object to `--key=value` CLI arguments.
pub fn json_to_cli_args(value: &serde_json::Value) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            let str_val = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            args.push(format!("--{}={}", key, str_val));
        }
    }
    args
}

#[cfg(test)]
mod tests;
