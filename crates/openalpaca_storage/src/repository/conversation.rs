//! ConversationRepository - Chat message persistence

use crate::Database;
use crate::models::conversation::{Conversation, ConversationMessage};
use anyhow::Result;
use rusqlite::OptionalExtension;

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
                "INSERT INTO conversation_messages (lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    &msg.lane_key,
                    &msg.role,
                    &msg.content,
                    &msg.source,
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
                "SELECT id, lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, created_at
                 FROM conversation_messages
                 WHERE lane_key = ?1
                 ORDER BY created_at ASC, id ASC
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

    /// List the most recent N messages for a lane, in chronological order.
    pub fn list_recent_by_lane(&self, lane_key: &str, limit: i64) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, created_at
                 FROM (
                     SELECT * FROM conversation_messages
                     WHERE lane_key = ?1
                     ORDER BY created_at DESC, id DESC
                     LIMIT ?2
                 )
                 ORDER BY created_at ASC, id ASC",
            )?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![lane_key, limit])?;
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

    // ========== Conversation Master ==========

    /// Get or create a conversation master record for the given lane_key.
    pub fn get_or_create_conversation(&self, lane_key: &str, source: &str) -> Result<Conversation> {
        // Try to find existing
        if let Some(conv) = self.get_conversation_by_lane(lane_key)? {
            return Ok(conv);
        }

        // Create new
        let id = uuid::Uuid::new_v4().to_string();
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO conversations (id, lane_key, source, title, message_count)
                 VALUES (?1, ?2, ?3, '', 0)",
                rusqlite::params![id, lane_key, source],
            )?;
            Ok(Conversation {
                id,
                lane_key: lane_key.to_string(),
                source: source.to_string(),
                title: String::new(),
                message_count: 0,
                last_message_at: None,
                created_at: String::new(),
                updated_at: String::new(),
                summary: String::new(),
                summary_version: 0,
                last_summarized_message_id: 0,
                summary_updated_at: None,
            })
        })
    }

    /// Get a conversation by lane_key.
    pub fn get_conversation_by_lane(&self, lane_key: &str) -> Result<Option<Conversation>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, lane_key, source, title, message_count, last_message_at, created_at, updated_at, summary, summary_version, last_summarized_message_id, summary_updated_at
                 FROM conversations WHERE lane_key = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![lane_key])?;
            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_conversation(row)?)),
                None => Ok(None),
            }
        })
    }

    /// Get a conversation by ID.
    pub fn get_conversation(&self, id: &str) -> Result<Option<Conversation>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, lane_key, source, title, message_count, last_message_at, created_at, updated_at, summary, summary_version, last_summarized_message_id, summary_updated_at
                 FROM conversations WHERE id = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![id])?;
            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_conversation(row)?)),
                None => Ok(None),
            }
        })
    }

    /// List conversations with optional source filter.
    pub fn list_conversations(
        &self,
        source_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Conversation>> {
        self.db.with_connection(|conn| {
            let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match source_filter {
                Some(source) => (
                    "SELECT id, lane_key, source, title, message_count, last_message_at, created_at, updated_at, summary, summary_version, last_summarized_message_id, summary_updated_at
                     FROM conversations WHERE source = ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3".to_string(),
                    vec![Box::new(source.to_string()), Box::new(limit), Box::new(offset)],
                ),
                None => (
                    "SELECT id, lane_key, source, title, message_count, last_message_at, created_at, updated_at, summary, summary_version, last_summarized_message_id, summary_updated_at
                     FROM conversations ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2".to_string(),
                    vec![Box::new(limit), Box::new(offset)],
                ),
            };

            let mut stmt = conn.prepare(&sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut rows = stmt.query(params_refs.as_slice())?;
            let mut conversations = Vec::new();
            while let Some(row) = rows.next()? {
                conversations.push(Self::row_to_conversation(row)?);
            }
            Ok(conversations)
        })
    }

    /// Increment the message count and update last_message_at for a conversation.
    pub fn increment_message_count(&self, lane_key: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE conversations
                 SET message_count = message_count + 1,
                     last_message_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE lane_key = ?1",
                [lane_key],
            )?;
            Ok(())
        })
    }

    /// Get conversation summary data for a lane.
    /// Returns (summary, summary_version, last_summarized_message_id).
    pub fn get_summary(&self, lane_key: &str) -> Result<(String, i64, i64)> {
        self.db.with_connection(|conn| {
            let result = conn.query_row(
                "SELECT summary, summary_version, last_summarized_message_id FROM conversations WHERE lane_key = ?1",
                [lane_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            ).optional()?;
            Ok(result.unwrap_or_else(|| (String::new(), 0, 0)))
        })
    }

    /// Update the summary with optimistic locking. Returns true if the update succeeded.
    pub fn update_summary_optimistic(
        &self,
        lane_key: &str,
        expected_version: i64,
        summary: &str,
        last_id: i64,
    ) -> Result<bool> {
        self.db.with_connection(|conn| {
            let rows = conn.execute(
                "UPDATE conversations SET summary = ?1, summary_version = summary_version + 1,
                 last_summarized_message_id = ?2, summary_updated_at = datetime('now'),
                 updated_at = datetime('now')
                 WHERE lane_key = ?3 AND summary_version = ?4",
                rusqlite::params![summary, last_id, lane_key, expected_version],
            )?;
            Ok(rows > 0)
        })
    }

    /// Clear the summary and reset counters for a lane.
    pub fn clear_summary(&self, lane_key: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE conversations SET summary = '', summary_version = 0,
                 last_summarized_message_id = 0, summary_updated_at = datetime('now'),
                 message_count = 0, last_message_at = NULL, updated_at = datetime('now')
                 WHERE lane_key = ?1",
                [lane_key],
            )?;
            Ok(())
        })
    }

    /// List messages in an ID range for a lane (exclusive bounds), ordered by id ASC.
    pub fn list_by_lane_id_range(
        &self,
        lane_key: &str,
        after_id: i64,
        before_id: i64,
        limit: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, created_at
                 FROM conversation_messages
                 WHERE lane_key = ?1 AND id > ?2 AND id < ?3
                 ORDER BY id ASC
                 LIMIT ?4",
            )?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![lane_key, after_id, before_id, limit])?;
            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
            }
            Ok(messages)
        })
    }

    fn row_to_conversation(row: &rusqlite::Row<'_>) -> Result<Conversation> {
        Ok(Conversation {
            id: row.get(0)?,
            lane_key: row.get(1)?,
            source: row.get(2)?,
            title: row.get(3)?,
            message_count: row.get(4)?,
            last_message_at: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            summary: row.get(8)?,
            summary_version: row.get(9)?,
            last_summarized_message_id: row.get(10)?,
            summary_updated_at: row.get(11)?,
        })
    }

    fn row_to_message(row: &rusqlite::Row<'_>) -> Result<ConversationMessage> {
        Ok(ConversationMessage {
            id: row.get(0)?,
            lane_key: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            source: row.get(4)?,
            model: row.get(5)?,
            tokens_in: row.get(6)?,
            tokens_out: row.get(7)?,
            duration_ms: row.get(8)?,
            created_at: row.get(9)?,
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
            source: None,
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
                source: None,
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
                source: None,
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
            source: None,
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

    #[test]
    fn test_list_recent_by_lane() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        for i in 0..10 {
            repo.insert(&ConversationMessage {
                id: 0,
                lane_key: "user:gui".to_string(),
                role: "user".to_string(),
                content: format!("Message {i}"),
                source: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                duration_ms: None,
                created_at: String::new(),
            })
            .unwrap();
        }

        // Should return last 3 messages in chronological order
        let recent = repo.list_recent_by_lane("user:gui", 3).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "Message 7");
        assert_eq!(recent[1].content, "Message 8");
        assert_eq!(recent[2].content, "Message 9");
    }

    #[test]
    fn test_get_or_create_conversation() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        let conv = repo.get_or_create_conversation("user1:telegram", "telegram").unwrap();
        assert_eq!(conv.lane_key, "user1:telegram");
        assert_eq!(conv.source, "telegram");
        assert_eq!(conv.message_count, 0);

        // Second call should return the same conversation
        let conv2 = repo.get_or_create_conversation("user1:telegram", "telegram").unwrap();
        assert_eq!(conv.id, conv2.id);
    }

    #[test]
    fn test_list_conversations() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        repo.get_or_create_conversation("user1:gui", "gui").unwrap();
        repo.get_or_create_conversation("user2:telegram", "telegram").unwrap();
        repo.get_or_create_conversation("user3:telegram", "telegram").unwrap();

        // List all
        let all = repo.list_conversations(None, 50, 0).unwrap();
        assert_eq!(all.len(), 3);

        // Filter by source
        let tg = repo.list_conversations(Some("telegram"), 50, 0).unwrap();
        assert_eq!(tg.len(), 2);

        let gui = repo.list_conversations(Some("gui"), 50, 0).unwrap();
        assert_eq!(gui.len(), 1);
    }

    #[test]
    fn test_increment_message_count() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        repo.get_or_create_conversation("user1:gui", "gui").unwrap();
        repo.increment_message_count("user1:gui").unwrap();
        repo.increment_message_count("user1:gui").unwrap();

        let conv = repo.get_conversation_by_lane("user1:gui").unwrap().unwrap();
        assert_eq!(conv.message_count, 2);
        assert!(conv.last_message_at.is_some());
    }

    #[test]
    fn test_list_recent_fewer_than_limit() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        for i in 0..2 {
            repo.insert(&ConversationMessage {
                id: 0,
                lane_key: "user:gui".to_string(),
                role: "user".to_string(),
                content: format!("Message {i}"),
                source: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                duration_ms: None,
                created_at: String::new(),
            })
            .unwrap();
        }

        // Limit is higher than total — should return all messages
        let recent = repo.list_recent_by_lane("user:gui", 50).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].content, "Message 0");
        assert_eq!(recent[1].content, "Message 1");
    }

    #[test]
    fn test_get_summary_default() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        repo.get_or_create_conversation("user1:gui", "gui").unwrap();
        let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
        assert_eq!(summary, "");
        assert_eq!(version, 0);
        assert_eq!(last_id, 0);
    }

    #[test]
    fn test_get_summary_no_row() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        // No conversation exists — should return defaults
        let (summary, version, last_id) = repo.get_summary("nonexistent:lane").unwrap();
        assert_eq!(summary, "");
        assert_eq!(version, 0);
        assert_eq!(last_id, 0);
    }

    #[test]
    fn test_update_summary_optimistic_success() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        repo.get_or_create_conversation("user1:gui", "gui").unwrap();

        // Update with correct version (0)
        let ok = repo.update_summary_optimistic("user1:gui", 0, "Test summary", 42).unwrap();
        assert!(ok);

        let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
        assert_eq!(summary, "Test summary");
        assert_eq!(version, 1);
        assert_eq!(last_id, 42);
    }

    #[test]
    fn test_update_summary_optimistic_conflict() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        repo.get_or_create_conversation("user1:gui", "gui").unwrap();

        // First update succeeds
        assert!(repo.update_summary_optimistic("user1:gui", 0, "Summary v1", 10).unwrap());

        // Second update with stale version (0) fails
        let ok = repo.update_summary_optimistic("user1:gui", 0, "Summary v2", 20).unwrap();
        assert!(!ok);

        // Original update preserved
        let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
        assert_eq!(summary, "Summary v1");
        assert_eq!(version, 1);
        assert_eq!(last_id, 10);
    }

    #[test]
    fn test_clear_summary() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        repo.get_or_create_conversation("user1:gui", "gui").unwrap();
        repo.update_summary_optimistic("user1:gui", 0, "Some summary", 50).unwrap();
        repo.increment_message_count("user1:gui").unwrap();

        repo.clear_summary("user1:gui").unwrap();

        let (summary, version, last_id) = repo.get_summary("user1:gui").unwrap();
        assert_eq!(summary, "");
        assert_eq!(version, 0);
        assert_eq!(last_id, 0);

        let conv = repo.get_conversation_by_lane("user1:gui").unwrap().unwrap();
        assert_eq!(conv.message_count, 0);
        assert!(conv.last_message_at.is_none());
    }

    #[test]
    fn test_list_by_lane_id_range() {
        let db = test_db();
        let repo = ConversationRepository::new(&db);

        let mut ids = Vec::new();
        for i in 0..10 {
            let id = repo.insert(&ConversationMessage {
                id: 0,
                lane_key: "user:gui".to_string(),
                role: "user".to_string(),
                content: format!("Message {i}"),
                source: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                duration_ms: None,
                created_at: String::new(),
            }).unwrap();
            ids.push(id);
        }

        // Query range: after id[2] and before id[7] → should get ids 3,4,5,6
        let msgs = repo.list_by_lane_id_range("user:gui", ids[2], ids[7], 100).unwrap();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].content, "Message 3");
        assert_eq!(msgs[1].content, "Message 4");
        assert_eq!(msgs[2].content, "Message 5");
        assert_eq!(msgs[3].content, "Message 6");

        // With limit
        let msgs = repo.list_by_lane_id_range("user:gui", ids[2], ids[7], 2).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "Message 3");
        assert_eq!(msgs[1].content, "Message 4");

        // Empty range
        let msgs = repo.list_by_lane_id_range("user:gui", ids[5], ids[5], 100).unwrap();
        assert_eq!(msgs.len(), 0);
    }
}
