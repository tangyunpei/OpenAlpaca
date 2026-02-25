use super::*;

#[test]
fn test_conversation_lane_message_tracking() {
    let lane = ConversationLane::new(LaneKey::new("u1", "cli"));
    assert_eq!(lane.message_count(), 0);

    lane.record_message();
    assert_eq!(lane.message_count(), 1);

    lane.record_message();
    lane.record_message();
    assert_eq!(lane.message_count(), 3);
}

#[test]
fn test_conversation_lane_last_active_at() {
    let lane = ConversationLane::new(LaneKey::new("u1", "cli"));
    let initial = lane.last_active_at();

    // Small sleep to ensure time advances
    std::thread::sleep(std::time::Duration::from_millis(10));
    lane.record_message();

    assert!(lane.last_active_at() >= initial);
}

#[test]
fn test_lane_key_display() {
    assert_eq!(
        LaneKey::new("user1", "telegram").to_string(),
        "user1:telegram"
    );
    assert_eq!(LaneKey::new("abc", "gui").to_string(), "abc:gui");
}

#[test]
fn test_lane_key_equality() {
    let k1 = LaneKey::new("u1", "cli");
    let k2 = LaneKey::new("u1", "cli");
    let k3 = LaneKey::new("u1", "gui");
    assert_eq!(k1, k2);
    assert_ne!(k1, k3);
}

#[test]
fn test_task_lane_status() {
    let lane = TaskLane::new("task-1");
    assert_eq!(lane.status(), TaskLaneStatus::Queued);

    lane.set_status(TaskLaneStatus::Running);
    assert_eq!(lane.status(), TaskLaneStatus::Running);

    lane.set_status(TaskLaneStatus::Completed);
    assert_eq!(lane.status(), TaskLaneStatus::Completed);
}

#[test]
fn test_task_lane_with_source() {
    let lane = TaskLane::new("task-1").with_source(LaneKey::new("u1", "cli"));
    assert_eq!(lane.source_lane, Some(LaneKey::new("u1", "cli")));
}

#[test]
fn test_task_lane_agents() {
    let lane = TaskLane::new("task-1");
    assert!(lane.assigned_agents().is_empty());

    lane.assign_agent("agent-1".into());
    lane.assign_agent("agent-2".into());

    let agents = lane.assigned_agents();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0], "agent-1");
    assert_eq!(agents[1], "agent-2");
}
