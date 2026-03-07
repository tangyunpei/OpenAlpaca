//! Repository for message feedback (thumbs up/down)

use crate::models::feedback::{MessageFeedback, SkillSatisfaction};
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

    /// Get satisfaction rates per skill by joining feedback with skill execution log.
    pub fn skill_satisfaction_rates(&self) -> Result<Vec<SkillSatisfaction>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT
                    sel.skill_id,
                    COUNT(mf.id) AS fb_count,
                    CAST(SUM(CASE WHEN mf.feedback = 'positive' THEN 1 ELSE 0 END) AS REAL)
                        / NULLIF(COUNT(mf.id), 0) AS satisfaction,
                    COUNT(sel.id) AS total_invocations,
                    CAST(COUNT(mf.id) AS REAL) / COUNT(sel.id) AS coverage
                FROM skill_execution_log sel
                LEFT JOIN message_feedback mf ON mf.message_id = sel.response_message_id
                GROUP BY sel.skill_id
                HAVING COUNT(sel.id) >= 5",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(SkillSatisfaction {
                    skill_id: row.get(0)?,
                    feedback_count: row.get::<_, i64>(1)? as u64,
                    user_satisfaction_rate: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    total_invocations: row.get::<_, i64>(3)? as u64,
                    feedback_coverage: row.get(4)?,
                })
            })?;
            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok(results)
        })
    }
}

#[cfg(test)]
mod tests;
