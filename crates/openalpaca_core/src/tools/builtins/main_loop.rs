//! Main-loop tool registration policy (Routing V2, Phase 2).
//!
//! Builds the per-request tool set for the tool-mode main loop:
//! - **Always-core**: `start_workflow`, `task_status`, `memory_store`,
//!   `memory_forget`, plus the globally-registered `memory_search` (definition
//!   only — its backend already lives in the shared registry).
//! - **Extension tools** (tool/skill wiring, Chunk 2): every MCP-bridged
//!   (`<server>__<tool>`) and plugin-provided (`<plugin>::<tool>`) tool in the
//!   global registry, minus `execution.skill_defaults.global_tool_deny`
//!   (definitions only — backends already live in the shared registry).
//! - **`invoke_skill`** (when an LLM router is available): per-request
//!   catalog-skill invocation via the nested-skill executor.
//! - **Workflow-aware** (only when the lane has active workflows AND steering
//!   is enabled): `steer_workflow`, `queue_followup`.
//!
//! All per-request instances are constructed fresh each turn (`SpawnSubagentTool`
//! precedent) and injected into the caller's per-request registry — never the
//! global one, which would leak them into every other tool-listing surface.

use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::{DaemonConfig, RoutingConfig};
use crate::orchestrator::dispatcher::TaskDispatcher;
use crate::orchestrator::skill_catalog::SkillCatalog;
use crate::runner::lead_agent::{QueueFollowupTool, queue_followup_tool_definition};
use crate::tools::ToolRegistry;
use crate::tools::registry::{BuiltInTool, ToolContext};
use arc_swap::ArcSwap;
use openalpaca_llm::{LlmRouter, ToolDefinition};
use std::sync::Arc;

use super::invoke_skill::{InvokeSkillTool, invoke_skill_tool_definition};
use super::memory_ops::{
    MemoryForgetTool, MemoryStoreTool, memory_forget_tool_definition, memory_store_tool_definition,
};
use super::start_workflow::{StartWorkflowTool, start_workflow_tool_definition};
use super::steer_workflow::{SteerWorkflowTool, steer_workflow_tool_definition};
use super::task_status::{TaskStatusTool, task_status_tool_definition};

/// The per-request main-loop tool set produced by [`main_loop_tool_set`].
pub struct MainLoopToolSet {
    /// Definitions to expose to the model this turn (core set ∪ extension
    /// tools ∪ `invoke_skill`; the caller unions these with its base picks).
    pub definitions: Vec<ToolDefinition>,
    /// Per-request instances to inject into the per-request registry,
    /// paired with their definitions. Excludes `memory_search`, whose
    /// backend is already globally registered.
    pub instances: Vec<(ToolDefinition, Arc<dyn BuiltInTool>)>,
    /// Kept for result-cell readback after the loop (structured delegation).
    pub start_workflow: Arc<StartWorkflowTool>,
    /// Present only when the workflow-aware tools were included; kept for
    /// result-cell readback (the turn's `steered` flag).
    pub steer_workflow: Option<Arc<SteerWorkflowTool>>,
}

impl MainLoopToolSet {
    /// Register the per-request instances into `registry` (a per-request
    /// clone of the global registry — never the global one itself). Follows
    /// the lead runner's `register_coordination_tools` precedent.
    pub fn register_into(&self, registry: &ToolRegistry) {
        for (definition, backend) in &self.instances {
            // Known-good builtin definitions — a name collision with the
            // global registry would only shadow a stale entry, so log and
            // continue rather than fail the request.
            if let Err(e) = registry.register(crate::tools::registry::RegisteredTool {
                definition: definition.clone(),
                backend: crate::tools::registry::ToolBackend::BuiltIn(backend.clone()),
                provides_capabilities: vec![],
                exempt_from_timeout: false,
                annotations: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                author: "builtin".to_string(),
                created_at: chrono::Utc::now(),
            }) {
                tracing::warn!(tool = %definition.name, "Failed to register main-loop tool: {e}");
            }
        }
    }
}

/// Model-relay contract for the tool-mode main loop (Routing V2): concise
/// guidance on relaying `start_workflow` / steering results in the model's
/// own words. Injected alongside the workflow-context block.
pub fn main_loop_relay_guidance() -> &'static str {
    "<workflow_relay_rules>\n\
     - When start_workflow succeeds, tell the user in your own words what was started \
     (mention the title and task id), that it runs in the background, and that they can \
     keep chatting, send corrections to steer it, or queue follow-up work while it runs.\n\
     - If start_workflow reports the workflow limit is reached, do NOT retry it. Explain \
     which workflows are running and offer the alternatives: steer one of them, queue the \
     work as a follow-up, or wait for one to finish.\n\
     - When steer_workflow or queue_followup succeeds, confirm in your own words what was \
     passed along or queued.\n\
     </workflow_relay_rules>"
}

/// Build the main-loop tool set for one request on `lane_key`.
///
/// `steer_workflow` + `queue_followup` are included only when the lane has
/// active workflows AND `routing.steering_enabled` — on lanes with nothing
/// running (the common case) the conditional set stays empty.
///
/// `skill_catalog` + `llm_router` feed the per-request `invoke_skill` tool
/// (skipped when no router is configured — the echo-stub path executes no
/// tools); `max_cost` is the caller's loop budget, threaded as the nested
/// skill invocation's budget ceiling.
#[allow(clippy::too_many_arguments)]
pub fn main_loop_tool_set(
    task_dispatcher: Arc<TaskDispatcher>,
    shared_context: Arc<SharedContext>,
    bus: EventBus,
    routing: &RoutingConfig,
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    skill_catalog: Arc<SkillCatalog>,
    llm_router: Option<Arc<LlmRouter>>,
    max_cost: f64,
    global_registry: &Arc<ToolRegistry>,
    lane_key: &str,
    ctx: &ToolContext,
) -> MainLoopToolSet {
    let mut definitions: Vec<ToolDefinition> = Vec::new();
    let mut instances: Vec<(ToolDefinition, Arc<dyn BuiltInTool>)> = Vec::new();

    // ── Always-core set ─────────────────────────────────────────────────
    let start_workflow = Arc::new(StartWorkflowTool::new(
        task_dispatcher,
        shared_context.clone(),
        bus.clone(),
        routing.clone(),
    ));
    let start_def = start_workflow_tool_definition();
    definitions.push(start_def.clone());
    instances.push((start_def, start_workflow.clone() as Arc<dyn BuiltInTool>));

    let status_def = task_status_tool_definition();
    definitions.push(status_def.clone());
    instances.push((
        status_def,
        Arc::new(TaskStatusTool::new(db.clone(), shared_context.clone()))
            as Arc<dyn BuiltInTool>,
    ));

    // Memory tools need a database; without one they'd only ever error
    // ("Memory system is not available"), so keep them off the surface.
    if db.is_some() {
        let store_def = memory_store_tool_definition();
        definitions.push(store_def.clone());
        instances.push((
            store_def,
            Arc::new(MemoryStoreTool::new(
                db.clone(),
                embedder,
                daemon_config.clone(),
            )) as Arc<dyn BuiltInTool>,
        ));

        let forget_def = memory_forget_tool_definition();
        definitions.push(forget_def.clone());
        instances.push((
            forget_def,
            Arc::new(MemoryForgetTool::new(db.clone())) as Arc<dyn BuiltInTool>,
        ));
    }

    // `memory_search` is globally registered (when a DB is configured) — the
    // per-request registry clone already carries its backend; only the
    // definition joins the core surface.
    if let Some(search) = global_registry.get("memory_search") {
        definitions.push(search.definition.clone());
    }

    // ── Extension tools (MCP-bridged + plugin-provided) ─────────────────
    // Part of the DEFAULT surface: every `<server>__<tool>` / `<plugin>::<tool>`
    // in the global registry joins, minus the global tool deny list (the
    // opt-out). Definitions only — their backends already live in the global
    // registry, so the caller's per-request registry clone carries them.
    let global_tool_deny = daemon_config
        .load()
        .execution
        .skill_defaults
        .global_tool_deny
        .clone();
    definitions.extend(global_registry.extension_tool_defs(&global_tool_deny));

    // ── invoke_skill (per-request; requires an LLM router to run skills) ─
    if let Some(router) = llm_router {
        let invoke_def = invoke_skill_tool_definition();
        definitions.push(invoke_def.clone());
        instances.push((
            invoke_def,
            Arc::new(InvokeSkillTool::new(
                skill_catalog,
                global_registry.clone(),
                router,
                bus.clone(),
                db.clone(),
                daemon_config,
                max_cost,
            )) as Arc<dyn BuiltInTool>,
        ));
    }

    // ── Workflow-aware set (active workflows + steering only) ───────────
    let lane_has_workflows = !shared_context.workflows_for_lane(lane_key).is_empty();
    let steer_workflow = if lane_has_workflows && routing.steering_enabled {
        let steer = Arc::new(SteerWorkflowTool::new(shared_context, bus.clone()));
        let steer_def = steer_workflow_tool_definition();
        definitions.push(steer_def.clone());
        instances.push((steer_def, steer.clone() as Arc<dyn BuiltInTool>));

        // Main-loop queue_followup: identity + workspace path come from the
        // invocation's ToolContext (Phase-1 gap fix); the constructor value
        // is only the fallback.
        let created_by = ctx
            .owner_id
            .clone()
            .unwrap_or_else(|| "system".to_string());
        let followup_def = queue_followup_tool_definition();
        definitions.push(followup_def.clone());
        instances.push((
            followup_def,
            Arc::new(QueueFollowupTool::for_main_loop(
                db,
                bus,
                lane_key.to_string(),
                created_by,
            )) as Arc<dyn BuiltInTool>,
        ));
        Some(steer)
    } else {
        None
    };

    MainLoopToolSet {
        definitions,
        instances,
        start_workflow,
        steer_workflow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{make_agent, template_from_agent};

    fn routing(steering_enabled: bool) -> RoutingConfig {
        RoutingConfig {
            steering_enabled,
            ..RoutingConfig::default()
        }
    }

    /// Minimal dispatcher setup mirroring `start_workflow`'s tests.
    fn setup() -> (Arc<SharedContext>, Arc<TaskDispatcher>, EventBus, Arc<ToolRegistry>) {
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
        let daemon_config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        let dispatcher = Arc::new(TaskDispatcher::new(
            ctx.clone(),
            lane_mgr,
            bus.clone(),
            None,
            gate,
            tool_registry.clone(),
            None,
            None,
            daemon_config,
            Arc::new(std::sync::RwLock::new(None)),
            Arc::new(SkillCatalog::new()),
            Arc::new(crate::prompt_ctx::ContextManager::noop()),
            Arc::new(crate::compose::ComposeEngine::new(16)),
        ));
        (ctx, dispatcher, bus, tool_registry)
    }

    fn names(defs: &[ToolDefinition]) -> Vec<&str> {
        defs.iter().map(|d| d.name.as_str()).collect()
    }

    fn build(
        shared: Arc<SharedContext>,
        dispatcher: Arc<TaskDispatcher>,
        bus: EventBus,
        registry: &Arc<ToolRegistry>,
        routing_cfg: &RoutingConfig,
        db: Option<openalpaca_storage::Database>,
        lane_key: &str,
    ) -> MainLoopToolSet {
        build_with(
            shared,
            dispatcher,
            bus,
            registry,
            routing_cfg,
            db,
            lane_key,
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_with(
        shared: Arc<SharedContext>,
        dispatcher: Arc<TaskDispatcher>,
        bus: EventBus,
        registry: &Arc<ToolRegistry>,
        routing_cfg: &RoutingConfig,
        db: Option<openalpaca_storage::Database>,
        lane_key: &str,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        llm_router: Option<Arc<LlmRouter>>,
    ) -> MainLoopToolSet {
        main_loop_tool_set(
            dispatcher,
            shared,
            bus,
            routing_cfg,
            db,
            None,
            daemon_config,
            Arc::new(SkillCatalog::new()),
            llm_router,
            1.0,
            registry,
            lane_key,
            &ToolContext {
                owner_id: Some("user1".to_string()),
                lane_key: Some(lane_key.to_string()),
                ..Default::default()
            },
        )
    }

    #[test]
    fn test_core_set_without_db_or_workflows() {
        let (shared, dispatcher, bus, registry) = setup();
        let set = build(
            shared, dispatcher, bus, &registry, &routing(true), None, "user1:cli",
        );
        // No db → memory tools (and the global memory_search) absent.
        assert_eq!(names(&set.definitions), vec!["start_workflow", "task_status"]);
        assert_eq!(set.instances.len(), 2);
        assert!(set.steer_workflow.is_none());
        assert!(set.start_workflow.outcome().is_none());
    }

    #[test]
    fn test_core_set_with_db_includes_memory_tools_and_search_def() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let (shared, dispatcher, bus, registry) = setup();
        // memory_search is globally registered in production; mirror that.
        let daemon_config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        for tool in super::super::builtin_tools(
            Some(db.clone()),
            None,
            Some(daemon_config),
            None,
            None,
        ) {
            if tool.definition.name == "memory_search" {
                registry.register(tool).unwrap();
            }
        }

        let set = build(
            shared, dispatcher, bus, &registry, &routing(false), Some(db), "user1:cli",
        );
        assert_eq!(
            names(&set.definitions),
            vec![
                "start_workflow",
                "task_status",
                "memory_store",
                "memory_forget",
                "memory_search",
            ]
        );
        // memory_search has no per-request instance (global backend).
        assert_eq!(set.instances.len(), 4);
        assert!(set.steer_workflow.is_none());
    }

    #[test]
    fn test_workflow_aware_set_requires_workflows_and_steering() {
        let (shared, dispatcher, bus, registry) = setup();
        shared.register_workflow_for_lane("user1:cli", "task-1");

        // Steering disabled → no steer/queue tools even with workflows.
        let set = build(
            shared.clone(),
            dispatcher.clone(),
            bus.clone(),
            &registry,
            &routing(false),
            None,
            "user1:cli",
        );
        assert!(!names(&set.definitions).contains(&"steer_workflow"));
        assert!(!names(&set.definitions).contains(&"queue_followup"));
        assert!(set.steer_workflow.is_none());

        // Steering enabled + active workflow → both appear.
        let set = build(
            shared.clone(),
            dispatcher.clone(),
            bus.clone(),
            &registry,
            &routing(true),
            None,
            "user1:cli",
        );
        assert!(names(&set.definitions).contains(&"steer_workflow"));
        assert!(names(&set.definitions).contains(&"queue_followup"));
        assert!(set.steer_workflow.is_some());
        assert_eq!(set.instances.len(), 4); // start, status, steer, queue

        // Another lane without workflows stays core-only.
        let set = build(
            shared, dispatcher, bus, &registry, &routing(true), None, "user2:telegram",
        );
        assert!(!names(&set.definitions).contains(&"steer_workflow"));
        assert!(set.steer_workflow.is_none());
    }

    // ── Extension tools + invoke_skill (tool/skill wiring, Chunk 2) ─────

    struct FakePluginExec;

    #[async_trait::async_trait]
    impl openalpaca_api::plugin_traits::PluginToolExecutor for FakePluginExec {
        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            Ok("plugin ok".to_string())
        }

        fn plugin_id(&self) -> &str {
            "plug"
        }
    }

    fn register_extension_tool(
        registry: &ToolRegistry,
        name: &str,
        backend: crate::tools::registry::ToolBackend,
    ) {
        registry
            .register(crate::tools::registry::RegisteredTool {
                definition: ToolDefinition {
                    name: name.to_string(),
                    description: format!("{name} tool"),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                    strict: None,
                    input_examples: None,
                },
                backend,
                provides_capabilities: vec![name.to_string()],
                exempt_from_timeout: false,
                annotations: None,
                version: "test-0.0.0".into(),
                author: "test".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
    }

    fn mcp_backend() -> crate::tools::registry::ToolBackend {
        crate::tools::registry::ToolBackend::Mcp {
            client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests("srv")),
            remote_name: "echo".to_string(),
            server_name: "srv".to_string(),
            generation: 0,
        }
    }

    fn plugin_backend() -> crate::tools::registry::ToolBackend {
        crate::tools::registry::ToolBackend::Plugin(Arc::new(FakePluginExec))
    }

    /// Minimal provider so a real `LlmRouter` can be constructed; never called.
    struct StubProvider;

    #[async_trait::async_trait]
    impl openalpaca_llm::LlmProvider for StubProvider {
        fn name(&self) -> &str {
            "stub"
        }

        fn supports_tools(&self) -> bool {
            true
        }

        async fn chat(
            &self,
            _request: openalpaca_llm::ChatRequest,
        ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
            Err(openalpaca_llm::LlmError::Http(
                "stub provider is never called".to_string(),
            ))
        }
    }

    fn stub_router() -> Arc<LlmRouter> {
        Arc::new(LlmRouter::single_provider(
            Arc::new(StubProvider),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-20250514".to_string(),
        ))
    }

    #[test]
    fn test_extension_tools_join_default_surface_minus_deny() {
        let (shared, dispatcher, bus, registry) = setup();
        register_extension_tool(&registry, "srv__echo", mcp_backend());
        register_extension_tool(&registry, "plug::do", plugin_backend());
        register_extension_tool(&registry, "srv__blocked", mcp_backend());

        let mut cfg = DaemonConfig::default();
        cfg.execution.skill_defaults.global_tool_deny = vec!["srv__blocked".to_string()];

        let set = build_with(
            shared,
            dispatcher,
            bus,
            &registry,
            &routing(true),
            None,
            "user1:cli",
            Arc::new(ArcSwap::from_pointee(cfg)),
            None,
        );
        let names = names(&set.definitions);
        assert!(names.contains(&"srv__echo"), "MCP tool missing: {names:?}");
        assert!(names.contains(&"plug::do"), "plugin tool missing: {names:?}");
        assert!(
            !names.contains(&"srv__blocked"),
            "denied tool must be excluded: {names:?}"
        );
        // No router → no invoke_skill; extension tools have no per-request
        // instances (their backends live in the global registry).
        assert!(!names.contains(&"invoke_skill"));
        assert_eq!(set.instances.len(), 2); // start_workflow, task_status
    }

    #[tokio::test]
    async fn test_extension_tools_executable_via_per_request_registry() {
        let (shared, dispatcher, bus, registry) = setup();
        register_extension_tool(&registry, "srv__echo", mcp_backend());
        register_extension_tool(&registry, "plug::do", plugin_backend());

        let set = build(
            shared, dispatcher, bus, &registry, &routing(true), None, "user1:cli",
        );

        // Mirror the handler: clone the global registry per-request and
        // inject the per-request instances.
        let per_request = (*registry).clone();
        set.register_into(&per_request);

        // Plugin tool executes through the clone (backend carried over).
        let out = per_request
            .execute_with_context(
                "plug::do",
                &serde_json::json!({}),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out, "plugin ok");

        // MCP backend is reachable too (a disconnected test client can't
        // round-trip, but resolution proves the sandbox path finds it).
        assert!(per_request.get("srv__echo").is_some());
    }

    #[test]
    fn test_invoke_skill_present_with_router() {
        let (shared, dispatcher, bus, registry) = setup();
        let set = build_with(
            shared,
            dispatcher,
            bus,
            &registry,
            &routing(true),
            None,
            "user1:cli",
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            Some(stub_router()),
        );
        assert!(names(&set.definitions).contains(&"invoke_skill"));
        // invoke_skill is a per-request instance (start, status, invoke_skill).
        assert_eq!(set.instances.len(), 3);

        let per_request = (*registry).clone();
        set.register_into(&per_request);
        assert!(per_request.get("invoke_skill").is_some());
    }
}
