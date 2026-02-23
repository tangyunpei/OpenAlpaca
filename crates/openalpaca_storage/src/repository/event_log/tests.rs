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
