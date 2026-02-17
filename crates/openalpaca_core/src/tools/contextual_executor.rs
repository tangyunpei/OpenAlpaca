//! ContextualToolExecutor: per-request wrapper that injects owner_id
//! into owner-scoped tools (e.g. memory_search) and handles workspace-scoped
//! tools (workspace_read, workspace_write) before delegating to the
//! underlying ToolRegistry.

use crate::orchestrator::task_state::{TaskState, WorkspaceEntryType};
use crate::security::sandbox::ToolExecutor;
use crate::tools::ToolRegistry;
use async_trait::async_trait;
use std::sync::Arc;

/// Tools whose arguments need owner_id injection.
const OWNER_SCOPED_TOOLS: &[&str] = &["memory_search", "update_user"];

/// Tools that operate on the task workspace (handled directly, not forwarded to registry).
const WORKSPACE_SCOPED_TOOLS: &[&str] = &["workspace_read", "workspace_write"];

/// Per-request execution context carrying the authenticated owner and optional task.
pub struct ToolExecutionContext {
    pub owner_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub db: Option<openalpaca_storage::Database>,
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

        // Enforce the documented 8KB content size limit
        const MAX_WORKSPACE_CONTENT_SIZE: usize = 8192;
        if content.len() > MAX_WORKSPACE_CONTENT_SIZE {
            return Err(format!(
                "Content size {} bytes exceeds the {} byte limit",
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

        const MAX_RETRIES: usize = 3;
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

            state.workspace.write(key, content, agent_id, entry_type.clone(), &[])?;

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
                    key, attempt + 1, MAX_RETRIES
                );
                // Brief async backoff to reduce collision probability
                tokio::time::sleep(std::time::Duration::from_millis(10 * (1 << attempt))).await;
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
            let agent_id = self
                .context
                .agent_id
                .as_deref()
                .unwrap_or("unknown");

            return match tool_name {
                "workspace_read" => self.handle_workspace_read(arguments),
                "workspace_write" => self.handle_workspace_write(arguments, agent_id).await,
                _ => Err(format!("Unknown workspace tool: {}", tool_name)),
            };
        }

        // Handle owner-scoped tools (inject owner_id)
        if OWNER_SCOPED_TOOLS.contains(&tool_name) {
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
        // Add workspace tools so they pass capability checks
        if self.context.task_id.is_some() {
            for name in WORKSPACE_SCOPED_TOOLS {
                if !tools.contains(&name.to_string()) {
                    tools.push(name.to_string());
                }
            }
        }
        tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use openalpaca_llm::ToolDefinition;

    struct CaptureTool;

    #[async_trait]
    impl BuiltInTool for CaptureTool {
        async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
            // Return the arguments as a string so tests can inspect them
            Ok(arguments.to_string())
        }
    }

    fn make_registry_with_tools(names: &[&str]) -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        for name in names {
            registry.register(RegisteredTool {
                definition: ToolDefinition {
                    name: name.to_string(),
                    description: format!("{} tool", name),
                    parameters: serde_json::json!({"type": "object"}),
                },
                backend: ToolBackend::BuiltIn(Arc::new(CaptureTool)),
            });
        }
        Arc::new(registry)
    }

    #[tokio::test]
    async fn test_injects_owner_id_for_memory_search() {
        let registry = make_registry_with_tools(&["memory_search"]);
        let executor = ContextualToolExecutor::new(
            registry,
            ToolExecutionContext {
                owner_id: Some("user-42".to_string()),
                task_id: None,
                agent_id: None,
                db: None,
            },
        );

        let result = executor
            .execute("memory_search", &serde_json::json!({"query": "hello"}))
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["owner_id"], "user-42");
        assert_eq!(parsed["query"], "hello");
    }

    #[tokio::test]
    async fn test_passes_through_non_memory_tools() {
        let registry = make_registry_with_tools(&["web_search"]);
        let executor = ContextualToolExecutor::new(
            registry,
            ToolExecutionContext {
                owner_id: Some("user-42".to_string()),
                task_id: None,
                agent_id: None,
                db: None,
            },
        );

        let result = executor
            .execute("web_search", &serde_json::json!({"query": "hello"}))
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // owner_id should NOT be injected for non-memory tools
        assert!(parsed.get("owner_id").is_none());
    }

    #[tokio::test]
    async fn test_overrides_spoofed_owner_id() {
        let registry = make_registry_with_tools(&["memory_search"]);
        let executor = ContextualToolExecutor::new(
            registry,
            ToolExecutionContext {
                owner_id: Some("real-owner".to_string()),
                task_id: None,
                agent_id: None,
                db: None,
            },
        );

        // LLM tries to spoof owner_id
        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "hello", "owner_id": "evil-owner"}),
            )
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // Should be overridden with the real owner
        assert_eq!(parsed["owner_id"], "real-owner");
    }

    #[tokio::test]
    async fn test_workspace_tools_listed_when_task_id_present() {
        let registry = make_registry_with_tools(&["web_search"]);
        let executor = ContextualToolExecutor::new(
            registry,
            ToolExecutionContext {
                owner_id: None,
                task_id: Some("task-1".to_string()),
                agent_id: Some("test-agent".to_string()),
                db: None,
            },
        );

        let tools = executor.registered_tools();
        assert!(tools.contains(&"workspace_read".to_string()));
        assert!(tools.contains(&"workspace_write".to_string()));
    }

    #[tokio::test]
    async fn test_workspace_tools_not_listed_without_task_id() {
        let registry = make_registry_with_tools(&["web_search"]);
        let executor = ContextualToolExecutor::new(
            registry,
            ToolExecutionContext {
                owner_id: None,
                task_id: None,
                agent_id: None,
                db: None,
            },
        );

        let tools = executor.registered_tools();
        assert!(!tools.contains(&"workspace_read".to_string()));
        assert!(!tools.contains(&"workspace_write".to_string()));
    }

    #[tokio::test]
    async fn test_workspace_read_without_task_context_errors() {
        let registry = make_registry_with_tools(&[]);
        let executor = ContextualToolExecutor::new(
            registry,
            ToolExecutionContext {
                owner_id: None,
                task_id: None,
                agent_id: None,
                db: None,
            },
        );

        let result = executor
            .execute("workspace_read", &serde_json::json!({"key": "test"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("requires a task context"));
    }
}
