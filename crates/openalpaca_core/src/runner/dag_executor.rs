//! DAG-aware parallel executor: runs independent sub-tasks concurrently
//! using `tokio::JoinSet`, respecting dependency edges in the TaskDag.

use crate::agent::subagent::SubAgent;
use crate::daemon_config::DaemonConfig;
use crate::orchestrator::replanner::{ReplanConfig, ReplanDecision, Replanner, merge_replanned_dag};
use crate::orchestrator::task_planner::{DagNode, DagNodeStatus, TaskDag};
use arc_swap::ArcSwap;
use crate::orchestrator::task_state::{TaskState, TaskWorkspace, WorkspaceEntryType};
use crate::runner::{LoopConfig, LoopFinishReason, LoopResult, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::ToolRegistry;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use openalpaca_llm::{ChatMessage, LlmRouter};
use openalpaca_storage::repository::LlmUsageRepository;
use openalpaca_storage::Database;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

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
            max_concurrent_agents: 3,
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
        if let Some(ref token) = cancel_token {
            if token.is_cancelled() {
                tracing::info!("DAG execution cancelled for task '{}' — aborting {} running tasks", task_id, running_count);
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
        }

        // Check total timeout
        if start.elapsed() > config.total_timeout {
            tracing::warn!("DAG execution timed out for task '{}' — aborting {} running tasks", task_id, running_count);
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
        let ready_info: Vec<(String, String, DagNode)> = dag
            .ready_nodes()
            .iter()
            .map(|n| (n.node_id.clone(), n.agent_id.clone(), (*n).clone()))
            .collect();

        if ready_info.is_empty() && running_count == 0 {
            break;
        }

        for (node_id, agent_id_str, node_snapshot) in ready_info {
            if running_count >= config.max_concurrent_agents {
                break;
            }

            // Verify node is still in a dispatchable state (may have changed during replanning).
            // Accept both Pending and Ready — mark_ready_nodes() promotes Pending → Ready
            // when dependencies are satisfied, so Ready is a valid dispatchable state.
            if let Some(current_node) = dag.nodes.iter().find(|n| n.node_id == node_id) {
                if !matches!(current_node.status, DagNodeStatus::Pending | DagNodeStatus::Ready) {
                    tracing::debug!("Node '{}' is not dispatchable (now {:?}), skipping", node_id, current_node.status);
                    continue;
                }
            } else {
                tracing::warn!("Node '{}' no longer exists in DAG, skipping", node_id);
                continue;
            }

            dag.mark_running(&node_id);
            running_count += 1;

            persist_dag_state(dag, task_id, &db);

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
                    persist_dag_state(dag, task_id, &db);

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
                        failed_nodes: dag.nodes.iter().filter(|n| n.status == DagNodeStatus::Failed).count(),
                    };
                    ctx.task_registry.update_progress(task_id, completed as i32, total as i32, Some(dag_summary));

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
                            node_id, task_id
                        );

                        // Write output to workspace
                        write_node_output_to_workspace(
                            dag, &node_result, task_id, &db,
                        ).await;

                        // Mark completed in DAG
                        dag.complete_node(&node_id, &node_result.final_content);
                        mark_ready_nodes(dag);
                        completed_since_last_replan += 1;
                    } else {
                        let retries = retry_counts.entry(node_id.clone()).or_insert(0);
                        if *retries < config.max_retries_per_node {
                            tracing::warn!(
                                "DAG node '{}' failed, retrying ({}/{})",
                                node_id, *retries + 1, config.max_retries_per_node
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
                                node_id, task_id
                            );
                            let error = node_result.final_content.clone();
                            dag.fail_node(&node_id, &error);
                        }
                    }

                    persist_dag_state(dag, task_id, &db);

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
                        failed_nodes: dag.nodes.iter().filter(|n| n.status == DagNodeStatus::Failed).count(),
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
                            replan_attempt, task_id, dag.completed_count()
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
                                tracing::info!("Replan #{}: plan is on track, continuing", replan_attempt);
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
                                let old_pending = dag.nodes.iter()
                                    .filter(|n| !matches!(n.status, DagNodeStatus::Completed | DagNodeStatus::Running))
                                    .count();
                                let new_count = new_dag.nodes.len();

                                tracing::info!(
                                    "Replan #{}: modifying DAG — removing {} pending nodes, adding {} new nodes",
                                    replan_attempt, old_pending, new_count
                                );

                                match merge_replanned_dag(dag, &new_dag, &idle_agents) {
                                    Ok(merged) => {
                                        *dag = merged;
                                        mark_ready_nodes(dag);
                                        persist_dag_state(dag, task_id, &db);

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
                                            replan_attempt, e
                                        );
                                    }
                                }
                            }
                            Ok(ReplanDecision::Abort { reason }) => {
                                tracing::warn!(
                                    "Replan #{}: aborting task '{}': {} — aborting {} running tasks",
                                    replan_attempt, task_id, reason, running_count
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
                                    replan_attempt, task_id, e, replan_errors, config.replan_config.max_replans
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
                    let running_node_ids: Vec<String> = dag.nodes.iter()
                        .filter(|n| n.status == DagNodeStatus::Running)
                        .map(|n| n.node_id.clone())
                        .collect();

                    // If the number of Running nodes exceeds running_count, the excess
                    // node(s) are the ones whose task panicked.
                    if running_node_ids.len() > running_count {
                        for nid in &running_node_ids[..running_node_ids.len() - running_count] {
                            let error_msg = format!("Agent task panicked: {}", join_error);
                            dag.fail_node(nid, &error_msg);
                            tracing::error!("Marked DAG node '{}' as failed due to JoinSet panic", nid);

                            bus.publish(SystemEvent::DagNodeCompleted {
                                task_id: task_id.to_string(),
                                node_id: nid.clone(),
                                node_title: dag.nodes.iter()
                                    .find(|n| n.node_id == *nid)
                                    .map(|n| n.title.clone())
                                    .unwrap_or_default(),
                                agent_id: dag.nodes.iter()
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
    let all_completed = dag.nodes.iter().all(|n| n.status == DagNodeStatus::Completed);
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
) -> NodeResult {
    let agent_id = agent.id.clone();

    // Build ContextualToolExecutor for workspace access
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

    // Build LoopConfig — agent constraints override daemon defaults
    let ad = &daemon_config.load().execution.agent_defaults;
    let loop_config = LoopConfig {
        max_rounds: ad.max_rounds,
        max_tools_per_round: ad.max_tools_per_round,
        max_tool_runtime: std::cmp::min(
            node_timeout,
            Duration::from_secs(agent.constraints.timeout_seconds.unwrap_or(ad.max_tool_runtime_secs)),
        ),
        max_cost: agent.constraints.max_cost_per_task.unwrap_or(ad.max_cost),
        model: agent.llm_config.model.clone(),
        fallback_models: agent.llm_config.fallback_models.clone(),
    };

    let sandbox_policy = SandboxPolicy::from_constraints(&agent_id, &agent.constraints);

    // Resolve tools via shared helper
    let tools = crate::tools::resolve_agent_tools(&agent, &tool_registry);

    // Build system prompt
    let tool_guidance = format_tool_guidance(&tools);
    let system_prompt = format!(
        "{}\n\nYour role: {}\n\nSub-task: {}\n\nComplete your assigned sub-task to the best of your ability.{}",
        agent.preset.persona, node.description, node.title, tool_guidance
    );

    // Build messages: system + task + workspace context for this node
    let mut messages = vec![
        ChatMessage::system(&system_prompt),
        ChatMessage::user(&task_description),
    ];

    // Inject workspace entries that this node depends on
    let workspace_context = load_workspace_context(&task_id, &db, &node.workspace_keys);
    if !workspace_context.is_empty() {
        messages.push(ChatMessage::user(&format!(
            "The following shared workspace contains results from previous sub-tasks. \
             Use this information to complete your assigned sub-task. You can also use the \
             workspace_read and workspace_write tools to access or update entries.\n\n{}",
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
        node.node_id, agent_id, result.finish_reason,
        result.rounds_used, result.total_input_tokens, result.total_output_tokens
    );

    let success = matches!(
        &result.finish_reason,
        LoopFinishReason::Complete | LoopFinishReason::MaxRounds
    );

    // Record LLM usage
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
        LoopFinishReason::Cancelled => "cancelled",
        LoopFinishReason::Error(_) => "error",
    };
    let call_error = match &result.finish_reason {
        LoopFinishReason::Error(msg) => Some(msg.as_str()),
        _ => None,
    };

    if let Some(db) = &db {
        let usage_repo = LlmUsageRepository::new(db);
        if let Err(e) = usage_repo.record_and_log(
            &agent_id,
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
            tracing::warn!("Failed to persist LLM usage for node '{}': {e}", node.node_id);
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

    // Record agent history
    if let Some(db) = &db {
        let subagent_repo = openalpaca_storage::SubAgentRepository::new(db);
        let history_entry = openalpaca_storage::AgentTaskHistory {
            id: Uuid::new_v4().to_string(),
            agent_id: agent_id.clone(),
            task_id: task_id.clone(),
            role: format!("dag_node:{}", node.node_id),
            status: if success { "completed" } else { "failed" }.to_string(),
            runtime_seconds: Some(agent_runtime),
            completed_at: Utc::now(),
        };
        if let Err(e) = subagent_repo.add_history(&history_entry) {
            tracing::warn!("Failed to record agent task history: {e}");
        }
        if success {
            let _ = subagent_repo.increment_completed(&agent_id, agent_runtime);
        } else {
            let _ = subagent_repo.increment_failed(&agent_id);
        }
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
fn mark_ready_nodes(dag: &mut TaskDag) {
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
        let protected_keys: Vec<String> = dag.nodes.iter()
            .filter(|n| matches!(n.status, DagNodeStatus::Pending | DagNodeStatus::Ready | DagNodeStatus::Running))
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
                key, node_result.node_id, e
            );
            return;
        }

        match repo.update_state(task_id, &state.to_json(), existing.state_version) {
            Ok(true) => return, // success
            Ok(false) => {
                if attempt < MAX_RETRIES - 1 {
                    tracing::debug!(
                        "Workspace write version conflict for key '{}' node '{}' (attempt {}/{}), retrying",
                        key, node_result.node_id, attempt + 1, MAX_RETRIES
                    );
                    // Brief async backoff to reduce collision probability
                    tokio::time::sleep(std::time::Duration::from_millis(10 * (1 << attempt))).await;
                } else {
                    tracing::warn!(
                        "Workspace write for key '{}' node '{}' failed after {} retries — data may be lost",
                        key, node_result.node_id, MAX_RETRIES
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to persist workspace entry '{}' for node '{}': {}",
                    key, node_result.node_id, e
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
fn persist_dag_state(dag: &TaskDag, task_id: &str, db: &Option<Database>) {
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
                        task_id, attempt + 1, MAX_RETRIES
                    );
                } else {
                    tracing::warn!(
                        "DAG state persist for task '{}' failed after {} retries — state may be stale",
                        task_id, MAX_RETRIES
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to persist DAG state for task '{}': {}",
                    task_id, e
                );
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, agent_id: &str, deps: &[&str]) -> DagNode {
        DagNode {
            node_id: id.to_string(),
            title: format!("Task {}", id),
            description: format!("Do {}", id),
            agent_id: agent_id.to_string(),
            agent_name: format!("Agent {}", agent_id),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            status: DagNodeStatus::Pending,
            result_summary: None,
            workspace_keys: vec![],
            output_key: Some(format!("{}_output", id)),
        }
    }

    #[test]
    fn test_mark_ready_nodes_initial() {
        let mut dag = TaskDag {
            nodes: vec![
                make_node("n1", "a1", &[]),
                make_node("n2", "a1", &[]),
                make_node("n3", "a1", &["n1", "n2"]),
            ],
        };
        mark_ready_nodes(&mut dag);
        assert_eq!(dag.nodes[0].status, DagNodeStatus::Ready);
        assert_eq!(dag.nodes[1].status, DagNodeStatus::Ready);
        assert_eq!(dag.nodes[2].status, DagNodeStatus::Pending);
    }

    #[test]
    fn test_mark_ready_nodes_after_completion() {
        let mut dag = TaskDag {
            nodes: vec![
                make_node("n1", "a1", &[]),
                make_node("n2", "a1", &["n1"]),
            ],
        };
        dag.nodes[0].status = DagNodeStatus::Completed;
        mark_ready_nodes(&mut dag);
        assert_eq!(dag.nodes[1].status, DagNodeStatus::Ready);
    }

    #[test]
    fn test_mark_ready_nodes_partial_deps() {
        let mut dag = TaskDag {
            nodes: vec![
                make_node("n1", "a1", &[]),
                make_node("n2", "a1", &[]),
                make_node("n3", "a1", &["n1", "n2"]),
            ],
        };
        dag.nodes[0].status = DagNodeStatus::Completed;
        mark_ready_nodes(&mut dag);
        assert_eq!(dag.nodes[2].status, DagNodeStatus::Pending);

        dag.nodes[1].status = DagNodeStatus::Completed;
        mark_ready_nodes(&mut dag);
        assert_eq!(dag.nodes[2].status, DagNodeStatus::Ready);
    }

    #[test]
    fn test_default_config() {
        let config = DagExecutorConfig::default();
        assert_eq!(config.max_concurrent_agents, 3);
        assert_eq!(config.node_timeout, Duration::from_secs(300));
        assert_eq!(config.total_timeout, Duration::from_secs(1800));
        assert_eq!(config.max_retries_per_node, 1);
    }

    #[test]
    fn test_dag_finish_reason_debug() {
        let reason = DagFinishReason::AllCompleted;
        let debug_str = format!("{:?}", reason);
        assert!(debug_str.contains("AllCompleted"));

        let reason = DagFinishReason::NodeFailed {
            node_id: "n1".to_string(),
            error: "oops".to_string(),
        };
        let debug_str = format!("{:?}", reason);
        assert!(debug_str.contains("n1"));
    }

    #[test]
    fn test_dag_execution_result_fields() {
        let result = DagExecutionResult {
            success: true,
            node_results: vec![],
            total_input_tokens: 100,
            total_output_tokens: 50,
            total_duration: Duration::from_secs(5),
            finish_reason: DagFinishReason::AllCompleted,
        };
        assert!(result.success);
        assert_eq!(result.total_input_tokens, 100);
        assert_eq!(result.total_output_tokens, 50);
    }
}
