use super::super::task_state::TaskState;
use super::update_state_with_retry;
use super::usage;
use super::{
    TaskDispatcher, finalize_task_with_outcome, format_task_result, persist_conversation,
    spawn_task_memory_extraction,
};
use crate::agent::registry::DestroyOutcome;
use crate::agent::subagent::SubAgent;
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use crate::runner::lead_agent::run_lead_agent;
use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

impl TaskDispatcher {
    /// Dispatch a task using the Lead Agent orchestration pattern.
    /// Spawns a lead agent instance from the "lead_agent" template (singleton),
    /// registers the task, and runs the lead agent execution loop.
    pub(crate) fn dispatch_lead_agent(
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
            let templates = self
                .shared_context
                .agent_registry
                .find_templates_by_skill("lead_orchestration");
            let mut spawned = None;
            for t in &templates {
                if let Ok(agent) = self
                    .shared_context
                    .agent_registry
                    .spawn_instance(&t.frontmatter.id, task_id.clone())
                {
                    spawned = Some(agent);
                    break;
                }
            }
            if spawned.is_none() {
                // Fallback: try spawning from any available template
                for t in self.shared_context.agent_registry.list_templates() {
                    if let Ok(agent) = self
                        .shared_context
                        .agent_registry
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
                outcome_json: None,
                outcome_kind: None,
                artifact_count: 0,
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
            let state_json = initial_state.to_json();
            match repo.update_state(&task_id, &state_json, 0) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(task_id = %task_id, "State init version conflict, retrying");
                    let _ = repo.update_state(&task_id, &state_json, 1);
                }
                Err(e) => {
                    tracing::error!(task_id = %task_id, error = %e, "Failed to initialize task state");
                }
            }
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
    #[allow(clippy::too_many_arguments)]
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

            tracing::info!(
                task_id = %task_id,
                lead_agent = %lead_agent.id,
                "Lead agent background execution starting"
            );

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

            // Mark step 0 as running now (before the agentic loop) so started_at is accurate
            if let Some(ref db) = db
                && !update_state_with_retry(
                    db,
                    &task_id,
                    |s| s.mark_step_running(0),
                    "lead_agent_mark_step_running",
                )
                .await
            {
                tracing::error!("Failed to persist lead_agent_mark_step_running for task '{}'", task_id);
            }

            tracing::info!(task_id = %task_id, "Task status: queued → running");

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
                Some(cancel_token),
                &connector_block,
            )
            .await;

            // Cleanup cancellation token
            ctx.remove_cancellation_token(&task_id);

            let now = Utc::now();
            let runtime_secs = start_time.elapsed().as_secs() as i64;

            tracing::info!(
                task_id = %task_id,
                success = result.success,
                rounds = result.loop_result.rounds_used,
                subagents = result.subagents_spawned,
                runtime_secs = runtime_secs,
                finish_reason = ?result.loop_result.finish_reason,
                "Lead agent execution returned"
            );

            // Update state_json: mark lead agent step completed or failed
            if let Some(ref db) = db {
                let success = result.success;
                let summary_text: String = result.final_content.chars().take(500).collect();
                let error_msg = format!("{:?}", result.loop_result.finish_reason);
                if !update_state_with_retry(
                    db,
                    &task_id,
                    move |state| {
                        if success {
                            state.mark_step_completed(0, &summary_text);
                        } else {
                            state.mark_step_failed(0, &error_msg);
                        }
                        state.scan_workspace_artifacts(0);
                    },
                    "lead_agent_step_complete",
                )
                .await
                {
                    tracing::error!("Failed to persist lead_agent_step_complete for task '{}'", task_id);
                }
            }

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

            // Persist LLM usage for the lead agent's own loop
            usage::record_llm_usage(
                &router,
                &result.loop_result,
                lead_agent.llm_config.model.as_deref(),
                &lead_agent.id,
                &task_id,
                start_time.elapsed().as_millis() as i64,
                db.as_ref(),
                &bus,
            );

            // Record agent task history
            if let Some(ref db) = db {
                usage::record_agent_history(
                    db,
                    &lead_agent.id,
                    &task_id,
                    "lead_agent",
                    result.success,
                    runtime_secs,
                );
            }

            // Persist final result to conversation before publishing completion,
            // so follow-up turns can immediately read the result from history.
            if let Some(ref db) = db {
                let content = format_task_result(&task_title, &final_content, result.success);
                // Resolve model name for conversation record
                let default_model = router.default_model();
                let actual_model = result
                    .loop_result
                    .model_used
                    .as_deref()
                    .or(lead_agent.llm_config.model.as_deref())
                    .unwrap_or(&default_model);
                persist_conversation(
                    db,
                    &lane_key,
                    &source,
                    content,
                    Some(actual_model.to_string()),
                    result.loop_result.total_input_tokens as i64,
                    result.loop_result.total_output_tokens as i64,
                    runtime_secs,
                );
            }

            // Update task status
            if result.success {
                tracing::info!(task_id = %task_id, "Task status: running → completed");
            } else {
                tracing::warn!(
                    task_id = %task_id,
                    finish_reason = ?result.loop_result.finish_reason,
                    "Task status: running → failed"
                );
            }
            finalize_task_with_outcome(
                &ctx,
                &bus,
                db.as_ref(),
                &task_id,
                &final_content,
                result.success,
            );

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
