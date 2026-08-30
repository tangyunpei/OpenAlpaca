//! `task_status` — Routing V2 main-loop tool wrapping the shared task-query
//! core in `orchestrator::task_ops` (the `/tasks`//status intent handler is a
//! thin wrapper over the same function, so both paths return identical JSON).
//!
//! Constructed per-request by the tool-mode main loop; NOT registered in the
//! global registry.

use crate::context::SharedContext;
use crate::orchestrator::task_status_query;
use crate::tools::registry::{BuiltInTool, ToolContext};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

/// Reports the status of one task, or lists the requester's active tasks.
pub struct TaskStatusTool {
    db: Option<openalpaca_storage::Database>,
    shared_context: Arc<SharedContext>,
}

impl TaskStatusTool {
    pub fn new(
        db: Option<openalpaca_storage::Database>,
        shared_context: Arc<SharedContext>,
    ) -> Self {
        Self { db, shared_context }
    }
}

#[async_trait]
impl BuiltInTool for TaskStatusTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("task_status requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let task_id = arguments
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);

        // Identity from context (same resolution as `start_workflow`'s
        // created_by): principal id first, then owner_id, then system.
        let created_by = match &ctx.principal {
            Some(crate::security::policy::Principal::System) => "system".to_string(),
            Some(crate::security::policy::Principal::User { global_id }) => global_id.clone(),
            Some(crate::security::policy::Principal::External { provider, id }) => {
                format!("{}:{}", provider, id)
            }
            None => ctx
                .owner_id
                .clone()
                .unwrap_or_else(|| "system".to_string()),
        };

        task_status_query(self.db.as_ref(), &self.shared_context, task_id, &created_by)
    }
}

/// Build the tool definition for `task_status`.
pub fn task_status_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "task_status".to_string(),
        description: "Look up background task status. With a task_id: that task's full \
                       record (status, progress, result summary, artifacts). Without: the \
                       user's active tasks, or their recent finished ones when nothing is \
                       running. Returns JSON."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Optional task id to look up; omit to list the user's tasks"
                }
            },
            "required": []
        }),
        strict: Some(true),
        input_examples: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_task_status_matches_shared_handler_core() {
        let shared = Arc::new(SharedContext::new());
        shared
            .task_registry
            .register("task-1".to_string(), "Research".to_string());
        let tool = TaskStatusTool::new(None, shared.clone());
        let ctx = ToolContext {
            owner_id: Some("user1".to_string()),
            principal: Some(crate::security::policy::Principal::User {
                global_id: "user1".to_string(),
            }),
            ..Default::default()
        };

        // Single-task lookup — byte-identical to the intent handler's core.
        let via_tool = tool
            .execute_with_context(&serde_json::json!({"task_id": "task-1"}), &ctx)
            .await
            .unwrap();
        let via_core = task_status_query(
            None,
            &shared,
            Some("task-1".to_string()),
            "user1",
        )
        .unwrap();
        assert_eq!(via_tool, via_core);
        assert!(via_tool.contains("task-1"));
        assert!(via_tool.contains("Research"));

        // List form (no task_id) — same equivalence.
        let via_tool = tool
            .execute_with_context(&serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let via_core = task_status_query(None, &shared, None, "user1").unwrap();
        assert_eq!(via_tool, via_core);
        let parsed: serde_json::Value = serde_json::from_str(&via_tool).unwrap();
        assert_eq!(parsed["count"], 1);
    }

    #[tokio::test]
    async fn test_task_status_unknown_task_reports_not_found() {
        let tool = TaskStatusTool::new(None, Arc::new(SharedContext::new()));
        let result = tool
            .execute_with_context(
                &serde_json::json!({"task_id": "ghost"}),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["error"], "not_found");
    }
}
