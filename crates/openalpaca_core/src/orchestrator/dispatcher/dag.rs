use super::{TaskDispatcher, format_task_result, spawn_task_memory_extraction};
use crate::agent::subagent::AgentStatus;
use crate::context::{DagSummary, TaskEntryStatus};
use crate::events::SystemEvent;
use crate::runner::dag_executor::{DagExecutorConfig, DagFinishReason, execute_dag};
use chrono::Utc;
use openalpaca_storage::{ConversationMessage, ConversationRepository};
use super::super::task_planner::TaskDag;
use std::time::Duration;

impl TaskDispatcher {
    /// Spawn DAG-parallel execution: independent nodes run concurrently.
    pub(super) fn spawn_dag_execution(
        &self,
        task_id: String,
        task_title: String,
        description: String,
        dag: TaskDag,
        created_by: String,
        lane_key: String,
        source: String,
    ) {
        let router = match &self.llm_router {
            Some(r) => r.clone(),
            None => {
                tracing::warn!(
                    "No LLM router configured — cannot execute DAG for task '{}'",
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
            let node_count = dag.nodes.len();

            // Update task status → Running
            ctx.task_registry.update_status(&task_id, TaskEntryStatus::Running);
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
                    enabled: false,
                    replan_after_every_n_nodes: dag_cfg.replan_after_every_n_nodes,
                    max_replans: dag_cfg.max_replans,
                },
            };
            let mut dag = dag;
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
            )
            .await;

            let now = Utc::now();
            let runtime_secs = start_time.elapsed().as_secs() as i64;

            // Release all agents back to Idle
            for node in &dag.nodes {
                ctx.agent_registry.update_status(&node.agent_id, AgentStatus::Idle);
                bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: node.agent_id.clone(),
                    status: "idle".to_string(),
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
                    .last()
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

            let db_summary = final_content.chars().take(2000).collect::<String>();

            // Update task status
            if result.success {
                ctx.task_registry.update_status(&task_id, TaskEntryStatus::Completed);
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Completed);
                    let _ = repo.set_result(&task_id, &db_summary);
                }
                bus.publish(SystemEvent::TaskCompleted {
                    task_id: task_id.clone(),
                    result_summary: Some(db_summary.clone()),
                    timestamp: now,
                });
            } else {
                ctx.task_registry.update_status(&task_id, TaskEntryStatus::Failed);
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Failed);
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
                let content = format_task_result(&task_title, &final_content, result.success);
                let conv_repo = ConversationRepository::new(db);
                let _ = conv_repo.get_or_create_conversation(&lane_key, &source);
                let _ = conv_repo.insert(&ConversationMessage {
                    id: 0,
                    lane_key: lane_key.clone(),
                    role: "assistant".to_string(),
                    content,
                    source: Some(source.clone()),
                    model: None,
                    tokens_in: Some(result.total_input_tokens as i64),
                    tokens_out: Some(result.total_output_tokens as i64),
                    duration_ms: Some(runtime_secs * 1000),
                    created_at: String::new(),
                });
                let _ = conv_repo.increment_message_count(&lane_key);
            }

            // Memory extraction from DAG output (non-blocking)
            if let Some(ref db) = db {
                // TODO: Wire workspace_id through dispatch paths
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
                    None,
                );
            }

            tracing::info!(
                "DAG execution for task '{}' finished: success={}, nodes={}/{}, runtime={}s",
                task_id, result.success, result.node_results.len(), node_count, runtime_secs
            );
        });
    }
}
