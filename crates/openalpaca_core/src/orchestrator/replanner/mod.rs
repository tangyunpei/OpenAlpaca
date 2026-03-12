//! Dynamic replanning: evaluates progress after DAG node completions
//! and decides whether to continue, modify the remaining DAG, or abort.

use crate::agent::subagent::SubAgent;
use crate::orchestrator::task_planner::{DagNode, DagNodeStatus, TaskDag, extract_json_block};
use crate::orchestrator::task_state::TaskWorkspace;
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
    pub async fn evaluate(
        router: &LlmRouter,
        dag: &TaskDag,
        workspace: &TaskWorkspace,
        original_objective: &str,
        idle_agents: &[SubAgent],
        replans_so_far: usize,
    ) -> Result<ReplanDecision, String> {
        let prompt = Self::build_replan_prompt(
            dag,
            workspace,
            original_objective,
            idle_agents,
            replans_so_far,
        );

        let messages = vec![
            ChatMessage::system(&prompt),
            ChatMessage::user(
                "Evaluate the current task progress and decide whether to continue, \
                 modify the plan, or abort.",
            ),
        ];

        let request = RouterRequest {
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
        };

        let response = router
            .complete(request)
            .await
            .map_err(|e| format!("Replanner LLM error: {e}"))?;

        Self::parse_decision(&response.content, idle_agents)
    }

    /// Build the system prompt for the replanner.
    fn build_replan_prompt(
        dag: &TaskDag,
        workspace: &TaskWorkspace,
        original_objective: &str,
        idle_agents: &[SubAgent],
        replans_so_far: usize,
    ) -> String {
        let mut prompt = String::from(
            "You are a task replanner for OpenAlpaca. Evaluate whether the current \
             execution plan is still on track or needs modification.\n\n",
        );

        // Original objective
        prompt.push_str(&format!(
            "<original_objective>\n{}\n</original_objective>\n\n",
            original_objective
        ));

        // Current DAG state
        prompt.push_str("<dag_state>\n");
        for node in &dag.nodes {
            let status = match &node.status {
                DagNodeStatus::Completed => {
                    let summary = node.result_summary.as_deref().unwrap_or("(no summary)");
                    // Cap summary to 200 chars for prompt
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
            prompt.push_str(&format!(
                "- [{}] \"{}\" (agent: {}) — {}\n",
                node.node_id, node.title, node.agent_name, status
            ));
        }
        prompt.push_str("</dag_state>\n\n");

        // Workspace contents (summaries only)
        let workspace_summary = workspace.format_for_prompt(&[]);
        if !workspace_summary.is_empty() {
            prompt.push_str("<workspace>\n");
            prompt.push_str(&workspace_summary);
            prompt.push_str("</workspace>\n\n");
        }

        // Available agents
        prompt.push_str("<available_agents>\n");
        if idle_agents.is_empty() {
            prompt.push_str("No agents are currently available.\n");
        } else {
            for agent in idle_agents {
                let desc = agent.description.as_deref().unwrap_or("No description");
                prompt.push_str(&format!(
                    "- ID: \"{}\", Name: \"{}\", Description: \"{}\"\n",
                    agent.id, agent.name, desc
                ));
            }
        }
        prompt.push_str("</available_agents>\n\n");

        // Context
        prompt.push_str(&format!(
            "<context>\nReplans so far: {} (be conservative — avoid unnecessary changes)\n</context>\n\n",
            replans_so_far
        ));

        // Response format and rules
        prompt.push_str(
            r#"<response_format>
Respond with ONLY a single JSON object. No markdown, no explanation, no other text.

If the plan is on track:
{"decision": "continue"}

If the plan needs modification (replace remaining PENDING/READY nodes with new nodes):
{"decision": "modify_dag", "dag": {"nodes": [
  {"node_id": "new_1", "title": "...", "description": "...", "agent_id": "...", "agent_name": "...", "depends_on": [], "workspace_keys": [], "output_key": "..."},
  ...
]}}

If the task should be abandoned:
{"decision": "abort", "reason": "Explanation of why the task cannot be completed"}
</response_format>

<rules>
- Prefer "continue" unless completed results clearly show the remaining plan is wrong
- A "modify_dag" replaces only PENDING/READY/SKIPPED nodes; COMPLETED/RUNNING nodes are kept
- New nodes in modify_dag can reference output_keys from already-completed nodes
- Use exact agent_id values from the Available Agents list
- 2-8 nodes max in modified DAG
- Only abort if the task is fundamentally impossible given completed results
</rules>
"#,
        );

        prompt
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
