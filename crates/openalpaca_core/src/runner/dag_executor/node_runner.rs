//! Per-node execution: build context, run agentic loop, return result.

use super::*;

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
) -> NodeResult {
    let agent_id = agent.id.clone();

    // Build ToolContext for workspace access
    let tool_ctx = ToolContext {
        agent_id: Some(agent_id.clone()),
        task_id: Some(task_id.clone()),
        owner_id: Some(created_by.clone()),
        workspace_id,
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

    // Build prompt via PromptBuilder
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

    let mut builder = crate::prompt::PromptBuilder::new(model_window);
    builder = builder
        .raw_system_block("agent_identity", &identity_block, crate::prompt_ctx::SectionPriority::High)
        .raw_system_block("assignment", &assignment_block, crate::prompt_ctx::SectionPriority::Critical)
        .raw_system_block("scope", scope_block, crate::prompt_ctx::SectionPriority::Normal)
        .raw_system_block("output_format", output_block, crate::prompt_ctx::SectionPriority::Normal)
        .tools(&tools);

    if !connector_suffix.is_empty() {
        builder = builder.raw_system_block("connector_guidance", &connector_suffix, crate::prompt_ctx::SectionPriority::Normal);
    }

    // Context package: workspace artifacts for telemetry
    {
        // Build workspace artifact content
        let ws_ctx = if let Some(ref state) = *workspace_snapshot {
            state.workspace.format_for_prompt(&node.workspace_keys)
        } else {
            String::new()
        };

        // Build a minimal ContextPackage for telemetry
        let mut package_sections = Vec::new();
        if !ws_ctx.is_empty() {
            package_sections.push(crate::prompt_ctx::PackageSection {
                kind: crate::prompt_ctx::PackageSectionKind::WorkspaceArtifact,
                content: ws_ctx.clone(),
                token_estimate: ws_ctx.len() / 4,
                priority: crate::prompt_ctx::SectionPriority::Normal,
            });
        }

        let total_tokens: usize = package_sections.iter().map(|s| s.token_estimate).sum();

        // Emit telemetry with new schema
        bus.publish(crate::events::SystemEvent::ContextPackageBuilt {
            request_id: uuid::Uuid::new_v4(),
            agent_id: agent.id.clone(),
            sections: package_sections.iter()
                .map(|s| (s.kind.source_name().to_string(), s.token_estimate))
                .collect(),
            total_tokens,
            budget: (model_window as f64 * 0.40) as usize,
            sub_agent_window: model_window,
            timestamp: chrono::Utc::now(),
        });

        // Workspace context is injected as a user message below (lines 165+),
        // not in the system prompt, to keep it in the untrusted zone.
    }

    let built = builder.build();

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
        budget_snapshot.register_section("system_prompt", built.total_prompt_tokens);
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

    let system_prompt = built.system_message;

    // Build messages: system + task + workspace context for this node
    let mut messages = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user(&task_description),
    ];

    // Inject workspace entries that this node depends on.
    // Uses pre-loaded snapshot (Opt-7c) to avoid redundant DB reads per node.
    let workspace_context = if let Some(ref state) = *workspace_snapshot {
        state.workspace.format_for_prompt(&node.workspace_keys)
    } else {
        super::progress::load_workspace_context(&task_id, &db, &node.workspace_keys)
    };
    if !workspace_context.is_empty() {
        messages.push(ChatMessage::user(&format!(
            "<workspace>\n\
             The following entries contain results from upstream sub-tasks. \
             Use this data to complete your assignment. You can also call \
             workspace_read and workspace_write to access or update entries.\n\n\
             {}\n\
             </workspace>",
            workspace_context
        )));
    }

    // Run agentic loop
    let agent_start = Instant::now();
    let result = run_agentic_loop_routed(
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
    )
    .await;

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
