//! Dynamic replanning: evaluates progress after DAG node completions
//! and decides whether to continue, modify the remaining DAG, or abort.

use crate::agent::subagent::SubAgent;
use crate::compose::{
    ComposeEngine, ComposeOverrides, ComposeRequest, DynamicContextInput, DynamicContextMode,
    HistoryInput, HistoryMode, PersonaInput, PersonaMode, PlanState, SectionPriority,
    StaticPromptInput, StaticPromptMode, SummaryWrapMode, SystemBlock, WorkspaceSnapshot,
};
use crate::middleware::prompt::SystemPersona;
use crate::orchestrator::task_planner::{DagNode, DagNodeStatus, TaskDag, extract_json_block};
use crate::orchestrator::task_state::TaskWorkspace;
use crate::prompt_ctx::{ContextBundle, ExecutionPath};
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Configuration ────────────────────────────────────────────────────

/// Configuration for the replanner.
#[derive(Debug, Clone)]
pub struct ReplanConfig {
    /// Whether replanning is enabled.
    pub enabled: bool,
    /// Replan after every N node completions.
    pub replan_after_every_n_nodes: usize,
    /// Maximum number of replans allowed per DAG execution.
    pub max_replans: usize,
}

impl Default for ReplanConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            replan_after_every_n_nodes: 2,
            max_replans: 3,
        }
    }
}

// ── Decision types ───────────────────────────────────────────────────

/// The result of a replan evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ReplanDecision {
    /// Plan is on track, no changes needed.
    Continue,
    /// Replace remaining DAG with a new version.
    ModifyDag { dag: TaskDag },
    /// Give up — task is unachievable or no longer makes sense.
    Abort { reason: String },
}

// ── Replanner ────────────────────────────────────────────────────────

pub struct Replanner;

impl Replanner {
    /// Evaluate the current DAG progress and decide whether to replan.
    ///
    /// Sends the original objective, current DAG state (completed/pending/failed),
    /// workspace contents, and available agents to the LLM.  Returns one of:
    /// - `Continue` — plan is on track
    /// - `ModifyDag` — new DAG for remaining work
    /// - `Abort` — task cannot be completed
    ///
    /// Routes prompt assembly through the layered compose engine (Phase 4
    /// Commit 2). The previous `build_replan_prompt` helper was absorbed into
    /// `compose::static_prompt::build_replanner_hierarchical`; this method
    /// serializes the structured inputs (DAG state, workspace, original
    /// objective, replans_so_far) into `StaticPromptInput.raw_blocks` before
    /// calling `compose_engine.compose(...)`.
    pub async fn evaluate(
        router: &LlmRouter,
        compose_engine: &ComposeEngine,
        dag: &TaskDag,
        workspace: &TaskWorkspace,
        original_objective: &str,
        idle_agents: &[SubAgent],
        replans_so_far: usize,
    ) -> Result<ReplanDecision, String> {
        // --- Render the DAG state block. ---
        let mut dag_block = String::from("<dag_state>\n");
        for node in &dag.nodes {
            let status = match &node.status {
                DagNodeStatus::Completed => {
                    let summary = node.result_summary.as_deref().unwrap_or("(no summary)");
                    let capped: String = summary.chars().take(200).collect();
                    format!("COMPLETED — {}", capped)
                }
                DagNodeStatus::Failed => {
                    let err = node.result_summary.as_deref().unwrap_or("(no error)");
                    let capped: String = err.chars().take(200).collect();
                    format!("FAILED — {}", capped)
                }
                DagNodeStatus::Skipped => "SKIPPED".to_string(),
                DagNodeStatus::Running => "RUNNING".to_string(),
                DagNodeStatus::Ready => "READY (waiting to run)".to_string(),
                DagNodeStatus::Pending => "PENDING (dependencies not met)".to_string(),
            };
            dag_block.push_str(&format!(
                "- [{}] \"{}\" (agent: {}) — {}\n",
                node.node_id, node.title, node.agent_name, status
            ));
        }
        dag_block.push_str("</dag_state>\n\n");

        // --- Render workspace block (only if non-empty). ---
        let workspace_summary = workspace.format_for_prompt(&[]);

        // --- Assemble raw_blocks in the loader-defined order:
        //     "original_objective", "dag_state", "workspace" (optional), "context".
        //     `build_replanner_hierarchical` emits them in raw_blocks iteration
        //     order, so this ordering is load-bearing. ---
        let mut raw_blocks: Vec<SystemBlock> = Vec::with_capacity(4);
        raw_blocks.push(SystemBlock {
            name: "original_objective",
            content: Arc::<str>::from(format!(
                "<original_objective>\n{}\n</original_objective>\n\n",
                original_objective
            )),
            priority: SectionPriority::High,
        });
        raw_blocks.push(SystemBlock {
            name: "dag_state",
            content: Arc::<str>::from(dag_block),
            priority: SectionPriority::High,
        });
        if !workspace_summary.is_empty() {
            raw_blocks.push(SystemBlock {
                name: "workspace",
                content: Arc::<str>::from(format!(
                    "<workspace>\n{}</workspace>\n\n",
                    workspace_summary
                )),
                priority: SectionPriority::Normal,
            });
        }
        raw_blocks.push(SystemBlock {
            name: "context",
            content: Arc::<str>::from(format!(
                "<context>\nReplans so far: {} (be conservative — avoid unnecessary changes)\n</context>\n\n",
                replans_so_far
            )),
            priority: SectionPriority::Normal,
        });

        // --- Persona: Minimal mode. The replanner doesn't surface persona in
        //     the final prompt — `build_replanner_hierarchical` ignores
        //     persona_output.blocks entirely — but the PersonaInput is still
        //     required by Layer 1 so cache shape stays uniform. ---
        let persona_input = PersonaInput {
            system_persona: Arc::new(SystemPersona::default()),
            user_document: Arc::new(None),
            identity_document: Arc::new(None),
            persona_version: 0,
            mode: PersonaMode::Minimal,
            identity_budget: None,
        };
        let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

        let static_prompt_input = StaticPromptInput {
            persona_output,
            agent_persona: None,
            agent_config_fingerprint: [0u8; 32],
            skill_block: None,
            skills_catalog: None,
            bootstrap: None,
            tools: Arc::new(Vec::new()),
            connector_status: Arc::new(Vec::new()),
            send_tool_context: None,
            message_source: None,
            raw_blocks,
            planner_agents: Some(Arc::new(idle_agents.to_vec())),
            planner_protocol_v2: false,
            mode: StaticPromptMode::ReplannerHierarchical,
            model_window: 8192,
        };

        let dynamic_context_input = DynamicContextInput {
            context_bundle: Arc::new(ContextBundle::empty()),
            query: Arc::from(""),
            memory_retrieval_hash: [0u8; 32],
            path: ExecutionPath::SimpleQuery,
            reserved_tokens: 0,
            mode: DynamicContextMode::Skip,
        };

        let history_input = HistoryInput {
            lane_tip_fingerprint: [0u8; 32],
            summary: None,
            summary_wrap_mode: SummaryWrapMode::Plain,
            recent_messages: Arc::new(Vec::new()),
            current_user_turn: Some(ChatMessage::user(
                "Evaluate the current task progress and decide whether to continue, \
                 modify the plan, or abort.",
            )),
            mode: HistoryMode::Default,
        };

        let request = ComposeRequest::Replanner {
            current_plan: Arc::new(PlanState::default()),
            workspace_snapshot: Arc::new(WorkspaceSnapshot::default()),
            overrides: ComposeOverrides::default(),
        };

        let composed = compose_engine.compose(
            &request,
            persona_input,
            static_prompt_input,
            dynamic_context_input,
            history_input,
            8192,
            Arc::new(Vec::new()),
            None, // Replanner::evaluate has no EventBus handle in scope — ok.
            None, // No per-lane cache for the replanner.
        );

        let messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

        let request_out = RouterRequest {
            model: None,
            messages: Arc::new(messages),
            tools: Arc::new(vec![]),
            temperature: Some(0.0),
            max_tokens: Some(2048),
            context: RequestContext::default(),
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
            context_management: None,
            fallback_models: Vec::new(),
            ephemeral_system_notice: None,
        };

        let response = router
            .complete(request_out)
            .await
            .map_err(|e| format!("Replanner LLM error: {e}"))?;

        Self::parse_decision(&response.content, idle_agents)
    }

    /// Parse the LLM response into a ReplanDecision.
    fn parse_decision(
        content: &str,
        available_agents: &[SubAgent],
    ) -> Result<ReplanDecision, String> {
        let json_str = extract_json_block(content);

        // Try direct parse
        if let Ok(decision) = serde_json::from_str::<ReplanDecision>(json_str) {
            // Validate modified DAG if present
            if let ReplanDecision::ModifyDag { ref dag } = decision {
                dag.validate(available_agents)
                    .map_err(|e| format!("Replanned DAG validation failed: {e}"))?;
            }
            return Ok(decision);
        }

        // Fallback: extract from loose Value
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json_str)
            && let Some(decision_str) = obj.get("decision").and_then(|v| v.as_str())
        {
            return match decision_str {
                "continue" => Ok(ReplanDecision::Continue),
                "abort" => {
                    let reason = obj
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("No reason provided")
                        .to_string();
                    Ok(ReplanDecision::Abort { reason })
                }
                "modify_dag" => {
                    let dag: TaskDag = obj
                        .get("dag")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .ok_or("modify_dag decision missing valid 'dag' field")?;
                    dag.validate(available_agents)
                        .map_err(|e| format!("Replanned DAG validation failed: {e}"))?;
                    Ok(ReplanDecision::ModifyDag { dag })
                }
                other => Err(format!("Unknown decision: '{other}'")),
            };
        }

        // If all parsing fails, default to continue (conservative)
        tracing::warn!(
            "Replanner: failed to parse response, defaulting to Continue. Raw: {}",
            &content.chars().take(200).collect::<String>()
        );
        Ok(ReplanDecision::Continue)
    }
}

// ── Merge logic ──────────────────────────────────────────────────────

/// Merge a replanned DAG into the existing DAG:
/// - Keep all COMPLETED and RUNNING nodes from the old DAG
/// - Replace PENDING/READY/SKIPPED nodes with nodes from the new DAG
/// - Validate the merged result: structure (deps, cycles) AND agent existence
///
/// Returns Err if the merged DAG has dangling dependency references,
/// cycles, or references agents not in `available_agents`.
pub fn merge_replanned_dag(
    existing: &TaskDag,
    new_dag: &TaskDag,
    available_agents: &[SubAgent],
) -> Result<TaskDag, String> {
    let mut merged_nodes: Vec<DagNode> = Vec::new();

    // Keep completed and running nodes
    for node in &existing.nodes {
        if matches!(
            node.status,
            DagNodeStatus::Completed | DagNodeStatus::Running
        ) {
            merged_nodes.push(node.clone());
        }
    }

    // Add all new nodes (from the replanned DAG)
    for node in &new_dag.nodes {
        // Skip if a node with the same ID already exists (from completed/running)
        if merged_nodes.iter().any(|n| n.node_id == node.node_id) {
            tracing::warn!(
                "Replanned DAG contains duplicate node_id '{}' — skipping (already completed/running)",
                node.node_id
            );
            continue;
        }
        merged_nodes.push(node.clone());
    }

    // Build merged DAG and validate structure (dependencies + cycles)
    let merged = TaskDag {
        nodes: merged_nodes,
    };

    // Full validation: dependency existence, cycle detection, AND agent existence
    merged
        .validate(available_agents)
        .map_err(|e| format!("Merged DAG validation failed: {}", e))?;

    // Warn about agents that exist but aren't currently available
    for node in &merged.nodes {
        if matches!(node.status, DagNodeStatus::Pending)
            && let Some(agent) = available_agents.iter().find(|a| a.id == node.agent_id)
            && !agent.status.is_available()
        {
            tracing::warn!(
                "Merged DAG node '{}' references agent '{}' which is currently {:?}",
                node.node_id,
                agent.id,
                agent.status
            );
        }
    }

    Ok(merged)
}

#[cfg(test)]
mod tests;
