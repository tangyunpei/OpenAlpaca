use super::*;
use crate::Database;
use tempfile::tempdir;

fn setup_db() -> Database {
    let dir = tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

#[test]
fn test_upsert_and_get() {
    let db = setup_db();

    // Insert a conversation message first (FK target)
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO conversation_messages (lane_key, role, content) VALUES ('test:gui', 'assistant', 'hello')",
            [],
        )?;
        Ok(())
    }).unwrap();

    let repo = MessageFeedbackRepository::new(&db);

    // Insert positive feedback
    repo.upsert(1, "positive", None).unwrap();
    let fb = repo.get_by_message(1).unwrap().unwrap();
    assert_eq!(fb.message_id, 1);
    assert_eq!(fb.feedback, "positive");
    assert!(fb.comment.is_none());

    // Upsert to negative with comment
    repo.upsert(1, "negative", Some("not helpful")).unwrap();
    let fb = repo.get_by_message(1).unwrap().unwrap();
    assert_eq!(fb.feedback, "negative");
    assert_eq!(fb.comment.as_deref(), Some("not helpful"));
}

#[test]
fn test_get_nonexistent() {
    let db = setup_db();
    let repo = MessageFeedbackRepository::new(&db);
    let fb = repo.get_by_message(999).unwrap();
    assert!(fb.is_none());
}

#[test]
fn test_delete() {
    let db = setup_db();

    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO conversation_messages (lane_key, role, content) VALUES ('test:gui', 'assistant', 'hello')",
            [],
        )?;
        Ok(())
    }).unwrap();

    let repo = MessageFeedbackRepository::new(&db);

    repo.upsert(1, "positive", None).unwrap();
    assert!(repo.get_by_message(1).unwrap().is_some());

    let deleted = repo.delete(1).unwrap();
    assert!(deleted);
    assert!(repo.get_by_message(1).unwrap().is_none());

    // Delete again returns false
    let deleted = repo.delete(1).unwrap();
    assert!(!deleted);
}

#[test]
fn test_skill_satisfaction_rates() {
    let db = setup_db();

    // Insert 5+ skill execution log entries with response_message_id
    db.with_connection(|conn| {
        for i in 1..=6 {
            conn.execute(
                "INSERT INTO conversation_messages (lane_key, role, content) VALUES (?1, 'assistant', 'response')",
                rusqlite::params![format!("test{i}:gui")],
            )?;
            conn.execute(
                "INSERT INTO skill_execution_log (request_id, skill_id, agent_id, status, duration_ms, response_message_id)
                 VALUES (?1, 'code-review', 'orchestrator', 'success', 1000, ?2)",
                rusqlite::params![format!("req-{i}"), i],
            )?;
        }
        Ok(())
    }).unwrap();

    let repo = MessageFeedbackRepository::new(&db);

    // Add feedback to some messages
    repo.upsert(1, "positive", None).unwrap();
    repo.upsert(2, "positive", None).unwrap();
    repo.upsert(3, "negative", None).unwrap();

    let rates = repo.skill_satisfaction_rates().unwrap();
    assert_eq!(rates.len(), 1);
    let r = &rates[0];
    assert_eq!(r.skill_id, "code-review");
    assert_eq!(r.feedback_count, 3);
    assert_eq!(r.total_invocations, 6);
    // 2 positive out of 3 feedback
    assert!((r.user_satisfaction_rate - 2.0 / 3.0).abs() < 0.01);
    // 3 feedback out of 6 invocations
    assert!((r.feedback_coverage - 0.5).abs() < 0.01);
}

#[test]
fn test_skill_satisfaction_below_threshold() {
    let db = setup_db();

    // Insert only 3 entries (below threshold of 5)
    db.with_connection(|conn| {
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO conversation_messages (lane_key, role, content) VALUES (?1, 'assistant', 'r')",
                rusqlite::params![format!("t{i}:gui")],
            )?;
            conn.execute(
                "INSERT INTO skill_execution_log (request_id, skill_id, agent_id, status, duration_ms, response_message_id)
                 VALUES (?1, 'small-skill', 'orchestrator', 'success', 500, ?2)",
                rusqlite::params![format!("req-s-{i}"), i],
            )?;
        }
        Ok(())
    }).unwrap();

    let repo = MessageFeedbackRepository::new(&db);
    repo.upsert(1, "positive", None).unwrap();

    let rates = repo.skill_satisfaction_rates().unwrap();
    assert!(rates.is_empty(), "Should not return skills with < 5 invocations");
}
