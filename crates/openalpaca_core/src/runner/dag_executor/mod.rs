//! DAG-aware parallel executor: runs independent sub-tasks concurrently
//! using `tokio::JoinSet`, respecting dependency edges in the TaskDag.

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

// ── Single node execution ────────────────────────────────────────────

/// Execute a single DAG node: build context, run agentic loop, return result.
/// All parameters are owned because this runs inside a spawned task.
#[allow(clippy::too_many_arguments)]
async fn execute_single_node(
    node: DagNode,
    router: Arc<LlmRouter>,
    tool_registry: Arc<ToolRegistry>,
    bus: EventBus,
    db: Option<Database>,
    task_id: String,
    task_description: String,
    created_by: String,
    agent: SubAgent,
    node_timeout: Duration,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    cancel_token: Option<CancellationToken>,
    workspace_id: Option<String>,
    connector_guidance: String,
    workspace_snapshot: Arc<Option<TaskState>>,
    confirmation_broker: Option<Arc<crate::security::confirmation::ConfirmationBroker>>,
) -> NodeResult {
    let agent_id = agent.id.clone();

    // Build ContextualToolExecutor for workspace access
    let ctx_exec = ToolExecutionContext {
        owner_id: Some(created_by.clone()),
        task_id: Some(task_id.clone()),
        agent_id: Some(agent_id.clone()),
        db: db.clone(),
        workspace_id,
    };
    let contextual_executor =
        Arc::new(ContextualToolExecutor::new(tool_registry.clone(), ctx_exec));
    let mut per_request_sandbox = SandboxManager::with_defaults(contextual_executor, bus.clone());
    if let Some(broker) = confirmation_broker {
        per_request_sandbox.set_confirmation_broker(broker);
    }

    // Build LoopConfig — agent constraints override daemon defaults, cap at node timeout
    let mut loop_config =
        LoopConfig::from_agent(&daemon_config.load().execution.agent_defaults, &agent)
            .with_context_window(router.model_registry(), agent.llm_config.model.as_deref());
    loop_config.max_tool_runtime = std::cmp::min(node_timeout, loop_config.max_tool_runtime);

    let mut sandbox_policy = SandboxPolicy::from_constraints(&agent_id, &agent.constraints);
    if daemon_config.load().security.auto_approve_confirmations {
        sandbox_policy.auto_approve = true;
    }

    // Resolve tools via shared helper
    let tools = crate::tools::resolve_agent_tools(&agent, &tool_registry);

    // Build system prompt
    let tool_guidance = format_tool_guidance(&tools);
    let connector_suffix = if !connector_guidance.is_empty() {
        format!("\n{}", connector_guidance)
    } else {
        String::new()
    };
    let system_prompt = format!(
        "<identity>\n{}\n</identity>\n\n\
         <assignment>\n\
         Sub-task: {}\n\
         Description: {}\n\
         </assignment>\n\n\
         <scope>\n\
         You are responsible for completing only the sub-task described above. \
         Do not attempt work outside your assignment. Your output will be stored \
         in the workspace for downstream nodes that depend on your results.\n\
         </scope>\n\n\
         <output-format>\n\
         Provide a complete, self-contained result for your sub-task. Other agents \
         will consume your output, so be specific and include all relevant details.\n\
         </output-format>{}{}",
        agent.preset.persona, node.title, node.description, tool_guidance, connector_suffix
    );

    // Build messages: system + task + workspace context for this node
    let mut messages = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user(&task_description),
    ];

    // Inject workspace entries that this node depends on.
    // Uses pre-loaded snapshot (Opt-7c) to avoid redundant DB reads per node.
    let workspace_context = if let Some(ref state) = *workspace_snapshot {
        state.workspace.format_for_prompt(&node.workspace_keys)
    } else {
        load_workspace_context(&task_id, &db, &node.workspace_keys)
    };
    if !workspace_context.is_empty() {
        messages.push(ChatMessage::user(&format!(
            "<workspace>\n\
             The following entries contain results from upstream sub-tasks. \
             Use this data to complete your assignment. You can also call \
             workspace_read and workspace_write to access or update entries.\n\n\
             {}\n\
             </workspace>",
            workspace_context
        )));
    }

    // Run agentic loop
    let agent_start = Instant::now();
    let result = run_agentic_loop_routed(
        router.as_ref(),
        messages,
        tools,
        &loop_config,
        Some(&per_request_sandbox),
        &agent_id,
        Some(&sandbox_policy),
        Some(&task_id),
        cancel_token,
    )
    .await;

    let agent_runtime = agent_start.elapsed().as_secs() as i64;

    tracing::info!(
        "DAG node '{}' (agent '{}'): reason={:?}, rounds={}, tokens={}/{}",
        node.node_id,
        agent_id,
        result.finish_reason,
        result.rounds_used,
        result.total_input_tokens,
        result.total_output_tokens
    );

    let success = matches!(
        &result.finish_reason,
        LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated
    );

    // Record LLM usage
    crate::orchestrator::dispatcher::usage::record_llm_usage(
        &router,
        &result,
        loop_config.model.as_deref(),
        &agent_id,
        &task_id,
        agent_start.elapsed().as_millis() as i64,
        db.as_ref(),
        &bus,
    );

    // Record agent history
    if let Some(db) = &db {
        let role = format!("dag_node:{}", node.node_id);
        crate::orchestrator::dispatcher::usage::record_agent_history(
            db,
            &agent_id,
            &task_id,
            &role,
            success,
            agent_runtime,
        );
    }

    NodeResult {
        node_id: node.node_id.clone(),
        node_title: node.title.clone(),
        agent_id,
        success,
        final_content: result.final_content.clone(),
        duration_ms: agent_start.elapsed().as_millis() as u64,
        loop_result: result,
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

/// Load workspace context filtered by the node's workspace_keys.
/// Load the full TaskState from the DB — used by Opt-7c to pre-load a shared
/// workspace snapshot once per batch instead of once per concurrent node.
fn load_task_state(task_id: &str, db: &Option<Database>) -> Option<TaskState> {
    let db = db.as_ref()?;
    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    let task = repo.get(task_id).ok()??;
    task.state_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
}

fn load_workspace_context(
    task_id: &str,
    db: &Option<Database>,
    workspace_keys: &[String],
) -> String {
    let Some(db) = db else {
        return String::new();
    };

    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    let task = match repo.get(task_id) {
        Ok(Some(t)) => t,
        _ => return String::new(),
    };

    let state: TaskState = match task.state_json.as_deref() {
        Some(json) => match serde_json::from_str(json) {
            Ok(s) => s,
            Err(_) => return String::new(),
        },
        None => return String::new(),
    };

    state.workspace.format_for_prompt(workspace_keys)
}

/// Write a completed node's output to the workspace under its output_key.
/// Uses a retry loop (max 3 attempts) to handle optimistic locking conflicts
/// when concurrent nodes complete simultaneously.
async fn write_node_output_to_workspace(
    dag: &TaskDag,
    node_result: &NodeResult,
    task_id: &str,
    db: &Option<Database>,
) {
    let Some(db) = db else { return };

    // Find the node's output_key
    let output_key = dag
        .nodes
        .iter()
        .find(|n| n.node_id == node_result.node_id)
        .and_then(|n| n.output_key.as_deref());

    let Some(key) = output_key else { return };
    if node_result.final_content.is_empty() {
        return;
    }

    const MAX_RETRIES: usize = 3;
    for attempt in 0..MAX_RETRIES {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let existing = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };
        let sj = match &existing.state_json {
            Some(s) => s,
            None => return,
        };
        let mut state: TaskState = match serde_json::from_str(sj) {
            Ok(s) => s,
            Err(_) => return,
        };

        // Collect workspace_keys from nodes that haven't completed yet
        let protected_keys: Vec<String> = dag
            .nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.status,
                    DagNodeStatus::Pending | DagNodeStatus::Ready | DagNodeStatus::Running
                )
            })
            .flat_map(|n| n.workspace_keys.iter().cloned())
            .collect();

        if let Err(e) = state.workspace.write(
            key,
            &node_result.final_content,
            &node_result.agent_id,
            WorkspaceEntryType::Context,
            &protected_keys,
        ) {
            tracing::warn!(
                "Failed to write workspace entry '{}' for node '{}': {}",
                key,
                node_result.node_id,
                e
            );
            return;
        }

        match repo.update_state(task_id, &state.to_json(), existing.state_version) {
            Ok(true) => return, // success
            Ok(false) => {
                if attempt < MAX_RETRIES - 1 {
                    tracing::debug!(
                        "Workspace write version conflict for key '{}' node '{}' (attempt {}/{}), retrying",
                        key,
                        node_result.node_id,
                        attempt + 1,
                        MAX_RETRIES
                    );
                    // Brief async backoff to reduce collision probability
                    tokio::time::sleep(std::time::Duration::from_millis(10 * (1 << attempt))).await;
                } else {
                    tracing::warn!(
                        "Workspace write for key '{}' node '{}' failed after {} retries — data may be lost",
                        key,
                        node_result.node_id,
                        MAX_RETRIES
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to persist workspace entry '{}' for node '{}': {}",
                    key,
                    node_result.node_id,
                    e
                );
                return;
            }
        }
    }
}

/// Load a snapshot of the workspace from the task's persisted state.
fn load_workspace_snapshot(task_id: &str, db: &Option<Database>) -> TaskWorkspace {
    let Some(db) = db else {
        return TaskWorkspace::default();
    };

    let repo = openalpaca_storage::repository::TaskRepository::new(db);
    let task = match repo.get(task_id) {
        Ok(Some(t)) => t,
        _ => return TaskWorkspace::default(),
    };

    match task.state_json.as_deref() {
        Some(json) => match serde_json::from_str::<TaskState>(json) {
            Ok(s) => s.workspace,
            Err(_) => TaskWorkspace::default(),
        },
        None => TaskWorkspace::default(),
    }
}

/// Persist the current DAG state back to the task's state_json.
/// Uses a retry loop (max 3 attempts) to handle optimistic locking conflicts
/// when concurrent nodes trigger DAG state updates simultaneously.
async fn persist_dag_state(dag: &TaskDag, task_id: &str, db: &Option<Database>) {
    let Some(db) = db else { return };

    const MAX_RETRIES: usize = 3;
    for attempt in 0..MAX_RETRIES {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let existing = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return,
        };
        let sj = match &existing.state_json {
            Some(s) => s,
            None => return,
        };
        let mut state: TaskState = match serde_json::from_str(sj) {
            Ok(s) => s,
            Err(_) => return,
        };

        state.dag = Some(dag.clone());

        match repo.update_state(task_id, &state.to_json(), existing.state_version) {
            Ok(true) => return, // success
            Ok(false) => {
                if attempt < MAX_RETRIES - 1 {
                    tracing::debug!(
                        "DAG state persist version conflict for task '{}' (attempt {}/{}), retrying",
                        task_id,
                        attempt + 1,
                        MAX_RETRIES
                    );
                    // Brief async backoff to reduce collision probability
                    tokio::time::sleep(Duration::from_millis(10 * (1 << attempt))).await;
                } else {
                    tracing::warn!(
                        "DAG state persist for task '{}' failed after {} retries — state may be stale",
                        task_id,
                        MAX_RETRIES
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Failed to persist DAG state for task '{}': {}", task_id, e);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
