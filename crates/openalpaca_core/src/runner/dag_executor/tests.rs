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

// ─── node_index HashMap correctness tests (Task 13 optimization) ─────

#[test]
fn test_node_index_maps_all_nodes() {
    let dag = TaskDag {
        nodes: vec![
            make_node("n1", "a1", &[]),
            make_node("n2", "a2", &["n1"]),
            make_node("n3", "a1", &["n1"]),
            make_node("n4", "a2", &["n2", "n3"]),
        ],
    };

    // Replicate the node_index construction from execute_dag
    let node_index: HashMap<String, usize> = dag
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.node_id.clone(), i))
        .collect();

    assert_eq!(node_index.len(), 4);
    assert_eq!(node_index["n1"], 0);
    assert_eq!(node_index["n2"], 1);
    assert_eq!(node_index["n3"], 2);
    assert_eq!(node_index["n4"], 3);

    // Verify index gives correct node
    assert_eq!(dag.nodes[node_index["n3"]].agent_id, "a1");
    assert_eq!(dag.nodes[node_index["n2"]].agent_id, "a2");
}

#[test]
fn test_node_index_lookup_status_after_mutations() {
    let mut dag = TaskDag {
        nodes: vec![
            make_node("n1", "a1", &[]),
            make_node("n2", "a1", &["n1"]),
            make_node("n3", "a1", &["n1"]),
        ],
    };

    let node_index: HashMap<String, usize> = dag
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.node_id.clone(), i))
        .collect();

    // Simulate the dispatcher's status check via node_index
    dag.nodes[node_index["n1"]].status = DagNodeStatus::Running;
    assert!(matches!(
        dag.nodes[node_index["n1"]].status,
        DagNodeStatus::Running
    ));

    // After completing n1, mark_ready_nodes should promote n2 and n3
    dag.nodes[node_index["n1"]].status = DagNodeStatus::Completed;
    mark_ready_nodes(&mut dag);

    assert_eq!(dag.nodes[node_index["n2"]].status, DagNodeStatus::Ready);
    assert_eq!(dag.nodes[node_index["n3"]].status, DagNodeStatus::Ready);
}

// ─── dependency chain tracking tests ─────────────────────────────────

#[test]
fn test_diamond_dag_dependency_tracking() {
    //   n1
    //  / \
    // n2  n3
    //  \ /
    //   n4
    let mut dag = TaskDag {
        nodes: vec![
            make_node("n1", "a1", &[]),
            make_node("n2", "a1", &["n1"]),
            make_node("n3", "a1", &["n1"]),
            make_node("n4", "a1", &["n2", "n3"]),
        ],
    };

    // Initially only n1 is ready (no deps)
    mark_ready_nodes(&mut dag);
    let ready: Vec<&str> = dag
        .ready_nodes()
        .iter()
        .map(|n| n.node_id.as_str())
        .collect();
    assert_eq!(ready, vec!["n1"]);

    // Complete n1 -> n2 and n3 become ready, n4 still blocked
    dag.complete_node("n1", "done");
    mark_ready_nodes(&mut dag);
    let mut ready: Vec<&str> = dag
        .ready_nodes()
        .iter()
        .map(|n| n.node_id.as_str())
        .collect();
    ready.sort();
    assert_eq!(ready, vec!["n2", "n3"]);
    assert_eq!(dag.nodes[3].status, DagNodeStatus::Pending); // n4 still pending

    // Complete n2 only -> n4 still blocked (needs n3 too)
    dag.complete_node("n2", "done");
    mark_ready_nodes(&mut dag);
    assert!(!dag
        .ready_nodes()
        .iter()
        .any(|n| n.node_id == "n4"));

    // Complete n3 -> n4 becomes ready
    dag.complete_node("n3", "done");
    mark_ready_nodes(&mut dag);
    let ready: Vec<&str> = dag
        .ready_nodes()
        .iter()
        .map(|n| n.node_id.as_str())
        .collect();
    assert_eq!(ready, vec!["n4"]);
}

#[test]
fn test_failure_skips_dependents() {
    // n1 -> n2 -> n3
    let mut dag = TaskDag {
        nodes: vec![
            make_node("n1", "a1", &[]),
            make_node("n2", "a1", &["n1"]),
            make_node("n3", "a1", &["n2"]),
        ],
    };

    mark_ready_nodes(&mut dag);
    dag.fail_node("n1", "boom");

    // n2 and n3 should be Skipped
    assert_eq!(dag.nodes[1].status, DagNodeStatus::Skipped);
    assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped);
    assert!(dag.is_finished());
}

#[test]
fn test_partial_failure_allows_independent_branches() {
    //   n1    n2  (independent roots)
    //   |      |
    //   n3    n4
    let mut dag = TaskDag {
        nodes: vec![
            make_node("n1", "a1", &[]),
            make_node("n2", "a1", &[]),
            make_node("n3", "a1", &["n1"]),
            make_node("n4", "a1", &["n2"]),
        ],
    };

    mark_ready_nodes(&mut dag);
    // Fail n1 -> n3 skipped, but n2 and n4 are unaffected
    dag.fail_node("n1", "error");

    assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped); // n3
    // n2 should still be Ready, n4 should still be Pending
    assert_eq!(dag.nodes[1].status, DagNodeStatus::Ready); // n2
    assert_eq!(dag.nodes[3].status, DagNodeStatus::Pending); // n4

    // Complete n2 -> n4 becomes ready
    dag.complete_node("n2", "ok");
    mark_ready_nodes(&mut dag);
    assert_eq!(dag.nodes[3].status, DagNodeStatus::Ready);

    dag.complete_node("n4", "ok");
    assert!(dag.is_finished());
}

// ─── execute_dag async tests ─────────────────────────────────────────

#[tokio::test]
async fn test_execute_dag_empty_returns_all_completed() {
    use crate::bus::EventBus;
    use crate::context::SharedContext;
    use crate::daemon_config::DaemonConfig;
    use arc_swap::ArcSwap;

    let mut dag = TaskDag { nodes: vec![] };
    let config = DagExecutorConfig::default();

    // Build minimal dependencies for execute_dag
    let bus = EventBus::default();
    let ctx = Arc::new(SharedContext::default());
    let daemon_config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));

    // We need a router, but it won't be called for an empty DAG
    let mock_provider = Arc::new(MockEmptyDagProvider);
    let router = Arc::new(openalpaca_llm::LlmRouter::single_provider(
        mock_provider,
        openalpaca_llm::ProviderType::Anthropic,
        "mock-model".to_string(),
    ));
    let tool_registry = Arc::new(crate::tools::ToolRegistry::default());

    let result = execute_dag(
        &mut dag,
        &config,
        router,
        tool_registry,
        bus,
        ctx,
        "test-task",
        "test description",
        None,        // db
        "tester",    // created_by
        &daemon_config,
        None,        // cancel_token
        None,        // workspace_id
        "",          // connector_guidance
        None,        // confirmation_broker
    )
    .await;

    assert!(result.success);
    assert!(result.node_results.is_empty());
    assert_eq!(result.total_input_tokens, 0);
    assert_eq!(result.total_output_tokens, 0);
    assert!(matches!(result.finish_reason, DagFinishReason::AllCompleted));
}

#[tokio::test]
async fn test_execute_dag_cancellation_before_start() {
    use crate::bus::EventBus;
    use crate::context::SharedContext;
    use crate::daemon_config::DaemonConfig;
    use arc_swap::ArcSwap;

    let mut dag = TaskDag {
        nodes: vec![make_node("n1", "a1", &[]), make_node("n2", "a1", &["n1"])],
    };
    let config = DagExecutorConfig::default();

    let bus = EventBus::default();
    let ctx = Arc::new(SharedContext::default());
    let daemon_config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));

    let mock_provider = Arc::new(MockEmptyDagProvider);
    let router = Arc::new(openalpaca_llm::LlmRouter::single_provider(
        mock_provider,
        openalpaca_llm::ProviderType::Anthropic,
        "mock-model".to_string(),
    ));
    let tool_registry = Arc::new(crate::tools::ToolRegistry::default());

    // Pre-cancel before execution
    let token = CancellationToken::new();
    token.cancel();

    let result = execute_dag(
        &mut dag,
        &config,
        router,
        tool_registry,
        bus,
        ctx,
        "test-task",
        "test description",
        None,
        "tester",
        &daemon_config,
        Some(token),
        None,
        "",
        None,
    )
    .await;

    assert!(!result.success);
    assert!(
        matches!(result.finish_reason, DagFinishReason::Aborted { ref reason } if reason.contains("cancelled")),
        "Expected Aborted with cancellation reason, got: {:?}",
        result.finish_reason,
    );
}

/// Trivial mock provider for DAG tests — never expected to be called in
/// empty-DAG or pre-cancelled scenarios.
struct MockEmptyDagProvider;

#[async_trait::async_trait]
impl openalpaca_llm::LlmProvider for MockEmptyDagProvider {
    fn name(&self) -> &str {
        "mock-dag"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(
        &self,
        _request: openalpaca_llm::ChatRequest,
    ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
        Err(openalpaca_llm::LlmError::NotConfigured)
    }
}
