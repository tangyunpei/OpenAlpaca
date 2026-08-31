//! `steer_workflow` — the Routing V2 main-loop tool that injects a user
//! message into a running lead-agent workflow on the current lane.
//!
//! Constructed PER-REQUEST (same pattern as `start_workflow`): the main loop
//! builds one instance per user turn and injects it into its per-request
//! registry — only when the lane has active workflows and steering is
//! enabled. NOT registered in the global registry.

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::runner::steering::{SteeringMsg, SteeringPushError, push_steering};
use crate::tools::registry::{BuiltInTool, ToolContext};
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;
use uuid::Uuid;

/// Per-request tool that pushes a steering message into a running workflow's
/// inbox via the shared [`push_steering`] helper (which emits
/// `WorkflowSteered` on success).
///
pub struct SteerWorkflowTool {
    shared_context: Arc<SharedContext>,
    bus: EventBus,
}

impl SteerWorkflowTool {
    pub fn new(shared_context: Arc<SharedContext>, bus: EventBus) -> Self {
        Self {
            shared_context,
            bus,
        }
    }
}

#[async_trait]
impl BuiltInTool for SteerWorkflowTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("steer_workflow requires execution context — use execute_with_context".to_string())
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
            .ok_or_else(|| "Missing required parameter: task_id".to_string())?;
        let message = arguments
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| "Missing required parameter: message".to_string())?;

        let lane_key = ctx
            .lane_key
            .as_deref()
            .ok_or_else(|| "steer_workflow requires a lane context".to_string())?;

        // Cross-lane guard: only workflows active on THIS lane are steerable.
        let active = self.shared_context.workflows_for_lane(lane_key);
        if !active.iter().any(|id| id == task_id) {
            return Err(if active.is_empty() {
                format!(
                    "Task '{}' is not a running workflow on this conversation — nothing is \
                     currently running here. Answer the user from the conversation instead, \
                     or use start_workflow for new work.",
                    task_id,
                )
            } else {
                format!(
                    "Task '{}' is not a running workflow on this conversation. Workflows \
                     running here: {}. Only these can be steered.",
                    task_id,
                    active.join(", "),
                )
            });
        }

        // Identity for follow-up conversion re-entry: prefer the threaded
        // principal, fall back to owner_id, then System (mirrors the
        // created_by resolution in `start_workflow`).
        let principal = match &ctx.principal {
            Some(p) => p.clone(),
            None => match &ctx.owner_id {
                Some(owner) => crate::security::policy::Principal::User {
                    global_id: owner.clone(),
                },
                None => crate::security::policy::Principal::System,
            },
        };
        let scope = ctx
            .scope
            .clone()
            .unwrap_or(crate::security::policy::Scope::Global);

        let msg = SteeringMsg {
            text: message.to_string(),
            request_id: ctx.request_id.unwrap_or_else(Uuid::nil),
            principal,
            scope,
            workspace_path: ctx.workspace_path.clone(),
            received_at: Utc::now(),
        };

        match push_steering(&self.shared_context, &self.bus, task_id, lane_key, msg) {
            Ok(depth) => {
                Ok(format!(
                    "Steering message queued for workflow {} ({} message{} waiting). The \
                     workflow picks it up at its next round. Confirm this to the user in \
                     your own words.",
                    task_id,
                    depth,
                    if depth == 1 { "" } else { "s" },
                ))
            }
            Err(SteeringPushError::Full) => Err(format!(
                "The steering queue for workflow {} is full — it has not caught up with \
                 earlier messages yet. Do NOT retry steer_workflow. Explain the backlog to \
                 the user and offer queue_followup to queue this for after the workflow \
                 finishes.",
                task_id,
            )),
            Err(SteeringPushError::Closed) => Err(format!(
                "Workflow {} just finished — its steering inbox is closed. Answer the user \
                 from the conversation instead; its results will appear here shortly (or \
                 already have).",
                task_id,
            )),
        }
    }
}

/// Build the tool definition for `steer_workflow`.
pub fn steer_workflow_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "steer_workflow".to_string(),
        description: "Inject a message into a background workflow that is running on this \
                       conversation — use when the user's message is a correction, refinement, \
                       or new information for a running workflow. The workflow sees it at its \
                       next round. Answer unrelated questions directly instead."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task id of the running workflow to steer (from the active workflows context)"
                },
                "message": {
                    "type": "string",
                    "description": "The user's guidance to inject, self-contained and in their intent"
                }
            },
            "required": ["task_id", "message"]
        }),
        strict: None,
        input_examples: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SystemEvent;
    use crate::runner::steering::SteeringInbox;
    use crate::security::policy::{Principal, Scope};

    fn lane_ctx(lane_key: &str) -> ToolContext {
        ToolContext {
            owner_id: Some("user1".to_string()),
            lane_key: Some(lane_key.to_string()),
            source: Some("cli".to_string()),
            request_id: Some(Uuid::new_v4()),
            principal: Some(Principal::User {
                global_id: "user1".to_string(),
            }),
            scope: Some(Scope::Global),
            workspace_path: Some("/ws/project".to_string()),
            ..Default::default()
        }
    }

    fn setup_with_workflow(cap: usize) -> (Arc<SharedContext>, EventBus, Arc<SteeringInbox>) {
        let ctx = Arc::new(SharedContext::new());
        let bus = EventBus::default();
        let inbox = Arc::new(SteeringInbox::new(cap));
        ctx.register_steering_inbox("task-1", inbox.clone());
        ctx.register_workflow_for_lane("user1:cli", "task-1");
        (ctx, bus, inbox)
    }

    #[tokio::test]
    async fn test_steer_ok_queues_message_with_ctx_identity() {
        let (shared, bus, inbox) = setup_with_workflow(16);
        let mut rx = bus.subscribe();
        let tool = SteerWorkflowTool::new(shared, bus.clone());

        let result = tool
            .execute_with_context(
                &serde_json::json!({"task_id": "task-1", "message": "focus on tests"}),
                &lane_ctx("user1:cli"),
            )
            .await
            .expect("push should succeed");
        assert!(result.contains("task-1"), "missing task id: {result}");

        // Message landed with the ctx identity + workspace path.
        let drained = inbox.drain_all();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "focus on tests");
        assert_eq!(
            drained[0].principal,
            Principal::User {
                global_id: "user1".to_string()
            }
        );
        assert_eq!(drained[0].workspace_path.as_deref(), Some("/ws/project"));

        // WorkflowSteered published by the shared push helper.
        let mut found = false;
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::WorkflowSteered {
                task_id, lane_key, ..
            } = event
            {
                assert_eq!(task_id, "task-1");
                assert_eq!(lane_key, "user1:cli");
                found = true;
            }
        }
        assert!(found, "WorkflowSteered was not published");
    }

    #[tokio::test]
    async fn test_steer_full_inbox_suggests_queue_followup() {
        let (shared, bus, inbox) = setup_with_workflow(1);
        let tool = SteerWorkflowTool::new(shared, bus);
        inbox
            .push(SteeringMsg {
                text: "earlier".to_string(),
                request_id: Uuid::new_v4(),
                principal: Principal::System,
                scope: Scope::Global,
                workspace_path: None,
                received_at: Utc::now(),
            })
            .unwrap();

        let err = tool
            .execute_with_context(
                &serde_json::json!({"task_id": "task-1", "message": "more"}),
                &lane_ctx("user1:cli"),
            )
            .await
            .expect_err("full inbox must reject");
        assert!(err.contains("queue_followup"), "missing followup directive: {err}");
    }

    #[tokio::test]
    async fn test_steer_closed_inbox_reports_finished() {
        let (shared, bus, inbox) = setup_with_workflow(16);
        let tool = SteerWorkflowTool::new(shared, bus);
        inbox.close_and_drain();

        let err = tool
            .execute_with_context(
                &serde_json::json!({"task_id": "task-1", "message": "late"}),
                &lane_ctx("user1:cli"),
            )
            .await
            .expect_err("closed inbox must reject");
        assert!(err.contains("finished"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn test_steer_cross_lane_rejected() {
        let (shared, bus, inbox) = setup_with_workflow(16);
        let tool = SteerWorkflowTool::new(shared.clone(), bus);

        // Another lane's workflow — not steerable from user1:cli.
        shared.register_workflow_for_lane("user2:telegram", "task-2");
        shared.register_steering_inbox("task-2", Arc::new(SteeringInbox::default()));

        let err = tool
            .execute_with_context(
                &serde_json::json!({"task_id": "task-2", "message": "hi"}),
                &lane_ctx("user1:cli"),
            )
            .await
            .expect_err("cross-lane steering must be rejected");
        assert!(
            err.contains("not a running workflow on this conversation"),
            "unexpected error: {err}"
        );
        // Own lane's inbox untouched.
        assert!(inbox.is_empty());
    }

    #[tokio::test]
    async fn test_steer_requires_lane_context() {
        let (shared, bus, _inbox) = setup_with_workflow(16);
        let tool = SteerWorkflowTool::new(shared, bus);
        let err = tool
            .execute_with_context(
                &serde_json::json!({"task_id": "task-1", "message": "hi"}),
                &ToolContext::default(),
            )
            .await
            .expect_err("missing lane context must fail");
        assert!(err.contains("lane context"), "unexpected error: {err}");
    }
}
