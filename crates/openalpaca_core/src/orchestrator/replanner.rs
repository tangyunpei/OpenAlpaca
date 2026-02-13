//! Dynamic replanning: evaluates progress after DAG node completions
//! and decides whether to continue, modify the remaining DAG, or abort.

use crate::agent::subagent::SubAgent;
use crate::orchestrator::task_planner::{DagNode, DagNodeStatus, TaskDag};
use crate::orchestrator::task_state::TaskWorkspace;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use serde::{Deserialize, Serialize};

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
    ModifyDag {
        dag: TaskDag,
    },
    /// Give up — task is unachievable or no longer makes sense.
    Abort {
        reason: String,
    },
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
            messages,
            tools: vec![],
            temperature: Some(0.0),
            max_tokens: Some(2048),
            context: RequestContext::default(),
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
            "## Original Objective\n{}\n\n",
            original_objective
        ));

        // Current DAG state
        prompt.push_str("## Current DAG State\n");
        for node in &dag.nodes {
            let status = match &node.status {
                DagNodeStatus::Completed => {
                    let summary = node
                        .result_summary
                        .as_deref()
                        .unwrap_or("(no summary)");
                    // Cap summary to 200 chars for prompt
                    let capped: String = summary.chars().take(200).collect();
                    format!("COMPLETED — {}", capped)
                }
                DagNodeStatus::Failed => {
                    let err = node
                        .result_summary
                        .as_deref()
                        .unwrap_or("(no error)");
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
        prompt.push('\n');

        // Workspace contents (summaries only)
        let workspace_summary = workspace.format_for_prompt(&[]);
        if !workspace_summary.is_empty() {
            prompt.push_str("## Workspace Contents (summaries of completed work)\n");
            prompt.push_str(&workspace_summary);
            prompt.push('\n');
        }

        // Available agents
        prompt.push_str("## Available Agents\n");
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
        prompt.push('\n');

        // Context
        prompt.push_str(&format!(
            "## Context\nReplans so far: {} (be conservative — avoid unnecessary changes)\n\n",
            replans_so_far
        ));

        // Response format
        prompt.push_str(
            r#"## Response Format
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

## Rules
- Prefer "continue" unless completed results clearly show the remaining plan is wrong
- A "modify_dag" replaces only PENDING/READY/SKIPPED nodes; COMPLETED/RUNNING nodes are kept
- New nodes in modify_dag can reference output_keys from already-completed nodes
- Use exact agent_id values from the Available Agents list
- 2-8 nodes max in modified DAG
- Only abort if the task is fundamentally impossible given completed results
"#,
        );

        prompt
    }

    /// Parse the LLM response into a ReplanDecision.
    fn parse_decision(
        content: &str,
        available_agents: &[SubAgent],
    ) -> Result<ReplanDecision, String> {
        let json_str = Self::extract_json(content);

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
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json_str) {
            if let Some(decision_str) = obj.get("decision").and_then(|v| v.as_str()) {
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
        }

        // If all parsing fails, default to continue (conservative)
        tracing::warn!(
            "Replanner: failed to parse response, defaulting to Continue. Raw: {}",
            &content.chars().take(200).collect::<String>()
        );
        Ok(ReplanDecision::Continue)
    }

    /// Extract JSON from a response that may be wrapped in markdown code fences.
    fn extract_json(content: &str) -> &str {
        let trimmed = content.trim();

        if let Some(start) = trimmed.find("```json") {
            let after_fence = &trimmed[start + 7..];
            if let Some(end) = after_fence.find("```") {
                return after_fence[..end].trim();
            }
        }

        if let Some(start) = trimmed.find("```") {
            let after_fence = &trimmed[start + 3..];
            if let Some(end) = after_fence.find("```") {
                return after_fence[..end].trim();
            }
        }

        trimmed
    }
}

// ── Merge logic ──────────────────────────────────────────────────────

/// Merge a replanned DAG into the existing DAG:
/// - Keep all COMPLETED and RUNNING nodes from the old DAG
/// - Replace PENDING/READY/SKIPPED nodes with nodes from the new DAG
/// - Validate that the merged result has no broken dependencies
///
/// Returns Err if the merged DAG has dangling dependency references.
pub fn merge_replanned_dag(existing: &TaskDag, new_dag: &TaskDag) -> Result<TaskDag, String> {
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
            continue;
        }
        merged_nodes.push(node.clone());
    }

    // Build merged DAG and validate structure (dependencies + cycles)
    let merged = TaskDag {
        nodes: merged_nodes,
    };

    // validate_structure checks: dependency existence AND cycle detection
    merged.validate_structure().map_err(|e| {
        format!("Merged DAG validation failed: {}", e)
    })?;

    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent(id: &str, name: &str) -> SubAgent {
        use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus};
        SubAgent {
            id: id.to_string(),
            name: name.to_string(),
            description: Some(format!("Test agent {}", name)),
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: vec![],
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }
    }

    fn make_agents() -> Vec<SubAgent> {
        vec![
            make_agent("agent-1", "Agent One"),
            make_agent("agent-2", "Agent Two"),
        ]
    }

    fn make_node(id: &str, agent_id: &str, deps: &[&str], status: DagNodeStatus) -> DagNode {
        DagNode {
            node_id: id.to_string(),
            title: format!("Task {}", id),
            description: format!("Do {}", id),
            agent_id: agent_id.to_string(),
            agent_name: format!("Agent {}", agent_id),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            status,
            result_summary: None,
            workspace_keys: vec![],
            output_key: Some(format!("{}_output", id)),
        }
    }

    // ── ReplanConfig tests ──────────────────────────────────────────

    #[test]
    fn test_default_config() {
        let config = ReplanConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.replan_after_every_n_nodes, 2);
        assert_eq!(config.max_replans, 3);
    }

    // ── parse_decision tests ────────────────────────────────────────

    #[test]
    fn test_parse_continue() {
        let json = r#"{"decision": "continue"}"#;
        let agents = make_agents();
        let result = Replanner::parse_decision(json, &agents).unwrap();
        assert!(matches!(result, ReplanDecision::Continue));
    }

    #[test]
    fn test_parse_abort() {
        let json = r#"{"decision": "abort", "reason": "Task is impossible"}"#;
        let agents = make_agents();
        let result = Replanner::parse_decision(json, &agents).unwrap();
        match result {
            ReplanDecision::Abort { reason } => {
                assert_eq!(reason, "Task is impossible");
            }
            _ => panic!("Expected Abort"),
        }
    }

    #[test]
    fn test_parse_modify_dag() {
        let json = r#"{"decision": "modify_dag", "dag": {"nodes": [
            {"node_id": "new_1", "title": "New task", "description": "Do new thing",
             "agent_id": "agent-1", "agent_name": "Agent One",
             "depends_on": [], "workspace_keys": [], "output_key": "new_output"}
        ]}}"#;
        let agents = make_agents();
        let result = Replanner::parse_decision(json, &agents).unwrap();
        match result {
            ReplanDecision::ModifyDag { dag } => {
                assert_eq!(dag.nodes.len(), 1);
                assert_eq!(dag.nodes[0].node_id, "new_1");
            }
            _ => panic!("Expected ModifyDag"),
        }
    }

    #[test]
    fn test_parse_modify_dag_invalid_agent() {
        let json = r#"{"decision": "modify_dag", "dag": {"nodes": [
            {"node_id": "new_1", "title": "New task", "description": "Do new thing",
             "agent_id": "nonexistent-agent", "agent_name": "Ghost",
             "depends_on": [], "workspace_keys": [], "output_key": "new_output"}
        ]}}"#;
        let agents = make_agents();
        let result = Replanner::parse_decision(json, &agents);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown agent"));
    }

    #[test]
    fn test_parse_malformed_defaults_to_continue() {
        let json = "this is not json at all";
        let agents = make_agents();
        let result = Replanner::parse_decision(json, &agents).unwrap();
        assert!(matches!(result, ReplanDecision::Continue));
    }

    #[test]
    fn test_parse_wrapped_in_code_fence() {
        let json = "```json\n{\"decision\": \"continue\"}\n```";
        let agents = make_agents();
        let result = Replanner::parse_decision(json, &agents).unwrap();
        assert!(matches!(result, ReplanDecision::Continue));
    }

    // ── merge_replanned_dag tests ───────────────────────────────────

    #[test]
    fn test_merge_keeps_completed_nodes() {
        let existing = TaskDag {
            nodes: vec![
                make_node("n1", "agent-1", &[], DagNodeStatus::Completed),
                make_node("n2", "agent-1", &["n1"], DagNodeStatus::Pending),
                make_node("n3", "agent-2", &["n2"], DagNodeStatus::Pending),
            ],
        };
        let new_dag = TaskDag {
            nodes: vec![
                make_node("n4", "agent-2", &["n1"], DagNodeStatus::Pending),
            ],
        };

        let merged = merge_replanned_dag(&existing, &new_dag).unwrap();
        assert_eq!(merged.nodes.len(), 2); // n1 (completed) + n4 (new)
        assert_eq!(merged.nodes[0].node_id, "n1");
        assert_eq!(merged.nodes[1].node_id, "n4");
    }

    #[test]
    fn test_merge_keeps_running_nodes() {
        let existing = TaskDag {
            nodes: vec![
                make_node("n1", "agent-1", &[], DagNodeStatus::Completed),
                make_node("n2", "agent-1", &["n1"], DagNodeStatus::Running),
                make_node("n3", "agent-2", &["n2"], DagNodeStatus::Pending),
            ],
        };
        let new_dag = TaskDag {
            nodes: vec![
                make_node("n5", "agent-2", &["n2"], DagNodeStatus::Pending),
            ],
        };

        let merged = merge_replanned_dag(&existing, &new_dag).unwrap();
        assert_eq!(merged.nodes.len(), 3); // n1, n2, n5
        assert_eq!(merged.nodes[0].node_id, "n1");
        assert_eq!(merged.nodes[1].node_id, "n2");
        assert_eq!(merged.nodes[2].node_id, "n5");
    }

    #[test]
    fn test_merge_deduplicates_node_ids() {
        let existing = TaskDag {
            nodes: vec![
                make_node("n1", "agent-1", &[], DagNodeStatus::Completed),
            ],
        };
        // New DAG reuses n1 id (should be skipped since it's already completed)
        let new_dag = TaskDag {
            nodes: vec![
                make_node("n1", "agent-2", &[], DagNodeStatus::Pending),
                make_node("n2", "agent-2", &["n1"], DagNodeStatus::Pending),
            ],
        };

        let merged = merge_replanned_dag(&existing, &new_dag).unwrap();
        assert_eq!(merged.nodes.len(), 2); // n1 (from existing) + n2 (new)
        assert_eq!(merged.nodes[0].agent_id, "agent-1"); // kept the completed version
        assert_eq!(merged.nodes[1].node_id, "n2");
    }

    #[test]
    fn test_merge_drops_skipped_and_failed() {
        let existing = TaskDag {
            nodes: vec![
                make_node("n1", "agent-1", &[], DagNodeStatus::Completed),
                make_node("n2", "agent-1", &["n1"], DagNodeStatus::Failed),
                make_node("n3", "agent-2", &["n2"], DagNodeStatus::Skipped),
            ],
        };
        let new_dag = TaskDag {
            nodes: vec![
                make_node("n4", "agent-2", &["n1"], DagNodeStatus::Pending),
            ],
        };

        let merged = merge_replanned_dag(&existing, &new_dag).unwrap();
        assert_eq!(merged.nodes.len(), 2); // n1 (completed) + n4 (new)
        assert!(!merged.nodes.iter().any(|n| n.node_id == "n2"));
        assert!(!merged.nodes.iter().any(|n| n.node_id == "n3"));
    }

    #[test]
    fn test_merge_rejects_broken_dependencies() {
        let existing = TaskDag {
            nodes: vec![
                make_node("n1", "agent-1", &[], DagNodeStatus::Completed),
                make_node("n2", "agent-1", &["n1"], DagNodeStatus::Pending),
            ],
        };
        // New DAG references n2, which was dropped (it was Pending in existing)
        let new_dag = TaskDag {
            nodes: vec![
                make_node("n4", "agent-2", &["n2"], DagNodeStatus::Pending),
            ],
        };

        let result = merge_replanned_dag(&existing, &new_dag);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown node"));
    }

    #[test]
    fn test_merge_rejects_cycle() {
        // Existing: n1 completed
        let existing = TaskDag {
            nodes: vec![
                make_node("n1", "agent-1", &[], DagNodeStatus::Completed),
            ],
        };
        // New DAG introduces a cycle: n2 depends on n3, n3 depends on n2
        let new_dag = TaskDag {
            nodes: vec![
                make_node("n2", "agent-1", &["n3"], DagNodeStatus::Pending),
                make_node("n3", "agent-2", &["n2"], DagNodeStatus::Pending),
            ],
        };

        let result = merge_replanned_dag(&existing, &new_dag);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cycle"));
    }

    // ── build_replan_prompt tests ───────────────────────────────────

    #[test]
    fn test_prompt_includes_objective() {
        let dag = TaskDag {
            nodes: vec![make_node("n1", "agent-1", &[], DagNodeStatus::Completed)],
        };
        let workspace = TaskWorkspace::default();
        let agents = make_agents();
        let prompt = Replanner::build_replan_prompt(&dag, &workspace, "Write a report", &agents, 0);
        assert!(prompt.contains("Write a report"));
    }

    #[test]
    fn test_prompt_includes_dag_state() {
        let mut node = make_node("n1", "agent-1", &[], DagNodeStatus::Completed);
        node.result_summary = Some("Found 5 articles".to_string());
        let dag = TaskDag { nodes: vec![node] };
        let workspace = TaskWorkspace::default();
        let agents = make_agents();
        let prompt = Replanner::build_replan_prompt(&dag, &workspace, "obj", &agents, 0);
        assert!(prompt.contains("COMPLETED"));
        assert!(prompt.contains("Found 5 articles"));
    }

    #[test]
    fn test_prompt_includes_agents() {
        let dag = TaskDag { nodes: vec![] };
        let workspace = TaskWorkspace::default();
        let agents = make_agents();
        let prompt = Replanner::build_replan_prompt(&dag, &workspace, "obj", &agents, 0);
        assert!(prompt.contains("agent-1"));
        assert!(prompt.contains("Agent One"));
    }

    #[test]
    fn test_prompt_includes_replan_count() {
        let dag = TaskDag { nodes: vec![] };
        let workspace = TaskWorkspace::default();
        let agents = make_agents();
        let prompt = Replanner::build_replan_prompt(&dag, &workspace, "obj", &agents, 2);
        assert!(prompt.contains("Replans so far: 2"));
    }
}
