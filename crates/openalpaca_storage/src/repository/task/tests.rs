use super::*;
use tempfile::tempdir;

fn setup_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

fn make_task(id: &str, title: &str) -> Task {
    let now = Utc::now();
    Task {
        id: id.to_string(),
        title: title.to_string(),
        description: None,
        status: TaskStatus::Queued,
        priority: 0,
        progress_current: None,
        progress_total: None,
        result_summary: None,
        created_by: "user1".to_string(),
        source_lane: "cli".to_string(),
        created_at: now,
        updated_at: now,
        completed_at: None,
        state_json: None,
        state_version: 0,
        outcome_json: None,
        outcome_kind: None,
        artifact_count: 0,
    }
}

#[test]
fn test_create_and_get() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    let task = make_task("t1", "Test Task");
    repo.create(&task).unwrap();

    let fetched = repo.get("t1").unwrap().unwrap();
    assert_eq!(fetched.id, "t1");
    assert_eq!(fetched.title, "Test Task");
    assert_eq!(fetched.status, TaskStatus::Queued);
}

#[test]
fn test_get_nonexistent() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);
    assert!(repo.get("nope").unwrap().is_none());
}

#[test]
fn test_list_by_creator() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task 1")).unwrap();
    repo.create(&make_task("t2", "Task 2")).unwrap();

    let mut other = make_task("t3", "Task 3");
    other.created_by = "user2".to_string();
    repo.create(&other).unwrap();

    let tasks = repo.list_by_creator("user1", 10).unwrap();
    assert_eq!(tasks.len(), 2);
}

#[test]
fn test_list_by_status() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task 1")).unwrap();

    let mut running = make_task("t2", "Task 2");
    running.status = TaskStatus::Running;
    repo.create(&running).unwrap();

    let queued = repo.list_by_status(TaskStatus::Queued, 10).unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, "t1");
}

#[test]
fn test_list_active() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Queued")).unwrap();

    let mut running = make_task("t2", "Running");
    running.status = TaskStatus::Running;
    repo.create(&running).unwrap();

    let mut completed = make_task("t3", "Completed");
    completed.status = TaskStatus::Completed;
    repo.create(&completed).unwrap();

    let active = repo.list_active(10).unwrap();
    assert_eq!(active.len(), 2);
}

#[test]
fn test_update_status() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();

    assert!(repo.update_status("t1", TaskStatus::Running).unwrap());

    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert!(task.completed_at.is_none());

    // Mark completed -> should set completed_at
    assert!(repo.update_status("t1", TaskStatus::Completed).unwrap());
    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    assert!(task.completed_at.is_some());
}

#[test]
fn test_update_status_nonexistent() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);
    assert!(!repo.update_status("nope", TaskStatus::Running).unwrap());
}

#[test]
fn test_update_progress() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();
    assert!(repo.update_progress("t1", 5, 10).unwrap());

    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.progress_current, Some(5));
    assert_eq!(task.progress_total, Some(10));
}

#[test]
fn test_set_result() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();
    assert!(repo.set_result("t1", "All done").unwrap());

    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.result_summary.as_deref(), Some("All done"));
}

#[test]
fn test_delete() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();
    repo.delete("t1").unwrap();
    assert!(repo.get("t1").unwrap().is_none());
}

#[test]
fn test_assignments() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();

    let assignment = TaskAgentAssignment {
        id: "a1".to_string(),
        task_id: "t1".to_string(),
        agent_id: "agent-1".to_string(),
        role: "executor".to_string(),
        status: AssignmentStatus::Pending,
        step_order: Some(1),
        started_at: None,
        completed_at: None,
        result_output: None,
    };
    repo.create_assignment(&assignment).unwrap();

    let assignments = repo.get_assignments("t1").unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].agent_id, "agent-1");
    assert_eq!(assignments[0].status, AssignmentStatus::Pending);

    // Update assignment status
    assert!(
        repo.update_assignment_status("a1", AssignmentStatus::Running)
            .unwrap()
    );
    let assignments = repo.get_assignments("t1").unwrap();
    assert_eq!(assignments[0].status, AssignmentStatus::Running);
    assert!(assignments[0].started_at.is_some());
}

#[test]
fn test_update_state_optimistic_locking() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();

    // Version 0 → 1 should succeed
    assert!(
        repo.update_state("t1", r#"{"objective":"test"}"#, 0)
            .unwrap()
    );

    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.state_version, 1);
    assert_eq!(task.state_json.as_deref(), Some(r#"{"objective":"test"}"#));

    // Stale version 0 should fail (current is 1)
    assert!(
        !repo
            .update_state("t1", r#"{"objective":"stale"}"#, 0)
            .unwrap()
    );

    // Version 1 → 2 should succeed
    assert!(
        repo.update_state("t1", r#"{"objective":"updated"}"#, 1)
            .unwrap()
    );
    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.state_version, 2);
    assert_eq!(
        task.state_json.as_deref(),
        Some(r#"{"objective":"updated"}"#)
    );
}

#[test]
fn test_list_active_by_creator() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    // user1: queued + running (should appear)
    repo.create(&make_task("t1", "Queued")).unwrap();

    let mut running = make_task("t2", "Running");
    running.status = TaskStatus::Running;
    repo.create(&running).unwrap();

    // user1: completed (should NOT appear)
    let mut completed = make_task("t3", "Completed");
    completed.status = TaskStatus::Completed;
    repo.create(&completed).unwrap();

    // user2: queued (should NOT appear for user1)
    let mut other = make_task("t4", "Other User");
    other.created_by = "user2".to_string();
    repo.create(&other).unwrap();

    let active = repo.list_active_by_creator("user1", 10).unwrap();
    assert_eq!(active.len(), 2);
    // Should be user1's tasks only
    assert!(active.iter().all(|t| t.created_by == "user1"));
    // Should be active statuses only
    assert!(active.iter().all(|t| !t.status.is_terminal()));
}

#[test]
fn test_delete_cascades_assignments() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();
    let assignment = TaskAgentAssignment {
        id: "a1".to_string(),
        task_id: "t1".to_string(),
        agent_id: "agent-1".to_string(),
        role: "executor".to_string(),
        status: AssignmentStatus::Pending,
        step_order: None,
        started_at: None,
        completed_at: None,
        result_output: None,
    };
    repo.create_assignment(&assignment).unwrap();

    // Delete task -> should cascade to assignments
    repo.delete("t1").unwrap();
    let assignments = repo.get_assignments("t1").unwrap();
    assert!(assignments.is_empty());
}

#[test]
fn test_set_outcome() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();
    assert!(repo
        .set_outcome(
            "t1",
            r#"{"summary":"Done","artifacts":[]}"#,
            OutcomeKind::TextOnly,
            0,
        )
        .unwrap());

    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.outcome_kind, Some(OutcomeKind::TextOnly));
    assert_eq!(task.artifact_count, 0);
    assert!(task.outcome_json.is_some());
    // result_summary is NOT set by set_outcome — it is handled by finalize_task
    assert!(task.result_summary.is_none());
}

#[test]
fn test_outcome_fields_default_null() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();
    let task = repo.get("t1").unwrap().unwrap();
    assert!(task.outcome_json.is_none());
    assert!(task.outcome_kind.is_none());
    assert_eq!(task.artifact_count, 0);
}

#[test]
fn test_set_outcome_updates_existing() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t1", "Task")).unwrap();

    // First set_outcome
    assert!(repo
        .set_outcome(
            "t1",
            r#"{"summary":"First","artifacts":[]}"#,
            OutcomeKind::TextOnly,
            0,
        )
        .unwrap());

    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(task.outcome_kind, Some(OutcomeKind::TextOnly));
    assert_eq!(task.artifact_count, 0);
    assert!(task.outcome_json.as_ref().unwrap().contains("First"));

    // Second set_outcome with different values — should overwrite
    assert!(repo
        .set_outcome(
            "t1",
            r#"{"summary":"Second","artifacts":[{"key":"report.pdf","label":"Report","agent_id":"a1","step_order":0}]}"#,
            OutcomeKind::Mixed,
            1,
        )
        .unwrap());

    let task = repo.get("t1").unwrap().unwrap();
    assert_eq!(
        task.outcome_kind,
        Some(OutcomeKind::Mixed),
        "outcome_kind should be updated to Mixed"
    );
    assert_eq!(
        task.artifact_count, 1,
        "artifact_count should be updated to 1"
    );
    assert!(
        task.outcome_json.as_ref().unwrap().contains("Second"),
        "outcome_json should contain updated summary"
    );
    assert!(
        !task.outcome_json.as_ref().unwrap().contains("First"),
        "outcome_json should not contain old summary"
    );
}
