//! ContextualToolExecutor: per-request wrapper that injects owner_id
//! into owner-scoped tools (e.g. memory_search) and handles workspace-scoped
//! tools (workspace_read, workspace_write) before delegating to the
//! underlying ToolRegistry.

use crate::orchestrator::task_state::{TaskState, WorkspaceEntryType};
use crate::security::sandbox::ToolExecutor;
use crate::tools::ToolRegistry;
use async_trait::async_trait;
use std::sync::Arc;

/// Tools that need owner_id injection only.
const OWNER_ONLY_TOOLS: &[&str] = &[
    "update_user",
    "update_soul",
    "update_identity",
];

/// Tools that need both owner_id and workspace_id injection.
const OWNER_AND_WORKSPACE_TOOLS: &[&str] = &[
    "memory_search",
];

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

/// A ToolExecutor that injects contextual data (owner_id) into
/// owner-scoped tools, handles workspace tools directly, and
/// delegates the rest to the inner ToolRegistry.
pub struct ContextualToolExecutor {
    registry: Arc<ToolRegistry>,
    context: ToolExecutionContext,
}

impl ContextualToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>, context: ToolExecutionContext) -> Self {
        Self { registry, context }
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
        tools
    }

    fn shell_like_tools(&self) -> Vec<String> {
        self.registry.command_backend_tool_names()
    }
}

#[cfg(test)]
mod tests;
