use super::*;

fn make_agent(id: &str, name: &str) -> SubAgent {
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus};
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
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
         "depends_on": [], "workspace_keys": [], "output_key": "new_output"},
        {"node_id": "new_2", "title": "Follow-up", "description": "Do follow-up",
         "agent_id": "agent-1", "agent_name": "Agent One",
         "depends_on": ["new_1"], "workspace_keys": ["new_output"], "output_key": "final_output"}
    ]}}"#;
    let agents = make_agents();
    let result = Replanner::parse_decision(json, &agents).unwrap();
    match result {
        ReplanDecision::ModifyDag { dag } => {
            assert_eq!(dag.nodes.len(), 2);
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
         "depends_on": [], "workspace_keys": [], "output_key": "new_output"},
        {"node_id": "new_2", "title": "Follow-up", "description": "Do follow-up",
         "agent_id": "agent-1", "agent_name": "Agent One",
         "depends_on": ["new_1"], "workspace_keys": ["new_output"], "output_key": "final_output"}
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
        nodes: vec![make_node("n4", "agent-2", &["n1"], DagNodeStatus::Pending)],
    };

    let merged = merge_replanned_dag(&existing, &new_dag, &make_agents()).unwrap();
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
        nodes: vec![make_node("n5", "agent-2", &["n2"], DagNodeStatus::Pending)],
    };

    let merged = merge_replanned_dag(&existing, &new_dag, &make_agents()).unwrap();
    assert_eq!(merged.nodes.len(), 3); // n1, n2, n5
    assert_eq!(merged.nodes[0].node_id, "n1");
    assert_eq!(merged.nodes[1].node_id, "n2");
    assert_eq!(merged.nodes[2].node_id, "n5");
}

#[test]
fn test_merge_deduplicates_node_ids() {
    let existing = TaskDag {
        nodes: vec![make_node("n1", "agent-1", &[], DagNodeStatus::Completed)],
    };
    // New DAG reuses n1 id (should be skipped since it's already completed)
    let new_dag = TaskDag {
        nodes: vec![
            make_node("n1", "agent-2", &[], DagNodeStatus::Pending),
            make_node("n2", "agent-2", &["n1"], DagNodeStatus::Pending),
        ],
    };

    let merged = merge_replanned_dag(&existing, &new_dag, &make_agents()).unwrap();
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
        nodes: vec![make_node("n4", "agent-2", &["n1"], DagNodeStatus::Pending)],
    };

    let merged = merge_replanned_dag(&existing, &new_dag, &make_agents()).unwrap();
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
        nodes: vec![make_node("n4", "agent-2", &["n2"], DagNodeStatus::Pending)],
    };

    let result = merge_replanned_dag(&existing, &new_dag, &make_agents());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown node"));
}

#[test]
fn test_merge_rejects_cycle() {
    // Existing: n1 completed
    let existing = TaskDag {
        nodes: vec![make_node("n1", "agent-1", &[], DagNodeStatus::Completed)],
    };
    // New DAG introduces a cycle: n2 depends on n3, n3 depends on n2
    let new_dag = TaskDag {
        nodes: vec![
            make_node("n2", "agent-1", &["n3"], DagNodeStatus::Pending),
            make_node("n3", "agent-2", &["n2"], DagNodeStatus::Pending),
        ],
    };

    let result = merge_replanned_dag(&existing, &new_dag, &make_agents());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cycle"));
}

#[test]
fn test_merge_rejects_unknown_agent() {
    let existing = TaskDag {
        nodes: vec![make_node("n1", "agent-1", &[], DagNodeStatus::Completed)],
    };
    // New DAG references an agent that doesn't exist in make_agents()
    let new_dag = TaskDag {
        nodes: vec![make_node(
            "n2",
            "ghost-agent",
            &["n1"],
            DagNodeStatus::Pending,
        )],
    };

    let result = merge_replanned_dag(&existing, &new_dag, &make_agents());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown agent"));
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
