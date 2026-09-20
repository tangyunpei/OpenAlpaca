use super::*;
use crate::test_util::test_db;

#[test]
fn test_event_log() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    // Log event (now with 4 args)
    let id = repo.log("test_event", None, None, None).unwrap();
    assert!(id > 0);

    // Get recent
    let events = repo.recent(10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "test_event");
}

#[test]
fn test_event_log_result_roundtrip() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    let detail = serde_json::json!({"key": "value"});
    let result = serde_json::json!({"ok": true, "code": 200});

    let _id = repo
        .log("test", Some("agent-1"), Some(&detail), Some(&result))
        .unwrap();

    let rows = repo.recent(1).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].detail.as_ref().unwrap()["key"], "value");
    assert_eq!(rows[0].result.as_ref().unwrap()["ok"], true);
    assert_eq!(rows[0].result.as_ref().unwrap()["code"], 200);
}

// ── GAP-10: the run-scoped log ──────────────────────────────────────────────

/// `log` is what task-less events keep using, and it leaves the column NULL —
/// so a run filter can never sweep an unattributed row in by accident.
#[test]
fn log_leaves_the_task_column_null() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    repo.log("daemon_config_changed", None, None, None).unwrap();

    let rows = repo.recent(1).unwrap();
    assert_eq!(rows[0].task_id, None);
}

/// The id goes in the indexed column, and the row still reads back whole.
#[test]
fn log_for_task_fills_the_indexed_column() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    let detail = serde_json::json!({"task_id": "t-1", "tool_name": "web_search"});
    repo.log_for_task(
        "tool_executed",
        Some("research_agent::a1b2"),
        Some("t-1"),
        Some(&detail),
        None,
    )
    .unwrap();

    let rows = repo.recent(1).unwrap();
    assert_eq!(rows[0].task_id.as_deref(), Some("t-1"));
    assert_eq!(rows[0].agent_id.as_deref(), Some("research_agent::a1b2"));
    // The id stays in `detail` too, so a reader written before the column
    // existed keeps working.
    assert_eq!(rows[0].detail.as_ref().unwrap()["task_id"], "t-1");
}

/// `?task_id=` sees this run's rows and nothing else — not another run's, and
/// not a row that carries the id only in its `detail` blob.
#[test]
fn query_filters_by_task() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    repo.log_for_task("tool_executed", None, Some("t-1"), None, None)
        .unwrap();
    repo.log_for_task("tool_executed", None, Some("t-2"), None, None)
        .unwrap();
    let buried = serde_json::json!({"task_id": "t-1"});
    repo.log("workflow_progress", None, Some(&buried), None)
        .unwrap();

    let rows = repo
        .query(&EventLogQuery {
            task_id: Some("t-1"),
            limit: 50,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].task_id.as_deref(), Some("t-1"));
    assert_eq!(rows[0].event_type, "tool_executed");
}

/// `?event_type=` is an exact match, and it composes with `?task_id=`.
#[test]
fn query_filters_by_event_type_exactly() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    repo.log_for_task("tool_executed", None, Some("t-1"), None, None)
        .unwrap();
    repo.log_for_task("task_status", None, Some("t-1"), None, None)
        .unwrap();
    repo.log_for_task("tool_executed", None, Some("t-2"), None, None)
        .unwrap();

    let rows = repo
        .query(&EventLogQuery {
            task_id: Some("t-1"),
            event_type: Some("tool_executed"),
            limit: 50,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_type, "tool_executed");

    // A prefix is not a match — the filter never widens on its own.
    let none = repo
        .query(&EventLogQuery {
            event_type: Some("tool_"),
            limit: 50,
            ..Default::default()
        })
        .unwrap();
    assert!(none.is_empty());
}

/// Pagination walks the autoincrement `id`, newest first, with `before` an
/// **exclusive** upper bound — so the boundary row is never served twice.
#[test]
fn query_paginates_on_id_with_before_exclusive() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(
            repo.log_for_task("tool_executed", None, Some("t-1"), None, None)
                .unwrap(),
        );
    }

    let first = repo
        .query(&EventLogQuery {
            task_id: Some("t-1"),
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        first.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![ids[4], ids[3]],
        "newest first"
    );

    let second = repo
        .query(&EventLogQuery {
            task_id: Some("t-1"),
            before: Some(ids[3]),
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        second.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1]],
        "`before` excludes the row it names"
    );
}

/// Two rows written inside the same second still order deterministically: the
/// timestamp column has a one-second resolution in its legacy spelling, the
/// `id` does not.
#[test]
fn query_orders_by_id_not_timestamp() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    let older = repo
        .log_for_task("tool_executed", None, Some("t-1"), None, None)
        .unwrap();
    let newer = repo
        .log_for_task("tool_executed", None, Some("t-1"), None, None)
        .unwrap();

    let rows = repo
        .query(&EventLogQuery {
            task_id: Some("t-1"),
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![newer, older]
    );
}

/// No filters at all is the plain "everything, newest first" page.
#[test]
fn query_without_filters_returns_the_newest_rows() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    repo.log("wake", None, None, None).unwrap();
    repo.log_for_task("task_status", None, Some("t-1"), None, None)
        .unwrap();

    let rows = repo
        .query(&EventLogQuery {
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].event_type, "task_status");
}

/// `?agent_id=` keeps working beside the new filters — the Settings event log
/// still reads by agent.
#[test]
fn query_filters_by_agent() {
    let db = test_db();
    let repo = EventLogRepository::new(&db);

    repo.log_for_task("tool_executed", Some("a-1"), Some("t-1"), None, None)
        .unwrap();
    repo.log_for_task("tool_executed", Some("a-2"), Some("t-1"), None, None)
        .unwrap();

    let rows = repo
        .query(&EventLogQuery {
            agent_id: Some("a-2"),
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent_id.as_deref(), Some("a-2"));
}
