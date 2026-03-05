use super::super::task_state::TaskState;
use super::usage;
use super::{
    TaskDispatcher, finalize_task_with_outcome, format_task_result, persist_conversation,
    retrieve_memory_block, spawn_task_memory_extraction,
};
use crate::agent::registry::DestroyOutcome;
use crate::agent::subagent::{AgentStatus, SubAgent};
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use crate::middleware::prompt::format_tool_guidance;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use chrono::Utc;
use openalpaca_llm::ChatMessage;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use super::update_state_with_retry;

/// Fetch the current workspace context string from the task's state in SQLite.
/// Returns an empty string if the DB is absent, the task doesn't exist, or parsing fails.
fn fetch_workspace_context(db: Option<&openalpaca_storage::Database>, task_id: &str) -> String {
    let Some(db) = db else {
        return String::new();
    };
    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    match repo.get(task_id) {
        Ok(Some(existing)) => existing
            .state_json
            .as_deref()
            .and_then(|sj| serde_json::from_str::<TaskState>(sj).ok())
            .map(|state| state.workspace.format_for_prompt(&[]))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

impl TaskDispatcher {
    /// Spawn a sequential pipeline: agents run in step_order, each receiving
    /// the previous agent's output as additional context.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_agent_pipeline(
        &self,
        task_id: String,
        task_title: String,
        description: String,
        agents_with_assignments: Vec<(SubAgent, Option<String>, String)>,
        lane_key: String,
        source: String,
        created_by: String,
        workspace_id: Option<String>,
    ) {
        let Some(router) = self.require_router(&task_id) else {
            return;
        };

        let bus = self.bus.clone();
        let ctx = self.shared_context.clone();
        let db = self.db.clone();
        let embedder = self.embedder.clone();
        let tool_registry = self.tool_registry.clone();
        let daemon_config = self.daemon_config.clone();
        let connector_block = self.connector_guidance_block();

        // Create cancellation token for this task
        let cancel_token = CancellationToken::new();
        ctx.register_cancellation_token(&task_id, cancel_token.clone());

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let total_agents = agents_with_assignments.len();

            // 1. Update task status → Running
            ctx.task_registry
                .update_status(&task_id, TaskEntryStatus::Running);
            bus.publish(SystemEvent::TaskUpdated {
                task_id: task_id.clone(),
                status: "running".to_string(),
                progress_current: Some(0),
                progress_total: Some(total_agents as i32),
                timestamp: Utc::now(),
            });
            if let Some(ref db) = db {
                let repo = openalpaca_storage::repository::TaskRepository::new(db);
                let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Running);
                let _ = repo.update_progress(&task_id, 0, total_agents as i32);
            }

            // Set initial pipeline progress in TaskRegistry (no DAG summary for sequential)
            ctx.task_registry
                .update_progress(&task_id, 0, total_agents as i32, None);

            // 2. Run agents sequentially — each receives the previous agent's output
            let mut previous_output: Option<String> = None;
            let mut pipeline_success = true;
            let mut pipeline_error: Option<String> = None;
            let mut final_content = String::new();
            let mut last_processed_step = 0usize;
            let mut total_input_tokens: u32 = 0;
            let mut total_output_tokens: u32 = 0;

            // Cache workspace context to avoid re-fetching from SQLite on every step.
            // Refreshed after each step completes (agent may have written to workspace).
            let mut cached_workspace_context = fetch_workspace_context(db.as_ref(), &task_id);

            for (step, (agent, assignment_id, role_description)) in
                agents_with_assignments.iter().enumerate()
            {
                // Check cancellation before starting each pipeline step
                if cancel_token.is_cancelled() {
                    tracing::info!(
                        task_id = %task_id,
                        step = step + 1,
                        total = total_agents,
                        "Pipeline cancelled before step"
                    );
                    pipeline_success = false;
                    pipeline_error = Some("Pipeline cancelled by user".to_string());
                    break;
                }

                last_processed_step = step;
                let agent_id = &agent.id;

                // Per-step sandbox with ContextualToolExecutor scoped to this agent.
                // Created inside the loop so each step gets the correct agent_id for
                // workspace attribution instead of "unknown".
                let ctx_exec = ToolExecutionContext {
                    owner_id: Some(created_by.clone()),
                    task_id: Some(task_id.clone()),
                    agent_id: Some(agent_id.clone()),
                    db: db.clone(),
                    workspace_id: workspace_id.clone(),
                };
                let contextual_executor =
                    Arc::new(ContextualToolExecutor::new(tool_registry.clone(), ctx_exec));
                let per_request_sandbox =
                    SandboxManager::with_defaults(contextual_executor, bus.clone());

                tracing::info!(
                    "Pipeline step {}/{}: agent '{}' starting on task '{}'",
                    step + 1,
                    total_agents,
                    agent_id,
                    task_id
                );

                // Mark agent as Busy (with RAII guard for panic safety)
                ctx.agent_registry.update_status(
                    agent_id,
                    AgentStatus::Busy {
                        task_id: task_id.clone(),
                    },
                );
                bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: agent_id.clone(),
                    instance_id: agent_id.clone(),
                    template_id: agent.template_id.clone(),
                    status: "busy".to_string(),
                    current_task_id: Some(task_id.clone()),
                    timestamp: Utc::now(),
                });
                let mut busy_guard = crate::runner::lead_agent::AgentBusyGuard::new(
                    agent_id.clone(),
                    agent.template_id.clone(),
                    ctx.agent_registry.clone(),
                    bus.clone(),
                );

                // Assignment → Running
                if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_assignment_status(
                        assign_id,
                        openalpaca_storage::AssignmentStatus::Running,
                    );
                }

                // Update state_json: mark step running + fix agent_id from template
                // to actual instance_id (non-singleton IDs are "template::uuid").
                if let Some(ref db) = db {
                    let step_i = step as i32;
                    let instance_id = agent_id.clone();
                    if !update_state_with_retry(
                        db,
                        &task_id,
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
                        tracing::error!("Failed to persist mark_step_running for task '{}'", task_id);
                    }
                }

                // Emit progress event for this step.
                // Use `step` (0-indexed) as progress_current, meaning "N steps completed so far".
                // This avoids showing "3/3" before the last agent even starts.
                bus.publish(SystemEvent::TaskUpdated {
                    task_id: task_id.clone(),
                    status: "running".to_string(),
                    progress_current: Some(step as i32),
                    progress_total: Some(total_agents as i32),
                    timestamp: Utc::now(),
                });
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_progress(&task_id, step as i32, total_agents as i32);
                }

                // Build LoopConfig — agent constraints override daemon defaults
                let loop_config =
                    LoopConfig::from_agent(&daemon_config.load().execution.agent_defaults, agent)
                        .with_context_window(
                            router.model_registry(),
                            agent.llm_config.model.as_deref(),
                        );

                let sandbox_policy = SandboxPolicy::from_constraints(agent_id, &agent.constraints);

                // Resolve tools via shared helper
                let tools = crate::tools::resolve_agent_tools(agent, &tool_registry);
                tracing::info!(
                    "Agent '{}' loaded {} tool definitions for skills: {:?}",
                    agent_id,
                    tools.len(),
                    agent.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
                );

                // Build system prompt with role description and tool awareness
                let tool_guidance = format_tool_guidance(&tools);
                let is_last_step = step == total_agents - 1;
                let output_note = if is_last_step {
                    "Your output is the final result delivered to the user. Make it complete and polished."
                } else {
                    "Your output will be passed to the next agent in the pipeline. Be thorough and \
                     include all relevant details so the next agent can build on your work."
                };
                let connector_suffix = if !connector_block.is_empty() {
                    format!("\n{}", connector_block)
                } else {
                    String::new()
                };
                let system_prompt = format!(
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
                    total_agents,
                    output_note,
                    tool_guidance,
                    connector_suffix,
                );

                // Build messages: system + task + workspace context
                let mut messages = vec![ChatMessage::system(&system_prompt)];

                // Inject memory context for the first agent in the pipeline
                if step == 0
                    && let Some(ref db) = db
                {
                    let scope_ctx = workspace_id.as_ref().map(|ws| {
                        crate::memory::scope_context::MemoryScopeContext::new(Some(ws.clone()))
                    });
                    let access_boost = daemon_config.load().orchestrator.memory.decay.access_boost;
                    if let Some(block) = retrieve_memory_block(
                        db,
                        embedder.as_ref(),
                        &created_by,
                        &description,
                        5,
                        scope_ctx.as_ref(),
                        access_boost,
                    )
                    .await
                    {
                        messages.push(ChatMessage::user(&block));
                    }
                }

                messages.push(ChatMessage::user(&description));

                // Inject shared workspace context (supplements previous_output for backward compat)
                // Uses cached value to avoid redundant SQLite + JSON deserialization per step.
                let workspace_context = &cached_workspace_context;

                if !workspace_context.is_empty() {
                    messages.push(ChatMessage::user(&format!(
                        "<workspace>\n\
                         Results from previous pipeline agents. Use this data to complete your role. \
                         You can also call workspace_read and workspace_write to access or update entries.\n\n\
                         {}\n\
                         </workspace>",
                        workspace_context
                    )));
                } else if let Some(ref prev) = previous_output {
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
                    router.as_ref(),
                    messages,
                    tools,
                    &loop_config,
                    Some(&per_request_sandbox),
                    agent_id,
                    Some(&sandbox_policy),
                    Some(&task_id),
                    Some(cancel_token.clone()),
                )
                .await;

                let agent_runtime = agent_start.elapsed().as_secs() as i64;

                tracing::info!(
                    "Agent '{}' finished step {}/{}: reason={:?}, rounds={}, tokens={}/{}",
                    agent_id,
                    step + 1,
                    total_agents,
                    result.finish_reason,
                    result.rounds_used,
                    result.total_input_tokens,
                    result.total_output_tokens
                );

                let agent_success = matches!(
                    &result.finish_reason,
                    LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated
                );

                // Accumulate token metrics
                total_input_tokens += result.total_input_tokens;
                total_output_tokens += result.total_output_tokens;

                // Persist LLM usage to DB and emit event (regardless of success/failure)
                usage::record_llm_usage(
                    &router,
                    &result,
                    loop_config.model.as_deref(),
                    agent_id,
                    &task_id,
                    agent_start.elapsed().as_millis() as i64,
                    db.as_ref(),
                    &bus,
                );

                // Assignment → Completed or Failed
                if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let status = if agent_success {
                        openalpaca_storage::AssignmentStatus::Completed
                    } else {
                        openalpaca_storage::AssignmentStatus::Failed
                    };
                    let _ = repo.update_assignment_status(assign_id, status);
                }

                // Persist per-agent output to DB
                if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let output = result.final_content.chars().take(5000).collect::<String>();
                    let _ = repo.set_assignment_output(assign_id, &output);
                }

                // Record per-agent history and metrics
                // Use template_id for DB operations — the agent table stores template IDs,
                // not instance IDs (e.g., "general_agent" not "general_agent::69dc734d").
                if let Some(ref db) = db {
                    usage::record_agent_history(
                        db,
                        &agent.template_id,
                        &task_id,
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
                    if let Some(ref db) = db {
                        let step_i = step as i32;
                        let raw_clone = raw_content.clone();
                        let aid = agent_id.clone();
                        if !update_state_with_retry(db, &task_id, move |state| {
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
                                    tracing::warn!("Failed to auto-write step {} output to workspace: {}", step_i, e);
                                }
                            }
                            state.scan_workspace_artifacts(step_i);
                        }, "mark_step_completed").await {
                            tracing::error!("Failed to persist mark_step_completed for task '{}'", task_id);
                        }
                    }

                    // Emit progress event showing step completed (step + 1 = "N+1 steps done")
                    bus.publish(SystemEvent::TaskUpdated {
                        task_id: task_id.clone(),
                        status: "running".to_string(),
                        progress_current: Some((step + 1) as i32),
                        progress_total: Some(total_agents as i32),
                        timestamp: Utc::now(),
                    });
                    if let Some(ref db) = db {
                        let repo = openalpaca_storage::repository::TaskRepository::new(db);
                        let _ =
                            repo.update_progress(&task_id, (step + 1) as i32, total_agents as i32);
                    }
                    ctx.task_registry.update_progress(
                        &task_id,
                        (step + 1) as i32,
                        total_agents as i32,
                        None,
                    );

                    // Refresh cached workspace context only if the agent made tool calls
                    // (conservative: assumes any tool call may have written to workspace).
                    // Steps with no tool calls (pure LLM reasoning) skip the DB round-trip.
                    if result.tool_calls_made > 0 {
                        cached_workspace_context = fetch_workspace_context(db.as_ref(), &task_id);
                    }

                    // Only pass actual content to next agent (not synthetic metadata)
                    if !raw_content.is_empty() {
                        previous_output = Some(raw_content);
                    }
                    // If empty, previous_output stays as-is from prior step

                    final_content = display_content;
                } else {
                    // Update state_json: mark step failed (with retry)
                    if let Some(ref db) = db {
                        let step_i = step as i32;
                        let error_msg = match &result.finish_reason {
                            LoopFinishReason::CostExceeded => {
                                "Agent cost limit exceeded".to_string()
                            }
                            LoopFinishReason::Error(err) => err.clone(),
                            _ => "Agent failed".to_string(),
                        };
                        if !update_state_with_retry(
                            db,
                            &task_id,
                            |s| s.mark_step_failed(step_i, &error_msg),
                            "mark_step_failed",
                        )
                        .await
                        {
                            tracing::error!("Failed to persist mark_step_failed for task '{}'", task_id);
                        }
                    }

                    pipeline_success = false;
                    pipeline_error = Some(match &result.finish_reason {
                        LoopFinishReason::CostExceeded => "Agent cost limit exceeded".to_string(),
                        LoopFinishReason::Error(err) => err.clone(),
                        _ => "Agent failed".to_string(),
                    });
                    break;
                }
            }

            // Cleanup cancellation token
            ctx.remove_cancellation_token(&task_id);

            // 3. Destroy remaining agent instances that never ran (pipeline broke early)
            let now = Utc::now();
            if !pipeline_success {
                for (step, (agent, assignment_id, _)) in agents_with_assignments.iter().enumerate()
                {
                    if step > last_processed_step {
                        let outcome = ctx.agent_registry.destroy_instance(&agent.id);
                        let status = match outcome {
                            DestroyOutcome::ResetToIdle => "idle",
                            _ => "destroyed",
                        };
                        if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                            let repo = openalpaca_storage::repository::TaskRepository::new(db);
                            let _ = repo.update_assignment_status(
                                assign_id,
                                openalpaca_storage::AssignmentStatus::Failed,
                            );
                        }
                        bus.publish(SystemEvent::AgentStatusChanged {
                            agent_id: agent.id.clone(),
                            instance_id: agent.id.clone(),
                            template_id: agent.template_id.clone(),
                            status: status.to_string(),
                            current_task_id: None,
                            timestamp: now,
                        });
                    }
                }
            }

            // 4. Persist final result to conversation (single message for entire pipeline)
            let runtime_secs = start_time.elapsed().as_secs() as i64;
            // Clone for memory extraction before final_content is consumed
            let extraction_content = if pipeline_success {
                Some(final_content.clone())
            } else {
                None
            };
            if let Some(ref db) = db {
                let chat_text = if pipeline_success {
                    final_content.clone()
                } else {
                    pipeline_error.clone().unwrap_or_default()
                };
                let content = format_task_result(&task_title, &chat_text, pipeline_success);
                persist_conversation(
                    db,
                    &lane_key,
                    &source,
                    content,
                    None,
                    total_input_tokens as i64,
                    total_output_tokens as i64,
                    runtime_secs,
                );
            }

            // 5. Build structured outcome + update task status
            let outcome_content = if pipeline_success {
                &final_content
            } else {
                pipeline_error.as_deref().unwrap_or_default()
            };
            finalize_task_with_outcome(
                &ctx,
                &bus,
                db.as_ref(),
                &task_id,
                outcome_content,
                pipeline_success,
            );

            // Memory extraction from pipeline output (non-blocking)
            if let (Some(db), Some(output)) = (&db, &extraction_content) {
                spawn_task_memory_extraction(
                    db,
                    &router,
                    &embedder,
                    &daemon_config,
                    &created_by,
                    &task_id,
                    &description,
                    output,
                    "pipeline",
                    pipeline_success,
                    workspace_id.clone(),
                );
            }
        });
    }
}
