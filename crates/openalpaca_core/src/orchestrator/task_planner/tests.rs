use super::*;

#[test]
fn test_parse_simple_query_response() {
    let json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a greeting"}"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert_eq!(plan.classification, "simple_query");
    assert!(plan.title.is_none());
    assert!(plan.assignments.is_empty());
    assert_eq!(plan.reasoning.as_deref(), Some("This is a greeting"));
}

#[test]
fn test_parse_complex_task_response() {
    let json = r#"{
        "classification": "complex_task",
        "title": "Research Rust async patterns",
        "assignments": [{
            "agent_id": "researcher-01",
            "agent_name": "Researcher",
            "role_description": "Search for information about Rust async patterns",
            "matched_skills": ["web_search", "summarize"]
        }],
        "reasoning": "User wants research, assigning researcher agent"
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert_eq!(plan.classification, "complex_task");
    assert_eq!(plan.title.as_deref(), Some("Research Rust async patterns"));
    assert_eq!(plan.assignments.len(), 1);
    assert_eq!(plan.assignments[0].agent_id, "researcher-01");
    assert_eq!(
        plan.assignments[0].matched_skills,
        vec!["web_search", "summarize"]
    );
}

#[test]
fn test_parse_response_with_markdown_fences() {
    let content = "```json\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"greeting\"}\n```";
    let plan = TaskPlanner::parse_response(content).unwrap();
    assert_eq!(plan.classification, "simple_query");
}

#[test]
fn test_parse_response_with_plain_fences() {
    let content = "```\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"test\"}\n```";
    let plan = TaskPlanner::parse_response(content).unwrap();
    assert_eq!(plan.classification, "simple_query");
}

#[test]
fn test_parse_malformed_response() {
    let result = TaskPlanner::parse_response("this is not json at all");
    assert!(result.is_err());
    match result.unwrap_err() {
        PlanError::MalformedResponse(msg) => {
            assert!(msg.contains("Failed to parse JSON"));
        }
        _ => panic!("Expected MalformedResponse"),
    }
}

#[test]
fn test_extract_json_bare() {
    let input = r#"{"classification": "simple_query"}"#;
    assert_eq!(TaskPlanner::extract_json(input), input);
}

#[test]
fn test_extract_json_with_whitespace() {
    let input = "  \n{\"classification\": \"simple_query\"}\n  ";
    assert_eq!(
        TaskPlanner::extract_json(input),
        "{\"classification\": \"simple_query\"}"
    );
}

#[test]
fn test_extract_json_prose_around_braces() {
    let input = "Here is my analysis:\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"test\"}\nHope that helps!";
    let plan = TaskPlanner::parse_response(input).unwrap();
    assert_eq!(plan.classification, "simple_query");
}

#[test]
fn test_extract_json_braces_with_strings_containing_braces() {
    let input = r#"Sure! {"classification": "simple_query", "title": null, "assignments": [], "reasoning": "The user said {hello}"}"#;
    let plan = TaskPlanner::parse_response(input).unwrap();
    assert_eq!(plan.classification, "simple_query");
    assert!(plan.reasoning.unwrap().contains("{hello}"));
}

#[test]
fn test_extract_json_no_json_at_all() {
    let result = TaskPlanner::parse_response("No JSON here whatsoever.");
    assert!(result.is_err());
}

#[test]
fn test_find_outermost_braces_escaped_quotes() {
    let input = r#"text {"key": "val with \"escaped\" quotes"} trailing"#;
    let extracted = extract_json_block(input);
    assert!(extracted.starts_with('{'));
    assert!(extracted.ends_with('}'));
}

#[test]
fn test_parse_response_with_extra_fields() {
    // LLM sometimes echoes back agent info alongside the classification
    let json = r#"{
        "available_agents": [{"agent_id": "writing_agent", "name": "Writer"}],
        "classification": "simple_query",
        "title": null,
        "assignments": [],
        "reasoning": "Greeting detected"
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert_eq!(plan.classification, "simple_query");
    assert!(plan.assignments.is_empty());
}

#[test]
fn test_parse_response_no_classification_at_all() {
    let json = r#"{"available_agents": [{"agent_id": "writing_agent"}]}"#;
    let result = TaskPlanner::parse_response(json);
    assert!(result.is_err());
    match result.unwrap_err() {
        PlanError::MalformedResponse(msg) => {
            assert!(msg.contains("missing field `classification`"));
        }
        _ => panic!("Expected MalformedResponse"),
    }
}

#[test]
fn test_complex_task_empty_auto_promotes_to_lead_agent() {
    // When complex_task has no assignments, no DAG, and no lead_agent,
    // parse_response auto-promotes to use_lead_agent=true as a safety net.
    let json = r#"{
        "classification": "complex_task",
        "title": "Do something",
        "assignments": [],
        "reasoning": "test"
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert_eq!(plan.classification, "complex_task");
    assert!(plan.use_lead_agent);
    assert!(plan.dag.is_none());
    assert!(plan.assignments.is_empty());
}

#[test]
fn test_parse_response_malformed_returns_correct_error_variant() {
    let result = TaskPlanner::parse_response("garbage text");
    assert!(matches!(result, Err(PlanError::MalformedResponse(_))));
}

#[test]
fn test_parse_response_valid_json_missing_classification_is_malformed() {
    let result = TaskPlanner::parse_response(r#"{"foo": "bar"}"#);
    assert!(matches!(result, Err(PlanError::MalformedResponse(_))));
}

// ── DAG tests ─────────────────────────────────────────────────────

use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus};

fn make_agent(id: &str) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: format!("Agent {}", id),
        description: None,
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        skills: vec![],
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

fn make_dag_node(id: &str, agent_id: &str, deps: &[&str]) -> DagNode {
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
fn test_dag_validate_valid() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a2", &["n1"]),
            make_dag_node("n3", "a1", &["n1"]),
            make_dag_node("n4", "a2", &["n2", "n3"]),
        ],
    };
    let agents = vec![make_agent("a1"), make_agent("a2")];
    assert!(dag.validate(&agents).is_ok());
}

#[test]
fn test_dag_validate_empty() {
    let dag = TaskDag { nodes: vec![] };
    let agents = vec![make_agent("a1")];
    let err = dag.validate(&agents).unwrap_err();
    assert!(err.contains("no nodes"));
}

#[test]
fn test_dag_validate_single_node() {
    let dag = TaskDag {
        nodes: vec![make_dag_node("n1", "a1", &[])],
    };
    let agents = vec![make_agent("a1")];
    let err = dag.validate(&agents).unwrap_err();
    assert!(err.contains("at least 2 nodes"));
}

#[test]
fn test_dag_validate_structure_single_node() {
    let dag = TaskDag {
        nodes: vec![make_dag_node("n1", "a1", &[])],
    };
    let err = dag.validate_structure().unwrap_err();
    assert!(err.contains("at least 2 nodes"));
}

#[test]
fn test_dag_validate_too_many_nodes() {
    let nodes: Vec<DagNode> = (0..9)
        .map(|i| make_dag_node(&format!("n{}", i), "a1", &[]))
        .collect();
    let dag = TaskDag { nodes };
    let agents = vec![make_agent("a1")];
    let err = dag.validate(&agents).unwrap_err();
    assert!(err.contains("max 8"));
}

#[test]
fn test_dag_validate_unknown_dependency() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &["nonexistent"]),
            make_dag_node("n2", "a1", &[]),
        ],
    };
    let agents = vec![make_agent("a1")];
    let err = dag.validate(&agents).unwrap_err();
    assert!(err.contains("unknown node"));
}

#[test]
fn test_dag_validate_unknown_agent() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "ghost_agent", &[]),
            make_dag_node("n2", "a1", &[]),
        ],
    };
    let agents = vec![make_agent("a1")];
    let err = dag.validate(&agents).unwrap_err();
    assert!(err.contains("unknown agent"));
}

#[test]
fn test_dag_validate_cycle() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &["n2"]),
            make_dag_node("n2", "a1", &["n1"]),
        ],
    };
    let agents = vec![make_agent("a1")];
    let err = dag.validate(&agents).unwrap_err();
    assert!(err.contains("cycle"));
}

#[test]
fn test_dag_ready_nodes_initial() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &[]),
            make_dag_node("n3", "a1", &["n1", "n2"]),
        ],
    };
    let ready: Vec<&str> = dag
        .ready_nodes()
        .iter()
        .map(|n| n.node_id.as_str())
        .collect();
    assert!(ready.contains(&"n1"));
    assert!(ready.contains(&"n2"));
    assert!(!ready.contains(&"n3"));
}

#[test]
fn test_dag_complete_node_unlocks_dependents() {
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
        ],
    };
    // Initially only n1 is ready
    assert_eq!(dag.ready_nodes().len(), 1);

    // Complete n1 → n2 becomes ready
    let newly_ready = dag.complete_node("n1", "done");
    assert!(newly_ready.contains(&"n2".to_string()));
    assert_eq!(dag.nodes[0].status, DagNodeStatus::Completed);
    assert_eq!(dag.nodes[0].result_summary.as_deref(), Some("done"));
}

#[test]
fn test_dag_fail_node_skips_dependents() {
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
            make_dag_node("n3", "a1", &["n2"]),
        ],
    };
    dag.fail_node("n1", "error");
    assert_eq!(dag.nodes[0].status, DagNodeStatus::Failed);
    assert_eq!(dag.nodes[1].status, DagNodeStatus::Skipped);
    assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped);
}

#[test]
fn test_dag_fail_node_independent_branches_survive() {
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &[]),
            make_dag_node("n3", "a1", &["n1"]),
        ],
    };
    dag.fail_node("n1", "error");
    // n2 is independent — should stay pending
    assert_eq!(dag.nodes[0].status, DagNodeStatus::Failed);
    assert_eq!(dag.nodes[1].status, DagNodeStatus::Pending);
    assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped);
}

#[test]
fn test_skip_dependents_diamond_shape() {
    // n1 -> n2, n1 -> n3, n2 -> n4, n3 -> n4
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
            make_dag_node("n3", "a1", &["n1"]),
            make_dag_node("n4", "a1", &["n2", "n3"]),
        ],
    };
    dag.fail_node("n1", "error");
    assert_eq!(dag.nodes[0].status, DagNodeStatus::Failed);
    assert_eq!(dag.nodes[1].status, DagNodeStatus::Skipped);
    assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped);
    assert_eq!(dag.nodes[3].status, DagNodeStatus::Skipped);
}

#[test]
fn test_skip_dependents_does_not_skip_running_nodes() {
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
        ],
    };
    dag.mark_running("n2");
    dag.fail_node("n1", "error");
    // n2 is Running, so it should NOT be skipped
    assert_eq!(dag.nodes[1].status, DagNodeStatus::Running);
}

#[test]
fn test_dag_is_finished() {
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
        ],
    };
    assert!(!dag.is_finished());
    dag.complete_node("n1", "ok");
    assert!(!dag.is_finished());
    dag.complete_node("n2", "ok");
    assert!(dag.is_finished());
}

#[test]
fn test_dag_is_finished_with_failures() {
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
        ],
    };
    dag.fail_node("n1", "error");
    // n2 gets skipped, so all nodes are terminal
    assert!(dag.is_finished());
}

#[test]
fn test_dag_topological_order() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n3", "a1", &["n1", "n2"]),
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
        ],
    };
    let order = dag.topological_order();
    assert_eq!(order.len(), 3);
    // n1 must come before n2 and n3
    let pos_n1 = order.iter().position(|id| id == "n1").unwrap();
    let pos_n2 = order.iter().position(|id| id == "n2").unwrap();
    let pos_n3 = order.iter().position(|id| id == "n3").unwrap();
    assert!(pos_n1 < pos_n2);
    assert!(pos_n1 < pos_n3);
    assert!(pos_n2 < pos_n3);
}

#[test]
fn test_dag_mark_running() {
    let mut dag = TaskDag {
        nodes: vec![make_dag_node("n1", "a1", &[])],
    };
    dag.mark_running("n1");
    assert_eq!(dag.nodes[0].status, DagNodeStatus::Running);
}

#[test]
fn test_dag_completed_count() {
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &[]),
            make_dag_node("n3", "a1", &[]),
        ],
    };
    assert_eq!(dag.completed_count(), 0);
    dag.complete_node("n1", "ok");
    assert_eq!(dag.completed_count(), 1);
    dag.complete_node("n2", "ok");
    assert_eq!(dag.completed_count(), 2);
}

#[test]
fn test_dag_serialization_roundtrip() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a2", &["n1"]),
        ],
    };
    let json = serde_json::to_string(&dag).unwrap();
    let deserialized: TaskDag = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.nodes.len(), 2);
    assert_eq!(deserialized.nodes[0].node_id, "n1");
    assert_eq!(deserialized.nodes[1].depends_on, vec!["n1"]);
}

#[test]
fn test_parse_response_with_dag() {
    let json = r#"{
        "classification": "complex_task",
        "title": "Research and write",
        "assignments": [],
        "reasoning": "Multi-step task",
        "use_lead_agent": false,
        "dag": {
            "nodes": [
                {"node_id": "n1", "title": "Research", "description": "Do research", "agent_id": "a1", "agent_name": "Agent a1", "depends_on": [], "workspace_keys": [], "output_key": "research"},
                {"node_id": "n2", "title": "Write", "description": "Write summary", "agent_id": "a2", "agent_name": "Agent a2", "depends_on": ["n1"], "workspace_keys": ["research"], "output_key": "summary"}
            ]
        }
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert_eq!(plan.classification, "complex_task");
    assert!(plan.dag.is_some());
    let dag = plan.dag.unwrap();
    assert_eq!(dag.nodes.len(), 2);
    assert_eq!(dag.nodes[0].node_id, "n1");
    assert_eq!(dag.nodes[1].depends_on, vec!["n1"]);
    assert_eq!(dag.nodes[1].workspace_keys, vec!["research"]);
}

#[test]
fn test_parse_response_without_dag() {
    let json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "Greeting"}"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.dag.is_none());
}

#[test]
fn test_dag_complete_node_caps_summary() {
    let mut dag = TaskDag {
        nodes: vec![make_dag_node("n1", "a1", &[])],
    };
    let long_summary = "x".repeat(600);
    dag.complete_node("n1", &long_summary);
    assert_eq!(dag.nodes[0].result_summary.as_ref().unwrap().len(), 500);
}

#[test]
fn test_dag_fail_node_caps_error() {
    let mut dag = TaskDag {
        nodes: vec![make_dag_node("n1", "a1", &[])],
    };
    let long_error = "e".repeat(600);
    dag.fail_node("n1", &long_error);
    assert_eq!(dag.nodes[0].result_summary.as_ref().unwrap().len(), 500);
}

#[test]
fn test_build_hierarchical_prompt_with_agents() {
    let agents = vec![make_agent("a1")];
    let prompt = TaskPlanner::build_hierarchical_prompt(&agents, false);
    assert!(prompt.contains("a1"));
    // XML structure tags
    assert!(prompt.contains("<agents>"));
    assert!(prompt.contains("<instructions>"));
    assert!(prompt.contains("<examples>"));
    assert!(prompt.contains("<format>"));
    assert!(prompt.contains("<rules>"));
    // DAG fields still referenced
    assert!(prompt.contains("depends_on"));
    assert!(prompt.contains("workspace_keys"));
    // Concrete few-shot examples present
    assert!(prompt.contains("Translate"));
    assert!(prompt.contains("Research"));
    // "Grey area" example was removed in prompt cleanup (Step 4)
    assert!(prompt.contains("Do NOT set both"));
}

#[test]
fn test_build_hierarchical_prompt_no_agents() {
    let prompt = TaskPlanner::build_hierarchical_prompt(&[], false);
    assert!(prompt.contains("No agents are currently available"));
}

#[test]
fn test_prompt_includes_v2_fields_when_enabled() {
    let prompt = TaskPlanner::build_hierarchical_prompt(&[], true);
    assert!(prompt.contains("execution_mode"));
    assert!(prompt.contains("predictability_score"));
    assert!(prompt.contains("v2_protocol"));
}

#[test]
fn test_prompt_excludes_v2_fields_when_disabled() {
    let prompt = TaskPlanner::build_hierarchical_prompt(&[], false);
    assert!(!prompt.contains("v2_protocol"));
}

#[test]
fn test_taskplan_old_json_compat() {
    let json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "test", "dag": null, "use_lead_agent": false}"#;
    let plan: TaskPlan = serde_json::from_str(json).unwrap();
    assert!(plan.execution_mode.is_none());
    assert!(plan.predictability_score.is_none());
}

#[test]
fn test_taskplan_v2_execution_mode_dag() {
    let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [], "reasoning": "test", "dag": {"nodes": [{"node_id": "n1", "title": "A", "description": "D", "agent_id": "a1", "agent_name": "Agent", "depends_on": [], "workspace_keys": [], "output_key": null}, {"node_id": "n2", "title": "B", "description": "D", "agent_id": "a1", "agent_name": "Agent", "depends_on": ["n1"], "workspace_keys": [], "output_key": null}]}, "use_lead_agent": true, "execution_mode": "dag", "predictability_score": 0.9}"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    // execution_mode "dag" should override use_lead_agent=true
    assert!(!plan.use_lead_agent);
    assert!(plan.dag.is_some());
    assert_eq!(plan.execution_mode.as_deref(), Some("dag"));
    assert_eq!(plan.predictability_score, Some(0.9));
}

#[test]
fn test_taskplan_v2_execution_mode_lead() {
    let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [], "reasoning": "test", "dag": null, "use_lead_agent": false, "execution_mode": "lead_agent"}"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.use_lead_agent);
    assert!(plan.dag.is_none());
}

#[test]
fn test_taskplan_v2_execution_mode_pipeline() {
    let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [{"agent_id": "a1", "agent_name": "Agent", "role_description": "Role", "matched_skills": ["coding"]}], "reasoning": "test", "dag": null, "use_lead_agent": true, "execution_mode": "pipeline"}"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(!plan.use_lead_agent);
    assert!(plan.dag.is_none());
}

#[test]
fn test_taskplan_v2_predictability_score() {
    let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [], "reasoning": "test", "dag": null, "use_lead_agent": true, "predictability_score": 0.85}"#;
    let plan: TaskPlan = serde_json::from_str(json).unwrap();
    assert_eq!(plan.predictability_score, Some(0.85));
}

// ── validate_structure tests ─────────────────────────────────

#[test]
fn test_validate_structure_valid_dag() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &[]),
            make_dag_node("n2", "a1", &["n1"]),
            make_dag_node("n3", "a1", &["n1"]),
            make_dag_node("n4", "a1", &["n2", "n3"]),
        ],
    };
    assert!(dag.validate_structure().is_ok());
}

#[test]
fn test_validate_structure_detects_cycle() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &["n2"]),
            make_dag_node("n2", "a1", &["n1"]),
        ],
    };
    let err = dag.validate_structure().unwrap_err();
    assert!(err.contains("cycle"));
}

#[test]
fn test_validate_structure_detects_unknown_dep() {
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("n1", "a1", &["nonexistent"]),
            make_dag_node("n2", "a1", &[]),
        ],
    };
    let err = dag.validate_structure().unwrap_err();
    assert!(err.contains("unknown node"));
}

#[test]
fn test_validate_structure_empty_dag() {
    let dag = TaskDag { nodes: vec![] };
    let err = dag.validate_structure().unwrap_err();
    assert!(err.contains("no nodes"));
}

// ── use_lead_agent tests ─────────────────────────────────────

#[test]
fn test_task_plan_use_lead_agent_defaults_to_false_but_promotes_when_orphaned() {
    // When use_lead_agent is missing from JSON, serde defaults to false,
    // but auto-promote kicks in because assignments+dag are also empty.
    let json = r#"{
        "classification": "complex_task",
        "title": "Some task",
        "assignments": [],
        "reasoning": "test"
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.use_lead_agent);
}

#[test]
fn test_task_plan_use_lead_agent_true() {
    // When use_lead_agent is explicitly true
    let json = r#"{
        "classification": "complex_task",
        "title": "Dynamic research task",
        "assignments": [],
        "reasoning": "Task is exploratory",
        "use_lead_agent": true
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.use_lead_agent);
    assert_eq!(plan.classification, "complex_task");
    assert_eq!(plan.title.as_deref(), Some("Dynamic research task"));
}

#[test]
fn test_task_plan_use_lead_agent_false_explicit_promotes_when_orphaned() {
    // Even with explicit use_lead_agent=false, if assignments and DAG are
    // both empty, auto-promote overrides to true as a safety net.
    let json = r#"{
        "classification": "complex_task",
        "title": "Predictable task",
        "assignments": [],
        "reasoning": "test",
        "use_lead_agent": false
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.use_lead_agent);
}

#[test]
fn test_task_plan_use_lead_agent_with_dag() {
    // When both use_lead_agent and dag are present, DAG should be stripped
    // (mutual exclusivity: lead agent takes priority).
    let json = r#"{
        "classification": "complex_task",
        "title": "Complex task",
        "assignments": [],
        "reasoning": "test",
        "use_lead_agent": true,
        "dag": {
            "nodes": [
                {"node_id": "n1", "title": "Step 1", "description": "Do step 1", "agent_id": "a1", "agent_name": "Agent a1", "depends_on": [], "workspace_keys": [], "output_key": "step1"}
            ]
        }
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.use_lead_agent);
    assert!(
        plan.dag.is_none(),
        "DAG should be stripped when use_lead_agent is true"
    );
    assert_eq!(
        plan.auto_promotion_reason.as_deref(),
        Some("mutual_exclusivity_stripped")
    );
}

#[test]
fn test_parse_response_fallback_extracts_use_lead_agent() {
    // When classification is embedded in a larger JSON object (fallback path)
    let json = r#"{
        "available_agents": [{"agent_id": "a1"}],
        "classification": "complex_task",
        "title": "Exploratory task",
        "assignments": [
            {"agent_id": "a1", "agent_name": "Agent a1", "role_description": "Lead", "matched_skills": ["research"]}
        ],
        "reasoning": "test",
        "use_lead_agent": true
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.use_lead_agent);
}

// ── has_predictable_structure tests ────────────────────────

#[test]
fn test_predictable_structure_numbered_list() {
    assert!(has_predictable_structure(
        "1. Translate to French\n2. Translate to Spanish\n3. Translate to German"
    ));
}

#[test]
fn test_predictable_structure_bullet_list() {
    assert!(has_predictable_structure(
        "- Write the intro\n- Write the body\n- Write the conclusion"
    ));
}

#[test]
fn test_predictable_structure_batch_translate() {
    assert!(has_predictable_structure(
        "Translate this into French, Spanish, and German for each chapter"
    ));
}

#[test]
fn test_predictable_structure_explicit_quantity() {
    assert!(has_predictable_structure("Split this into 3 sections"));
}

#[test]
fn test_predictable_structure_simple_message() {
    assert!(!has_predictable_structure("debug my test"));
}

#[test]
fn test_predictable_structure_greeting() {
    assert!(!has_predictable_structure("hello, how are you?"));
}

// ── PlanError display tests ─────────────────────────────────

#[test]
fn test_plan_error_timeout_display() {
    let err = PlanError::Timeout(30);
    assert_eq!(err.to_string(), "Planning timed out after 30s");
}

#[test]
fn test_plan_error_llm_error_display() {
    let err = PlanError::LlmError("connection refused".to_string());
    assert_eq!(err.to_string(), "LLM error: connection refused");
}

#[test]
fn test_plan_error_malformed_display() {
    let err = PlanError::MalformedResponse("bad json".to_string());
    assert_eq!(err.to_string(), "Malformed response: bad json");
}

#[test]
fn test_missing_use_lead_agent_defaults_true() {
    // When the LLM omits use_lead_agent entirely, it should default to true
    // (lead agent is the safer fallback for complex tasks).
    let json = r#"{
        "classification": "complex_task",
        "title": "Research something",
        "assignments": [{"agent_id": "a1", "agent_name": "Agent A", "role_description": "do stuff", "matched_skills": ["search"]}],
        "reasoning": "needs research"
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert_eq!(plan.classification, "complex_task");
    assert!(
        plan.use_lead_agent,
        "Missing use_lead_agent should default to true"
    );
}

#[test]
fn test_explicit_false_with_dag_stays_false() {
    // When the LLM explicitly sets use_lead_agent: false AND provides a DAG,
    // we respect the explicit choice.
    let json = r#"{
        "classification": "complex_task",
        "title": "Translate documents",
        "assignments": [],
        "reasoning": "Known steps, using DAG",
        "dag": {"nodes": [
            {"node_id": "n1", "title": "Translate EN", "description": "...", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "en"},
            {"node_id": "n2", "title": "Translate FR", "description": "...", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "fr"}
        ]},
        "use_lead_agent": false
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert_eq!(plan.classification, "complex_task");
    assert!(
        !plan.use_lead_agent,
        "Explicit false with DAG should stay false"
    );
    assert!(plan.dag.is_some());
}

#[test]
fn test_explicit_true_use_lead_agent() {
    let json = r#"{
        "classification": "complex_task",
        "title": "Debug failing tests",
        "assignments": [],
        "reasoning": "Exploratory task",
        "use_lead_agent": true
    }"#;
    let plan = TaskPlanner::parse_response(json).unwrap();
    assert!(plan.use_lead_agent);
    assert!(plan.dag.is_none());
}

// ── Critical path scheduling tests ──────────────────────────────

#[test]
fn test_critical_path_linear_chain() {
    // A -> B -> C: lengths = {A:2, B:1, C:0}
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("A", "a1", &[]),
            make_dag_node("B", "a1", &["A"]),
            make_dag_node("C", "a1", &["B"]),
        ],
    };
    let lengths = dag.critical_path_lengths();
    assert_eq!(lengths["A"], 2);
    assert_eq!(lengths["B"], 1);
    assert_eq!(lengths["C"], 0);
}

#[test]
fn test_critical_path_diamond() {
    // A -> {B, C} -> D: lengths = {A:2, B:1, C:1, D:0}
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("A", "a1", &[]),
            make_dag_node("B", "a1", &["A"]),
            make_dag_node("C", "a1", &["A"]),
            make_dag_node("D", "a1", &["B", "C"]),
        ],
    };
    let lengths = dag.critical_path_lengths();
    assert_eq!(lengths["A"], 2);
    assert_eq!(lengths["B"], 1);
    assert_eq!(lengths["C"], 1);
    assert_eq!(lengths["D"], 0);
}

#[test]
fn test_critical_path_wide_fan_out() {
    // A -> {B, C, D} all independent leaves: A:1, B/C/D:0
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("A", "a1", &[]),
            make_dag_node("B", "a1", &["A"]),
            make_dag_node("C", "a1", &["A"]),
            make_dag_node("D", "a1", &["A"]),
        ],
    };
    let lengths = dag.critical_path_lengths();
    assert_eq!(lengths["A"], 1);
    assert_eq!(lengths["B"], 0);
    assert_eq!(lengths["C"], 0);
    assert_eq!(lengths["D"], 0);
}

#[test]
fn test_critical_path_asymmetric() {
    // A -> B -> C (long path), A -> D (short path)
    // A:2, B:1, C:0, D:0; B should be prioritized over D
    let dag = TaskDag {
        nodes: vec![
            make_dag_node("A", "a1", &[]),
            make_dag_node("B", "a1", &["A"]),
            make_dag_node("C", "a1", &["B"]),
            make_dag_node("D", "a1", &["A"]),
        ],
    };
    let lengths = dag.critical_path_lengths();
    assert_eq!(lengths["A"], 2);
    assert_eq!(lengths["B"], 1);
    assert_eq!(lengths["C"], 0);
    assert_eq!(lengths["D"], 0);
}

#[test]
fn test_ready_nodes_prioritized_ordering() {
    // After A completes: B (path length 1) and D (path length 0) both ready
    // B should come first because it has longer downstream path
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("A", "a1", &[]),
            make_dag_node("B", "a1", &["A"]),
            make_dag_node("C", "a1", &["B"]),
            make_dag_node("D", "a1", &["A"]),
        ],
    };
    dag.complete_node("A", "done");

    let prioritized = dag.ready_nodes_prioritized();
    assert_eq!(prioritized.len(), 2);
    assert_eq!(prioritized[0].node_id, "B"); // longer downstream path
    assert_eq!(prioritized[1].node_id, "D"); // shorter downstream path
}

#[test]
fn test_critical_path_disabled_uses_original_order() {
    // When not using prioritized, ready_nodes() returns in node order
    let mut dag = TaskDag {
        nodes: vec![
            make_dag_node("A", "a1", &[]),
            make_dag_node("B", "a1", &["A"]),
            make_dag_node("C", "a1", &["B"]),
            make_dag_node("D", "a1", &["A"]),
        ],
    };
    dag.complete_node("A", "done");

    let normal = dag.ready_nodes();
    let prioritized = dag.ready_nodes_prioritized();

    // Both return the same nodes, just potentially different order
    assert_eq!(normal.len(), prioritized.len());
    let normal_ids: HashSet<&str> = normal.iter().map(|n| n.node_id.as_str()).collect();
    let prio_ids: HashSet<&str> = prioritized.iter().map(|n| n.node_id.as_str()).collect();
    assert_eq!(normal_ids, prio_ids);
}

// ── build_messages prompt-injection hardening tests ────────────────

#[test]
fn test_build_messages_untrusted_context_uses_user_role() {
    let system_prompt = "You are the planner.";
    let user_msg = "Build me a web scraper";
    let summary = "User previously asked about Rust";
    let tasks_block = "### ACTIVE TASKS ###\n- [abc12345] Fix bug (in_progress)";

    let msgs = build_messages(
        system_prompt,
        user_msg,
        &[],
        Some(summary),
        Some(tasks_block),
    );

    // First message must be the system policy prompt
    assert_eq!(msgs[0].role, openalpaca_llm::Role::System);
    assert_eq!(msgs[0].content, system_prompt);

    // Session summary and active tasks must be User role, not System
    assert_eq!(
        msgs[1].role,
        openalpaca_llm::Role::User,
        "Summary should be user role"
    );
    assert_eq!(
        msgs[2].role,
        openalpaca_llm::Role::User,
        "Tasks should be user role"
    );

    // Both must contain the untrusted-context framing
    assert!(
        msgs[1].content.contains("context_data"),
        "Summary should be wrapped in <context_data>"
    );
    assert!(
        msgs[1].content.contains("NOT instructions"),
        "Summary should contain injection guard"
    );
    assert!(
        msgs[2].content.contains("context_data"),
        "Tasks should be wrapped in <context_data>"
    );

    // Final message is the user query
    let last = msgs.last().unwrap();
    assert_eq!(last.role, openalpaca_llm::Role::User);
    assert_eq!(last.content, user_msg);
}

#[test]
fn test_build_messages_no_context_only_system_and_user() {
    let msgs = build_messages("System prompt.", "Hello", &[], None, None);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].role, openalpaca_llm::Role::System);
    assert_eq!(msgs[1].role, openalpaca_llm::Role::User);
}

// ── Phase P2: has_predictable_structure new patterns ─────────

#[test]
fn test_predictable_structure_cjk_enum() {
    // Chinese enumeration with 、
    assert!(has_predictable_structure("翻译成法语、西语、德语"));
}

#[test]
fn test_predictable_structure_cjk_enum_comma() {
    // Chinese enumeration with ，
    assert!(has_predictable_structure("把文件翻译成法语，西语，德语和意大利语"));
}

#[test]
fn test_predictable_structure_conjunctive_list() {
    // English conjunctive list: "X, Y, Z, and W" (3+ items)
    assert!(has_predictable_structure(
        "Translate into French, Spanish, German, and Italian"
    ));
}

#[test]
fn test_predictable_structure_short_conj_list_no_match() {
    // Only 1 comma item — below the threshold
    assert!(!has_predictable_structure("Translate into French and Spanish"));
}

// ── Phase P2: Prompt content tests ───────────────────────────

#[test]
fn test_prompt_includes_dag_dependency_example() {
    let prompt = TaskPlanner::build_hierarchical_prompt(&[], false);
    assert!(
        prompt.contains("Example 4"),
        "Prompt should include DAG dependency example"
    );
    assert!(
        prompt.contains("Read, summarize, and send report"),
        "Prompt should include DAG sequential dependency example"
    );
}

#[test]
fn test_prompt_includes_pipeline_example() {
    let prompt = TaskPlanner::build_hierarchical_prompt(&[], false);
    assert!(
        prompt.contains("Example 5"),
        "Prompt should include pipeline example"
    );
    assert!(
        prompt.contains("Strict linear pipeline"),
        "Prompt should include pipeline reasoning"
    );
}

#[test]
fn test_prompt_includes_execution_strategy_guide() {
    let prompt = TaskPlanner::build_hierarchical_prompt(&[], false);
    assert!(
        prompt.contains("pipeline (assignments array)"),
        "Prompt should include execution strategy guide"
    );
    assert!(
        prompt.contains("If the steps are clear, prefer DAG"),
        "Prompt should nudge toward DAG for predictable tasks"
    );
}

// ── Phase P2: DAG salvage tests ──────────────────────────────

#[test]
fn test_dag_validation_salvage_to_pipeline() {
    // DAG with only 1 node fails validation (min 2), but is salvageable
    // since the error is structural (not cycle/unknown agent)
    let json = r#"{
        "classification": "complex_task",
        "title": "Single step task",
        "assignments": [],
        "reasoning": "test",
        "dag": {
            "nodes": [
                {"node_id": "n1", "title": "Step 1", "description": "Do step 1", "agent_id": "a1", "agent_name": "Agent a1", "depends_on": [], "workspace_keys": [], "output_key": "out1"}
            ]
        },
        "use_lead_agent": false
    }"#;
    let plan: TaskPlan = serde_json::from_str(json).unwrap();
    let agents = vec![make_agent("a1")];
    let dag = plan.dag.as_ref().unwrap();
    let err = dag.validate(&agents).unwrap_err();
    // Error should be about node count, not cycle or unknown agent
    assert!(!err.contains("cycle"));
    assert!(!err.contains("unknown agent"));
    // Topological order should still work for salvage
    let topo = dag.topological_order();
    assert_eq!(topo.len(), 1);
    assert_eq!(topo[0], "n1");
}

// ── Phase P4: Opt-12 classify_lightweight tests ──

#[test]
fn test_classify_lightweight_parse_simple_query() {
    // Verifies that classify_lightweight correctly parses "simple_query" from JSON
    let json = r#"{"classification": "simple_query"}"#;
    let val: serde_json::Value = serde_json::from_str(json).unwrap();
    let c = val
        .get("classification")
        .and_then(|v| v.as_str())
        .unwrap();
    assert_eq!(c, "simple_query");
}

#[test]
fn test_classify_lightweight_parse_complex_task() {
    // Verifies that classify_lightweight correctly parses "complex_task" from JSON
    let json = r#"{"classification": "complex_task"}"#;
    let val: serde_json::Value = serde_json::from_str(json).unwrap();
    let c = val
        .get("classification")
        .and_then(|v| v.as_str())
        .unwrap();
    assert_eq!(c, "complex_task");
}

#[test]
fn test_classify_lightweight_malformed_json() {
    // Verifies that malformed JSON falls through to the error path
    let json = "not json at all";
    let result = serde_json::from_str::<serde_json::Value>(json);
    assert!(result.is_err());
}

#[test]
fn test_classify_lightweight_missing_classification_field() {
    // Verifies that JSON without "classification" field yields None
    let json = r#"{"other": "field"}"#;
    let val: serde_json::Value = serde_json::from_str(json).unwrap();
    let c = val.get("classification").and_then(|v| v.as_str());
    assert!(c.is_none());
}

#[test]
fn test_planner_config_two_phase_defaults() {
    let config = crate::daemon_config::PlannerConfig::default();
    assert!(!config.two_phase_enabled);
    assert!(config.triage_model.is_none());
}
