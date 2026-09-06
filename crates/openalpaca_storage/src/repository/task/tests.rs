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
        workspace_id: None,
        source_task_id: None,
        session_id: None,
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

#[test]
fn test_fail_all_non_terminal_sweeps_only_live_rows() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    // One row per status.
    for (id, status) in [
        ("queued", TaskStatus::Queued),
        ("running", TaskStatus::Running),
        ("paused", TaskStatus::Paused),
        ("completed", TaskStatus::Completed),
        ("failed", TaskStatus::Failed),
        ("cancelled", TaskStatus::Cancelled),
    ] {
        let mut task = make_task(id, id);
        task.status = status;
        repo.create(&task).unwrap();
    }
    // A running row with an existing summary must keep it.
    let mut with_summary = make_task("running-with-summary", "has summary");
    with_summary.status = TaskStatus::Running;
    with_summary.result_summary = Some("partial progress".to_string());
    repo.create(&with_summary).unwrap();

    let swept = repo
        .fail_all_non_terminal("daemon restarted — task orphaned")
        .unwrap();
    assert_eq!(swept, 4, "queued + running + paused + running-with-summary");

    // Non-terminal rows flipped to Failed with the reason + completed_at.
    for id in ["queued", "running", "paused"] {
        let task = repo.get(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Failed, "task {id}");
        assert_eq!(
            task.result_summary.as_deref(),
            Some("daemon restarted — task orphaned"),
            "task {id}"
        );
        assert!(task.completed_at.is_some(), "task {id}");
    }
    // Existing summary preserved.
    let task = repo.get("running-with-summary").unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.result_summary.as_deref(), Some("partial progress"));

    // Terminal rows untouched.
    for (id, status) in [
        ("completed", TaskStatus::Completed),
        ("failed", TaskStatus::Failed),
        ("cancelled", TaskStatus::Cancelled),
    ] {
        let task = repo.get(id).unwrap().unwrap();
        assert_eq!(task.status, status, "terminal task {id} must be untouched");
        assert!(task.result_summary.is_none(), "terminal task {id}");
    }

    // Idempotent: a second sweep finds nothing.
    assert_eq!(repo.fail_all_non_terminal("again").unwrap(), 0);
}

/// Migration 036's `task.workspace_id` — the project a run belonged to, so a
/// rerun and the Library can filter by project. It is the *request's*
/// workspace root (never the daemon CWD, ruling R22), and `NULL` for every
/// turn that arrived without one.
#[test]
fn workspace_id_round_trips_and_defaults_to_none() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    let mut in_project = make_task("t-project", "Ship the release");
    in_project.workspace_id = Some("/Users/dev/openalpaca".to_string());
    repo.create(&in_project).unwrap();
    repo.create(&make_task("t-loose", "Answer a Telegram question"))
        .unwrap();

    assert_eq!(
        repo.get("t-project").unwrap().unwrap().workspace_id.as_deref(),
        Some("/Users/dev/openalpaca")
    );
    assert_eq!(repo.get("t-loose").unwrap().unwrap().workspace_id, None);

    // Every list path reads the same column, so a Library filter sees it too.
    let listed = repo.list_by_creator("user1", 10).unwrap();
    let project_row = listed.iter().find(|t| t.id == "t-project").unwrap();
    assert_eq!(
        project_row.workspace_id.as_deref(),
        Some("/Users/dev/openalpaca")
    );
    let recent = repo.list_recent(10).unwrap();
    assert_eq!(
        recent.iter().find(|t| t.id == "t-loose").unwrap().workspace_id,
        None
    );
}

/// Migration 037's `task.source_task_id` — the provenance link a re-run writes
/// from the new row back to the one it copied (GAP-06). `NULL` on every row
/// that was not born of a re-run, which is nearly all of them.
#[test]
fn source_task_id_round_trips_and_defaults_to_none() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    repo.create(&make_task("t-original", "Ship the release"))
        .unwrap();
    let mut copy = make_task("t-copy", "Ship the release");
    copy.source_task_id = Some("t-original".to_string());
    repo.create(&copy).unwrap();

    assert_eq!(repo.get("t-original").unwrap().unwrap().source_task_id, None);
    assert_eq!(
        repo.get("t-copy")
            .unwrap()
            .unwrap()
            .source_task_id
            .as_deref(),
        Some("t-original")
    );

    // Every list path reads the same column, so a client that lists runs can
    // see which one a row came from without a second request.
    let listed = repo.list_recent(10).unwrap();
    let copy = listed.iter().find(|t| t.id == "t-copy").unwrap();
    assert_eq!(copy.source_task_id.as_deref(), Some("t-original"));
}

// ============================================================================
// upsert_queued — D5's `start` keeps the task id
// ============================================================================

/// The plan's acknowledged "least clean" code, isolated here so it has exactly
/// one caller and one test: `start` re-launches a stored row **under its own
/// id**, so the dispatcher's persist step cannot be a plain `INSERT`.
#[test]
fn upsert_queued_creates_a_row_that_does_not_exist_yet() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    let mut task = make_task("t1", "Ship the release");
    task.description = Some("do the thing".to_string());
    repo.upsert_queued(&task).unwrap();

    let stored = repo.get("t1").unwrap().unwrap();
    assert_eq!(stored.title, "Ship the release");
    assert_eq!(stored.description.as_deref(), Some("do the thing"));
    assert_eq!(stored.status, TaskStatus::Queued);
}

/// Re-launching an existing row resets it to a fresh queued run: the previous
/// attempt's outcome, summary, progress and state are cleared, because leaving
/// them would describe a run that is no longer the one this row names. The
/// row's identity — its id, its creation time and its priority — survives.
#[test]
fn upsert_queued_relaunches_an_existing_row_in_place() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    let mut original = make_task("t1", "Ship the release");
    original.priority = 7;
    original.description = Some("first attempt".to_string());
    repo.create(&original).unwrap();
    repo.update_state("t1", r#"{"objective":"old"}"#, 0).unwrap();
    repo.set_result("t1", "cancelled halfway").unwrap();
    repo.set_outcome("t1", r#"{"summary":"partial"}"#, OutcomeKind::Mixed, 2)
        .unwrap();
    repo.update_status("t1", TaskStatus::Cancelled).unwrap();
    let created_at = repo.get("t1").unwrap().unwrap().created_at;

    let mut relaunch = make_task("t1", "Ship the release");
    relaunch.description = Some("second attempt".to_string());
    relaunch.workspace_id = Some("/Users/dev/openalpaca".to_string());
    repo.upsert_queued(&relaunch).unwrap();

    let stored = repo.get("t1").unwrap().unwrap();
    assert_eq!(stored.status, TaskStatus::Queued);
    assert_eq!(stored.description.as_deref(), Some("second attempt"));
    assert_eq!(
        stored.workspace_id.as_deref(),
        Some("/Users/dev/openalpaca")
    );
    assert!(stored.result_summary.is_none(), "the old summary is gone");
    assert!(stored.outcome_json.is_none(), "the old outcome is gone");
    assert!(stored.outcome_kind.is_none());
    assert_eq!(stored.artifact_count, 0);
    assert!(stored.completed_at.is_none(), "it has not finished again");
    assert!(stored.state_json.is_none());
    assert_eq!(
        stored.state_version, 0,
        "the dispatcher's state init writes against version 0"
    );

    // Identity is not re-minted: same row, same age, same priority.
    assert_eq!(stored.created_at, created_at);
    assert_eq!(stored.priority, 7);

    // And exactly one row still answers to the id.
    assert_eq!(repo.list_recent(10).unwrap().len(), 1);
}

/// Idempotent in the sense the dispatcher needs: calling it twice leaves one
/// queued row, not two rows or an error.
#[test]
fn upsert_queued_is_idempotent() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);

    let task = make_task("t1", "Ship the release");
    repo.upsert_queued(&task).unwrap();
    repo.upsert_queued(&task).unwrap();

    assert_eq!(repo.list_recent(10).unwrap().len(), 1);
    assert_eq!(repo.get("t1").unwrap().unwrap().status, TaskStatus::Queued);
}

// ============================================================================
// titles_for — the artifact list's join (R26)
// ============================================================================

#[test]
fn titles_for_returns_one_entry_per_known_id() {
    let db = setup_db();
    let repo = TaskRepository::new(&db);
    for (id, title) in [("t1", "Run one"), ("t2", "Run two"), ("t3", "Run three")] {
        repo.create(&make_task(id, title)).unwrap();
    }

    let ids = ["t1".to_string(), "t3".to_string(), "gone".to_string()];
    let titles = repo.titles_for(&ids).unwrap();

    assert_eq!(titles.len(), 2, "a task that no longer exists is absent");
    assert_eq!(titles.get("t1").map(String::as_str), Some("Run one"));
    assert_eq!(titles.get("t3").map(String::as_str), Some("Run three"));
    assert!(!titles.contains_key("gone"));
    // t2 was not asked for.
    assert!(!titles.contains_key("t2"));
}

#[test]
fn titles_for_an_empty_slice_is_an_empty_map() {
    let db = setup_db();
    assert!(TaskRepository::new(&db).titles_for(&[]).unwrap().is_empty());
}

/// More ids than one `IN (…)` may carry: the helper chunks rather than
/// building a statement with thousands of placeholders (SQLite's variable
/// limit) or falling back to one query per id.
#[test]
fn titles_for_chunks_past_the_in_limit() {
    let db = setup_db();
    let count = 1_200;
    db.with_connection(|conn| {
        let tx = conn.unchecked_transaction()?;
        for n in 0..count {
            tx.execute(
                "INSERT INTO task (id, title, created_by, source_lane)
                 VALUES (?1, ?2, 'user1', 'cli')",
                [format!("t{n:05}"), format!("Run {n}")],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .unwrap();

    let ids: Vec<String> = (0..count).map(|n| format!("t{n:05}")).collect();
    let titles = TaskRepository::new(&db).titles_for(&ids).unwrap();

    assert_eq!(titles.len(), count);
    assert_eq!(titles.get("t00000").map(String::as_str), Some("Run 0"));
    assert_eq!(titles.get("t01199").map(String::as_str), Some("Run 1199"));
}
