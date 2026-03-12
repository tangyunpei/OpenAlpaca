//! Per-step execution logic for sequential pipeline: sandbox creation, prompt building,
//! LLM call, progress tracking, and state persistence.

use super::outcome::update_state_with_retry;
use super::usage;
use super::retrieve_memory_block;
use crate::agent::subagent::{AgentStatus, SubAgent};
use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::middleware::prompt::format_tool_guidance;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::{ContextualToolExecutor, ToolExecutionContext, ToolRegistry};
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

    // Per-step sandbox with ContextualToolExecutor scoped to this agent.
    let ctx_exec = ToolExecutionContext {
        owner_id: Some(pctx.created_by.clone()),
        task_id: Some(pctx.task_id.clone()),
        agent_id: Some(agent_id.clone()),
        db: pctx.db.clone(),
        workspace_id: pctx.workspace_id.clone(),
    };
    let contextual_executor =
        Arc::new(ContextualToolExecutor::new(pctx.tool_registry.clone(), ctx_exec));
    let mut per_request_sandbox =
        SandboxManager::with_defaults(contextual_executor, pctx.bus.clone());
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

    // Assignment → Running
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

    // Build LoopConfig — agent constraints override daemon defaults
    let mut loop_config = LoopConfig::from_agent(
        &pctx.daemon_config.load().execution.agent_defaults,
        agent,
    )
    .with_context_window(
        pctx.router.model_registry(),
        agent.llm_config.model.as_deref(),
    );

    // Set compaction model from daemon config
    loop_config.compaction_model = pctx.daemon_config.load()
        .execution.context.compaction_model.clone();

    // Instantiate ContextBudgetManager for budget-aware compaction
    let context_budget = {
        let default_model = pctx.router.default_model();
        let model_id = agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let context_window = pctx.router.model_registry()
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
        agent.capabilities.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    // Build system prompt with role description and tool awareness
    let tool_guidance = format_tool_guidance(&tools);
    let is_last_step = step == pctx.total_agents - 1;
    let output_note = if is_last_step {
        "Your output is the final result delivered to the user. Make it complete and polished."
    } else {
        "Your output will be passed to the next agent in the pipeline. Be thorough and \
         include all relevant details so the next agent can build on your work."
    };
    let connector_suffix = if !pctx.connector_block.is_empty() {
        format!("\n{}", pctx.connector_block)
    } else {
        String::new()
    };
    let mut system_prompt = format!(
        "<identity>\n{}\n</identity>\n\n\
         <assignment>\n\
         Role: {}\n\
         Pipeline step: {} of {}\n\
         </assignment>\n\n\
         <scope>\n\
         Focus on completing your assigned role. Do not attempt work outside your role description.\n\
         </scope>\n\n\
         <output-format>\n\
         {}\n\
         </output-format>{}{}",
        agent.preset.persona,
        role_description,
        step + 1,
        pctx.total_agents,
        output_note,
        tool_guidance,
        connector_suffix,
    );

    // --- Context Package (Phase C) ---
    {
        let denied_sections = agent.constraints.denied_sections.clone();

        let mut pkg_builder = crate::context_budget::ContextPackageBuilder::new(
            role_description.to_string(),
        );

        // Add workspace artifacts from previous_output
        if let Some(ref output) = *previous_output {
            pkg_builder = pkg_builder.workspace_artifact(output.clone());
        }

        // Add cached workspace context as artifact if non-empty
        if !cached_workspace_context.is_empty() {
            pkg_builder = pkg_builder.workspace_artifact(cached_workspace_context.to_string());
        }

        // Apply denied_sections from agent constraints
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
        pctx.bus.publish(crate::events::SystemEvent::ContextPackageBuilt {
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

    // Build messages: system + task + workspace context
    let mut messages = vec![ChatMessage::system(&system_prompt)];

    // Inject memory context for the first agent in the pipeline
    if step == 0
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
        if let Some(block) = retrieve_memory_block(
            db,
            pctx.embedder.as_ref(),
            &pctx.created_by,
            &pctx.description,
            5,
            scope_ctx.as_ref(),
            access_boost,
        )
        .await
        {
            messages.push(ChatMessage::user(&block));
        }
    }

    messages.push(ChatMessage::user(&pctx.description));

    // Inject shared workspace context (supplements previous_output for backward compat)
    if !cached_workspace_context.is_empty() {
        messages.push(ChatMessage::user(&format!(
            "<workspace>\n\
             Results from previous pipeline agents. Use this data to complete your role. \
             You can also call workspace_read and workspace_write to access or update entries.\n\n\
             {}\n\
             </workspace>",
            cached_workspace_context
        )));
    } else if let Some(prev) = previous_output {
        // Backward compat: if workspace is empty but previous_output exists
        messages.push(ChatMessage::user(&format!(
            "<previous-agent-output>\n\
             The previous agent in the pipeline produced this result. \
             Build on this output to complete your role.\n\n\
             {}\n\
             </previous-agent-output>",
            prev
        )));
    }

    // Run agentic loop for this agent
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
    )
    .await;

    let agent_runtime = agent_start.elapsed().as_secs() as i64;

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
        agent_start.elapsed().as_millis() as i64,
        pctx.db.as_ref(),
        &pctx.bus,
    );

    // Assignment → Completed or Failed
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
