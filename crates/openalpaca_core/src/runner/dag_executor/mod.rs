//! DAG-aware parallel executor: runs independent sub-tasks concurrently
//! using `tokio::JoinSet`, respecting dependency edges in the TaskDag.

mod node_runner;
mod progress;

use crate::agent::subagent::SubAgent;
use crate::daemon_config::DaemonConfig;
use crate::orchestrator::replanner::{
    ReplanConfig, ReplanDecision, Replanner, merge_replanned_dag,
};
use crate::orchestrator::task_planner::{DagNode, DagNodeStatus, TaskDag};
use crate::orchestrator::task_state::{TaskState, TaskWorkspace, WorkspaceEntryType};
use crate::runner::{LoopConfig, LoopFinishReason, LoopResult, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::ToolRegistry;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use arc_swap::ArcSwap;
use openalpaca_llm::{ChatMessage, LlmRouter};
use openalpaca_storage::Database;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::bus::EventBus;
use crate::context::{DagSummary, SharedContext};
use crate::events::SystemEvent;
use crate::middleware::prompt::format_tool_guidance;
use chrono::Utc;

use node_runner::execute_single_node;
use progress::{
    load_task_state, load_workspace_snapshot, persist_dag_state, write_node_output_to_workspace,
};

// ── Configuration ────────────────────────────────────────────────────

/// Configuration for the DAG executor.
#[derive(Debug, Clone)]
pub struct DagExecutorConfig {
    /// Maximum number of agents running concurrently.
    pub max_concurrent_agents: usize,
    /// Timeout for a single DAG node execution.
    pub node_timeout: Duration,
    /// Timeout for the entire DAG execution.
    pub total_timeout: Duration,
    /// Maximum retries per node on transient failure.
    pub max_retries_per_node: usize,
    /// Configuration for dynamic replanning.
    pub replan_config: ReplanConfig,
}

impl Default for DagExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent_agents: 4,
            node_timeout: Duration::from_secs(300),
            total_timeout: Duration::from_secs(1800),
            max_retries_per_node: 1,
            replan_config: ReplanConfig::default(),
        }
    }
}

// ── Result types ─────────────────────────────────────────────────────

/// Result of a single DAG node execution.
#[derive(Debug, Clone)]
pub struct NodeResult {
    pub node_id: String,
    pub node_title: String,
    pub agent_id: String,
    pub success: bool,
    pub final_content: String,
    pub duration_ms: u64,
    pub loop_result: LoopResult,
}

/// How the entire DAG execution finished.
#[derive(Debug, Clone)]
pub enum DagFinishReason {
    AllCompleted,
    NodeFailed { node_id: String, error: String },
    Timeout,
    Aborted { reason: String },
}

/// Result of the entire DAG execution.
#[derive(Debug, Clone)]
pub struct DagExecutionResult {
    pub success: bool,
    pub node_results: Vec<NodeResult>,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub total_duration: Duration,
    pub finish_reason: DagFinishReason,
}

// ── Executor ─────────────────────────────────────────────────────────

/// Execute a TaskDag with parallel fan-out for independent nodes.
///
/// Algorithm:
/// 1. Compute ready_set = nodes with no unmet dependencies
/// 2. While DAG is not finished and not timed out:
///    a. Spawn up to max_concurrent_agents from ready_set
///    b. Wait for ANY node to complete (JoinSet::join_next)
///    c. On completion: update DAG, persist state, compute new ready nodes
///    d. On failure: mark failed, skip dependents, continue independent branches
#[allow(clippy::too_many_arguments)]
pub async fn execute_dag(
    dag: &mut TaskDag,
    config: &DagExecutorConfig,
    router: Arc<LlmRouter>,
    tool_registry: Arc<ToolRegistry>,
    bus: EventBus,
    ctx: Arc<SharedContext>,
    task_id: &str,
    task_description: &str,
    db: Option<Database>,
    created_by: &str,
    daemon_config: &Arc<ArcSwap<DaemonConfig>>,
    cancel_token: Option<CancellationToken>,
    workspace_id: Option<String>,
    connector_guidance: &str,
    confirmation_broker: Option<Arc<crate::security::confirmation::ConfirmationBroker>>,
) -> DagExecutionResult {
    let start = Instant::now();

    // Early return for empty DAG — avoids misleading Timeout finish reason
    if dag.nodes.is_empty() {
        return DagExecutionResult {
            success: true,
            node_results: vec![],
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_duration: start.elapsed(),
            finish_reason: DagFinishReason::AllCompleted,
        };
    }

    let mut node_results: Vec<NodeResult> = Vec::new();
    let mut total_input_tokens: u32 = 0;
    let mut total_output_tokens: u32 = 0;
    let mut retry_counts: HashMap<String, usize> = HashMap::new();
    let mut join_set: JoinSet<NodeResult> = JoinSet::new();
    let mut running_count: usize = 0;

    // Replanning state
    let mut completed_since_last_replan: usize = 0;
    let mut replans_done: usize = 0;
    let mut replan_errors: usize = 0;

    // Mark initial ready nodes
    mark_ready_nodes(dag);

    loop {
        // Check cancellation
        if let Some(ref token) = cancel_token
            && token.is_cancelled()
        {
            tracing::info!(
                "DAG execution cancelled for task '{}' — aborting {} running tasks",
                task_id,
                running_count
            );
            join_set.abort_all();
            return DagExecutionResult {
                success: false,
                node_results,
                total_input_tokens,
                total_output_tokens,
                total_duration: start.elapsed(),
                finish_reason: DagFinishReason::Aborted {
                    reason: "Task cancelled by user".to_string(),
                },
            };
        }

        // Check total timeout
        if start.elapsed() > config.total_timeout {
            tracing::warn!(
                "DAG execution timed out for task '{}' — aborting {} running tasks",
                task_id,
                running_count
            );
            join_set.abort_all();
            return DagExecutionResult {
                success: false,
                node_results,
                total_input_tokens,
                total_output_tokens,
                total_duration: start.elapsed(),
                finish_reason: DagFinishReason::Timeout,
            };
        }

        // Check if finished
        if dag.is_finished() {
            break;
        }

        // Collect ready node IDs and data BEFORE mutating the DAG.
        // This avoids holding an immutable borrow while we need mutable access.
        // When critical path scheduling is enabled, prioritize nodes on the
        // longest downstream path for better overall DAG completion time.
        let dcfg = daemon_config.load();
        let ready_refs = if dcfg.execution.dag.critical_path_scheduling_enabled {
            dag.ready_nodes_prioritized()
        } else {
            dag.ready_nodes()
        };
        let ready_info: Vec<(String, String, DagNode)> = ready_refs
            .iter()
            .map(|n| (n.node_id.clone(), n.agent_id.clone(), (*n).clone()))
            .collect();

        if ready_info.is_empty() && running_count == 0 {
            break;
        }

        // Build index for O(1) node lookups by ID (owned keys to avoid borrowing dag)
        let node_index: std::collections::HashMap<String, usize> = dag
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.node_id.clone(), i))
            .collect();

        // Opt-7c: Pre-load workspace snapshot once per batch instead of N times
        // for N concurrent nodes. Each node filters by its own workspace_keys.
        let workspace_snapshot: Arc<Option<TaskState>> = Arc::new(load_task_state(task_id, &db));

        for (node_id, agent_id_str, node_snapshot) in ready_info {
            if running_count >= config.max_concurrent_agents {
                break;
            }

            // Verify node is still in a dispatchable state (may have changed during replanning).
            // Accept both Pending and Ready — mark_ready_nodes() promotes Pending → Ready
            // when dependencies are satisfied, so Ready is a valid dispatchable state.
            if let Some(&idx) = node_index.get(&node_id) {
                if !matches!(
                    dag.nodes[idx].status,
                    DagNodeStatus::Pending | DagNodeStatus::Ready
                ) {
                    tracing::debug!(
                        "Node '{}' is not dispatchable (now {:?}), skipping",
                        node_id,
                        dag.nodes[idx].status
                    );
                    continue;
                }
            } else {
                tracing::warn!("Node '{}' no longer exists in DAG, skipping", node_id);
                continue;
            }

            dag.mark_running(&node_id);
            running_count += 1;

            persist_dag_state(dag, task_id, &db).await;

            // Emit DagNodeStarted event
            bus.publish(SystemEvent::DagNodeStarted {
                task_id: task_id.to_string(),
                node_id: node_id.clone(),
                node_title: node_snapshot.title.clone(),
                agent_id: agent_id_str.clone(),
                timestamp: Utc::now(),
            });

            // Look up the agent
            let agent = match ctx.agent_registry.get(&agent_id_str) {
                Some(a) => a,
                None => {
                    tracing::error!("Agent '{}' not found for node '{}'", agent_id_str, node_id);
                    dag.fail_node(&node_id, &format!("Agent '{}' not found", agent_id_str));
                    running_count -= 1;
                    persist_dag_state(dag, task_id, &db).await;

                    // Emit DagNodeCompleted for the failed node so clients see it resolve
                    bus.publish(SystemEvent::DagNodeCompleted {
                        task_id: task_id.to_string(),
                        node_id: node_id.clone(),
                        node_title: node_snapshot.title.clone(),
                        agent_id: agent_id_str.clone(),
                        success: false,
                        duration_ms: 0,
                        output_preview: Some(format!("Agent '{}' not found", agent_id_str)),
                        timestamp: Utc::now(),
                    });

                    // Update TaskRegistry progress
                    let completed = dag.completed_count();
                    let total = dag.nodes.len();
                    let dag_summary = DagSummary {
                        total_nodes: total,
                        completed_nodes: completed,
                        running_nodes: running_count,
                        failed_nodes: dag
                            .nodes
                            .iter()
                            .filter(|n| n.status == DagNodeStatus::Failed)
                            .count(),
                    };
                    ctx.task_registry.update_progress(
                        task_id,
                        completed as i32,
                        total as i32,
                        Some(dag_summary),
                    );

                    continue;
                }
            };

            // Clone shared resources for the spawned task
            let router_clone = router.clone();
            let tool_registry_clone = tool_registry.clone();
            let bus_clone = bus.clone();
            let db_clone = db.clone();
            let task_id_owned = task_id.to_string();
            let description_owned = task_description.to_string();
            let created_by_owned = created_by.to_string();
            let node_timeout = config.node_timeout;
            let daemon_config_clone = daemon_config.clone();
            let token_clone = cancel_token.clone();

            let workspace_id_clone = workspace_id.clone();
            let connector_guidance_clone = connector_guidance.to_string();
            let ws_snapshot = workspace_snapshot.clone();
            let broker_clone = confirmation_broker.clone();
            join_set.spawn(async move {
                execute_single_node(
                    node_snapshot,
                    router_clone,
                    tool_registry_clone,
                    bus_clone,
                    db_clone,
                    task_id_owned,
                    description_owned,
                    created_by_owned,
                    agent,
                    node_timeout,
                    daemon_config_clone,
                    token_clone,
                    workspace_id_clone,
                    connector_guidance_clone,
                    ws_snapshot,
                    broker_clone,
                )
                .await
            });
        }

        // Wait for any node to complete
        if let Some(join_result) = join_set.join_next().await {
            running_count -= 1;

            match join_result {
                Ok(node_result) => {
                    let node_id = node_result.node_id.clone();
                    total_input_tokens += node_result.loop_result.total_input_tokens;
                    total_output_tokens += node_result.loop_result.total_output_tokens;

                    // Track whether this is a retry (don't record intermediate failures)
                    let mut should_record = true;

                    if node_result.success {
                        tracing::info!(
                            "DAG node '{}' completed successfully for task '{}'",
                            node_id,
                            task_id
                        );

                        // Write output to workspace
                        write_node_output_to_workspace(dag, &node_result, task_id, &db).await;

                        // Mark completed in DAG
                        dag.complete_node(&node_id, &node_result.final_content);
                        mark_ready_nodes(dag);
                        completed_since_last_replan += 1;
                    } else {
                        let retries = retry_counts.entry(node_id.clone()).or_insert(0);
                        if *retries < config.max_retries_per_node {
                            tracing::warn!(
                                "DAG node '{}' failed, retrying ({}/{})",
                                node_id,
                                *retries + 1,
                                config.max_retries_per_node
                            );
                            *retries += 1;
                            // Reset to Pending for retry
                            if let Some(n) = dag.nodes.iter_mut().find(|n| n.node_id == node_id) {
                                n.status = DagNodeStatus::Pending;
                                n.result_summary = None;
                            }
                            mark_ready_nodes(dag);
                            should_record = false;
                        } else {
                            tracing::error!(
                                "DAG node '{}' failed permanently for task '{}'",
                                node_id,
                                task_id
                            );
                            let error = node_result.final_content.clone();
                            dag.fail_node(&node_id, &error);
                        }
                    }

                    persist_dag_state(dag, task_id, &db).await;

                    // Emit progress event
                    let completed = dag.completed_count();
                    let total = dag.nodes.len();
                    bus.publish(SystemEvent::TaskUpdated {
                        task_id: task_id.to_string(),
                        status: "running".to_string(),
                        progress_current: Some(completed as i32),
                        progress_total: Some(total as i32),
                        timestamp: Utc::now(),
                    });

                    // Emit DagNodeCompleted event
                    bus.publish(SystemEvent::DagNodeCompleted {
                        task_id: task_id.to_string(),
                        node_id: node_id.clone(),
                        node_title: node_result.node_title.clone(),
                        agent_id: node_result.agent_id.clone(),
                        success: node_result.success,
                        duration_ms: node_result.duration_ms,
                        output_preview: if node_result.final_content.is_empty() {
                            None
                        } else {
                            Some(node_result.final_content.chars().take(200).collect())
                        },
                        timestamp: Utc::now(),
                    });

                    // Update in-memory TaskRegistry with DAG progress
                    let dag_summary = DagSummary {
                        total_nodes: total,
                        completed_nodes: completed,
                        running_nodes: running_count,
                        failed_nodes: dag
                            .nodes
                            .iter()
                            .filter(|n| n.status == DagNodeStatus::Failed)
                            .count(),
                    };
                    ctx.task_registry.update_progress(
                        task_id,
                        completed as i32,
                        total as i32,
                        Some(dag_summary),
                    );

                    // Only record final results (success or permanent failure), not retries
                    if should_record {
                        node_results.push(node_result);
                    }

                    // ── Dynamic replanning check ─────────────────────
                    if config.replan_config.enabled
                        && completed_since_last_replan >= config.replan_config.replan_after_every_n_nodes
                        && (replans_done + replan_errors) < config.replan_config.max_replans
                        && running_count == 0  // Only replan when no nodes are running
                        && !dag.is_finished()
                    {
                        completed_since_last_replan = 0;
                        let replan_attempt = replans_done + 1;

                        let workspace = load_workspace_snapshot(task_id, &db);
                        let idle_agents = ctx.agent_registry.list_idle();

                        tracing::info!(
                            "Running replan #{} for task '{}' ({} nodes completed)",
                            replan_attempt,
                            task_id,
                            dag.completed_count()
                        );

                        match Replanner::evaluate(
                            router.as_ref(),
                            dag,
                            &workspace,
                            task_description,
                            &idle_agents,
                            replan_attempt,
                        )
                        .await
                        {
                            Ok(ReplanDecision::Continue) => {
                                replans_done += 1;
                                tracing::info!(
                                    "Replan #{}: plan is on track, continuing",
                                    replan_attempt
                                );
                                bus.publish(SystemEvent::TaskReplanned {
                                    task_id: task_id.to_string(),
                                    replan_number: replan_attempt,
                                    decision: "continue".to_string(),
                                    nodes_added: 0,
                                    nodes_removed: 0,
                                    timestamp: Utc::now(),
                                });
                            }
                            Ok(ReplanDecision::ModifyDag { dag: new_dag }) => {
                                replans_done += 1;
                                let old_pending = dag
                                    .nodes
                                    .iter()
                                    .filter(|n| {
                                        !matches!(
                                            n.status,
                                            DagNodeStatus::Completed | DagNodeStatus::Running
                                        )
                                    })
                                    .count();
                                let new_count = new_dag.nodes.len();

                                tracing::info!(
                                    "Replan #{}: modifying DAG — removing {} pending nodes, adding {} new nodes",
                                    replan_attempt,
                                    old_pending,
                                    new_count
                                );

                                match merge_replanned_dag(dag, &new_dag, &idle_agents) {
                                    Ok(merged) => {
                                        *dag = merged;
                                        mark_ready_nodes(dag);
                                        persist_dag_state(dag, task_id, &db).await;

                                        bus.publish(SystemEvent::TaskReplanned {
                                            task_id: task_id.to_string(),
                                            replan_number: replan_attempt,
                                            decision: "modify".to_string(),
                                            nodes_added: new_count,
                                            nodes_removed: old_pending,
                                            timestamp: Utc::now(),
                                        });
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Replan #{}: merged DAG validation failed: {}, continuing without changes",
                                            replan_attempt,
                                            e
                                        );
                                    }
                                }
                            }
                            Ok(ReplanDecision::Abort { reason }) => {
                                tracing::warn!(
                                    "Replan #{}: aborting task '{}': {} — aborting {} running tasks",
                                    replan_attempt,
                                    task_id,
                                    reason,
                                    running_count
                                );
                                join_set.abort_all();
                                bus.publish(SystemEvent::TaskReplanned {
                                    task_id: task_id.to_string(),
                                    replan_number: replan_attempt,
                                    decision: "abort".to_string(),
                                    nodes_added: 0,
                                    nodes_removed: 0,
                                    timestamp: Utc::now(),
                                });
                                return DagExecutionResult {
                                    success: false,
                                    node_results,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_duration: start.elapsed(),
                                    finish_reason: DagFinishReason::Aborted { reason },
                                };
                            }
                            Err(e) => {
                                // Increment replan_errors to cap total attempts and prevent unbounded LLM cost
                                replan_errors += 1;
                                tracing::warn!(
                                    "Replan #{} failed for task '{}': {}, continuing without changes (errors: {}/{})",
                                    replan_attempt,
                                    task_id,
                                    e,
                                    replan_errors,
                                    config.replan_config.max_replans
                                );
                            }
                        }
                    }
                }
                Err(join_error) => {
                    tracing::error!("JoinSet task panicked or was cancelled: {}", join_error);

                    // A tokio task panicked/cancelled — we can't determine which node
                    // from the JoinError alone. Find nodes still in Running status
                    // and check if they have a corresponding task in the join set.
                    // Since running_count was already decremented above, at least one
                    // Running node has no live task. Fail the first Running node found.
                    let running_node_ids: Vec<String> = dag
                        .nodes
                        .iter()
                        .filter(|n| n.status == DagNodeStatus::Running)
                        .map(|n| n.node_id.clone())
                        .collect();

                    // If the number of Running nodes exceeds running_count, the excess
                    // node(s) are the ones whose task panicked.
                    if running_node_ids.len() > running_count {
                        for nid in &running_node_ids[..running_node_ids.len() - running_count] {
                            let error_msg = format!("Agent task panicked: {}", join_error);
                            dag.fail_node(nid, &error_msg);
                            tracing::error!(
                                "Marked DAG node '{}' as failed due to JoinSet panic",
                                nid
                            );

                            bus.publish(SystemEvent::DagNodeCompleted {
                                task_id: task_id.to_string(),
                                node_id: nid.clone(),
                                node_title: dag
                                    .nodes
                                    .iter()
                                    .find(|n| n.node_id == *nid)
                                    .map(|n| n.title.clone())
                                    .unwrap_or_default(),
                                agent_id: dag
                                    .nodes
                                    .iter()
                                    .find(|n| n.node_id == *nid)
                                    .map(|n| n.agent_id.clone())
                                    .unwrap_or_default(),
                                success: false,
                                duration_ms: 0,
                                output_preview: Some(format!("Task panicked: {}", join_error)),
                                timestamp: Utc::now(),
                            });
                        }
                    }
                }
            }
        } else if running_count == 0 {
            break;
        }
    }

    // Determine overall result
    let all_completed = dag
        .nodes
        .iter()
        .all(|n| n.status == DagNodeStatus::Completed);
    let any_failed = dag.nodes.iter().any(|n| n.status == DagNodeStatus::Failed);

    let finish_reason = if all_completed {
        DagFinishReason::AllCompleted
    } else if any_failed {
        match dag.nodes.iter().find(|n| n.status == DagNodeStatus::Failed) {
            Some(failed_node) => DagFinishReason::NodeFailed {
                node_id: failed_node.node_id.clone(),
                error: failed_node.result_summary.clone().unwrap_or_default(),
            },
            None => DagFinishReason::NodeFailed {
                node_id: "unknown".to_string(),
                error: "Failed node not found in DAG".to_string(),
            },
        }
    } else {
        DagFinishReason::Timeout
    };

    DagExecutionResult {
        success: all_completed,
        node_results,
        total_input_tokens,
        total_output_tokens,
        total_duration: start.elapsed(),
        finish_reason,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Mark nodes whose dependencies are all completed as Ready.
pub(crate) fn mark_ready_nodes(dag: &mut TaskDag) {
    let completed: std::collections::HashSet<String> = dag
        .nodes
        .iter()
        .filter(|n| n.status == DagNodeStatus::Completed)
        .map(|n| n.node_id.clone())
        .collect();

    for node in &mut dag.nodes {
        if node.status == DagNodeStatus::Pending
            && node.depends_on.iter().all(|dep| completed.contains(dep))
        {
            node.status = DagNodeStatus::Ready;
        }
    }
}

#[cfg(test)]
mod tests;
