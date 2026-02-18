use super::{TaskDispatcher, format_task_result, spawn_task_memory_extraction};
use crate::agent::registry::DestroyOutcome;
use crate::agent::subagent::SubAgent;
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use crate::runner::lead_agent::run_lead_agent;
use crate::runner::LoopFinishReason;
use chrono::Utc;
use openalpaca_storage::{ConversationMessage, ConversationRepository};
use super::super::task_state::TaskState;
use uuid::Uuid;

impl TaskDispatcher {
    /// Dispatch a task using the Lead Agent orchestration pattern.
    /// Spawns a lead agent instance from the "lead_agent" template (singleton),
    /// registers the task, and runs the lead agent execution loop.
    pub(super) fn dispatch_lead_agent(
        &self,
        description: &str,
        title: String,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace_id: Option<String>,
    ) -> Result<String, String> {
        let task_id = Uuid::new_v4().to_string();
        let now = Utc::now();

        // Spawn a lead agent instance from the singleton template.
        // Prefer templates with "lead_orchestration" skill, fall back to any template.
        let lead_agent = {
            let templates = self.shared_context.agent_registry
                .find_templates_by_skill("lead_orchestration");
            let mut spawned = None;
            for t in &templates {
                if let Ok(agent) = self.shared_context.agent_registry
                    .spawn_instance(&t.frontmatter.id, task_id.clone())
                {
                    spawned = Some(agent);
                    break;
                }
            }
            if spawned.is_none() {
                // Fallback: try spawning from any available template
                for t in self.shared_context.agent_registry.list_templates() {
                    if let Ok(agent) = self.shared_context.agent_registry
                        .spawn_instance(&t.frontmatter.id, task_id.clone())
                    {
                        spawned = Some(agent);
                        break;
                    }
                }
            }
            spawned.ok_or_else(|| {
                "No agents available to act as Lead Agent. All agents are busy.".to_string()
            })?
        };

        // Register in task_registry
        self.shared_context
            .task_registry
            .register(task_id.clone(), title.clone());

        // Create TaskLane
        let task_lane = self.lane_manager.create_task_lane(&task_id);
        task_lane.assign_agent(lead_agent.id.clone());

        // Emit status change — lead agent instance just spawned
        self.bus.publish(SystemEvent::AgentStatusChanged {
            agent_id: lead_agent.id.clone(),
            instance_id: lead_agent.id.clone(),
            template_id: lead_agent.template_id.clone(),
            status: "spawned".to_string(),
            current_task_id: Some(task_id.clone()),
            timestamp: now,
        });

        // Emit TaskCreated
        self.bus.publish(SystemEvent::TaskCreated {
            task_id: task_id.clone(),
            title: title.clone(),
            created_by: created_by.to_string(),
            timestamp: now,
        });

        // Persist task to DB
        if let Some(ref db) = self.db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            let task = openalpaca_storage::Task {
                id: task_id.clone(),
                title: title.clone(),
                description: Some(description.to_string()),
                status: openalpaca_storage::TaskStatus::Queued,
                priority: 0,
                progress_current: None,
                progress_total: None,
                result_summary: None,
                created_by: created_by.to_string(),
                source_lane: lane_key.to_string(),
                created_at: now,
                updated_at: now,
                completed_at: None,
                state_json: None,
                state_version: 0,
            };
            if let Err(e) = repo.create(&task) {
                tracing::warn!("Failed to persist lead agent task to DB: {e}");
            }

            // Initialize state_json with workspace
            let step_info = vec![(
                lead_agent.id.clone(),
                lead_agent.name.clone(),
                "lead_orchestrator".to_string(),
            )];
            let initial_state = TaskState::initial(description, &step_info);
            let _ = repo.update_state(&task_id, &initial_state.to_json(), 0);
        }

        // Spawn the lead agent execution
        self.spawn_lead_agent_execution(
            task_id.clone(),
            title.clone(),
            description.to_string(),
            lead_agent,
            lane_key.to_string(),
            source.to_string(),
            created_by.to_string(),
            workspace_id,
        );

        Ok(format!(
            "I've created a task and assigned it to the Lead Agent for dynamic orchestration:\n\n\
             - Lead Agent will analyze, delegate to subagents, and synthesize results\n\n\
             Task: {}\nYou'll see the results here when the task completes.",
            title
        ))
    }

    /// Spawn the lead agent execution in a background tokio task.
    /// The lead agent runs a full agentic loop with `spawn_subagent` tool access.
    fn spawn_lead_agent_execution(
        &self,
        task_id: String,
        task_title: String,
        description: String,
        lead_agent: SubAgent,
        lane_key: String,
        source: String,
        created_by: String,
        workspace_id: Option<String>,
    ) {
        let router = match &self.llm_router {
            Some(r) => r.clone(),
            None => {
                tracing::warn!(
                    "No LLM router configured — cannot execute lead agent for task '{}'",
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

            // Update task status → Running
            ctx.task_registry
                .update_status(&task_id, TaskEntryStatus::Running);
            bus.publish(SystemEvent::TaskUpdated {
                task_id: task_id.clone(),
                status: "running".to_string(),
                progress_current: Some(0),
                progress_total: None, // Lead agent has dynamic progress
                timestamp: Utc::now(),
            });
            if let Some(ref db) = db {
                let repo = openalpaca_storage::repository::TaskRepository::new(db);
                let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Running);
            }

            // Run the lead agent
            let result = run_lead_agent(
                &lead_agent,
                &description,
                router.clone(),
                tool_registry,
                ctx.clone(),
                bus.clone(),
                db.clone(),
                embedder.clone(),
                &task_id,
                &created_by,
                &daemon_config,
                workspace_id.clone(),
            )
            .await;

            let now = Utc::now();
            let runtime_secs = start_time.elapsed().as_secs() as i64;

            // Destroy lead agent instance (resets singleton to Idle)
            let outcome = ctx.agent_registry.destroy_instance(&lead_agent.id);
            let destroy_status = match outcome {
                DestroyOutcome::ResetToIdle => "idle",
                _ => "destroyed",
            };
            bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: lead_agent.id.clone(),
                instance_id: lead_agent.id.clone(),
                template_id: lead_agent.template_id.clone(),
                status: destroy_status.to_string(),
                current_task_id: None,
                timestamp: now,
            });

            // Build final content
            let final_content = if result.success {
                if result.final_content.is_empty() {
                    format!(
                        "Lead agent completed: {} subagents spawned, {} rounds, {} tokens used",
                        result.subagents_spawned,
                        result.loop_result.rounds_used,
                        result.loop_result.total_input_tokens
                            + result.loop_result.total_output_tokens,
                    )
                } else {
                    result.final_content.clone()
                }
            } else {
                format!(
                    "Lead agent failed: {:?}. {} subagents were spawned before failure.",
                    result.loop_result.finish_reason, result.subagents_spawned
                )
            };

            let db_summary = final_content.chars().take(2000).collect::<String>();

            // Persist LLM usage for the lead agent's own loop
            let default_model = router.default_model();
            let actual_model = result
                .loop_result
                .model_used
                .as_deref()
                .or(lead_agent.llm_config.model.as_deref())
                .unwrap_or(&default_model);
            let resolved_provider = router
                .model_registry()
                .resolve_provider(actual_model)
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let call_cost = router.cost_tracker.calculate_cost(
                actual_model,
                result.loop_result.total_input_tokens,
                result.loop_result.total_output_tokens,
            );

            if let Some(ref db) = db {
                let usage_repo =
                    openalpaca_storage::repository::LlmUsageRepository::new(db);
                if let Err(e) = usage_repo.record_and_log(
                    &lead_agent.id,
                    Some(&task_id),
                    &resolved_provider,
                    actual_model,
                    result.loop_result.total_input_tokens as i32,
                    result.loop_result.total_output_tokens as i32,
                    call_cost,
                    start_time.elapsed().as_millis() as i64,
                    if result.success { "success" } else { "error" },
                    match &result.loop_result.finish_reason {
                        LoopFinishReason::Error(msg) => Some(msg.as_str()),
                        _ => None,
                    },
                ) {
                    tracing::warn!("Failed to persist LLM usage for lead agent: {e}");
                }

                // Record agent task history
                let subagent_repo = openalpaca_storage::SubAgentRepository::new(db);
                let history_entry = openalpaca_storage::AgentTaskHistory {
                    id: Uuid::new_v4().to_string(),
                    agent_id: lead_agent.id.clone(),
                    task_id: task_id.clone(),
                    role: "lead_agent".to_string(),
                    status: if result.success { "completed" } else { "failed" }
                        .to_string(),
                    runtime_seconds: Some(runtime_secs),
                    completed_at: now,
                };
                if let Err(e) = subagent_repo.add_history(&history_entry) {
                    tracing::warn!("Failed to record lead agent task history: {e}");
                }
                if result.success {
                    let _ = subagent_repo
                        .increment_completed(&lead_agent.id, runtime_secs);
                } else {
                    let _ = subagent_repo.increment_failed(&lead_agent.id);
                }
            }

            // Update task status
            if result.success {
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
                ctx.task_registry
                    .update_status(&task_id, TaskEntryStatus::Failed);
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo
                        .update_status(&task_id, openalpaca_storage::TaskStatus::Failed);
                    let _ = repo.set_result(&task_id, &db_summary);
                }
                bus.publish(SystemEvent::TaskFailed {
                    task_id: task_id.clone(),
                    error: db_summary.clone(),
                    timestamp: now,
                });
            }

            // Persist final result to conversation
            if let Some(ref db) = db {
                let content =
                    format_task_result(&task_title, &final_content, result.success);
                let conv_repo = ConversationRepository::new(db);
                let _ = conv_repo.get_or_create_conversation(&lane_key, &source);
                let _ = conv_repo.insert(&ConversationMessage {
                    id: 0,
                    lane_key: lane_key.clone(),
                    role: "assistant".to_string(),
                    content,
                    source: Some(source.clone()),
                    model: Some(actual_model.to_string()),
                    tokens_in: Some(
                        result.loop_result.total_input_tokens as i64,
                    ),
                    tokens_out: Some(
                        result.loop_result.total_output_tokens as i64,
                    ),
                    duration_ms: Some(runtime_secs * 1000),
                    created_at: String::new(),
                });
                let _ = conv_repo.increment_message_count(&lane_key);
            }

            // Memory extraction from lead agent output (non-blocking)
            if let Some(ref db) = db {
                spawn_task_memory_extraction(
                    db,
                    &router,
                    &embedder,
                    &daemon_config,
                    &created_by,
                    &task_id,
                    &description,
                    &final_content,
                    "lead_agent",
                    result.success,
                    workspace_id,
                );
            }

            tracing::info!(
                "Lead agent execution for task '{}' finished: success={}, subagents={}, rounds={}, runtime={}s",
                task_id,
                result.success,
                result.subagents_spawned,
                result.loop_result.rounds_used,
                runtime_secs
            );
        });
    }
}
