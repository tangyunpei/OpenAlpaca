use super::*;
use crate::agent::subagent::SubAgent;
use crate::orchestrator::task_planner::TaskPlan;
use crate::test_util::{make_agent, template_from_agent};

fn setup(agents: Vec<SubAgent>) -> TaskDispatcher {
    setup_with_config(agents, DaemonConfig::default())
}

fn setup_with_config(agents: Vec<SubAgent>, config: DaemonConfig) -> TaskDispatcher {
    let ctx = Arc::new(SharedContext::new());
    for a in &agents {
        // Register both template (for spawn_instance) and instance (for backward compat)
        ctx.agent_registry.register_template(template_from_agent(a));
        ctx.agent_registry.register(a.clone());
    }
    let lane_mgr = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let tool_registry = Arc::new(crate::tools::ToolRegistry::new());
    let executor = Arc::new(crate::tools::RegistryToolExecutor::new(
        tool_registry.clone(),
    ));
    let sandbox = Arc::new(crate::security::sandbox::SandboxManager::with_defaults(
        executor,
        bus.clone(),
    ));
    let gate = Arc::new(crate::security::gate::SecurityGate::new(sandbox));
    let daemon_config = Arc::new(ArcSwap::from_pointee(config));
    TaskDispatcher::new(
        ctx,
        lane_mgr,
        bus,
        None,
        gate,
        tool_registry,
        None,
        None,
        daemon_config,
        Arc::new(std::sync::RwLock::new(None)),
    )
}

#[test]
fn test_creates_task_and_lane() {
    let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
    let result = dispatcher.dispatch(
        Uuid::new_v4(),
        "cli",
        "Search for Rust docs",
        &["web_search".to_string()],
        "user1",
        "user1:cli",
        None,
    );

    assert!(result.is_ok());
    let text = result.unwrap();
    // Response is now human-readable, not JSON
    assert!(text.contains("assigned"));
    assert!(text.contains("Agent a1"));

    // Verify task registered
    assert_eq!(dispatcher.shared_context.task_registry.count(), 1);
    // Verify lane created
    assert_eq!(dispatcher.lane_manager.task_count(), 1);
}

#[test]
fn test_fails_no_matching() {
    let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
    let result = dispatcher.dispatch(
        Uuid::new_v4(),
        "cli",
        "Generate text",
        &["text_generate".to_string()],
        "user1",
        "user1:cli",
        None,
    );
    assert!(result.is_err());
}

#[test]
fn test_assigns_multiple() {
    let dispatcher = setup(vec![
        make_agent("a1", vec!["web_search"]),
        make_agent("a2", vec!["text_generate"]),
    ]);
    let result = dispatcher
        .dispatch(
            Uuid::new_v4(),
            "cli",
            "Research and write report",
            &["web_search".to_string(), "text_generate".to_string()],
            "user1",
            "user1:cli",
            None,
        )
        .unwrap();

    // Human-readable response should mention both agents
    assert!(result.contains("Agent a1"));
    assert!(result.contains("Agent a2"));
}

#[test]
fn test_title_generation_short() {
    // Test the generate_title function directly
    assert_eq!(generate_title("Short task"), "Short task");
}

#[test]
fn test_title_generation_strips_filler() {
    assert_eq!(
        generate_title("can you search for Rust docs"),
        "Search for rust docs"
    );
    assert_eq!(
        generate_title("please help me find the answer"),
        "Find the answer"
    );
}

#[test]
fn test_title_generation_truncates_long() {
    let long_desc = "search for the latest version of rust and all related documentation";
    let title = generate_title(long_desc);
    assert!(title.len() <= 53); // 50 + "..."
    assert!(title.ends_with("..."));
}

#[test]
fn test_dispatch_planned_with_use_lead_agent_routes_correctly() {
    // When use_lead_agent is true, dispatch_planned should use the lead agent path
    // regardless of assignments being empty.
    let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
    let plan = TaskPlan {
        classification: "complex_task".to_string(),
        title: Some("Lead agent test".to_string()),
        assignments: vec![], // empty assignments — would fail normal path
        reasoning: None,
        dag: None,
        use_lead_agent: true,
        auto_promotion_reason: None,
        execution_mode: None,
        predictability_score: None,
    };

    let result = dispatcher.dispatch_planned(
        Uuid::new_v4(),
        "Research and synthesize a complex topic",
        plan,
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    // Should succeed because lead agent path doesn't require assignments
    assert!(result.is_ok());
    let text = result.unwrap();
    assert!(text.contains("Lead Agent"));
    assert!(text.contains("dynamic orchestration"));

    // Task should be registered
    assert_eq!(dispatcher.shared_context.task_registry.count(), 1);
    // Lane should be created
    assert_eq!(dispatcher.lane_manager.task_count(), 1);
}

#[test]
fn test_dispatch_lead_agent_marks_agent_busy() {
    let dispatcher = setup(vec![
        make_agent("lead-01", vec!["lead_orchestration"]),
        make_agent("worker-01", vec!["web_search"]),
    ]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex research task",
        "Test Lead Agent Task".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_ok());

    // The lead agent should be marked Busy
    let lead = dispatcher
        .shared_context
        .agent_registry
        .get("lead-01")
        .unwrap();
    assert_eq!(lead.status.as_str(), "busy");

    // The worker should still be Idle
    let worker = dispatcher
        .shared_context
        .agent_registry
        .get("worker-01")
        .unwrap();
    assert!(worker.status.is_available());
}

#[test]
fn test_dispatch_lead_agent_prefers_lead_orchestration_skill() {
    // When an agent with "lead_orchestration" skill exists, it should be preferred
    let dispatcher = setup(vec![
        make_agent("worker-01", vec!["web_search"]),
        make_agent("lead-01", vec!["lead_orchestration"]),
    ]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex task",
        "Test".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_ok());

    // lead-01 should be busy (it has lead_orchestration skill)
    let lead = dispatcher
        .shared_context
        .agent_registry
        .get("lead-01")
        .unwrap();
    assert_eq!(lead.status.as_str(), "busy");

    // worker-01 should still be idle
    let worker = dispatcher
        .shared_context
        .agent_registry
        .get("worker-01")
        .unwrap();
    assert!(worker.status.is_available());
}

#[test]
fn test_dispatch_lead_agent_fallback_to_any_idle_agent() {
    // When no agent has "lead_orchestration" skill, any idle agent template is used.
    // The worker-01 template is non-singleton, so spawn_instance creates a new instance
    // with a UUID suffix. We verify that:
    // 1. dispatch succeeds
    // 2. a new instance exists for the worker-01 template
    let dispatcher = setup(vec![make_agent("worker-01", vec!["web_search"])]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex task",
        "Test".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_ok());

    // A new instance spawned from worker-01 template should exist and be busy
    assert!(
        dispatcher
            .shared_context
            .agent_registry
            .count_instances_of("worker-01")
            >= 1
    );
    let instances = dispatcher.shared_context.agent_registry.list_instances();
    let busy_worker = instances
        .iter()
        .find(|a| a.template_id == "worker-01" && !a.status.is_available());
    assert!(
        busy_worker.is_some(),
        "Expected a busy instance of worker-01"
    );
}

#[test]
fn test_dispatch_lead_agent_fails_no_agents() {
    // When no agents are available at all, should fail
    let dispatcher = setup(vec![]);

    let result = dispatcher.dispatch_lead_agent(
        "Complex task",
        "Test".to_string(),
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No agents available"));
}

#[test]
fn test_dispatch_planned_empty_assignments_promotes_to_lead_agent() {
    // When use_lead_agent is false and assignments are empty,
    // dispatch_planned auto-promotes to lead agent as a last-resort safety net.
    let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
    let plan = TaskPlan {
        classification: "complex_task".to_string(),
        title: Some("Normal test".to_string()),
        assignments: vec![], // empty → last-resort lead agent promotion
        reasoning: None,
        dag: None,
        use_lead_agent: false,
        auto_promotion_reason: None,
        execution_mode: None,
        predictability_score: None,
    };

    let result = dispatcher.dispatch_planned(
        Uuid::new_v4(),
        "Normal task",
        plan,
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    // Should succeed — dispatched to lead agent as last-resort fallback
    assert!(result.is_ok());
    assert!(result.unwrap().contains("Lead Agent"));
}

#[test]
fn test_dispatch_planned_pipeline_empty_assignments_decision_and_execution_agree() {
    // Regression: execution_mode="pipeline" with empty assignments must record
    // LeadAgent (not SequentialPipeline) AND actually execute the lead-agent path.
    // This catches the drift where analyze_plan() would say "pipeline" but
    // dispatch_planned() would actually run lead-agent.
    use crate::events::SystemEvent;

    let mut config = DaemonConfig::default();
    config.execution.planner.dispatch_analysis_enabled = true;

    let dispatcher = setup_with_config(vec![make_agent("a1", vec!["web_search"])], config);

    // Subscribe to the event bus BEFORE dispatching
    let mut rx = dispatcher.bus.subscribe();

    let plan = TaskPlan {
        classification: "complex_task".to_string(),
        title: Some("Pipeline drift test".to_string()),
        assignments: vec![], // empty → should NOT be treated as pipeline
        reasoning: None,
        dag: None,
        use_lead_agent: false,
        auto_promotion_reason: None,
        execution_mode: Some("pipeline".to_string()), // planner says pipeline
        predictability_score: Some(0.75),
    };

    let result = dispatcher.dispatch_planned(
        Uuid::new_v4(),
        "Test pipeline drift",
        plan,
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    // 1. Execution must route to lead-agent (not fail trying to run empty pipeline)
    assert!(result.is_ok(), "Expected dispatch to succeed");
    assert!(
        result.unwrap().contains("Lead Agent"),
        "Expected lead-agent execution, not pipeline"
    );

    // 2. The DispatchDecision event must record lead_agent, not sequential_pipeline
    let mut found_decision = false;
    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::DispatchDecision { mode, reason, .. } = event {
            assert_eq!(
                mode, "lead_agent",
                "Decision should record lead_agent, not pipeline"
            );
            assert_eq!(
                reason, "empty_assignments_fallback",
                "Reason should be empty_assignments_fallback"
            );
            found_decision = true;
            break;
        }
    }
    assert!(
        found_decision,
        "Expected DispatchDecision event to be published"
    );
}

// ── build_task_outcome tests ─────────────────────────────────────

#[test]
fn test_build_task_outcome_no_db_success() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "Research completed successfully", true);
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert_eq!(outcome.summary, "Research completed successfully");
    assert!(outcome.artifacts.is_empty());
    assert!(outcome.no_artifact_reason.is_some());
}

#[test]
fn test_build_task_outcome_no_db_failure() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "Network timeout", false);
    assert_eq!(outcome.outcome_kind, OutcomeKind::Failed);
    assert_eq!(outcome.summary, "Network timeout");
    assert!(outcome.artifacts.is_empty());
    assert!(outcome.no_artifact_reason.is_none());
}

#[test]
fn test_build_task_outcome_empty_content_fallback() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "", true);
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert_eq!(outcome.summary, "Task completed.");
}

#[test]
fn test_build_task_outcome_empty_content_failure() {
    use super::build_task_outcome;
    use openalpaca_storage::OutcomeKind;

    let outcome = build_task_outcome(None, "t1", "", false);
    assert_eq!(outcome.outcome_kind, OutcomeKind::Failed);
    assert_eq!(outcome.summary, "Task failed.");
}
