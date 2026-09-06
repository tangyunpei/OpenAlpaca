use super::*;
use openalpaca_storage::{Task, TaskRepository, TaskStatus};

fn setup_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    let now = Utc::now();
    TaskRepository::new(&db)
        .create(&Task {
            id: "task-1".to_string(),
            title: "a run".to_string(),
            description: None,
            status: TaskStatus::Running,
            priority: 0,
            progress_current: None,
            progress_total: None,
            result_summary: None,
            created_by: "tester".to_string(),
            source_lane: "user:cli".to_string(),
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
        })
        .unwrap();
    (dir, db)
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<SystemEvent>) -> Vec<SystemEvent> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

#[test]
fn opening_a_span_announces_the_row_it_wrote() {
    let (_dir, db) = setup_db();
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();

    let label = open_span(
        Some(&db),
        &bus,
        "task-1",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        "check the numbers",
    );
    assert_eq!(label.as_deref(), Some("review·1"));

    let events = drain(&mut rx);
    assert_eq!(events.len(), 1);
    match &events[0] {
        SystemEvent::SubagentSpan {
            task_id,
            span_id,
            label,
            template_id,
            agent_instance_id,
            state,
            detail,
            started_at,
            ended_at,
            duration_ms,
            output_preview,
            ..
        } => {
            assert_eq!(task_id, "task-1");
            assert_eq!(span_id, "node-1");
            assert_eq!(label, "review·1");
            assert_eq!(template_id, "review_agent");
            assert_eq!(agent_instance_id, "review_agent::abcd");
            assert_eq!(state, "running");
            assert!(detail.is_none());
            assert!(!started_at.is_empty());
            assert!(ended_at.is_none());
            assert!(duration_ms.is_none());
            assert!(output_preview.is_none());
        }
        other => panic!("Expected SubagentSpan, got {other:?}"),
    }
}

#[test]
fn the_announced_start_is_the_stored_start() {
    let (_dir, db) = setup_db();
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();

    open_span(
        Some(&db),
        &bus,
        "task-1",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        "go",
    );
    let announced = match &drain(&mut rx)[0] {
        SystemEvent::SubagentSpan { started_at, .. } => started_at.clone(),
        other => panic!("Expected SubagentSpan, got {other:?}"),
    };

    let stored = openalpaca_storage::SubagentSpanRepository::new(&db)
        .list_for_task("task-1")
        .unwrap();
    assert_eq!(stored[0].started_at, announced);
}

#[test]
fn closing_a_span_announces_the_terminal_state() {
    let (_dir, db) = setup_db();
    let bus = EventBus::new(16);
    open_span(
        Some(&db),
        &bus,
        "task-1",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        "go",
    );
    let mut rx = bus.subscribe();

    close_span(
        Some(&db),
        &bus,
        "node-1",
        SpanState::Done,
        None,
        Some("all clear"),
    );

    let events = drain(&mut rx);
    assert_eq!(events.len(), 1);
    match &events[0] {
        SystemEvent::SubagentSpan {
            state,
            ended_at,
            duration_ms,
            output_preview,
            label,
            ..
        } => {
            assert_eq!(state, "done");
            assert!(ended_at.is_some());
            assert!(duration_ms.is_some());
            assert_eq!(output_preview.as_deref(), Some("all clear"));
            // The close reads its own label back out of the row.
            assert_eq!(label, "review·1");
        }
        other => panic!("Expected SubagentSpan, got {other:?}"),
    }
}

#[test]
fn a_second_close_announces_nothing() {
    let (_dir, db) = setup_db();
    let bus = EventBus::new(16);
    open_span(
        Some(&db),
        &bus,
        "task-1",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        "go",
    );
    close_span(Some(&db), &bus, "node-1", SpanState::Done, None, None);
    let mut rx = bus.subscribe();

    close_span(
        Some(&db),
        &bus,
        "node-1",
        SpanState::Failed,
        Some("late"),
        None,
    );

    assert!(
        drain(&mut rx).is_empty(),
        "a closed span has no second transition to announce"
    );
}

#[test]
fn an_empty_output_preview_is_stored_as_none() {
    let (_dir, db) = setup_db();
    let bus = EventBus::new(16);
    open_span(
        Some(&db),
        &bus,
        "task-1",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        "go",
    );
    close_span(Some(&db), &bus, "node-1", SpanState::Done, None, Some(""));

    let stored = openalpaca_storage::SubagentSpanRepository::new(&db)
        .list_for_task("task-1")
        .unwrap();
    assert!(stored[0].output_preview.is_none());
}

#[test]
fn a_long_preview_and_detail_are_capped() {
    let (_dir, db) = setup_db();
    let bus = EventBus::new(16);
    open_span(
        Some(&db),
        &bus,
        "task-1",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        &"o".repeat(2_000),
    );
    close_span(
        Some(&db),
        &bus,
        "node-1",
        SpanState::Failed,
        Some(&"d".repeat(2_000)),
        Some(&"p".repeat(2_000)),
    );

    let stored = &openalpaca_storage::SubagentSpanRepository::new(&db)
        .list_for_task("task-1")
        .unwrap()[0];
    assert_eq!(stored.objective.as_deref().unwrap().chars().count(), 500);
    assert_eq!(stored.detail.as_deref().unwrap().chars().count(), 200);
    assert_eq!(
        stored.output_preview.as_deref().unwrap().chars().count(),
        200
    );
}

#[test]
fn without_a_database_there_is_no_span_and_no_event() {
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();

    let label = open_span(
        None,
        &bus,
        "task-1",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        "go",
    );
    close_span(None, &bus, "node-1", SpanState::Done, None, None);

    assert!(label.is_none());
    assert!(drain(&mut rx).is_empty());
}

#[test]
fn an_unwritable_span_never_announces_one() {
    let (_dir, db) = setup_db();
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();

    // No such task — the FK refuses the row.
    let label = open_span(
        Some(&db),
        &bus,
        "ghost",
        "node-1",
        "review_agent",
        "review_agent::abcd",
        "go",
    );

    assert!(label.is_none());
    assert!(drain(&mut rx).is_empty());
}

#[test]
fn cancellation_is_its_own_state_not_a_failure() {
    assert_eq!(
        span_state_for(&LoopFinishReason::Cancelled),
        SpanState::Cancelled
    );
    assert_eq!(span_state_for(&LoopFinishReason::Complete), SpanState::Done);
    assert_eq!(
        span_state_for(&LoopFinishReason::MaxRounds),
        SpanState::Done
    );
    assert_eq!(
        span_state_for(&LoopFinishReason::Truncated),
        SpanState::Done
    );
    assert_eq!(
        span_state_for(&LoopFinishReason::CostExceeded),
        SpanState::Failed
    );
    assert_eq!(
        span_state_for(&LoopFinishReason::Error("boom".into())),
        SpanState::Failed
    );
}

#[test]
fn a_cancelled_plugin_lane_is_cancelled_not_failed() {
    // `PluginLoopOutcome::Failed { error: "Cancelled" }` is what a cancelled
    // plugin agent returns, so the token — not the message — decides.
    assert_eq!(plugin_span_state(false, true), SpanState::Cancelled);
    assert_eq!(plugin_span_state(false, false), SpanState::Failed);
    assert_eq!(plugin_span_state(true, false), SpanState::Done);
    // A run cancelled *after* the plugin already reported success is still a
    // completed lane: the work happened.
    assert_eq!(plugin_span_state(true, true), SpanState::Done);
}

#[test]
fn a_clean_finish_needs_no_trailing_detail() {
    assert!(span_detail_for(&LoopFinishReason::Complete).is_none());
    assert_eq!(
        span_detail_for(&LoopFinishReason::Error("boom".into())).as_deref(),
        Some("boom")
    );
    assert_eq!(
        span_detail_for(&LoopFinishReason::CostExceeded).as_deref(),
        Some("cost cap reached")
    );
}
