//! Main-loop tool registration policy (Routing V2, Phase 2).
//!
//! Builds the per-request tool set for the tool-mode main loop:
//! - **Always-core**: `start_workflow`, `task_status`, and — when the daemon
//!   has a `MemoryService` — the three memory writers `memory_store`,
//!   `memory_update`, `memory_forget` (design §7: only the main conversation
//!   writes), plus the globally-registered read surface `memory_search`,
//!   `memory_read`, `memory_list`, `memory_timeline` (definitions only —
//!   their backends already live in the shared registry). On an attended turn of the GUI's or the
//!   CLI's chat, `memory_confirm` too (design 09 §4.1): the assistant raises a
//!   suggestion, and the person's card answers it.
//! - **Readers** (R42): `artifact_read` on every turn with a database
//!   (definition only); `file_read` and `file_list` are offered by the query
//!   handler on a project turn, and `file_list` wherever `file_read` is
//!   (R-T6, R-T14).
//! - **Extension tools** (tool/skill wiring, Chunk 2): every MCP-bridged
//!   (`<server>__<tool>`) and plugin-provided (`<plugin>::<tool>`) tool in the
//!   global registry whose extension is enabled (definitions only — backends
//!   already live in the shared registry).
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
use super::memory_tools::{
    IN_CHAT_SOURCE, MemoryConfirmTool, MemoryForgetTool, MemoryStoreTool, MemoryUpdateTool,
    memory_confirm_tool_definition, memory_forget_tool_definition, memory_store_tool_definition,
    memory_update_tool_definition,
};
use super::start_workflow::{StartWorkflowTool, start_workflow_tool_definition};
use super::steer_workflow::{SteerWorkflowTool, steer_workflow_tool_definition};
use super::task_status::{TaskStatusTool, task_status_tool_definition};
use crate::memory::service::MemoryService;

/// The per-request main-loop tool set produced by [`main_loop_tool_set`].
pub struct MainLoopToolSet {
    /// Definitions to expose to the model this turn (core set ∪ extension
    /// tools ∪ `invoke_skill`; the caller unions these with its base picks).
    pub definitions: Vec<ToolDefinition>,
    /// Per-request instances to inject into the per-request registry,
    /// paired with their definitions. Excludes the memory read tools, whose
    /// backends are already globally registered.
    pub instances: Vec<(ToolDefinition, Arc<dyn BuiltInTool>)>,
    /// Kept for result-cell readback after the loop (structured delegation).
    pub start_workflow: Arc<StartWorkflowTool>,
    /// Present only when the workflow-aware tools were included; kept for
    /// result-cell readback (the turn's `steered` flag).
    pub steer_workflow: Option<Arc<SteerWorkflowTool>>,
    /// `memory_confirm` is on this turn's surface (design 09 §4.1), so the
    /// turn may be told what is waiting in `<memory_suggestions>` (§4.5).
    pub memory_confirm_offered: bool,
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
            // With the annotations the name carries: the sandbox derives the
            // confirmation set from what is registered, so this is what makes
            // `memory_forget` and `memory_update` prompt (design §7).
            if let Err(e) = registry.register(crate::tools::registry::RegisteredTool {
                definition: definition.clone(),
                backend: crate::tools::registry::ToolBackend::BuiltIn(backend.clone()),
                provides_capabilities: vec![],
                exempt_from_timeout: false,
                annotations: super::annotations_for_builtin(&definition.name),
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
     - A workflow starts ONLY through a start_workflow call in THIS turn. Never say one was \
     started, and never state a task id, unless a start_workflow call in this turn returned \
     it — task ids in earlier messages belong to earlier runs.\n\
     - Writing a file, editing one or running a command in the project you do yourself \
     when those tools are on your surface; start_workflow is for a substantial multi-step \
     task, an explicit request for a workflow or a background run. When no tool that \
     writes a file is on your surface, a file the user asks for is `start_workflow`'s. \
     Small questions are still answered directly.\n\
     </workflow_relay_rules>"
}

/// The main loop's memory guidance (design 09 §13.1, MD-14): the layered
/// workflow its three read tools teach, said once in the system prompt — one
/// line, given only to a turn whose surface carries `memory_search` — and,
/// since owner decision LQ19 (F-C), one line on naming the note a new fact
/// changes, without which a saved note gets no suggestion (the commute,
/// report 10 §4.5) — and, since design 11 §4 (owner decision CQ-5), one line
/// on the note a `previous_conversation` block names: the rule sits here, the
/// block's content stays quoted data.
pub fn main_loop_memory_guidance() -> &'static str {
    "<memory_rules>\n\
     Memory: memory_search returns an index, one line per note and no body — pick the ids that \
     matter and read only those with memory_read, several in one call; memory_timeline shows \
     what surrounds a note in time.\n\
     Before saving a fact about the user that changes an earlier one, memory_search for the \
     note it changes and pass its id in supersedes (or its fact_key).\n\
     A previous_conversation block names the memory note that records it; memory_read it when \
     the user continues that work.\n\
     </memory_rules>"
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
    memory_service: Option<Arc<MemoryService>>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    skill_catalog: Arc<SkillCatalog>,
    llm_router: Option<Arc<LlmRouter>>,
    max_cost: f64,
    global_registry: &Arc<ToolRegistry>,
    lane_key: &str,
    ctx: &ToolContext,
    // M6 — the client behind this turn cannot answer a confirmation prompt,
    // so a workflow it starts must refuse a tool that needs one instead of
    // blocking on a prompt nobody will see.
    unattended: bool,
) -> MainLoopToolSet {
    let mut definitions: Vec<ToolDefinition> = Vec::new();
    let mut instances: Vec<(ToolDefinition, Arc<dyn BuiltInTool>)> = Vec::new();
    let mut memory_confirm_offered = false;

    // ── Always-core set ─────────────────────────────────────────────────
    let start_workflow = Arc::new(StartWorkflowTool::new(
        task_dispatcher,
        shared_context.clone(),
        bus.clone(),
        routing.clone(),
        unattended,
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

    // The memory writers need the service; without one (tests, a daemon
    // with no store) they stay off the surface. `ModelTool` calls all
    // (design §7a): a store records `extracted_user`.
    if let Some(service) = memory_service {
        let store_def = memory_store_tool_definition();
        definitions.push(store_def.clone());
        instances.push((
            store_def,
            Arc::new(MemoryStoreTool::new(service.clone())) as Arc<dyn BuiltInTool>,
        ));

        let update_def = memory_update_tool_definition();
        definitions.push(update_def.clone());
        instances.push((
            update_def,
            Arc::new(MemoryUpdateTool::new(service.clone())) as Arc<dyn BuiltInTool>,
        ));

        let forget_def = memory_forget_tool_definition();
        definitions.push(forget_def.clone());
        instances.push((
            forget_def,
            Arc::new(MemoryForgetTool::new(service.clone())) as Arc<dyn BuiltInTool>,
        ));

        // Design 09 §4.1 (ruling LR13): `memory_confirm` only where a person
        // can answer its card on a user-directed channel — an attended turn
        // of the GUI's or the CLI's chat. Never on a connector lane (its
        // `/yes` is keyed by chat, not sender), an unattended turn, the
        // skill tier, the lead or a subagent, none of which build this set.
        if !unattended && ctx.source.as_deref() == Some(IN_CHAT_SOURCE) {
            let confirm_def = memory_confirm_tool_definition();
            definitions.push(confirm_def.clone());
            instances.push((
                confirm_def,
                Arc::new(MemoryConfirmTool::new(service)) as Arc<dyn BuiltInTool>,
            ));
            memory_confirm_offered = true;
        }
    }

    // The memory read surface is globally registered (`memory_read_tools`,
    // when the daemon has a service) — the per-request registry clone
    // already carries the backends; only the definitions join the core
    // surface.
    for name in [
        "memory_search",
        "memory_read",
        "memory_list",
        "memory_timeline",
    ] {
        if let Some(tool) = global_registry.get(name) {
            definitions.push(tool.definition.clone());
        }
    }

    // ── read_result (only where a spill can happen) ─────────────────────
    // R84: the spill stub tells the model to page with `read_result`, so every
    // surface that can produce a stub has to offer the tool — without it a large
    // result shrank from base's 32 KiB head to a 2 KiB preview and a reference
    // nothing could follow. Definition only: the backend is globally registered
    // (`builtins::register_builtin_tools`), so the per-request clone carries it,
    // and the main loop derives its allowlist from this same surface, which is
    // what admits the call. With no session log nothing spills and the tool
    // could only answer "this call has none" — the same reason the memory tools
    // stay off without a database.
    if shared_context.session_log().is_some()
        && db.is_some()
        && let Some(read_result) = global_registry.get("read_result")
    {
        definitions.push(read_result.definition.clone());
    }

    // ── artifact_read (every turn with a database, R42) ─────────────────
    // A by-product's id is named by artifact_write's result, by file_write's
    // when there is no project, by a workspace entry and by task_status, so
    // any turn may be handed one to read back. Definition only, like
    // read_result's: the backend is globally registered, the per-request
    // clone carries it, and this surface is the allowlist that admits it.
    // The file readers are the query handler's offer (`offer_file_readers`).
    if db.is_some()
        && let Some(artifact_read) = global_registry.get("artifact_read")
    {
        definitions.push(artifact_read.definition.clone());
    }

    // ── Extension tools (MCP-bridged + plugin-provided) ─────────────────
    // Part of the DEFAULT surface: every `<server>__<tool>` / `<plugin>::<tool>`
    // in the global registry joins, less those whose extension is not enabled
    // (hygiene — the gate is what refuses). There is no per-tool opt-out; the
    // ENABLE axis is one toggle per server / per plugin (design §1 S1, §11).
    // Definitions only — their backends already live in the global registry,
    // so the caller's per-request registry clone carries them.
    definitions.extend(global_registry.extension_tool_defs());

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
        let steer = Arc::new(SteerWorkflowTool::new(
            shared_context,
            bus.clone(),
            db.clone(),
        ));
        let steer_def = steer_workflow_tool_definition();
        definitions.push(steer_def.clone());
        instances.push((steer_def, steer.clone() as Arc<dyn BuiltInTool>));

        // Main-loop queue_followup: identity and the turn's resolved project
        // root (`request_workspace_root`, F4) come from the invocation's
        // ToolContext; the constructor value is only the identity fallback.
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
                // S5: the promise is kept by a later turn the runner starts,
                // by which time this client may be long gone.
                unattended,
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
        memory_confirm_offered,
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
            daemon_config,
            Arc::new(std::sync::RwLock::new(None)),
            Arc::new(SkillCatalog::new()),
            Arc::new(crate::prompt_ctx::ContextManager::noop()),
            Arc::new(crate::compose::ComposeEngine::new(16)),
            Arc::new(std::sync::RwLock::new(None)),
            Arc::new(std::sync::RwLock::new(None)),
            Arc::new(crate::orchestrator::BackgroundJobs::new()),
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
        memory_service: Option<Arc<crate::memory::service::MemoryService>>,
    ) -> MainLoopToolSet {
        main_loop_tool_set(
            dispatcher,
            shared,
            bus,
            routing_cfg,
            db,
            memory_service,
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
            false,
        )
    }

    /// H2, and §15: the relay rules say what the model may claim and when to
    /// delegate — and no longer that only a workflow can write a file: on a
    /// project turn the main chat writes, edits and runs commands itself when
    /// those tools are on its surface, and a file asked for with no writing
    /// tool on the surface is still start_workflow's (the lead's ruling
    /// R-T7). Snapshot-style: the block is a contract with the model, so the
    /// clauses are pinned.
    #[test]
    fn the_relay_rules_forbid_an_unearned_claim_and_name_the_delegate_cases() {
        let rules = main_loop_relay_guidance();
        for clause in [
            "starts ONLY through a start_workflow call in THIS turn",
            "never state a task id",
            "task ids in earlier messages belong to earlier runs",
            "Writing a file, editing one or running a command in the project you do yourself \
             when those tools are on your surface",
            "start_workflow is for a substantial multi-step task, an explicit request for a \
             workflow or a background run",
            // The lead's ruling R-T7: where chat cannot write, round 13's
            // routing stays.
            "When no tool that writes a file is on your surface, a file the user asks for is \
             `start_workflow`'s.",
            "Small questions are still answered directly",
        ] {
            assert!(
                rules.contains(clause),
                "the relay rules must carry {clause:?}:\n{rules}"
            );
        }
        for gone in [
            "no file- or artifact-writing tool of your own",
            "write or save an artifact or a file, call start_workflow",
        ] {
            assert!(!rules.contains(gone), "{gone:?} is no longer true:\n{rules}");
        }
        // One block, still closed — the guidance is injected as a single
        // user message and a stray tag would reach the model as prose.
        assert!(rules.starts_with("<workflow_relay_rules>"));
        assert!(rules.ends_with("</workflow_relay_rules>"));
    }

    /// H2's other half: `start_workflow`'s own description agrees with the
    /// rules — an explicit request and a substantial task, and no longer
    /// "only a workflow can write a file" (§15).
    #[test]
    fn start_workflow_describes_the_explicit_request_and_claims_no_file_monopoly() {
        let description = start_workflow_tool_definition().description;
        for clause in [
            "substantial, multi-step task",
            "explicitly asks for a workflow or a background run",
            "ONLY way to start a run",
        ] {
            assert!(
                description.contains(clause),
                "start_workflow's description must carry {clause:?}:\n{description}"
            );
        }
        for gone in ["which only a workflow can do", "write or save an artifact or a file"] {
            assert!(
                !description.contains(gone),
                "{gone:?} is no longer true:\n{description}"
            );
        }
    }

    #[test]
    fn test_core_set_without_db_or_workflows() {
        let (shared, dispatcher, bus, registry) = setup();
        let set = build(
            shared, dispatcher, bus, &registry, &routing(true), None, "user1:cli",
        );
        // No service → the memory writers (and the global read tools) absent.
        assert_eq!(names(&set.definitions), vec!["start_workflow", "task_status"]);
        assert_eq!(set.instances.len(), 2);
        assert!(set.steer_workflow.is_none());
        assert!(set.start_workflow.outcome().is_none());
    }

    #[test]
    fn test_core_set_with_service_includes_memory_writers_and_read_defs() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let service = Arc::new(crate::memory::service::MemoryService::new(
            Some(db.clone()),
            openalpaca_storage::memory_index::MemoryIndex::open_in_memory().unwrap(),
        ));
        let (shared, dispatcher, bus, registry) = setup();
        // The read surface is globally registered in production; mirror that.
        for tool in crate::tools::builtins::memory_read_tools(service.clone()) {
            registry.register(tool).unwrap();
        }

        let set = build_with(
            shared,
            dispatcher,
            bus,
            &registry,
            &routing(false),
            Some(db),
            "user1:cli",
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            None,
            Some(service),
        );
        assert_eq!(
            names(&set.definitions),
            vec![
                "start_workflow",
                "task_status",
                "memory_store",
                "memory_update",
                "memory_forget",
                "memory_search",
                "memory_read",
                "memory_list",
                "memory_timeline",
            ]
        );
        // The read tools have no per-request instance (global backends).
        assert_eq!(set.instances.len(), 5);
        assert!(set.steer_workflow.is_none());
    }

    /// R84: the spill stub names `read_result`, so the surface that can produce
    /// one offers the tool — and the main loop's allowlist, derived from this
    /// same surface, is what admits the call. Without a session log nothing
    /// spills, so it stays off, like the memory tools without a database.
    #[test]
    fn read_result_joins_the_surface_when_a_session_log_is_present() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let sessions = tempfile::tempdir().unwrap();

        // No session log service on the context yet.
        let (shared, dispatcher, bus, registry) = setup();
        for tool in super::super::builtin_tools(Some(db.clone()), None, None) {
            if tool.definition.name == "read_result" {
                registry.register(tool).unwrap();
            }
        }
        assert!(
            registry.get("read_result").is_some(),
            "production registers it globally; the fixture mirrors that"
        );
        let set = build(
            shared,
            dispatcher,
            bus,
            &registry,
            &routing(false),
            Some(db.clone()),
            "user1:cli",
        );
        assert!(
            !names(&set.definitions).contains(&"read_result"),
            "nothing can spill without a session log: {:?}",
            names(&set.definitions)
        );

        // With one, as every daemon that resolved a store has.
        let (shared, dispatcher, bus, registry) = setup();
        for tool in super::super::builtin_tools(Some(db.clone()), None, None) {
            if tool.definition.name == "read_result" {
                registry.register(tool).unwrap();
            }
        }
        shared.set_session_log(
            crate::session_log::SessionLogService::new(
                sessions.path().to_path_buf(),
                Some(db.clone()),
                crate::session_log::SessionLogLimits::default(),
                "test".to_string(),
            )
            .into_arc(),
        );
        let set = build(
            shared,
            dispatcher,
            bus,
            &registry,
            &routing(false),
            Some(db),
            "user1:cli",
        );
        assert!(
            names(&set.definitions).contains(&"read_result"),
            "the surface that emits the stub must offer the tool: {:?}",
            names(&set.definitions)
        );
        // Definition only — the backend is already in the global registry.
        assert!(
            !set.instances
                .iter()
                .any(|(def, _)| def.name == "read_result"),
            "no per-request instance is built for a globally registered backend"
        );
    }

    /// R42: `artifact_read` is on every main-loop turn with a database — any
    /// turn may be handed a by-product's id. Definition only: the backend is
    /// the global registry's, which the per-request clone carries. The file
    /// readers are not this set's: the query handler offers them, from the
    /// turn's one project-turn predicate (the lead's ruling R-T14).
    #[test]
    fn artifact_read_joins_every_turn_with_a_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let surface = |db: Option<openalpaca_storage::Database>, root: Option<&str>| {
            let (shared, dispatcher, bus, registry) = setup();
            for tool in super::super::builtin_tools(db.clone(), None, None) {
                registry.register(tool).unwrap();
            }
            let set = main_loop_tool_set(
                dispatcher,
                shared,
                bus,
                &RoutingConfig::default(),
                db,
                None,
                Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
                Arc::new(SkillCatalog::new()),
                None,
                1.0,
                &registry,
                "user1:gui",
                &ToolContext {
                    owner_id: Some("user1".to_string()),
                    lane_key: Some("user1:gui".to_string()),
                    request_workspace_root: root.map(str::to_string),
                    ..Default::default()
                },
                false,
            );
            for (def, _) in &set.instances {
                assert_ne!(def.name, "artifact_read", "artifact_read is offered by definition only");
            }
            names(&set.definitions)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let has = |names: &[String], name: &str| names.iter().any(|n| n == name);

        let chat = surface(Some(db.clone()), None);
        assert!(has(&chat, "artifact_read"), "{chat:?}");

        let project = surface(Some(db.clone()), Some("/the/project"));
        assert!(has(&project, "artifact_read"), "{project:?}");
        for name in ["file_read", "file_list"] {
            assert!(!has(&project, name), "{name} is the query handler's offer: {project:?}");
        }

        let no_db = surface(None, Some("/the/project"));
        assert!(!has(&no_db, "artifact_read"), "{no_db:?}");
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
    fn test_extension_tools_join_default_surface() {
        let (shared, dispatcher, bus, registry) = setup();
        register_extension_tool(&registry, "srv__echo", mcp_backend());
        register_extension_tool(&registry, "plug::do", plugin_backend());

        let set = build_with(
            shared,
            dispatcher,
            bus,
            &registry,
            &routing(true),
            None,
            "user1:cli",
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            None,
            None,
        );
        let names = names(&set.definitions);
        assert!(names.contains(&"srv__echo"), "MCP tool missing: {names:?}");
        assert!(names.contains(&"plug::do"), "plugin tool missing: {names:?}");
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

    /// R29's tripwire (storage design §11), the main loop's per-request half
    /// (S1's M2): every handler the fullest main-loop surface injects per
    /// request — the memory writers and `memory_confirm` (an attended GUI
    /// turn), `invoke_skill` (a router), `steer_workflow` and
    /// `queue_followup` (an active workflow, steering on) — and the memory
    /// read tools the daemon registers globally beside the builtins say
    /// their arguments reach no program, so the sanitizer's `../` rule stays
    /// off them as it is off every first-party builtin. The name list says
    /// what the walk saw: a new per-request tool lands in it.
    #[test]
    fn every_per_request_main_loop_tool_says_its_arguments_reach_no_program() {
        use crate::tools::registry::ToolBackend;

        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let service = Arc::new(crate::memory::service::MemoryService::new(
            Some(db.clone()),
            openalpaca_storage::memory_index::MemoryIndex::open_in_memory().unwrap(),
        ));
        let (shared, dispatcher, bus, registry) = setup();
        let lane = format!("user1:{IN_CHAT_SOURCE}");
        shared.register_workflow_for_lane(&lane, "task-1");

        let set = main_loop_tool_set(
            dispatcher,
            shared,
            bus,
            &routing(true),
            Some(db),
            Some(service.clone()),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            Arc::new(SkillCatalog::new()),
            Some(stub_router()),
            1.0,
            &registry,
            &lane,
            &ToolContext {
                owner_id: Some("user1".to_string()),
                lane_key: Some(lane.clone()),
                source: Some(IN_CHAT_SOURCE.to_string()),
                ..Default::default()
            },
            false,
        );

        let mut seen: Vec<&str> = set
            .instances
            .iter()
            .map(|(definition, _)| definition.name.as_str())
            .collect();
        seen.sort_unstable();
        assert_eq!(
            seen,
            vec![
                "invoke_skill",
                "memory_confirm",
                "memory_forget",
                "memory_store",
                "memory_update",
                "queue_followup",
                "start_workflow",
                "steer_workflow",
                "task_status",
            ]
        );
        for (definition, handler) in &set.instances {
            assert!(
                !handler.arguments_reach_a_program(),
                "{} keeps the ../ rule",
                definition.name
            );
        }
        for tool in crate::tools::builtins::memory_read_tools(service) {
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
            None,
        );
        assert!(names(&set.definitions).contains(&"invoke_skill"));
        // invoke_skill is a per-request instance (start, status, invoke_skill).
        assert_eq!(set.instances.len(), 3);

        let per_request = (*registry).clone();
        set.register_into(&per_request);
        assert!(per_request.get("invoke_skill").is_some());
    }

    /// Design 09 §4.1 (ruling LR13): `memory_confirm` is on the surface of an
    /// attended turn of the GUI's or the CLI's chat (`source == "gui"`) and on
    /// no other — not a connector lane, whose `/yes` is keyed by chat rather
    /// than sender (G2), and not an unattended turn, where nobody can answer.
    /// Where it is offered it prompts, like the other destructive writers.
    #[test]
    fn memory_confirm_is_absent_on_connector_and_unattended_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let service = Arc::new(crate::memory::service::MemoryService::new(
            Some(db.clone()),
            openalpaca_storage::memory_index::MemoryIndex::open_in_memory().unwrap(),
        ));
        let surface = |source: &str, unattended: bool| {
            let (shared, dispatcher, bus, registry) = setup();
            let set = main_loop_tool_set(
                dispatcher,
                shared,
                bus,
                &RoutingConfig::default(),
                Some(db.clone()),
                Some(service.clone()),
                Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
                Arc::new(SkillCatalog::new()),
                None,
                1.0,
                &registry,
                &format!("user1:{source}"),
                &ToolContext {
                    owner_id: Some("user1".to_string()),
                    lane_key: Some(format!("user1:{source}")),
                    source: Some(source.to_string()),
                    ..Default::default()
                },
                unattended,
            );
            let offered = names(&set.definitions).contains(&"memory_confirm");
            let instance = set
                .instances
                .iter()
                .any(|(def, _)| def.name == "memory_confirm");
            assert_eq!(offered, instance, "{source}/{unattended}");
            assert_eq!(offered, set.memory_confirm_offered, "{source}/{unattended}");
            (offered, set, registry)
        };
        let (offered, set, registry) = surface("gui", false);
        assert!(offered, "an attended GUI/CLI chat turn offers it");
        let per_request = (*registry).clone();
        set.register_into(&per_request);
        let policy = SandboxPolicy {
            agent_id: crate::orchestrator::MAIN_LOOP_AGENT_ID.to_string(),
            allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
            denied_capabilities: vec![],
            require_confirmation_for: vec![],
            max_tool_calls: None,
            max_tool_runtime_secs: 60,
            stream_id: None,
            lane_key: None,
            confirmation_timeout_secs: None,
            auto_approve: false,
            unattended: false,
        };
        assert!(
            effective_confirmation_set(&policy, &per_request)
                .contains(&"memory_confirm".to_string())
        );
        for (source, unattended) in [
            ("gui", true),
            ("telegram", false),
            ("discord", false),
            ("imessage", false),
            ("internal", false),
            ("cli", false),
        ] {
            assert!(
                !surface(source, unattended).0,
                "{source} (unattended: {unattended}) must not offer memory_confirm"
            );
        }
        // Without a memory service, nothing memory is offered at all.
        let (shared, dispatcher, bus, registry) = setup();
        let set = build(
            shared,
            dispatcher,
            bus,
            &registry,
            &routing(false),
            None,
            "user1:gui",
        );
        assert!(!set.memory_confirm_offered);
    }

    use crate::security::sandbox::effective_confirmation_set;
    use crate::security::sandbox::SandboxPolicy;

    /// Design §7, *Capabilities*: the sandbox derives the main loop's
    /// confirmation set from the annotations of what is registered, so the
    /// per-request instances must be registered with theirs — before this,
    /// `register_into` registered every instance with `annotations: None` and
    /// no memory tool could ever prompt.
    #[test]
    fn the_main_loop_confirmation_set_names_forget_and_update_and_not_store() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
        let service = Arc::new(crate::memory::service::MemoryService::new(
            Some(db.clone()),
            openalpaca_storage::memory_index::MemoryIndex::open_in_memory().unwrap(),
        ));
        let (shared, dispatcher, bus, registry) = setup();
        for tool in crate::tools::builtins::memory_read_tools(service.clone()) {
            registry.register(tool).unwrap();
        }
        let set = main_loop_tool_set(
            dispatcher,
            shared,
            bus,
            &RoutingConfig::default(),
            Some(db),
            Some(service),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            Arc::new(SkillCatalog::new()),
            None,
            1.0,
            &registry,
            "user1:cli",
            &ToolContext {
                owner_id: Some("user1".to_string()),
                lane_key: Some("user1:cli".to_string()),
                ..Default::default()
            },
            false,
        );
        let per_request = (*registry).clone();
        set.register_into(&per_request);
        let policy = SandboxPolicy {
            agent_id: crate::orchestrator::MAIN_LOOP_AGENT_ID.to_string(),
            allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
            denied_capabilities: vec![],
            require_confirmation_for: vec![],
            max_tool_calls: None,
            max_tool_runtime_secs: 60,
            stream_id: None,
            lane_key: None,
            confirmation_timeout_secs: None,
            auto_approve: false,
            unattended: false,
        };
        let confirmation_set = effective_confirmation_set(&policy, &per_request);
        for name in ["memory_forget", "memory_update"] {
            assert!(
                confirmation_set.contains(&name.to_string()),
                "{name} prompts: {confirmation_set:?}"
            );
        }
        for name in [
            "memory_store",
            "memory_search",
            "memory_read",
            "memory_list",
            "memory_timeline",
            "start_workflow",
            "task_status",
        ] {
            assert!(
                !confirmation_set.contains(&name.to_string()),
                "{name} never prompts: {confirmation_set:?}"
            );
        }
    }
}
