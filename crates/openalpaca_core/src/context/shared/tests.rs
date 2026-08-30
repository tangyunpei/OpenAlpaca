use super::*;

#[test]
fn test_shared_context_creation() {
    let ctx = SharedContext::new();
    assert_eq!(ctx.task_registry.count(), 0);
    assert_eq!(ctx.agent_registry.count(), 0);
}

#[test]
fn test_task_registry_empty() {
    let reg = TaskRegistry::new();
    assert_eq!(reg.count(), 0);
}

#[test]
fn test_task_registry_register_and_remove() {
    let reg = TaskRegistry::new();
    assert!(reg.register("t1".into(), "task one".into()));
    assert!(!reg.register("t1".into(), "duplicate".into()));
    assert_eq!(reg.count(), 1);
    assert!(reg.remove("t1"));
    assert_eq!(reg.count(), 0);
    assert!(!reg.remove("t1"));
}

#[test]
fn test_task_registry_update_status() {
    let reg = TaskRegistry::new();
    reg.register("t1".into(), "task one".into());

    assert!(reg.update_status("t1", TaskEntryStatus::Running));
    let entry = reg.get("t1").unwrap();
    assert_eq!(entry.status, TaskEntryStatus::Running);

    assert!(!reg.update_status("nope", TaskEntryStatus::Running));
}

#[test]
fn test_task_registry_list_active() {
    let reg = TaskRegistry::new();
    reg.register("t1".into(), "queued".into());
    reg.register("t2".into(), "will run".into());
    reg.register("t3".into(), "will complete".into());

    reg.update_status("t2", TaskEntryStatus::Running);
    reg.update_status("t3", TaskEntryStatus::Completed);

    let active = reg.list_active();
    assert_eq!(active.len(), 2); // t1 (queued) and t2 (running)
}

#[test]
fn test_task_entry_status_terminal() {
    assert!(!TaskEntryStatus::Queued.is_terminal());
    assert!(!TaskEntryStatus::Running.is_terminal());
    assert!(!TaskEntryStatus::Paused.is_terminal());
    assert!(TaskEntryStatus::Completed.is_terminal());
    assert!(TaskEntryStatus::Failed.is_terminal());
    assert!(TaskEntryStatus::Cancelled.is_terminal());
}

#[test]
fn test_agent_registry_in_shared_context() {
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent,
    };

    let ctx = SharedContext::new();
    let agent = SubAgent {
        id: "a1".to_string(),
        template_id: "a1".to_string(),
        name: "Test Agent".to_string(),
        description: None,
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: vec![],
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    };
    assert!(ctx.agent_registry.register(agent));
    assert_eq!(ctx.agent_registry.count(), 1);
    assert!(ctx.agent_registry.get("a1").is_some());
}

#[test]
fn test_steering_inbox_registry() {
    use crate::runner::steering::SteeringInbox;

    let ctx = SharedContext::new();
    assert!(ctx.steering_inbox("t1").is_none());

    let inbox = Arc::new(SteeringInbox::default());
    ctx.register_steering_inbox("t1", Arc::clone(&inbox));
    let found = ctx.steering_inbox("t1").expect("inbox should be registered");
    assert!(Arc::ptr_eq(&found, &inbox));

    let removed = ctx.remove_steering_inbox("t1").expect("inbox should be removable");
    assert!(Arc::ptr_eq(&removed, &inbox));
    assert!(ctx.steering_inbox("t1").is_none());
    assert!(ctx.remove_steering_inbox("t1").is_none());
}

#[test]
fn test_workflows_by_lane_registry() {
    let ctx = SharedContext::new();
    assert!(ctx.workflows_for_lane("lane-a").is_empty());

    ctx.register_workflow_for_lane("lane-a", "t1");
    ctx.register_workflow_for_lane("lane-a", "t2");
    ctx.register_workflow_for_lane("lane-a", "t1"); // dedup
    ctx.register_workflow_for_lane("lane-b", "t3");

    assert_eq!(ctx.workflows_for_lane("lane-a"), vec!["t1".to_string(), "t2".to_string()]);
    assert_eq!(ctx.workflows_for_lane("lane-b"), vec!["t3".to_string()]);

    ctx.deregister_workflow_for_lane("lane-a", "t1");
    assert_eq!(ctx.workflows_for_lane("lane-a"), vec!["t2".to_string()]);

    // Deregistering an unknown task/lane is a no-op.
    ctx.deregister_workflow_for_lane("lane-a", "nope");
    ctx.deregister_workflow_for_lane("lane-x", "t2");
    assert_eq!(ctx.workflows_for_lane("lane-a"), vec!["t2".to_string()]);

    // Removing the last workflow drops the lane entry entirely.
    ctx.deregister_workflow_for_lane("lane-a", "t2");
    assert!(ctx.workflows_for_lane("lane-a").is_empty());
}
