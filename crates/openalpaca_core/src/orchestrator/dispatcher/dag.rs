use super::super::task_planner::TaskDag;
use super::{
    TaskDispatcher, finalize_task_with_outcome, format_task_result, persist_conversation,
    spawn_task_memory_extraction, update_state_with_retry,
};
use crate::agent::registry::DestroyOutcome;
use crate::context::{DagSummary, TaskEntryStatus};
use crate::events::SystemEvent;
use crate::runner::dag_executor::{DagExecutorConfig, DagFinishReason, execute_dag};
use chrono::Utc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl TaskDispatcher {
    /// Spawn DAG-parallel execution: independent nodes run concurrently.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_dag_execution(
        &self,
        task_id: String,
        task_title: String,
        description: String,
        dag: TaskDag,
        created_by: String,
        lane_key: String,
        source: String,
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
        let compose_engine = self.compose_engine.clone();

        // Create cancellation token for this task
        let cancel_token = CancellationToken::new();
        ctx.register_cancellation_token(&task_id, cancel_token.clone());

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let node_count = dag.nodes.len();

            // Update task status → Running
            ctx.task_registry
                .update_status(&task_id, TaskEntryStatus::Running);
            bus.publish(SystemEvent::TaskUpdated {
                task_id: task_id.clone(),
                status: "running".to_string(),
                progress_current: Some(0),
                progress_total: Some(node_count as i32),
                timestamp: Utc::now(),
            });
            if let Some(ref db) = db {
                let repo = openalpaca_storage::repository::TaskRepository::new(db);
                let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Running);
                let _ = repo.update_progress(&task_id, 0, node_count as i32);
            }

            // Set initial DAG progress in TaskRegistry
            ctx.task_registry.update_progress(
                &task_id,
                0,
                node_count as i32,
                Some(DagSummary {
                    total_nodes: node_count,
                    completed_nodes: 0,
                    running_nodes: 0,
                    failed_nodes: 0,
                }),
            );

            // Execute DAG — read config from daemon_config
            let dcfg = daemon_config.load();
            let dag_cfg = &dcfg.execution.dag;
            let config = DagExecutorConfig {
                max_concurrent_agents: dag_cfg.max_concurrent_agents,
                node_timeout: Duration::from_secs(dag_cfg.node_timeout_secs),
                total_timeout: Duration::from_secs(dag_cfg.total_timeout_secs),
                max_retries_per_node: dag_cfg.max_retries_per_node,
                replan_config: crate::orchestrator::replanner::ReplanConfig {
                    enabled: dag_cfg.replan_enabled,
                    replan_after_every_n_nodes: dag_cfg.replan_after_every_n_nodes,
                    max_replans: dag_cfg.max_replans,
                },
            };
            let mut dag = dag;
            let workspace_id_for_memory = workspace_id.clone();
            let result = execute_dag(
                &mut dag,
                &config,
                router.clone(),
                tool_registry,
                bus.clone(),
                ctx.clone(),
                &task_id,
                &description,
                db.clone(),
                &created_by,
                &daemon_config,
                Some(cancel_token),
                workspace_id,
                &connector_block,
                broker,
                compose_engine,
            )
            .await;

            // Cleanup cancellation token
            ctx.remove_cancellation_token(&task_id);

            let now = Utc::now();
            let runtime_secs = start_time.elapsed().as_secs() as i64;

            // Destroy all agent instances (resets singletons to Idle, removes non-singletons)
            for node in &dag.nodes {
                let outcome = ctx.agent_registry.destroy_instance(&node.agent_id);
                let status = match outcome {
                    DestroyOutcome::ResetToIdle => "idle",
                    _ => "destroyed",
                };
                // Retrieve template_id from instance before it was destroyed
                // (node.agent_id is the instance_id assigned during spawn)
                let template_id = node
                    .agent_id
                    .split("::")
                    .next()
                    .unwrap_or(&node.agent_id)
                    .to_string();
                bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: node.agent_id.clone(),
                    instance_id: node.agent_id.clone(),
                    template_id,
                    status: status.to_string(),
                    current_task_id: None,
                    timestamp: now,
                });
            }

            // Build final content from completed nodes
            let final_content = if result.success {
                result
                    .node_results
                    .iter()
                    .filter(|nr| nr.success && !nr.final_content.is_empty())
                    .map(|nr| nr.final_content.clone())
                    .next_back()
                    .unwrap_or_else(|| {
                        format!(
                            "DAG completed: {}/{} nodes succeeded ({} tokens used)",
                            result.node_results.iter().filter(|n| n.success).count(),
                            node_count,
                            result.total_input_tokens + result.total_output_tokens,
                        )
                    })
            } else {
                match &result.finish_reason {
                    DagFinishReason::NodeFailed { node_id, error } => {
                        format!("DAG execution failed at node '{}': {}", node_id, error)
                    }
                    DagFinishReason::Timeout => "DAG execution timed out".to_string(),
                    DagFinishReason::AllCompleted => "Completed".to_string(),
                    DagFinishReason::Aborted { reason } => {
                        format!("DAG execution aborted: {}", reason)
                    }
                }
            };

            // Persist final result to conversation before publishing completion,
            // so follow-up turns can immediately read the result from history.
            if let Some(ref db) = db {
                let content = format_task_result(&task_title, &final_content, result.success);
                persist_conversation(
                    db,
                    &lane_key,
                    &source,
                    content,
                    None,
                    result.total_input_tokens as i64,
                    result.total_output_tokens as i64,
                    runtime_secs,
                );
            }

            // Write authoritative DAG state before building outcome.
            // The in-memory `dag` has all node statuses and result_summaries
            // from execute_dag(). DB copy may be stale if earlier
            // persist_dag_state() calls exhausted retries under contention.
            if let Some(ref db) = db {
                let final_dag = dag.clone();
                if !update_state_with_retry(
                    db,
                    &task_id,
                    move |state| {
                        state.dag = Some(final_dag.clone());
                        let step_orders: Vec<i32> =
                            state.steps.iter().map(|s| s.step_order).collect();
                        for order in step_orders {
                            state.scan_workspace_artifacts(order);
                        }
                    },
                    "dag_final_state",
                )
                .await
                {
                    tracing::error!("Failed to persist dag_final_state for task '{}'", task_id);
                }
            }

            // Update task status with structured outcome
            finalize_task_with_outcome(
                &ctx,
                &bus,
                db.as_ref(),
                &task_id,
                &final_content,
                result.success,
            );

            // Memory extraction from DAG output (non-blocking)
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
                    "dag",
                    result.success,
                    workspace_id_for_memory,
                );
            }

            tracing::info!(
                "DAG execution for task '{}' finished: success={}, nodes={}/{}, runtime={}s",
                task_id,
                result.success,
                result.node_results.len(),
                node_count,
                runtime_secs
            );
        });
    }
}
