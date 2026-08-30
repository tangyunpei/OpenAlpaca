//! Per-node execution: build context, run agentic loop, return result.

use super::*;

use crate::compose::{
    ComposeOverrides, ComposeRequest, DynamicContextInput, DynamicContextMode, HistoryInput,
    HistoryMode, PersonaInput, PersonaMode, StaticPromptInput, StaticPromptMode, SummaryWrapMode,
    SystemBlock,
};
use crate::middleware::prompt::{SystemPersona, format_tool_guidance};
use crate::prompt_ctx::{ContextBundle, ExecutionPath, SectionPriority};

/// Execute a single DAG node: build context, run agentic loop, return result.
/// All parameters are owned because this runs inside a spawned task.
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_single_node(
    node: DagNode,
    router: Arc<LlmRouter>,
    tool_registry: Arc<ToolRegistry>,
    bus: EventBus,
    db: Option<Database>,
    task_id: String,
    task_description: String,
    created_by: String,
    agent: SubAgent,
    node_timeout: Duration,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    cancel_token: Option<CancellationToken>,
    workspace_id: Option<String>,
    connector_guidance: String,
    workspace_snapshot: Arc<Option<TaskState>>,
    confirmation_broker: Option<Arc<crate::security::confirmation::ConfirmationBroker>>,
    compose_engine: Arc<crate::compose::ComposeEngine>,
) -> NodeResult {
    let agent_id = agent.id.clone();

    // Build ToolContext for workspace access
    let tool_ctx = ToolContext {
        agent_id: Some(agent_id.clone()),
        task_id: Some(task_id.clone()),
        owner_id: Some(created_by.clone()),
        workspace_id,
        skill_stack: vec![],
        effective_constraints: None,
        // DAG nodes are lane-detached: no lane/request threading here.
        lane_key: None,
        source: None,
        request_id: None,
        principal: None,
        scope: None,
        workspace_path: None,
    };
    let mut per_request_sandbox = SandboxManager::with_defaults(tool_registry.clone(), bus.clone());
    if let Some(broker) = confirmation_broker {
        per_request_sandbox.set_confirmation_broker(broker);
    }

    // Build LoopConfig — agent constraints override daemon defaults, cap at node timeout
    let mut loop_config =
        LoopConfig::from_agent(&daemon_config.load().execution.agent_defaults, &agent)
            .with_context_window(router.model_registry(), agent.llm_config.model.as_deref());
    loop_config.max_tool_runtime = std::cmp::min(node_timeout, loop_config.max_tool_runtime);

    // Set compaction model from daemon config
    loop_config.compaction_model = daemon_config.load()
        .execution.context.compaction_model.clone();
    loop_config.event_bus = Some(bus.clone());
    loop_config.experimental_ephemeral_pressure =
        daemon_config.load().experimental.ephemeral_pressure_layer;

    // Instantiate ContextBudgetManager for budget-aware compaction
    let context_budget = {
        let default_model = router.default_model();
        let model_id = agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let context_window = router.model_registry()
            .get_model_info(model_id)
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);
        crate::context_budget::ContextBudgetManager::new(
            context_window,
            &daemon_config.load().execution.context,
        )
    };

    let mut sandbox_policy = SandboxPolicy::from_constraints(&agent_id, &agent.constraints);
    if daemon_config.load().security.auto_approve_confirmations {
        sandbox_policy.auto_approve = true;
    }

    // Resolve tools via shared helper
    let tools = crate::tools::resolve_agent_tools(&agent, &tool_registry);

    // ── Phase 6 Commit 2: route system-prompt + message-list assembly through
    // the layered compose engine. `PersonaMode::Skip` +
    // `StaticPromptMode::SubagentMinimal` (raw-blocks-only) +
    // `DynamicContextMode::Default` (empty bundle → zero messages) +
    // `HistoryMode::Default` reproduces the pre-migration PromptBuilder chain
    // byte-identically. See `test_golden_dag_node_byte_identical` in
    // `compose/tests.rs` for the invariant.
    //
    // Pre-migration (node_runner.rs:85-215) did NOT call `.context_bundle()`,
    // so there is no `ContextPackage` routed through Layer 3 — the
    // ContextPackage below is purely for telemetry. `ContextPackageBuilt` is
    // still emitted unchanged so downstream observability matches pre-migration.
    let model_window = context_budget.model_context_window();
    let identity_block = format!("<identity>\n{}\n</identity>", agent.preset.persona);
    let assignment_block = format!(
        "<assignment>\nSub-task: {}\nDescription: {}\n</assignment>",
        node.title, node.description
    );
    let scope_block = "<scope>\nYou are responsible for completing only the sub-task described above. \
        Do not attempt work outside your assignment. Your output will be stored \
        in the workspace for downstream nodes that depend on your results.\n</scope>";
    let output_block = "<output-format>\nProvide a complete, self-contained result for your sub-task. Other agents \
        will consume your output, so be specific and include all relevant details.\n</output-format>";

    let connector_suffix = if !connector_guidance.is_empty() {
        format!("\n{}", connector_guidance)
    } else {
        String::new()
    };

    // Resolve workspace context (used for both the telemetry ContextPackage
    // and the post-compose user-message injection below). Uses pre-loaded
    // snapshot (Opt-7c) to avoid redundant DB reads per node.
    let workspace_context = if let Some(ref state) = *workspace_snapshot {
        state.workspace.format_for_prompt(&node.workspace_keys)
    } else {
        super::progress::load_workspace_context(&task_id, &db, &node.workspace_keys)
    };

    // Context package: workspace artifacts for telemetry. Keep ContextPackageBuilt
    // emission unchanged — this is purely observability.
    {
        let mut package_sections = Vec::new();
        if !workspace_context.is_empty() {
            package_sections.push(crate::prompt_ctx::PackageSection {
                kind: crate::prompt_ctx::PackageSectionKind::WorkspaceArtifact,
                content: workspace_context.clone(),
                token_estimate: workspace_context.len() / 4,
                priority: SectionPriority::Normal,
            });
        }

        let total_tokens: usize = package_sections.iter().map(|s| s.token_estimate).sum();

        bus.publish(crate::events::SystemEvent::ContextPackageBuilt {
            request_id: uuid::Uuid::new_v4(),
            agent_id: agent.id.clone(),
            sections: package_sections
                .iter()
                .map(|s| (s.kind.source_name().to_string(), s.token_estimate))
                .collect(),
            total_tokens,
            budget: (model_window as f64 * 0.40) as usize,
            sub_agent_window: model_window,
            timestamp: chrono::Utc::now(),
        });
    }

    // raw_blocks replicate the pre-migration `.raw_system_block(...)` / `.tools()` /
    // `.raw_system_block("connector_guidance", ...)` order at node_runner.rs:97-107.
    // Tools are pre-rendered via format_tool_guidance (same helper
    // PromptBuilder::tools calls internally) and pushed as a raw_block named
    // "tools". Connector suffix — already prefixed with "\n" by the branch
    // above — is pushed as a raw_block named "connector_guidance" only when
    // non-empty.
    let mut raw_blocks: Vec<SystemBlock> = Vec::with_capacity(6);
    raw_blocks.push(SystemBlock {
        name: "agent_identity",
        content: Arc::<str>::from(identity_block),
        priority: SectionPriority::High,
    });
    raw_blocks.push(SystemBlock {
        name: "assignment",
        content: Arc::<str>::from(assignment_block.clone()),
        priority: SectionPriority::Critical,
    });
    raw_blocks.push(SystemBlock {
        name: "scope",
        content: Arc::<str>::from(scope_block.to_string()),
        priority: SectionPriority::Normal,
    });
    raw_blocks.push(SystemBlock {
        name: "output_format",
        content: Arc::<str>::from(output_block.to_string()),
        priority: SectionPriority::Normal,
    });
    let tools_rendered = format_tool_guidance(&tools);
    if !tools_rendered.is_empty() {
        raw_blocks.push(SystemBlock {
            name: "tools",
            content: Arc::<str>::from(tools_rendered),
            priority: SectionPriority::Normal,
        });
    }
    if !connector_suffix.is_empty() {
        raw_blocks.push(SystemBlock {
            name: "connector_guidance",
            content: Arc::<str>::from(connector_suffix),
            priority: SectionPriority::Normal,
        });
    }

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Skip,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

    let tools_arc: Arc<Vec<openalpaca_llm::ToolDefinition>> = Arc::new(tools.clone());

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: crate::compose::fingerprint::hash_agent_config(&agent),
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: tools_arc.clone(),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks,
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SubagentMinimal,
        model_window: model_window as u32,
    };

    // DAG Node pre-migration never calls `.context_bundle()`, so Layer 3
    // receives an empty bundle and emits no messages/blocks.
    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(task_description.as_str()),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::DagNode {
            node_id: node.node_id.clone(),
        },
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    // Pre-migration messages vec at node_runner.rs:193-215:
    //   [system, user(task_description), user(<workspace>...</workspace>)]
    // Mapped to HistoryMode::Default:
    //   - recent_messages[0] = user(task_description)
    //   - current_user_turn = Some(user(workspace wrap)) when workspace is non-empty
    let workspace_wrapped = if !workspace_context.is_empty() {
        Some(format!(
            "<workspace>\n\
             The following entries contain results from upstream sub-tasks. \
             Use this data to complete your assignment. You can also call \
             workspace_read and workspace_write to access or update entries.\n\n\
             {}\n\
             </workspace>",
            workspace_context
        ))
    } else {
        None
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(vec![ChatMessage::user(&task_description)]),
        current_user_turn: workspace_wrapped
            .as_deref()
            .map(ChatMessage::user),
        mode: HistoryMode::Default,
    };

    let compose_request = ComposeRequest::DagNode {
        agent: Arc::new(agent.clone()),
        assignment: Arc::<str>::from(assignment_block),
        workspace_context: Arc::<str>::from(workspace_context.as_str()),
        tools: tools_arc.clone(),
        overrides: ComposeOverrides::default(),
    };

    let composed = compose_engine.compose(
        &compose_request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        tools_arc.clone(),
        Some(&bus),
        None, // lane: DAG nodes have no natural lane key (per-lane cache deferred).
    );

    // --- Context Budget Telemetry ---
    {
        let default_model = router.default_model();
        let model_id = agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let request_id = uuid::Uuid::new_v4();
        let mut budget_snapshot =
            crate::context_budget::ContextBudgetManager::new(
                model_window,
                &daemon_config.load().execution.context,
            );
        // Register the static-prompt tokens under `system_prompt` to preserve
        // the pre-migration budget accounting (pre-migration
        // `built.total_prompt_tokens` summed all registered sections; DAG Node
        // pre-migration only registered raw_system_blocks + tools, all of
        // which now live inside Layer 2's system_message).
        let system_prompt_tokens = composed.token_budget.static_prompt_tokens as usize;
        budget_snapshot.register_section("system_prompt", system_prompt_tokens);
        budget_snapshot.register_section("tools", tools.len() * 200);

        tracing::debug!(
            request_id = %request_id,
            agent_id = %agent_id,
            model_window,
            fixed_zone = budget_snapshot.fixed_zone_tokens(),
            free_zone = budget_snapshot.free_zone_capacity(),
            buffer = budget_snapshot.autocompact_buffer(),
            "Context budget computed (DAG node)"
        );

        bus.publish(crate::events::SystemEvent::ContextBudgetComputed {
            request_id,
            model: model_id.to_string(),
            window_size: model_window,
            fixed_zone_tokens: budget_snapshot.fixed_zone_tokens(),
            free_zone_tokens: budget_snapshot.free_zone_capacity(),
            buffer_size: budget_snapshot.autocompact_buffer(),
            section_breakdown: budget_snapshot
                .section_breakdown()
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
            timestamp: chrono::Utc::now(),
        });
    }

    // The compose engine's Arc'd messages vec: owned by ComposedRequest. Clone
    // the contents out because `run_agentic_loop_routed` needs `Vec<ChatMessage>`
    // (and will mutate the vec during the agentic loop).
    let messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

    // Run agentic loop, bounded by the node timeout. Previously `node_timeout`
    // only clamped per-tool runtime (above), so a node could run for
    // max_rounds × (LLM latency + tool time) — far past the configured limit,
    // also stalling `total_timeout`. Enforce it on the whole node here.
    let agent_start = Instant::now();
    let loop_fut = run_agentic_loop_routed(
        router.as_ref(),
        messages,
        tools,
        &loop_config,
        Some(&per_request_sandbox),
        &agent_id,
        Some(&sandbox_policy),
        Some(&task_id),
        Some(&context_budget),
        cancel_token,
        Some(&tool_ctx),
        None,
    );
    let result = match tokio::time::timeout(node_timeout, loop_fut).await {
        Ok(r) => r,
        Err(_) => {
            tracing::warn!(
                "DAG node '{}' (agent '{}') exceeded node timeout of {}s; aborting node",
                node.node_id,
                agent_id,
                node_timeout.as_secs()
            );
            LoopResult {
                final_content: format!("Node timed out after {}s", node_timeout.as_secs()),
                rounds_used: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                tool_calls_made: 0,
                finish_reason: LoopFinishReason::Error(format!(
                    "node timeout ({}s) exceeded",
                    node_timeout.as_secs()
                )),
                model_used: loop_config.model.clone(),
                elapsed: agent_start.elapsed(),
                estimated_cost: 0.0,
            }
        }
    };

    let agent_elapsed = agent_start.elapsed();
    let agent_runtime = agent_elapsed.as_secs() as i64;

    tracing::info!(
        "DAG node '{}' (agent '{}'): reason={:?}, rounds={}, tokens={}/{}",
        node.node_id,
        agent_id,
        result.finish_reason,
        result.rounds_used,
        result.total_input_tokens,
        result.total_output_tokens
    );

    let success = matches!(
        &result.finish_reason,
        LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated
    );

    // Record LLM usage
    crate::orchestrator::dispatcher::usage::record_llm_usage(
        &router,
        &result,
        loop_config.model.as_deref(),
        &agent_id,
        &task_id,
        agent_elapsed.as_millis() as i64,
        db.as_ref(),
        &bus,
    );

    // Record agent history
    if let Some(db) = &db {
        let role = format!("dag_node:{}", node.node_id);
        crate::orchestrator::dispatcher::usage::record_agent_history(
            db,
            &agent_id,
            &task_id,
            &role,
            success,
            agent_runtime,
        );
    }

    NodeResult {
        node_id: node.node_id.clone(),
        node_title: node.title.clone(),
        agent_id,
        success,
        final_content: result.final_content.clone(),
        duration_ms: agent_elapsed.as_millis() as u64,
        loop_result: result,
    }
}
