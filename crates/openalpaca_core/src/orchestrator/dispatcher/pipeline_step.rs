//! Per-step execution logic for sequential pipeline: sandbox creation, prompt building,
//! LLM call, progress tracking, and state persistence.

use super::outcome::update_state_with_retry;
use super::retrieve_memory_block;
use super::usage;
use crate::agent::subagent::{AgentStatus, SubAgent};
use crate::bus::EventBus;
use crate::compose::{
    ComposeOverrides, ComposeRequest, DynamicContextInput, DynamicContextMode, HistoryInput,
    HistoryMode, PersonaInput, PersonaMode, StaticPromptInput, StaticPromptMode, SummaryWrapMode,
    SystemBlock,
};
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::middleware::prompt::format_tool_guidance;
use crate::prompt_ctx::package::{AgentSummary, HandoffContext};
use crate::prompt_ctx::{ExecutionPath, SectionPriority};
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::registry::ToolContext;
use crate::tools::ToolRegistry;
use arc_swap::ArcSwap;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter};
use openalpaca_storage::Database;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Shared immutable context for all pipeline steps (avoids passing 15+ parameters).
pub(super) struct PipelineStepContext {
    pub ctx: Arc<SharedContext>,
    pub bus: EventBus,
    pub db: Option<Database>,
    pub embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    pub tool_registry: Arc<ToolRegistry>,
    pub daemon_config: Arc<ArcSwap<DaemonConfig>>,
    pub router: Arc<LlmRouter>,
    pub connector_block: String,
    pub broker: Option<Arc<crate::security::confirmation::ConfirmationBroker>>,
    pub task_id: String,
    pub description: String,
    pub created_by: String,
    pub workspace_id: Option<String>,
    pub total_agents: usize,
    pub cancel_token: CancellationToken,
    /// Phase 6 Commit 1: shared with the owning `TaskDispatcher` so pipeline
    /// step prompt caches are globally visible. Routed via
    /// `execute_pipeline_step` → `ComposeEngine::compose(ComposeRequest::PipelineStep{...})`.
    pub compose_engine: Arc<crate::compose::ComposeEngine>,
}

/// Result from executing a single pipeline step.
pub(super) struct PipelineStepResult {
    pub success: bool,
    pub error: Option<String>,
    /// Raw content output from the agent (empty if agent produced no text).
    pub raw_content: String,
    /// Display-friendly content (synthetic summary if agent produced no text).
    pub display_content: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Whether the agent made any tool calls (used to decide workspace cache refresh).
    pub tool_calls_made: usize,
}

/// Fetch the current workspace context string from the task's state in SQLite.
/// Returns an empty string if the DB is absent, the task doesn't exist, or parsing fails.
pub(super) fn fetch_workspace_context(
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
) -> String {
    let Some(db) = db else {
        return String::new();
    };
    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    match repo.get(task_id) {
        Ok(Some(existing)) => existing
            .state_json
            .as_deref()
            .and_then(|sj| {
                serde_json::from_str::<crate::orchestrator::task_state::TaskState>(sj).ok()
            })
            .map(|state| state.workspace.format_for_prompt(&[]))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Execute a single pipeline step: set up sandbox, build prompt, run LLM, track progress.
#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_pipeline_step(
    pctx: &PipelineStepContext,
    step: usize,
    agent: &SubAgent,
    assignment_id: Option<&String>,
    role_description: &str,
    previous_output: &Option<String>,
    cached_workspace_context: &str,
) -> PipelineStepResult {
    let agent_id = &agent.id;

    // Per-step sandbox scoped to this agent via ToolContext.
    let tool_ctx = ToolContext {
        agent_id: Some(agent_id.clone()),
        task_id: Some(pctx.task_id.clone()),
        owner_id: Some(pctx.created_by.clone()),
        workspace_id: pctx.workspace_id.clone(),
        skill_stack: vec![],
        effective_constraints: None,
    };
    let mut per_request_sandbox =
        SandboxManager::with_defaults(pctx.tool_registry.clone(), pctx.bus.clone());
    if let Some(ref broker) = pctx.broker {
        per_request_sandbox.set_confirmation_broker(broker.clone());
    }

    tracing::info!(
        "Pipeline step {}/{}: agent '{}' starting on task '{}'",
        step + 1,
        pctx.total_agents,
        agent_id,
        pctx.task_id
    );

    // Mark agent as Busy (with RAII guard for panic safety)
    pctx.ctx.agent_registry.update_status(
        agent_id,
        AgentStatus::Busy {
            task_id: pctx.task_id.clone(),
        },
    );
    pctx.bus.publish(SystemEvent::AgentStatusChanged {
        agent_id: agent_id.clone(),
        instance_id: agent_id.clone(),
        template_id: agent.template_id.clone(),
        status: "busy".to_string(),
        current_task_id: Some(pctx.task_id.clone()),
        timestamp: Utc::now(),
    });
    let mut busy_guard = crate::runner::lead_agent::AgentBusyGuard::new(
        agent_id.clone(),
        agent.template_id.clone(),
        pctx.ctx.agent_registry.clone(),
        pctx.bus.clone(),
    );

    // Assignment -> Running
    if let (Some(db), Some(assign_id)) = (&pctx.db, assignment_id) {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let _ = repo.update_assignment_status(
            assign_id,
            openalpaca_storage::AssignmentStatus::Running,
        );
    }

    // Update state_json: mark step running + fix agent_id from template
    // to actual instance_id (non-singleton IDs are "template::uuid").
    if let Some(ref db) = pctx.db {
        let step_i = step as i32;
        let instance_id = agent_id.clone();
        if !update_state_with_retry(
            db,
            &pctx.task_id,
            move |s| {
                if let Some(step) = s.steps.iter_mut().find(|st| st.step_order == step_i) {
                    step.agent_id = instance_id.clone();
                }
                s.mark_step_running(step_i);
            },
            "mark_step_running",
        )
        .await
        {
            tracing::error!(
                "Failed to persist mark_step_running for task '{}'",
                pctx.task_id
            );
        }
    }

    // Emit progress event for this step.
    // Use `step` (0-indexed) as progress_current, meaning "N steps completed so far".
    pctx.bus.publish(SystemEvent::TaskUpdated {
        task_id: pctx.task_id.clone(),
        status: "running".to_string(),
        progress_current: Some(step as i32),
        progress_total: Some(pctx.total_agents as i32),
        timestamp: Utc::now(),
    });
    if let Some(ref db) = pctx.db {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let _ = repo.update_progress(&pctx.task_id, step as i32, pctx.total_agents as i32);
    }

    // Build LoopConfig -- agent constraints override daemon defaults
    let mut loop_config = LoopConfig::from_agent(
        &pctx.daemon_config.load().execution.agent_defaults,
        agent,
    )
    .with_context_window(
        pctx.router.model_registry(),
        agent.llm_config.model.as_deref(),
    );

    // Set compaction model from daemon config
    loop_config.compaction_model = pctx
        .daemon_config
        .load()
        .execution
        .context
        .compaction_model
        .clone();
    loop_config.event_bus = Some(pctx.bus.clone());
    loop_config.experimental_ephemeral_pressure = pctx
        .daemon_config
        .load()
        .experimental
        .ephemeral_pressure_layer;

    // Instantiate ContextBudgetManager for budget-aware compaction
    let context_budget = {
        let default_model = pctx.router.default_model();
        let model_id = agent
            .llm_config
            .model
            .as_deref()
            .unwrap_or(&default_model);
        let context_window = pctx
            .router
            .model_registry()
            .get_model_info(model_id)
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);
        crate::context_budget::ContextBudgetManager::new(
            context_window,
            &pctx.daemon_config.load().execution.context,
        )
    };

    let sandbox_policy = SandboxPolicy::from_constraints(agent_id, &agent.constraints);

    // Resolve tools via shared helper
    let tools = crate::tools::resolve_agent_tools(agent, &pctx.tool_registry);
    tracing::info!(
        "Agent '{}' loaded {} tool definitions for capabilities: {:?}",
        agent_id,
        tools.len(),
        agent
            .capabilities
            .iter()
            .map(|s| &s.name)
            .collect::<Vec<_>>()
    );

    // --- Context Package (Phase C) -> HandoffContext-based ---
    let is_last_step = step == pctx.total_agents - 1;
    let output_note = if is_last_step {
        "Your output is the final result delivered to the user. Make it complete and polished."
    } else {
        "Your output will be passed to the next agent in the pipeline. Be thorough and \
         include all relevant details so the next agent can build on your work."
    };

    // Build HandoffContext from predecessor output (if present)
    let handoff = previous_output.as_ref().map(|output| HandoffContext {
        producer: AgentSummary {
            name: if step > 0 {
                format!("step_{}", step - 1)
            } else {
                "unknown".to_string()
            },
            role: "pipeline_predecessor".to_string(),
            step: if step > 0 { step - 1 } else { 0 },
        },
        task_assigned: pctx.description.clone(),
        output: output.clone(),
        decisions: vec![],
        handoff_notes: None,
    });

    // Build ContextPackage sections from handoff + workspace
    let denied_sections: std::collections::HashSet<String> = agent
        .constraints
        .denied_sections
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    // Phase 6 Commit 1: Task description is re-routed into the StaticPromptInput's
    // `raw_blocks` field (as a "task_description" raw_block) rather than flowing
    // through the ContextPackage's TaskDescription SystemPrompt injection. The
    // pre-migration PromptBuilder chain appended TaskDescription inline to the
    // system_message via `.context_bundle()`; Layer 3's `build_from_bundle`
    // would instead surface it as a separate `ChatMessage::system`, breaking
    // byte-identity. Keep the metadata consistent: the package we report to
    // `ContextPackageBuilt` still includes TaskDescription so downstream
    // observability matches pre-migration signal. The bundle fed to the compose
    // engine omits TaskDescription.
    let mut package_sections_full = Vec::new();

    // Task description (Critical) -- retained for the telemetry event.
    package_sections_full.push(crate::prompt_ctx::PackageSection {
        kind: crate::prompt_ctx::PackageSectionKind::TaskDescription,
        content: role_description.to_string(),
        token_estimate: role_description.len() / 4,
        priority: SectionPriority::Critical,
    });

    // Predecessor output (High) -- from HandoffContext
    if let Some(ref h) = handoff
        && !denied_sections.contains("workspace_artifacts")
    {
        package_sections_full.push(crate::prompt_ctx::PackageSection {
            kind: crate::prompt_ctx::PackageSectionKind::PredecessorOutput,
            content: h.format(),
            token_estimate: h.token_estimate(),
            priority: SectionPriority::High,
        });
    }

    // Workspace context (Normal)
    if !cached_workspace_context.is_empty()
        && !denied_sections.contains("workspace_artifacts")
    {
        package_sections_full.push(crate::prompt_ctx::PackageSection {
            kind: crate::prompt_ctx::PackageSectionKind::WorkspaceArtifact,
            content: cached_workspace_context.to_string(),
            token_estimate: cached_workspace_context.len() / 4,
            priority: SectionPriority::Normal,
        });
    }

    let total_tokens: usize = package_sections_full.iter().map(|s| s.token_estimate).sum();
    let context_package = crate::prompt_ctx::ContextPackage {
        sections: package_sections_full,
        total_tokens,
        budget: total_tokens + 1000, // generous budget for pipeline steps
        sub_agent_window: context_budget.model_context_window(),
    };

    // Emit telemetry (preserves pre-migration ContextPackageBuilt signal
    // including TaskDescription).
    pctx.bus
        .publish(crate::events::SystemEvent::ContextPackageBuilt {
            request_id: uuid::Uuid::new_v4(),
            agent_id: agent.id.clone(),
            sections: context_package
                .sections
                .iter()
                .map(|s| (s.kind.source_name().to_string(), s.token_estimate))
                .collect(),
            total_tokens,
            budget: context_package.budget,
            sub_agent_window: context_package.sub_agent_window,
            timestamp: chrono::Utc::now(),
        });

    // Bundle fed to Layer 3: copy the full package minus TaskDescription (which
    // is now re-emitted as a raw_block named "task_description"). See
    // `StaticPromptMode::SubagentMinimal` / `test_golden_pipeline_step_byte_identical`.
    let bundle_package = crate::prompt_ctx::ContextPackage {
        sections: context_package
            .sections
            .iter()
            .filter(|s| s.kind != crate::prompt_ctx::PackageSectionKind::TaskDescription)
            .cloned()
            .collect(),
        total_tokens: context_package
            .sections
            .iter()
            .filter(|s| s.kind != crate::prompt_ctx::PackageSectionKind::TaskDescription)
            .map(|s| s.token_estimate)
            .sum(),
        budget: context_package.budget,
        sub_agent_window: context_package.sub_agent_window,
    };
    let bundle = bundle_package.to_bundle();

    // ── Phase 6 Commit 1: route system-prompt + message-list assembly through
    // the layered compose engine. `PersonaMode::Skip` +
    // `StaticPromptMode::SubagentMinimal` (raw-blocks-only) +
    // `DynamicContextMode::Default` (bundle → user messages) +
    // `HistoryMode::Default` reproduces the pre-migration PromptBuilder chain
    // byte-identically. See `test_golden_pipeline_step_byte_identical` in
    // `compose/tests.rs` for the invariant.
    //
    // Spec errata: the default-dispatch table listed `PersonaMode::Minimal` for
    // PipelineStep; pre-migration emits NO SystemPersona content, so Skip
    // matches byte-identically. FirstStepOnly was considered for the memory
    // injection but can only emit a single user message — pipeline step 0 has
    // both a memory block AND the step_description, so we use HistoryMode::Default
    // and attach the memory as a `recent_messages` entry.
    let model_window = context_budget.model_context_window();
    let identity_block = format!("<identity>\n{}\n</identity>", agent.preset.persona);
    let assignment_block = format!(
        "<assignment>\nRole: {}\nPipeline step: {} of {}\n</assignment>",
        role_description,
        step + 1,
        pctx.total_agents
    );
    let scope_block = "<scope>\nFocus on completing your assigned role. \
                        Do not attempt work outside your role description.\n</scope>";
    let output_block = format!("<output-format>\n{}\n</output-format>", output_note);

    // Retrieve memory block first so we can attach it to HistoryInput.recent_messages
    // before calling compose(). Preserves the pre-migration side effect order
    // (the DB query runs before the prompt is assembled either way).
    let memory_block_text: Option<String> = if step == 0
        && let Some(ref db) = pctx.db
    {
        let scope_ctx = pctx.workspace_id.as_ref().map(|ws| {
            crate::memory::scope_context::MemoryScopeContext::new(Some(ws.clone()))
        });
        let access_boost = pctx
            .daemon_config
            .load()
            .orchestrator
            .memory
            .decay
            .access_boost;
        retrieve_memory_block(
            db,
            pctx.embedder.as_ref(),
            &pctx.created_by,
            &pctx.description,
            5,
            scope_ctx.as_ref(),
            access_boost,
        )
        .await
    } else {
        None
    };

    // raw_blocks replicate the pre-migration `.raw_system_block(...)` / `.tools()`
    // / raw_system_block("connector_guidance") / bundle-SystemPrompt order at
    // pipeline_step.rs:355-371. Tools are pre-rendered via format_tool_guidance
    // (same helper `PromptBuilder::tools` calls internally) and pushed as a
    // raw_block named "tools". Connector guidance is pushed as a raw_block
    // named "connector_guidance". Task description goes last to match the
    // pre-migration TaskDescription-SystemPrompt append-inline position.
    let mut raw_blocks: Vec<SystemBlock> = Vec::with_capacity(7);
    raw_blocks.push(SystemBlock {
        name: "agent_identity",
        content: Arc::<str>::from(identity_block),
        priority: SectionPriority::High,
    });
    raw_blocks.push(SystemBlock {
        name: "assignment",
        content: Arc::<str>::from(assignment_block),
        priority: SectionPriority::Critical,
    });
    raw_blocks.push(SystemBlock {
        name: "scope",
        content: Arc::<str>::from(scope_block.to_string()),
        priority: SectionPriority::Normal,
    });
    raw_blocks.push(SystemBlock {
        name: "output_format",
        content: Arc::<str>::from(output_block),
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
    if !pctx.connector_block.is_empty() {
        raw_blocks.push(SystemBlock {
            name: "connector_guidance",
            content: Arc::<str>::from(pctx.connector_block.clone()),
            priority: SectionPriority::Normal,
        });
    }
    raw_blocks.push(SystemBlock {
        name: "task_description",
        content: Arc::<str>::from(role_description.to_string()),
        priority: SectionPriority::Critical,
    });

    // PersonaMode::Skip: Layer 1 emits NO content regardless of the underlying
    // SystemPersona. Use a default-constructed persona to keep the input shape
    // minimal (no RwLock fetch needed, no clone of the orchestrator's persona).
    let persona_input = PersonaInput {
        system_persona: Arc::new(crate::middleware::prompt::SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Skip,
    };
    let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

    let tools_arc: Arc<Vec<openalpaca_llm::ToolDefinition>> = Arc::new(tools.clone());

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
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

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(bundle),
        query: Arc::from(pctx.description.as_str()),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::PipelineStep {
            step,
            total: pctx.total_agents,
        },
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    let recent_messages: Vec<ChatMessage> = match memory_block_text.as_deref() {
        Some(block) => vec![ChatMessage::user(block)],
        None => Vec::new(),
    };
    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(recent_messages),
        current_user_turn: Some(ChatMessage::user(&pctx.description)),
        mode: HistoryMode::Default,
    };

    let compose_request = ComposeRequest::PipelineStep {
        agent: Arc::new(agent.clone()),
        step_index: step,
        step_description: Arc::<str>::from(pctx.description.as_str()),
        scope_block: Arc::<str>::from(scope_block),
        output_block: Arc::<str>::from(format!("<output-format>\n{}\n</output-format>", output_note)),
        context_package: Arc::new(context_package),
        memory_block: memory_block_text.as_deref().map(Arc::<str>::from),
        overrides: ComposeOverrides::default(),
    };

    let composed = pctx.compose_engine.compose(
        &compose_request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        tools_arc.clone(),
        Some(&pctx.bus),
        None, // lane: per-lane cache for PipelineStep deferred (no natural lane key).
    );

    // --- Context Budget Telemetry ---
    {
        let default_model = pctx.router.default_model();
        let model_id = agent
            .llm_config
            .model
            .as_deref()
            .unwrap_or(&default_model);
        let request_id = uuid::Uuid::new_v4();
        let mut budget_snapshot =
            crate::context_budget::ContextBudgetManager::new(
                model_window,
                &pctx.daemon_config.load().execution.context,
            );
        // Register the combined static-prompt + dynamic-context tokens under
        // `system_prompt` to preserve the pre-migration budget accounting
        // (pre-migration `built.total_prompt_tokens` summed all registered
        // sections including context_bundle sections).
        let system_prompt_tokens = (composed.token_budget.static_prompt_tokens
            + composed.token_budget.dynamic_context_tokens)
            as usize;
        budget_snapshot.register_section("system_prompt", system_prompt_tokens);
        budget_snapshot.register_section("tools", tools.len() * 200);

        tracing::debug!(
            request_id = %request_id,
            agent_id = %agent_id,
            model_window,
            fixed_zone = budget_snapshot.fixed_zone_tokens(),
            free_zone = budget_snapshot.free_zone_capacity(),
            buffer = budget_snapshot.autocompact_buffer(),
            "Context budget computed (pipeline step)"
        );

        pctx.bus.publish(SystemEvent::ContextBudgetComputed {
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
            timestamp: Utc::now(),
        });
    }

    // The compose engine's Arc'd messages vec: pipeline's downstream path needs
    // ownership because run_agentic_loop_routed takes Vec<ChatMessage>. The
    // runner will mutate the vec during the agentic loop, so we clone out.
    let system_prompt = composed
        .messages
        .first()
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

    // ── Plugin agent dispatch path ──────────────────────────────────────
    // If the agent's template has AgentSource::Plugin, route to the
    // PluginAgentExecutor instead of the internal agentic loop.
    if let Some(template) = pctx.ctx.agent_registry.get_template(&agent.template_id) {
        if let crate::agent::AgentSource::Plugin { ref executor, .. } = template.source {
            tracing::info!(
                "Agent '{}' is plugin-backed — routing to PluginAgentExecutor",
                agent_id
            );

            let plugin_ctx = serde_json::json!({
                "previous_output": previous_output,
                "task_id": pctx.task_id,
            });
            let instructions = format!("{}\n\nRole: {}", system_prompt, role_description);

            let plugin_start = std::time::Instant::now();

            let accepted = match executor
                .spawn(agent_id, &pctx.task_id, &instructions, &plugin_ctx)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    let error = format!("plugin agent spawn failed: {e}");
                    tracing::error!("{}", error);
                    busy_guard.restore();
                    return PipelineStepResult {
                        success: false,
                        error: Some(error),
                        raw_content: String::new(),
                        display_content: String::new(),
                        input_tokens: 0,
                        output_tokens: 0,
                        tool_calls_made: 0,
                    };
                }
            };

            if !accepted {
                let error = "Plugin agent rejected the task".to_string();
                tracing::warn!("{}", error);
                busy_guard.restore();
                return PipelineStepResult {
                    success: false,
                    error: Some(error),
                    raw_content: String::new(),
                    display_content: String::new(),
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_calls_made: 0,
                };
            }

            // Step loop: poll the plugin agent, proxying tool requests
            const MAX_PLUGIN_ITERATIONS: usize = 50;
            let mut tool_results: Option<serde_json::Value> = None;
            let mut total_tool_calls: usize = 0;

            for iteration in 0..MAX_PLUGIN_ITERATIONS {
                let (status, output, tool_calls) = match executor
                    .step(agent_id, tool_results.as_ref())
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        let error = format!("plugin agent step failed: {e}");
                        tracing::error!("{}", error);
                        let _ = executor.stop(agent_id).await;
                        busy_guard.restore();
                        return PipelineStepResult {
                            success: false,
                            error: Some(error),
                            raw_content: String::new(),
                            display_content: String::new(),
                            input_tokens: 0,
                            output_tokens: 0,
                            tool_calls_made: total_tool_calls,
                        };
                    }
                };

                match status.as_str() {
                    "complete" => {
                        let runtime = plugin_start.elapsed().as_secs() as i64;
                        tracing::info!(
                            "Plugin agent '{}' completed in {} iterations ({}s)",
                            agent_id,
                            iteration + 1,
                            runtime,
                        );
                        busy_guard.restore();
                        return PipelineStepResult {
                            success: true,
                            error: None,
                            raw_content: output.clone(),
                            display_content: if output.is_empty() {
                                format!(
                                    "Plugin agent completed in {} iterations ({} tool calls)",
                                    iteration + 1,
                                    total_tool_calls,
                                )
                            } else {
                                output
                            },
                            input_tokens: 0,
                            output_tokens: 0,
                            tool_calls_made: total_tool_calls,
                        };
                    }
                    "failed" => {
                        tracing::error!("Plugin agent '{}' failed: {}", agent_id, output);
                        busy_guard.restore();
                        return PipelineStepResult {
                            success: false,
                            error: Some(output),
                            raw_content: String::new(),
                            display_content: String::new(),
                            input_tokens: 0,
                            output_tokens: 0,
                            tool_calls_made: total_tool_calls,
                        };
                    }
                    "tool_request" => {
                        let mut results = Vec::new();
                        for call in &tool_calls {
                            let name = call
                                .get("tool")
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let args = call
                                .get("arguments")
                                .cloned()
                                .unwrap_or_default();
                            let result = pctx
                                .tool_registry
                                .execute_with_context(name, &args, &tool_ctx)
                                .await;
                            results.push(serde_json::json!({
                                "tool": name,
                                "result": result.unwrap_or_else(|e| e),
                            }));
                            total_tool_calls += 1;
                        }
                        tool_results = Some(serde_json::json!(results));
                    }
                    _ => {
                        // "working" — wait briefly then poll again, respecting cancellation
                        tool_results = None;
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                            _ = pctx.cancel_token.cancelled() => {
                                tracing::info!(agent_id, "plugin agent cancelled during poll");
                                let _ = executor.stop(agent_id).await;
                                busy_guard.restore();
                                return PipelineStepResult {
                                    success: false,
                                    error: Some("Cancelled".to_string()),
                                    raw_content: String::new(),
                                    display_content: String::new(),
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    tool_calls_made: total_tool_calls,
                                };
                            }
                        }
                    }
                }
            }

            // Exceeded max iterations
            let error =
                format!("Plugin agent '{}' exceeded max iterations ({MAX_PLUGIN_ITERATIONS})", agent_id);
            tracing::error!("{}", error);
            let _ = executor.stop(agent_id).await;
            busy_guard.restore();
            return PipelineStepResult {
                success: false,
                error: Some(error),
                raw_content: String::new(),
                display_content: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                tool_calls_made: total_tool_calls,
            };
        }
    }

    // ── Internal agent: run agentic loop ────────────────────────────────
    let agent_start = std::time::Instant::now();
    let result = run_agentic_loop_routed(
        pctx.router.as_ref(),
        messages,
        tools,
        &loop_config,
        Some(&per_request_sandbox),
        agent_id,
        Some(&sandbox_policy),
        Some(&pctx.task_id),
        Some(&context_budget),
        Some(pctx.cancel_token.clone()),
        Some(&tool_ctx),
        None,
    )
    .await;

    let agent_elapsed = agent_start.elapsed();
    let agent_runtime = agent_elapsed.as_secs() as i64;

    tracing::info!(
        "Agent '{}' finished step {}/{}: reason={:?}, rounds={}, tokens={}/{}",
        agent_id,
        step + 1,
        pctx.total_agents,
        result.finish_reason,
        result.rounds_used,
        result.total_input_tokens,
        result.total_output_tokens
    );

    let agent_success = matches!(
        &result.finish_reason,
        LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated
    );

    // Persist LLM usage to DB and emit event (regardless of success/failure)
    usage::record_llm_usage(
        &pctx.router,
        &result,
        loop_config.model.as_deref(),
        agent_id,
        &pctx.task_id,
        agent_elapsed.as_millis() as i64,
        pctx.db.as_ref(),
        &pctx.bus,
    );

    // Assignment -> Completed or Failed
    if let (Some(db), Some(assign_id)) = (&pctx.db, assignment_id) {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let status = if agent_success {
            openalpaca_storage::AssignmentStatus::Completed
        } else {
            openalpaca_storage::AssignmentStatus::Failed
        };
        let _ = repo.update_assignment_status(assign_id, status);
    }

    // Persist per-agent output to DB
    if let (Some(db), Some(assign_id)) = (&pctx.db, assignment_id) {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let output = result.final_content.chars().take(5000).collect::<String>();
        let _ = repo.set_assignment_output(assign_id, &output);
    }

    // Record per-agent history and metrics
    if let Some(ref db) = pctx.db {
        usage::record_agent_history(
            db,
            &agent.template_id,
            &pctx.task_id,
            "executor",
            agent_success,
            agent_runtime,
        );
    }

    // Release this agent back to Idle (explicit; guard is backup for panics)
    busy_guard.restore();

    if agent_success {
        let raw_content = result.final_content.clone();

        // For display/DB: synthetic summary if agent produced no text
        let display_content = if raw_content.is_empty() {
            format!(
                "Agent completed in {} rounds ({} tool calls, {} tokens used)",
                result.rounds_used,
                result.tool_calls_made,
                result.total_input_tokens + result.total_output_tokens
            )
        } else {
            raw_content.clone()
        };

        // Update state_json: mark step completed + auto-write output to workspace (with retry)
        if let Some(ref db) = pctx.db {
            let step_i = step as i32;
            let raw_clone = raw_content.clone();
            let aid = agent_id.clone();
            if !update_state_with_retry(
                db,
                &pctx.task_id,
                move |state| {
                    let summary: String = raw_clone.chars().take(500).collect();
                    state.mark_step_completed(step_i, &summary);
                    if !raw_clone.is_empty() {
                        let ws_key = format!("step_{}_output", step_i);
                        if let Err(e) = state.workspace.write(
                            &ws_key,
                            &raw_clone,
                            &aid,
                            crate::orchestrator::task_state::WorkspaceEntryType::Context,
                            &[],
                        ) {
                            tracing::warn!(
                                "Failed to auto-write step {} output to workspace: {}",
                                step_i,
                                e
                            );
                        }
                    }
                    state.scan_workspace_artifacts(step_i);
                },
                "mark_step_completed",
            )
            .await
            {
                tracing::error!(
                    "Failed to persist mark_step_completed for task '{}'",
                    pctx.task_id
                );
            }
        }

        // Emit progress event showing step completed (step + 1 = "N+1 steps done")
        pctx.bus.publish(SystemEvent::TaskUpdated {
            task_id: pctx.task_id.clone(),
            status: "running".to_string(),
            progress_current: Some((step + 1) as i32),
            progress_total: Some(pctx.total_agents as i32),
            timestamp: Utc::now(),
        });
        if let Some(ref db) = pctx.db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            let _ = repo.update_progress(
                &pctx.task_id,
                (step + 1) as i32,
                pctx.total_agents as i32,
            );
        }
        pctx.ctx.task_registry.update_progress(
            &pctx.task_id,
            (step + 1) as i32,
            pctx.total_agents as i32,
            None,
        );

        PipelineStepResult {
            success: true,
            error: None,
            raw_content,
            display_content,
            input_tokens: result.total_input_tokens,
            output_tokens: result.total_output_tokens,
            tool_calls_made: result.tool_calls_made,
        }
    } else {
        // Update state_json: mark step failed (with retry)
        if let Some(ref db) = pctx.db {
            let step_i = step as i32;
            let error_msg = match &result.finish_reason {
                LoopFinishReason::CostExceeded => "Agent cost limit exceeded".to_string(),
                LoopFinishReason::Error(err) => err.clone(),
                _ => "Agent failed".to_string(),
            };
            if !update_state_with_retry(
                db,
                &pctx.task_id,
                |s| s.mark_step_failed(step_i, &error_msg),
                "mark_step_failed",
            )
            .await
            {
                tracing::error!(
                    "Failed to persist mark_step_failed for task '{}'",
                    pctx.task_id
                );
            }
        }

        let error = match &result.finish_reason {
            LoopFinishReason::CostExceeded => "Agent cost limit exceeded".to_string(),
            LoopFinishReason::Error(err) => err.clone(),
            _ => "Agent failed".to_string(),
        };

        PipelineStepResult {
            success: false,
            error: Some(error),
            raw_content: String::new(),
            display_content: String::new(),
            input_tokens: result.total_input_tokens,
            output_tokens: result.total_output_tokens,
            tool_calls_made: result.tool_calls_made,
        }
    }
}
