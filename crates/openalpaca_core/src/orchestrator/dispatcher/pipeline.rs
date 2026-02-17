use super::{TaskDispatcher, format_task_result, retrieve_memory_block, spawn_task_memory_extraction};
use crate::agent::subagent::{AgentStatus, SubAgent};
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use crate::middleware::prompt::format_tool_guidance;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use chrono::Utc;
use openalpaca_llm::ChatMessage;
use openalpaca_storage::repository::LlmUsageRepository;
use openalpaca_storage::{ConversationMessage, ConversationRepository};
use std::sync::Arc;
use std::time::Duration;
use super::super::task_state::TaskState;
use uuid::Uuid;

impl TaskDispatcher {
    /// Spawn a sequential pipeline: agents run in step_order, each receiving
    /// the previous agent's output as additional context.
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
        let router = match &self.llm_router {
            Some(r) => r.clone(),
            None => {
                tracing::warn!(
                    "No LLM router configured — cannot execute pipeline for task '{}'",
                    task_id
                );
                return;
            }
        };

        let bus = self.bus.clone();
        let ctx = self.shared_context.clone();
        let db = self.db.clone();
        let embedder = self.embedder.clone();
        let tool_registry = self.tool_registry.clone();
        let daemon_config = self.daemon_config.clone();

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let total_agents = agents_with_assignments.len();

            // 1. Update task status → Running
            ctx.task_registry.update_status(&task_id, TaskEntryStatus::Running);
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
            ctx.task_registry.update_progress(
                &task_id,
                0,
                total_agents as i32,
                None,
            );

            // 2. Run agents sequentially — each receives the previous agent's output
            let mut previous_output: Option<String> = None;
            let mut pipeline_success = true;
            let mut pipeline_error: Option<String> = None;
            let mut final_content = String::new();
            let mut last_processed_step = 0usize;
            let mut total_input_tokens: u32 = 0;
            let mut total_output_tokens: u32 = 0;

            for (step, (agent, assignment_id, role_description)) in agents_with_assignments.iter().enumerate() {
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
                };
                let contextual_executor = Arc::new(ContextualToolExecutor::new(
                    tool_registry.clone(), ctx_exec,
                ));
                let per_request_sandbox = SandboxManager::with_defaults(contextual_executor, bus.clone());

                tracing::info!(
                    "Pipeline step {}/{}: agent '{}' starting on task '{}'",
                    step + 1, total_agents, agent_id, task_id
                );

                // Assignment → Running
                if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_assignment_status(
                        assign_id,
                        openalpaca_storage::AssignmentStatus::Running,
                    );
                }

                // Update state_json: mark step running
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    if let Ok(Some(existing)) = repo.get(&task_id) {
                        if let Some(ref sj) = existing.state_json {
                            if let Ok(mut state) = serde_json::from_str::<TaskState>(sj) {
                                state.mark_step_running(step as i32);
                                let _ = repo.update_state(&task_id, &state.to_json(), existing.state_version);
                            }
                        }
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
                let ad = &daemon_config.load().execution.agent_defaults;
                let loop_config = LoopConfig {
                    max_rounds: ad.max_rounds,
                    max_tools_per_round: ad.max_tools_per_round,
                    max_tool_runtime: Duration::from_secs(
                        agent.constraints.timeout_seconds.unwrap_or(ad.max_tool_runtime_secs),
                    ),
                    max_cost: agent.constraints.max_cost_per_task.unwrap_or(ad.max_cost),
                    model: agent.llm_config.model.clone(),
                    fallback_models: agent.llm_config.fallback_models.clone(),
                };

                let sandbox_policy =
                    SandboxPolicy::from_constraints(agent_id, &agent.constraints);

                // Resolve tools via shared helper
                let tools = crate::tools::resolve_agent_tools(&agent, &tool_registry);
                tracing::info!(
                    "Agent '{}' loaded {} tool definitions for skills: {:?}",
                    agent_id,
                    tools.len(),
                    agent.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
                );

                // Build system prompt with role description and tool awareness
                let tool_guidance = format_tool_guidance(&tools);
                let system_prompt = format!(
                    "{}\n\nYour role: {}\n\nComplete your assigned role to the best of your ability.{}",
                    agent.preset.persona, role_description, tool_guidance
                );

                // Build messages: system + task + workspace context
                let mut messages = vec![
                    ChatMessage::system(&system_prompt),
                ];

                // Inject memory context for the first agent in the pipeline
                if step == 0 {
                    if let Some(ref db) = db {
                        let scope_ctx = workspace_id.as_ref().map(|ws| {
                            crate::memory::scope_context::MemoryScopeContext::new(Some(ws.clone()))
                        });
                        let access_boost = daemon_config.load().orchestrator.memory.decay.access_boost;
                        if let Some(block) = retrieve_memory_block(
                            db, embedder.as_ref(), &created_by, &description, 5, scope_ctx.as_ref(), access_boost,
                        ).await {
                            messages.push(ChatMessage::system(&block));
                        }
                    }
                }

                messages.push(ChatMessage::user(&description));

                // Inject shared workspace context (supplements previous_output for backward compat)
                // Load current workspace from TaskState
                let workspace_context = if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    if let Ok(Some(existing)) = repo.get(&task_id) {
                        if let Some(ref sj) = existing.state_json {
                            if let Ok(state) = serde_json::from_str::<TaskState>(sj) {
                                state.workspace.format_for_prompt(&[])
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                if !workspace_context.is_empty() {
                    messages.push(ChatMessage::user(&format!(
                        "The following shared workspace contains results from previous agents. \
                         Use this information to complete your role. You can also use the \
                         workspace_read and workspace_write tools to access or update entries.\n\n{}",
                        workspace_context
                    )));
                } else if let Some(ref prev) = previous_output {
                    // Backward compat: if workspace is empty but previous_output exists
                    messages.push(ChatMessage::user(&format!(
                        "## Previous Agent Output\n\
                         The previous agent produced the following result. \
                         Use this information to complete your role:\n\n{}",
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
                )
                .await;

                let agent_runtime = agent_start.elapsed().as_secs() as i64;
                let now = Utc::now();

                tracing::info!(
                    "Agent '{}' finished step {}/{}: reason={:?}, rounds={}, tokens={}/{}",
                    agent_id, step + 1, total_agents, result.finish_reason,
                    result.rounds_used, result.total_input_tokens, result.total_output_tokens
                );

                let agent_success = matches!(
                    &result.finish_reason,
                    LoopFinishReason::Complete | LoopFinishReason::MaxRounds
                );

                // Accumulate token metrics
                total_input_tokens += result.total_input_tokens;
                total_output_tokens += result.total_output_tokens;

                // Persist LLM usage to DB and emit event (regardless of success/failure)
                let default_model = router.default_model();
                let actual_model = result.model_used.as_deref()
                    .or(loop_config.model.as_deref())
                    .unwrap_or(&default_model);
                let resolved_provider = router.model_registry()
                    .resolve_provider(actual_model)
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let call_cost = router.cost_tracker.calculate_cost(
                    actual_model,
                    result.total_input_tokens,
                    result.total_output_tokens,
                );
                let call_latency_ms = agent_start.elapsed().as_millis() as i64;

                let call_status = match &result.finish_reason {
                    LoopFinishReason::Complete | LoopFinishReason::MaxRounds => "success",
                    LoopFinishReason::CostExceeded => "cost_exceeded",
                    LoopFinishReason::Error(_) => "error",
                };
                let call_error = match &result.finish_reason {
                    LoopFinishReason::Error(msg) => Some(msg.as_str()),
                    _ => None,
                };

                if let Some(ref db) = db {
                    let usage_repo = LlmUsageRepository::new(db);
                    if let Err(e) = usage_repo.record_and_log(
                        agent_id,
                        Some(&task_id),
                        &resolved_provider,
                        actual_model,
                        result.total_input_tokens as i32,
                        result.total_output_tokens as i32,
                        call_cost,
                        call_latency_ms,
                        call_status,
                        call_error,
                    ) {
                        tracing::warn!("Failed to persist LLM usage: {e}");
                    }
                }

                bus.publish(SystemEvent::LlmCallCompleted {
                    agent_id: agent_id.clone(),
                    model: actual_model.to_string(),
                    input_tokens: result.total_input_tokens,
                    output_tokens: result.total_output_tokens,
                    cost_usd: call_cost,
                    timestamp: Utc::now(),
                });

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
                if let Some(ref db) = db {
                    let subagent_repo = openalpaca_storage::SubAgentRepository::new(db);
                    let history_entry = openalpaca_storage::AgentTaskHistory {
                        id: Uuid::new_v4().to_string(),
                        agent_id: agent_id.clone(),
                        task_id: task_id.clone(),
                        role: "executor".to_string(),
                        status: if agent_success { "completed" } else { "failed" }
                            .to_string(),
                        runtime_seconds: Some(agent_runtime),
                        completed_at: now,
                    };
                    if let Err(e) = subagent_repo.add_history(&history_entry) {
                        tracing::warn!("Failed to record agent task history: {e}");
                    }
                    if agent_success {
                        let _ =
                            subagent_repo.increment_completed(agent_id, agent_runtime);
                    } else {
                        let _ = subagent_repo.increment_failed(agent_id);
                    }
                }

                // Release this agent back to Idle (available for other tasks)
                ctx.agent_registry.update_status(agent_id, AgentStatus::Idle);
                bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: agent_id.clone(),
                    status: "idle".to_string(),
                    current_task_id: None,
                    timestamp: now,
                });

                if agent_success {
                    let raw_content = result.final_content.clone();

                    // For display/DB: synthetic summary if agent produced no text
                    let display_content = if raw_content.is_empty() {
                        format!(
                            "Agent completed in {} rounds ({} tool calls, {} tokens used)",
                            result.rounds_used, result.tool_calls_made,
                            result.total_input_tokens + result.total_output_tokens
                        )
                    } else {
                        raw_content.clone()
                    };

                    // Update state_json: mark step completed + auto-write output to workspace
                    if let Some(ref db) = db {
                        let repo = openalpaca_storage::repository::TaskRepository::new(db);
                        if let Ok(Some(existing)) = repo.get(&task_id) {
                            if let Some(ref sj) = existing.state_json {
                                if let Ok(mut state) = serde_json::from_str::<TaskState>(sj) {
                                    let summary: String = raw_content.chars().take(500).collect();
                                    state.mark_step_completed(step as i32, &summary);
                                    // Auto-write agent output to shared workspace
                                    if !raw_content.is_empty() {
                                        let ws_key = format!("step_{}_output", step);
                                        if let Err(e) = state.workspace.write(
                                            &ws_key,
                                            &raw_content,
                                            agent_id,
                                            crate::orchestrator::task_state::WorkspaceEntryType::Context,
                                            &[],
                                        ) {
                                            tracing::warn!("Failed to auto-write step {} output to workspace: {}", step, e);
                                        }
                                    }
                                    match repo.update_state(&task_id, &state.to_json(), existing.state_version) {
                                        Ok(false) => tracing::warn!("Version conflict persisting step {} state — data may be stale", step),
                                        Err(e) => tracing::warn!("Failed to persist step {} state: {}", step, e),
                                        Ok(true) => {}
                                    }
                                }
                            }
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
                        let _ = repo.update_progress(&task_id, (step + 1) as i32, total_agents as i32);
                    }
                    ctx.task_registry.update_progress(&task_id, (step + 1) as i32, total_agents as i32, None);

                    // Only pass actual content to next agent (not synthetic metadata)
                    if !raw_content.is_empty() {
                        previous_output = Some(raw_content);
                    }
                    // If empty, previous_output stays as-is from prior step

                    final_content = display_content;
                } else {
                    // Update state_json: mark step failed
                    if let Some(ref db) = db {
                        let repo = openalpaca_storage::repository::TaskRepository::new(db);
                        if let Ok(Some(existing)) = repo.get(&task_id) {
                            if let Some(ref sj) = existing.state_json {
                                if let Ok(mut state) = serde_json::from_str::<TaskState>(sj) {
                                    let error_msg = match &result.finish_reason {
                                        LoopFinishReason::CostExceeded => "Agent cost limit exceeded".to_string(),
                                        LoopFinishReason::Error(err) => err.clone(),
                                        _ => "Agent failed".to_string(),
                                    };
                                    state.mark_step_failed(step as i32, &error_msg);
                                    let _ = repo.update_state(&task_id, &state.to_json(), existing.state_version);
                                }
                            }
                        }
                    }

                    pipeline_success = false;
                    pipeline_error = Some(match &result.finish_reason {
                        LoopFinishReason::CostExceeded => {
                            "Agent cost limit exceeded".to_string()
                        }
                        LoopFinishReason::Error(err) => err.clone(),
                        _ => "Agent failed".to_string(),
                    });
                    break;
                }
            }

            // 3. Release remaining agents that never ran (pipeline broke early)
            let now = Utc::now();
            if !pipeline_success {
                for (step, (agent, _, _)) in agents_with_assignments.iter().enumerate() {
                    if step > last_processed_step {
                        ctx.agent_registry
                            .update_status(&agent.id, AgentStatus::Idle);
                        bus.publish(SystemEvent::AgentStatusChanged {
                            agent_id: agent.id.clone(),
                            status: "idle".to_string(),
                            current_task_id: None,
                            timestamp: now,
                        });
                    }
                }
            }

            // 4. Update task status
            let db_summary = if pipeline_success {
                final_content.chars().take(2000).collect::<String>()
            } else {
                pipeline_error.clone().unwrap_or_default()
            };

            if pipeline_success {
                ctx.task_registry
                    .update_status(&task_id, TaskEntryStatus::Completed);
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_status(
                        &task_id,
                        openalpaca_storage::TaskStatus::Completed,
                    );
                    let _ = repo.set_result(&task_id, &db_summary);
                }
                bus.publish(SystemEvent::TaskCompleted {
                    task_id: task_id.clone(),
                    result_summary: Some(db_summary.clone()),
                    timestamp: now,
                });
            } else {
                let err = pipeline_error.clone().unwrap_or_default();
                ctx.task_registry
                    .update_status(&task_id, TaskEntryStatus::Failed);
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ =
                        repo.update_status(&task_id, openalpaca_storage::TaskStatus::Failed);
                    let _ = repo.set_result(&task_id, &err);
                }
                bus.publish(SystemEvent::TaskFailed {
                    task_id: task_id.clone(),
                    error: err,
                    timestamp: now,
                });
            }

            // 5. Persist final result to conversation (single message for entire pipeline)
            let runtime_secs = start_time.elapsed().as_secs() as i64;
            // Clone for memory extraction before final_content is consumed
            let extraction_content = if pipeline_success {
                Some(final_content.clone())
            } else {
                None
            };
            if let Some(ref db) = db {
                let chat_text = if pipeline_success {
                    final_content
                } else {
                    pipeline_error.unwrap_or_default()
                };
                let content =
                    format_task_result(&task_title, &chat_text, pipeline_success);
                let conv_repo = ConversationRepository::new(db);
                // Ensure conversation master row exists and update counters
                // (mirrors Gateway persistence pattern)
                let _ = conv_repo.get_or_create_conversation(&lane_key, &source);
                let _ = conv_repo.insert(&ConversationMessage {
                    id: 0,
                    lane_key: lane_key.clone(),
                    role: "assistant".to_string(),
                    content,
                    source: Some(source.clone()),
                    model: None,
                    tokens_in: Some(total_input_tokens as i64),
                    tokens_out: Some(total_output_tokens as i64),
                    duration_ms: Some(runtime_secs * 1000),
                    created_at: String::new(),
                });
                let _ = conv_repo.increment_message_count(&lane_key);
            }

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
