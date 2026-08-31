//! Repository for message feedback (thumbs up/down)

use crate::models::feedback::MessageFeedback;
use crate::Database;
use anyhow::{Context, Result};

/// Repository for message feedback operations.
pub struct MessageFeedbackRepository<'a> {
    db: &'a Database,
}

impl<'a> MessageFeedbackRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Upsert feedback for a message. Replaces existing feedback if present.
    pub fn upsert(
        &self,
        message_id: i64,
        feedback: &str,
        comment: Option<&str>,
    ) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO message_feedback (message_id, feedback, comment)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(message_id) DO UPDATE SET
                     feedback = excluded.feedback,
                     comment = excluded.comment,
                     updated_at = datetime('now')",
                rusqlite::params![message_id, feedback, comment],
            )
            .context("Failed to upsert message feedback")?;
            Ok(())
        })
    }

    /// Get feedback for a specific message.
    pub fn get_by_message(&self, message_id: i64) -> Result<Option<MessageFeedback>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, message_id, feedback, comment, created_at, updated_at
                 FROM message_feedback WHERE message_id = ?",
            )?;
            let mut rows = stmt.query_map(rusqlite::params![message_id], |row| {
                Ok(MessageFeedback {
                    id: row.get(0)?,
                    message_id: row.get(1)?,
                    feedback: row.get(2)?,
                    comment: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })?;
            match rows.next() {
                Some(row) => Ok(Some(row?)),
                None => Ok(None),
            }
        })
    }

    /// Delete feedback for a message. Returns true if a row was deleted.
    pub fn delete(&self, message_id: i64) -> Result<bool> {
        self.db.with_connection(|conn| {
            let changed = conn.execute(
                "DELETE FROM message_feedback WHERE message_id = ?",
                rusqlite::params![message_id],
            )?;
            Ok(changed > 0)
        })
    }

}

#[cfg(test)]
mod tests;
