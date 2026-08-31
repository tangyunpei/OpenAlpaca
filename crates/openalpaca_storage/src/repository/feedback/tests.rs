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

