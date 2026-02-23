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
        nodes: vec![make_node("n1", "a1", &[]), make_node("n2", "a1", &["n1"])],
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
    assert_eq!(config.max_concurrent_agents, 4);
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
