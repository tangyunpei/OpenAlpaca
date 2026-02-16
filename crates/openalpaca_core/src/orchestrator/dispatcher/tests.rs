use super::*;
use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent};
use crate::orchestrator::task_planner::TaskPlan;

fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        name: format!("Agent {}", id),
        description: Some(format!("{} agent", id)),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        skills: skills
            .into_iter()
            .map(|s| Skill {
                name: s.to_string(),
                category: "test".to_string(),
                proficiency: 1.0,
            })
            .collect(),
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

fn setup(agents: Vec<SubAgent>) -> TaskDispatcher {
    let ctx = Arc::new(SharedContext::new());
    for a in agents {
        ctx.agent_registry.register(a);
    }
    let lane_mgr = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let tool_registry = Arc::new(crate::tools::ToolRegistry::new());
    let executor = Arc::new(crate::tools::RegistryToolExecutor::new(tool_registry.clone()));
    let sandbox = Arc::new(crate::security::sandbox::SandboxManager::with_defaults(executor, bus.clone()));
    let gate = Arc::new(crate::security::gate::SecurityGate::new(sandbox));
    let daemon_config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    TaskDispatcher::new(ctx, lane_mgr, bus, None, gate, tool_registry, None, None, daemon_config)
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
    };

    let result = dispatcher.dispatch_planned(
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
    let lead = dispatcher.shared_context.agent_registry.get("lead-01").unwrap();
    assert_eq!(lead.status.as_str(), "busy");

    // The worker should still be Idle
    let worker = dispatcher.shared_context.agent_registry.get("worker-01").unwrap();
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
    let lead = dispatcher.shared_context.agent_registry.get("lead-01").unwrap();
    assert_eq!(lead.status.as_str(), "busy");

    // worker-01 should still be idle
    let worker = dispatcher.shared_context.agent_registry.get("worker-01").unwrap();
    assert!(worker.status.is_available());
}

#[test]
fn test_dispatch_lead_agent_fallback_to_any_idle_agent() {
    // When no agent has "lead_orchestration" skill, any idle agent is used
    let dispatcher = setup(vec![
        make_agent("worker-01", vec!["web_search"]),
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

    // worker-01 should be busy (used as fallback lead)
    let worker = dispatcher.shared_context.agent_registry.get("worker-01").unwrap();
    assert_eq!(worker.status.as_str(), "busy");
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
fn test_dispatch_planned_use_lead_agent_false_goes_normal_path() {
    // When use_lead_agent is false, the normal validation path is used
    let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
    let plan = TaskPlan {
        classification: "complex_task".to_string(),
        title: Some("Normal test".to_string()),
        assignments: vec![], // empty → should fail in normal path
        reasoning: None,
        dag: None,
        use_lead_agent: false,
    };

    let result = dispatcher.dispatch_planned(
        "Normal task",
        plan,
        "user1",
        "user1:cli",
        "cli",
        None,
    );

    // Should fail because assignments is empty and use_lead_agent is false
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No agents assigned"));
}
