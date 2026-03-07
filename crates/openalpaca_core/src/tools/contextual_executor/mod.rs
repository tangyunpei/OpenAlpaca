//! ContextualToolExecutor: per-request wrapper that injects owner_id
//! into owner-scoped tools (e.g. memory_search) and handles workspace-scoped
//! tools (workspace_read, workspace_write) before delegating to the
//! underlying ToolRegistry.
//!
//! Also handles skill-bundled script tools (`skill_script:*` prefix) when
//! a `ScriptExecutionContext` is attached via `with_scripts()`.

use crate::orchestrator::task_state::{TaskState, WorkspaceEntryType};
use crate::security::sandbox::ToolExecutor;
use crate::tools::ToolRegistry;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;

/// Tools that need owner_id injection only.
const OWNER_ONLY_TOOLS: &[&str] = &["update_persona"];

/// Tools that need both owner_id and workspace_id injection.
const OWNER_AND_WORKSPACE_TOOLS: &[&str] = &["memory_search"];

/// Tools that operate on the task workspace (handled directly, not forwarded to registry).
const WORKSPACE_SCOPED_TOOLS: &[&str] = &["workspace_read", "workspace_write"];

/// Per-request execution context carrying the authenticated owner and optional task.
pub struct ToolExecutionContext {
    pub owner_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub db: Option<openalpaca_storage::Database>,
    pub workspace_id: Option<String>,
}

// ---------------------------------------------------------------------------
// ScriptExecutionContext: skill-bundled script tools
// ---------------------------------------------------------------------------

/// Runtime context for skill-bundled script tools.
///
/// Created per skill invocation from the skill's `scripts` frontmatter config.
/// Maps prefixed tool names (`skill_script:X`) to resolved script paths.
pub struct ScriptExecutionContext {
    /// Map from prefixed tool name ("skill_script:X") to (resolved_path, interpreter, timeout).
    pub(crate) scripts: HashMap<String, (PathBuf, Option<String>, u64)>,
    /// The skill directory (used as working directory for script execution).
    skill_dir: PathBuf,
}

impl ScriptExecutionContext {
    pub fn new(
        skill_dir: &std::path::Path,
        configs: &[crate::middleware::skill::ScriptConfig],
    ) -> Result<Self, String> {
        let mut scripts = HashMap::new();
        for cfg in configs {
            let script_path = skill_dir.join("scripts").join(&cfg.file);
            // Security: ensure resolved path stays within skill_dir/scripts/
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
            let tool_name = format!("skill_script:{}", cfg.name);
            scripts.insert(tool_name, (canonical, cfg.interpreter.clone(), cfg.timeout_secs));
        }
        Ok(Self {
            scripts,
            skill_dir: skill_dir.to_path_buf(),
        })
    }

    fn has_script(&self, tool_name: &str) -> bool {
        self.scripts.contains_key(tool_name)
    }

    async fn execute_script(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        let (path, interpreter, timeout_secs) = self.scripts.get(tool_name)
            .ok_or_else(|| format!("Unknown script tool: {}", tool_name))?;

        let args = json_to_cli_args(arguments);

        let mut cmd = if let Some(interp) = interpreter {
            let mut c = Command::new(interp);
            c.arg(path);
            c
        } else {
            Command::new(path)
        };
        cmd.args(&args);
        cmd.current_dir(&self.skill_dir);

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(*timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| format!("Script '{}' timed out after {}s", tool_name, timeout_secs))?
        .map_err(|e| format!("Failed to execute script '{}': {}", tool_name, e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "Script '{}' failed (exit {}): {}",
                tool_name,
                output.status.code().unwrap_or(-1),
                stderr.chars().take(500).collect::<String>()
            ))
        }
    }
}

/// Convert JSON object to `--key=value` CLI arguments.
fn json_to_cli_args(value: &serde_json::Value) -> Vec<String> {
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

// ---------------------------------------------------------------------------
// ContextualToolExecutor
// ---------------------------------------------------------------------------

/// A ToolExecutor that injects contextual data (owner_id) into
/// owner-scoped tools, handles workspace tools directly, handles
/// skill-bundled script tools, and delegates the rest to the inner ToolRegistry.
pub struct ContextualToolExecutor {
    registry: Arc<ToolRegistry>,
    context: ToolExecutionContext,
    scripts: Option<ScriptExecutionContext>,
}

impl ContextualToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>, context: ToolExecutionContext) -> Self {
        Self { registry, context, scripts: None }
    }

    pub fn with_scripts(
        registry: Arc<ToolRegistry>,
        context: ToolExecutionContext,
        scripts: ScriptExecutionContext,
    ) -> Self {
        Self { registry, context, scripts: Some(scripts) }
    }

    /// Handle workspace_read: returns matching entries as JSON.
    fn handle_workspace_read(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let task_id = self
            .context
            .task_id
            .as_deref()
            .ok_or_else(|| "workspace_read requires a task context".to_string())?;
        let db = self
            .context
            .db
            .as_ref()
            .ok_or_else(|| "workspace_read requires database access".to_string())?;

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

        let key = arguments.get("key").and_then(|v| v.as_str()).unwrap_or("");

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

    /// Handle workspace_write: upserts an entry, persists to DB.
    /// Retries up to 3 times on optimistic locking conflicts, which are
    /// expected during concurrent DAG execution.
    async fn handle_workspace_write(
        &self,
        arguments: &serde_json::Value,
        agent_id: &str,
    ) -> Result<String, String> {
        let task_id = self
            .context
            .task_id
            .as_deref()
            .ok_or_else(|| "workspace_write requires a task context".to_string())?;
        let db = self
            .context
            .db
            .as_ref()
            .ok_or_else(|| "workspace_write requires database access".to_string())?;

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
                // Brief async backoff to reduce collision probability
                tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt))).await;
            }
        }

        Err(format!(
            "Workspace write for key '{}' failed after {} retries due to concurrent modifications",
            key, MAX_RETRIES
        ))
    }
}

#[async_trait]
impl ToolExecutor for ContextualToolExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        // Handle skill script tools (skill_script:* prefix)
        if tool_name.starts_with("skill_script:") {
            if let Some(ref scripts) = self.scripts
                && scripts.has_script(tool_name)
            {
                return scripts.execute_script(tool_name, arguments).await;
            }
            return Err(format!("Unknown script tool: {}", tool_name));
        }

        // Handle workspace tools directly (no registry delegation)
        if WORKSPACE_SCOPED_TOOLS.contains(&tool_name) {
            let agent_id = self.context.agent_id.as_deref().unwrap_or("unknown");

            return match tool_name {
                "workspace_read" => self.handle_workspace_read(arguments),
                "workspace_write" => self.handle_workspace_write(arguments, agent_id).await,
                _ => Err(format!("Unknown workspace tool: {}", tool_name)),
            };
        }

        // Handle tools that need both owner_id and workspace_id
        if OWNER_AND_WORKSPACE_TOOLS.contains(&tool_name) {
            let owner_id = self.context.owner_id.as_ref().ok_or_else(|| {
                format!(
                    "Tool '{}' requires owner_id but none provided in execution context",
                    tool_name
                )
            })?;
            let mut args = arguments.clone();
            if let Some(obj) = args.as_object_mut() {
                obj.insert(
                    "owner_id".to_string(),
                    serde_json::Value::String(owner_id.clone()),
                );
                if let Some(ref ws_id) = self.context.workspace_id {
                    obj.insert(
                        "workspace_id".to_string(),
                        serde_json::Value::String(ws_id.clone()),
                    );
                }
            }
            return self.registry.execute(tool_name, &args).await;
        }

        // Handle tools that need owner_id only (no workspace_id)
        if OWNER_ONLY_TOOLS.contains(&tool_name) {
            let owner_id = self.context.owner_id.as_ref().ok_or_else(|| {
                format!(
                    "Tool '{}' requires owner_id but none provided in execution context",
                    tool_name
                )
            })?;
            let mut args = arguments.clone();
            if let Some(obj) = args.as_object_mut() {
                obj.insert(
                    "owner_id".to_string(),
                    serde_json::Value::String(owner_id.clone()),
                );
            }
            return self.registry.execute(tool_name, &args).await;
        }

        // Default: pass through to registry
        self.registry.execute(tool_name, arguments).await
    }

    fn registered_tools(&self) -> Vec<String> {
        let mut tools = self.registry.registered_tool_names();
        // Always advertise workspace tools so agents see them in capability
        // checks. The execute() handlers already return clear errors when
        // task_id is absent ("workspace_read requires a task context", etc.).
        for name in WORKSPACE_SCOPED_TOOLS {
            if !tools.iter().any(|t| t == name) {
                tools.push((*name).to_string());
            }
        }
        // Advertise skill-bundled script tools when attached
        if let Some(ref scripts) = self.scripts {
            for name in scripts.scripts.keys() {
                if !tools.iter().any(|t| t == name) {
                    tools.push(name.clone());
                }
            }
        }
        tools
    }

    fn shell_like_tools(&self) -> Vec<String> {
        self.registry.command_backend_tool_names()
    }
}

#[cfg(test)]
mod tests;
