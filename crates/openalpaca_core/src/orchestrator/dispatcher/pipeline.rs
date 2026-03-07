use super::{
    TaskDispatcher, finalize_task_with_outcome, format_task_result, persist_conversation,
    spawn_task_memory_extraction,
};
use super::pipeline_step::{PipelineStepContext, execute_pipeline_step, fetch_workspace_context};
use crate::agent::registry::DestroyOutcome;
use crate::agent::subagent::SubAgent;
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use chrono::Utc;
use tokio_util::sync::CancellationToken;

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
        let broker = self.confirmation_broker.read().ok().and_then(|g| g.clone());

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

            // Build shared step context
            let pctx = PipelineStepContext {
                ctx: ctx.clone(),
                bus: bus.clone(),
                db: db.clone(),
                embedder: embedder.clone(),
                tool_registry: tool_registry.clone(),
                daemon_config: daemon_config.clone(),
                router: router.clone(),
                connector_block,
                broker,
                task_id: task_id.clone(),
                description: description.clone(),
                created_by: created_by.clone(),
                workspace_id: workspace_id.clone(),
                total_agents,
                cancel_token: cancel_token.clone(),
            };

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

                let step_result = execute_pipeline_step(
                    &pctx,
                    step,
                    agent,
                    assignment_id.as_ref(),
                    role_description,
                    &previous_output,
                    &cached_workspace_context,
                )
                .await;

                total_input_tokens += step_result.input_tokens;
                total_output_tokens += step_result.output_tokens;

                if step_result.success {
                    // Refresh cached workspace context if the agent made tool calls
                    if step_result.tool_calls_made > 0 {
                        cached_workspace_context = fetch_workspace_context(db.as_ref(), &task_id);
                    }

                    // Only pass actual content to next agent (not synthetic metadata)
                    if !step_result.raw_content.is_empty() {
                        previous_output = Some(step_result.raw_content);
                    }
                    // If empty, previous_output stays as-is from prior step

                    final_content = step_result.display_content;
                } else {
                    pipeline_success = false;
                    pipeline_error = step_result.error;
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
