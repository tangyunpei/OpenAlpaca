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

    /// Insert a conversation message with structured content (multimodal).
    pub fn insert_with_structured(
        &self,
        msg: &ConversationMessage,
        content_json: &str,
        display_text: &str,
    ) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO conversation_messages (lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, content_json, display_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                (
                    &msg.lane_key,
                    &msg.role,
                    &msg.content,
                    &msg.source,
                    &msg.model,
                    msg.tokens_in,
                    msg.tokens_out,
                    msg.duration_ms,
                    content_json,
                    display_text,
                ),
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// List messages for a lane, ordered by creation time ascending.
    pub fn list_by_lane(
        &self,
        lane_key: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, created_at, content_json, display_text
                 FROM conversation_messages
                 WHERE lane_key = ?1
                 ORDER BY created_at ASC, id ASC
                 LIMIT ?2 OFFSET ?3",
            )?;

            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![lane_key, limit, offset])?;

            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message_extended(row)?);
            }

            Ok(messages)
        })
    }

    /// List the most recent N messages for a lane, in chronological order.
    pub fn list_recent_by_lane(
        &self,
        lane_key: &str,
        limit: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, created_at, content_json, display_text
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
                messages.push(Self::row_to_message_extended(row)?);
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

        // Create new. Two callers can race here (chat reply + task-completion
        // persist) — `conversations.lane_key` is UNIQUE, so use ON CONFLICT and
        // then re-read the canonical row (ours, or the concurrent winner's)
        // instead of failing the loser with a UNIQUE-constraint error.
        let id = uuid::Uuid::new_v4().to_string();
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO conversations (id, lane_key, source, title, message_count)
                 VALUES (?1, ?2, ?3, '', 0)
                 ON CONFLICT(lane_key) DO NOTHING",
                rusqlite::params![id, lane_key, source],
            )?;
            Ok(())
        })?;

        self.get_conversation_by_lane(lane_key)?.ok_or_else(|| {
            anyhow::anyhow!("conversation row missing after upsert for lane {lane_key}")
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

    /// List conversations for a specific owner (lane_key starts with "{owner_id}:").
    pub fn list_conversations_for_owner(
        &self,
        owner_id: &str,
        source_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Conversation>> {
        self.db.with_connection(|conn| {
            let lane_pattern = format!("{}:%", owner_id);
            let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match source_filter {
                Some(source) => (
                    "SELECT id, lane_key, source, title, message_count, last_message_at, created_at, updated_at, summary, summary_version, last_summarized_message_id, summary_updated_at
                     FROM conversations WHERE lane_key LIKE ?1 AND source = ?2 ORDER BY updated_at DESC LIMIT ?3 OFFSET ?4".to_string(),
                    vec![Box::new(lane_pattern), Box::new(source.to_string()), Box::new(limit), Box::new(offset)],
                ),
                None => (
                    "SELECT id, lane_key, source, title, message_count, last_message_at, created_at, updated_at, summary, summary_version, last_summarized_message_id, summary_updated_at
                     FROM conversations WHERE lane_key LIKE ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3".to_string(),
                    vec![Box::new(lane_pattern), Box::new(limit), Box::new(offset)],
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
            content_json: None,
            display_text: None,
        })
    }

    fn row_to_message_extended(row: &rusqlite::Row<'_>) -> Result<ConversationMessage> {
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
            content_json: row.get(10)?,
            display_text: row.get(11)?,
        })
    }
}

#[cfg(test)]
mod tests;
