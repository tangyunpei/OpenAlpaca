//! `start_workflow` — the Routing V2 main-loop tool that starts a background
//! lead-agent workflow for the current lane.
//!
//! Constructed PER-REQUEST (same pattern as the lead runner's per-request
//! registry clone): the main loop builds one instance per user turn, injects
//! it into its per-request registry, and reads the `DispatchOutcome` back out
//! of the result cell after the loop finishes to populate structured
//! delegation metadata. NOT registered in the global registry.

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::RoutingConfig;
use crate::events::SystemEvent;
use crate::orchestrator::dispatcher::{generate_title, DispatchOutcome, TaskDispatcher};
use crate::tools::registry::{BuiltInTool, ToolContext};
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::ToolDefinition;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Per-request tool that dispatches a lead-agent workflow in the background.
///
/// Holds a result cell so the caller can read the `DispatchOutcome`
/// programmatically after the loop returns (exact precedent:
/// `SpawnSubagentTool` + `spawn_tool.spawn_count()`).
pub struct StartWorkflowTool {
    task_dispatcher: Arc<TaskDispatcher>,
    shared_context: Arc<SharedContext>,
    bus: EventBus,
    routing: RoutingConfig,
    /// Result cell: the outcome of the (single) successful dispatch this
    /// request made, if any.
    outcome: Arc<Mutex<Option<DispatchOutcome>>>,
}

impl StartWorkflowTool {
    pub fn new(
        task_dispatcher: Arc<TaskDispatcher>,
        shared_context: Arc<SharedContext>,
        bus: EventBus,
        routing: RoutingConfig,
    ) -> Self {
        Self {
            task_dispatcher,
            shared_context,
            bus,
            routing,
            outcome: Arc::new(Mutex::new(None)),
        }
    }

    /// The outcome of the workflow this request started, if any.
    /// Read by the main loop after the agentic loop returns to populate
    /// structured delegation metadata.
    pub fn outcome(&self) -> Option<DispatchOutcome> {
        self.outcome
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

#[async_trait]
impl BuiltInTool for StartWorkflowTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("start_workflow requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let goal = arguments
            .get("goal")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|g| !g.is_empty())
            .ok_or_else(|| "Missing required parameter: goal".to_string())?;

        let title = arguments
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| generate_title(goal));

        let lane_key = ctx
            .lane_key
            .as_deref()
            .ok_or_else(|| "start_workflow requires a lane context".to_string())?;

        // 1. Enforce the per-lane concurrent-workflow cap (tool mode ONLY —
        // never inside dispatch_lead_agent, which legacy paths also call).
        let active = self.shared_context.workflows_for_lane(lane_key);
        if active.len() >= self.routing.max_workflows_per_lane {
            return Err(format!(
                "Workflow limit reached: {} of {} workflows are already running on this \
                 conversation (task ids: {}). Do NOT retry start_workflow. Instead, use \
                 steer_workflow to redirect one of the running workflows, or queue_followup \
                 to queue this work for after one finishes.",
                active.len(),
                self.routing.max_workflows_per_lane,
                active.join(", "),
            ));
        }

        // 2. Dispatch the lead-agent workflow (detached background execution).
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
        let source = ctx.source.as_deref().unwrap_or("internal");
        let outcome = self.task_dispatcher.dispatch_lead_agent(
            goal,
            title,
            &created_by,
            lane_key,
            source,
            ctx.workspace_id.clone(),
        )?;

        // 3. Store the outcome in the result cell for the caller.
        *self
            .outcome
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(outcome.clone());

        // 4. Record the routing decision (Routing V2): the model's
        // tool call IS the dispatch decision, recorded unconditionally.
        let request_id = ctx.request_id.unwrap_or_else(Uuid::nil);
        self.task_dispatcher
            .record_tool_dispatch_decision(&request_id.to_string(), &outcome.task_id);

        // 5. Publish WorkflowStarted (Routing V2).
        self.bus.publish(SystemEvent::WorkflowStarted {
            request_id,
            task_id: outcome.task_id.clone(),
            lane_key: lane_key.to_string(),
            title: outcome.title.clone(),
            timestamp: Utc::now(),
        });

        // 6. Short result the model relays in its own words.
        Ok(format!(
            "Workflow started in the background (task id: {}, title: \"{}\"). \
             It will post its results to this conversation when it completes. \
             Let the user know in your own words.",
            outcome.task_id, outcome.title,
        ))
    }
}

/// Build the tool definition for `start_workflow`.
pub fn start_workflow_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "start_workflow".to_string(),
        description: "Start a background workflow for a substantial, multi-step task. A lead \
                       agent will plan the work, delegate to subagents, and post a completion \
                       report to this conversation when done. Use for real tasks (research, \
                       builds, multi-file changes) — answer simple questions directly instead."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "A clear, self-contained description of what the workflow should accomplish"
                },
                "title": {
                    "type": "string",
                    "description": "Optional short title for the task (a concise one is generated from the goal if omitted)"
                }
            },
            "required": ["goal"]
        }),
        strict: None,
        input_examples: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{make_agent, template_from_agent};

    fn routing_with_cap(cap: usize) -> RoutingConfig {
        RoutingConfig {
            max_workflows_per_lane: cap,
            ..RoutingConfig::default()
        }
    }

    /// Minimal dispatcher setup mirroring `orchestrator/dispatcher/tests.rs`.
    /// No router: `dispatch_lead_agent` still returns Ok(DispatchOutcome) —
    /// the spawned execution just aborts at `require_router`.
    fn setup() -> (Arc<SharedContext>, Arc<TaskDispatcher>, EventBus) {
        setup_with(None, crate::daemon_config::DaemonConfig::default())
    }

    fn setup_with(
        db: Option<openalpaca_storage::Database>,
        config: crate::daemon_config::DaemonConfig,
    ) -> (Arc<SharedContext>, Arc<TaskDispatcher>, EventBus) {
        let ctx = Arc::new(SharedContext::new());
        let lead = make_agent("lead", vec!["orchestration"]);
        ctx.agent_registry.register_template(template_from_agent(&lead));
        ctx.agent_registry.register(lead);

        let lane_mgr = Arc::new(crate::lane::LaneManager::new());
        let bus = EventBus::default();
        let tool_registry = Arc::new(crate::tools::ToolRegistry::default());
        let sandbox = Arc::new(crate::security::sandbox::SandboxManager::with_defaults(
            tool_registry.clone(),
            bus.clone(),
        ));
        let gate = Arc::new(crate::security::gate::SecurityGate::new(sandbox));
        let daemon_config = Arc::new(arc_swap::ArcSwap::from_pointee(config));
        let dispatcher = Arc::new(TaskDispatcher::new(
            ctx.clone(),
            lane_mgr,
            bus.clone(),
            None,
            gate,
            tool_registry,
            db,
            None,
            daemon_config,
            Arc::new(std::sync::RwLock::new(None)),
            Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
            Arc::new(crate::prompt_ctx::ContextManager::noop()),
            Arc::new(crate::compose::ComposeEngine::new(16)),
        ));
        (ctx, dispatcher, bus)
    }

    fn lane_ctx(lane_key: &str, request_id: Uuid) -> ToolContext {
        ToolContext {
            owner_id: Some("user1".to_string()),
            lane_key: Some(lane_key.to_string()),
            source: Some("cli".to_string()),
            request_id: Some(request_id),
            principal: Some(crate::security::policy::Principal::User {
                global_id: "user1".to_string(),
            }),
            scope: Some(crate::security::policy::Scope::Global),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_start_workflow_dispatches_and_publishes() {
        let (shared, dispatcher, bus) = setup();
        let mut rx = bus.subscribe();
        let request_id = Uuid::new_v4();
        let tool = StartWorkflowTool::new(
            dispatcher,
            shared.clone(),
            bus.clone(),
            routing_with_cap(3),
        );
        assert!(tool.outcome().is_none());

        let result = tool
            .execute_with_context(
                &serde_json::json!({"goal": "Research the Rust borrow checker", "title": "Borrow checker research"}),
                &lane_ctx("user1:cli", request_id),
            )
            .await
            .expect("dispatch should succeed");

        // Result cell populated with the dispatched task's identity.
        let outcome = tool.outcome().expect("result cell should hold the outcome");
        assert_eq!(outcome.title, "Borrow checker research");
        assert!(!outcome.task_id.is_empty());

        // Tool result names the task id + title so the model can relay it.
        assert!(result.contains(&outcome.task_id));
        assert!(result.contains("Borrow checker research"));

        // The task actually registered.
        assert_eq!(shared.task_registry.count(), 1);

        // WorkflowStarted published with the request's identity.
        let mut found = false;
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::WorkflowStarted {
                request_id: rid,
                task_id,
                lane_key,
                title,
                ..
            } = event
            {
                assert_eq!(rid, request_id);
                assert_eq!(task_id, outcome.task_id);
                assert_eq!(lane_key, "user1:cli");
                assert_eq!(title, "Borrow checker research");
                found = true;
            }
        }
        assert!(found, "WorkflowStarted was not published");
    }

    #[tokio::test]
    async fn test_start_workflow_generates_title_when_omitted() {
        let (shared, dispatcher, bus) = setup();
        let tool =
            StartWorkflowTool::new(dispatcher, shared, bus, routing_with_cap(3));

        tool.execute_with_context(
            &serde_json::json!({"goal": "please research the Rust borrow checker"}),
            &lane_ctx("user1:cli", Uuid::new_v4()),
        )
        .await
        .expect("dispatch should succeed");

        let outcome = tool.outcome().expect("result cell should hold the outcome");
        assert_eq!(
            outcome.title,
            generate_title("please research the Rust borrow checker")
        );
    }

    #[tokio::test]
    async fn test_start_workflow_cap_returns_directive_error() {
        let (shared, dispatcher, bus) = setup();
        let mut rx = bus.subscribe();
        let tool = StartWorkflowTool::new(
            dispatcher,
            shared.clone(),
            bus.clone(),
            routing_with_cap(1),
        );

        // Lane already at the cap.
        shared.register_workflow_for_lane("user1:cli", "existing-task");

        let err = tool
            .execute_with_context(
                &serde_json::json!({"goal": "Another big task"}),
                &lane_ctx("user1:cli", Uuid::new_v4()),
            )
            .await
            .expect_err("cap must reject the dispatch");

        // Directive error steers the model to the alternatives.
        assert!(err.contains("steer_workflow"), "missing steer directive: {err}");
        assert!(err.contains("queue_followup"), "missing followup directive: {err}");
        assert!(err.contains("existing-task"), "missing active task id: {err}");

        // Nothing dispatched, nothing published.
        assert!(tool.outcome().is_none());
        assert_eq!(shared.task_registry.count(), 0);
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(event, SystemEvent::WorkflowStarted { .. }),
                "WorkflowStarted must not be published at the cap"
            );
        }
    }

    #[tokio::test]
    async fn test_start_workflow_records_decision_unconditionally() {
        // Routing V2: the tool path records its DispatchDecision
        // UNCONDITIONALLY — the model's `start_workflow` call IS the
        // routing decision (the planner-era analysis gate is gone).
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
        let config = crate::daemon_config::DaemonConfig::default();
        let (shared, dispatcher, bus) = setup_with(Some(db.clone()), config);
        let mut rx = bus.subscribe();
        let request_id = Uuid::new_v4();
        let tool = StartWorkflowTool::new(
            dispatcher,
            shared,
            bus.clone(),
            routing_with_cap(3),
        );

        tool.execute_with_context(
            &serde_json::json!({"goal": "Research the Rust borrow checker"}),
            &lane_ctx("user1:cli", request_id),
        )
        .await
        .expect("dispatch should succeed");
        let task_id = tool.outcome().unwrap().task_id;

        // Event published with the REAL task id and the new reason.
        let mut saw_decision = false;
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::DispatchDecision {
                request_id: rid,
                task_id: tid,
                mode,
                reason,
                ..
            } = event
            {
                assert_eq!(rid, request_id.to_string());
                assert_eq!(tid.as_deref(), Some(task_id.as_str()));
                assert_eq!(mode, "lead_agent");
                assert_eq!(reason, "model_tool_call");
                saw_decision = true;
            }
        }
        assert!(saw_decision, "DispatchDecision was not published");

        // Row persisted with the real task id (never ack prose).
        let rows = openalpaca_storage::repository::dispatch_decision::DispatchDecisionRepository::new(&db)
            .query(None, None, None, 10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].request_id, request_id.to_string());
        assert_eq!(rows[0].task_id.as_deref(), Some(task_id.as_str()));
        assert_eq!(rows[0].mode, "lead_agent");
        assert_eq!(rows[0].reason, "model_tool_call");
    }

    #[tokio::test]
    async fn test_start_workflow_requires_lane_context() {
        let (shared, dispatcher, bus) = setup();
        let tool =
            StartWorkflowTool::new(dispatcher, shared.clone(), bus, routing_with_cap(3));

        let err = tool
            .execute_with_context(
                &serde_json::json!({"goal": "A task"}),
                &ToolContext::default(),
            )
            .await
            .expect_err("missing lane context must fail");
        assert!(err.contains("lane context"), "unexpected error: {err}");
        assert!(tool.outcome().is_none());
        assert_eq!(shared.task_registry.count(), 0);
    }
}
