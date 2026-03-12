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

    // Build ContextualToolExecutor for workspace access
    let ctx_exec = ToolExecutionContext {
        owner_id: Some(created_by.clone()),
        task_id: Some(task_id.clone()),
        agent_id: Some(agent_id.clone()),
        db: db.clone(),
        workspace_id,
    };
    let contextual_executor =
        Arc::new(ContextualToolExecutor::new(tool_registry.clone(), ctx_exec));
    let mut per_request_sandbox = SandboxManager::with_defaults(contextual_executor, bus.clone());
    if let Some(broker) = confirmation_broker {
        per_request_sandbox.set_confirmation_broker(broker);
    }

    // Build LoopConfig — agent constraints override daemon defaults, cap at node timeout
    let mut loop_config =
        LoopConfig::from_agent(&daemon_config.load().execution.agent_defaults, &agent)
            .with_context_window(router.model_registry(), agent.llm_config.model.as_deref());
    loop_config.max_tool_runtime = std::cmp::min(node_timeout, loop_config.max_tool_runtime);

    let mut sandbox_policy = SandboxPolicy::from_constraints(&agent_id, &agent.constraints);
    if daemon_config.load().security.auto_approve_confirmations {
        sandbox_policy.auto_approve = true;
    }

    // Resolve tools via shared helper
    let tools = crate::tools::resolve_agent_tools(&agent, &tool_registry);

    // Build system prompt
    let tool_guidance = format_tool_guidance(&tools);
    let connector_suffix = if !connector_guidance.is_empty() {
        format!("\n{}", connector_guidance)
    } else {
        String::new()
    };
    let mut system_prompt = format!(
        "<identity>\n{}\n</identity>\n\n\
         <assignment>\n\
         Sub-task: {}\n\
         Description: {}\n\
         </assignment>\n\n\
         <scope>\n\
         You are responsible for completing only the sub-task described above. \
         Do not attempt work outside your assignment. Your output will be stored \
         in the workspace for downstream nodes that depend on your results.\n\
         </scope>\n\n\
         <output-format>\n\
         Provide a complete, self-contained result for your sub-task. Other agents \
         will consume your output, so be specific and include all relevant details.\n\
         </output-format>{}{}",
        agent.preset.persona, node.title, node.description, tool_guidance, connector_suffix
    );

    // --- Context Package (Phase C) ---
    {
        let denied_sections = agent.constraints.denied_sections.clone();

        let mut pkg_builder = crate::context_budget::ContextPackageBuilder::new(
            node.description.clone(),
        );

        // Workspace context will be loaded after this block, so we check workspace_snapshot
        // to pre-populate artifacts. The actual workspace injection into messages happens later.
        if let Some(ref state) = *workspace_snapshot {
            let ws_ctx = state.workspace.format_for_prompt(&node.workspace_keys);
            if !ws_ctx.is_empty() {
                pkg_builder = pkg_builder.workspace_artifact(ws_ctx);
            }
        }

        if !denied_sections.is_empty() {
            pkg_builder = pkg_builder.denied_sections(&denied_sections);
        }

        let context_package = pkg_builder.build();

        // Emit telemetry
        let injected_sections_tokens = {
            let mut t = 0usize;
            if let Some(ref s) = context_package.conversation_summary { t += s.len() / 4 + 20; }
            for m in &context_package.relevant_memories { t += m.len() / 4 + 10; }
            if let Some(ref s) = context_package.user_context { t += s.len() / 4 + 20; }
            for a in &context_package.workspace_artifacts { t += a.len() / 4 + 20; }
            t
        };
        bus.publish(crate::events::SystemEvent::ContextPackageBuilt {
            request_id: uuid::Uuid::new_v4(),
            agent_id: agent.id.clone(),
            sections_included: context_package.sections_included()
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            total_tokens: injected_sections_tokens,
            memories_count: context_package.relevant_memories.len(),
            timestamp: chrono::Utc::now(),
        });

        // Append context package optional sections to system_prompt
        if let Some(ref summary) = context_package.conversation_summary {
            system_prompt.push_str(&format!(
                "\n\n<conversation-context>\n{}\n</conversation-context>",
                summary
            ));
        }
        if !context_package.relevant_memories.is_empty() {
            let mem_block = context_package.relevant_memories.join("\n- ");
            system_prompt.push_str(&format!(
                "\n\n<relevant-memories>\n- {}\n</relevant-memories>",
                mem_block
            ));
        }
        if let Some(ref ctx) = context_package.user_context {
            system_prompt.push_str(&format!(
                "\n\n<user-context>\n{}\n</user-context>",
                ctx
            ));
        }
    }

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
        None, // context_budget
        cancel_token,
    )
    .await;

    let agent_runtime = agent_start.elapsed().as_secs() as i64;

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
        agent_start.elapsed().as_millis() as i64,
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
        duration_ms: agent_start.elapsed().as_millis() as u64,
        loop_result: result,
    }
}
