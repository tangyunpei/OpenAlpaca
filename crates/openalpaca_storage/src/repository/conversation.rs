//! ConversationRepository - Chat message persistence

use crate::Database;
use crate::models::conversation::ConversationMessage;
use anyhow::Result;

/// Repository for conversation message CRUD operations
pub struct ConversationRepository<'a> {
    db: &'a Database,
}

impl<'a> ConversationRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Insert a conversation message and return its ID.
    pub fn insert(&self, msg: &ConversationMessage) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO conversation_messages (lane_key, role, content, model, tokens_in, tokens_out, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    &msg.lane_key,
                    &msg.role,
                    &msg.content,
                    &msg.model,
                    msg.tokens_in,
                    msg.tokens_out,
                    msg.duration_ms,
                ),
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// List messages for a lane, ordered by creation time ascending.
    pub fn list_by_lane(&self, lane_key: &str, limit: i64, offset: i64) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, lane_key, role, content, model, tokens_in, tokens_out, duration_ms, created_at
                 FROM conversation_messages
                 WHERE lane_key = ?1
                 ORDER BY created_at ASC
                 LIMIT ?2 OFFSET ?3",
            )?;

            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![lane_key, limit, offset])?;

            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
            }

            Ok(messages)
        })
    }

    /// Delete all messages for a lane. Returns the number of deleted rows.
    pub fn delete_by_lane(&self, lane_key: &str) -> Result<u64> {
        self.db.with_connection(|conn| {
            let count = conn.execute(
                "DELETE FROM conversation_messages WHERE lane_key = ?1",
                [lane_key],
            )?;
            Ok(count as u64)
        })
    }

    /// Count messages for a lane.
    pub fn count_by_lane(&self, lane_key: &str) -> Result<i64> {
        self.db.with_connection(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE lane_key = ?1",
                [lane_key],
                |row| row.get(0),
            )?;
            Ok(count)
        })
    }

    fn row_to_message(row: &rusqlite::Row<'_>) -> Result<ConversationMessage> {
        Ok(ConversationMessage {
            id: row.get(0)?,
            lane_key: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            model: row.get(4)?,
            tokens_in: row.get(5)?,
            tokens_out: row.get(6)?,
            duration_ms: row.get(7)?,
            created_at: row.get(8)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_insert_and_list() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        let msg = ConversationMessage {
            id: 0,
            lane_key: "user:gui".to_string(),
            role: "user".to_string(),
            content: "Hello world".to_string(),
            model: None,
            tokens_in: None,
            tokens_out: None,
            duration_ms: None,
            created_at: String::new(),
        };

        let id = repo.insert(&msg).unwrap();
        assert!(id > 0);

        let messages = repo.list_by_lane("user:gui", 50, 0).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "Hello world");
    }

    #[test]
    fn test_count_and_delete() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        for i in 0..3 {
            repo.insert(&ConversationMessage {
                id: 0,
                lane_key: "user:gui".to_string(),
                role: "user".to_string(),
                content: format!("Message {i}"),
                model: None,
                tokens_in: None,
                tokens_out: None,
                duration_ms: None,
                created_at: String::new(),
            })
            .unwrap();
        }

        assert_eq!(repo.count_by_lane("user:gui").unwrap(), 3);
        assert_eq!(repo.count_by_lane("other:lane").unwrap(), 0);

        let deleted = repo.delete_by_lane("user:gui").unwrap();
        assert_eq!(deleted, 3);
        assert_eq!(repo.count_by_lane("user:gui").unwrap(), 0);
    }

    #[test]
    fn test_list_with_offset() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        for i in 0..5 {
            repo.insert(&ConversationMessage {
                id: 0,
                lane_key: "user:gui".to_string(),
                role: "user".to_string(),
                content: format!("Message {i}"),
                model: None,
                tokens_in: None,
                tokens_out: None,
                duration_ms: None,
                created_at: String::new(),
            })
            .unwrap();
        }

        let page = repo.list_by_lane("user:gui", 2, 2).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].content, "Message 2");
        assert_eq!(page[1].content, "Message 3");
    }

    #[test]
    fn test_insert_with_metadata() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        let msg = ConversationMessage {
            id: 0,
            lane_key: "user:gui".to_string(),
            role: "assistant".to_string(),
            content: "Response text".to_string(),
            model: Some("claude-3".to_string()),
            tokens_in: Some(100),
            tokens_out: Some(200),
            duration_ms: Some(1500),
            created_at: String::new(),
        };

        repo.insert(&msg).unwrap();

        let messages = repo.list_by_lane("user:gui", 50, 0).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].model.as_deref(), Some("claude-3"));
        assert_eq!(messages[0].tokens_in, Some(100));
        assert_eq!(messages[0].tokens_out, Some(200));
        assert_eq!(messages[0].duration_ms, Some(1500));
    }
}
