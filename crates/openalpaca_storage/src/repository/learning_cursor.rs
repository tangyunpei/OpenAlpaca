//! The per-session learning cursor (Learning v2, design 09 §1.3, ruling LR4).
//!
//! `learning_cursor.extracted_through` is the id of the last message of a
//! session that background learning has read. Everything after it is the
//! session's unlearned window. The table is a sidecar rather than a column on
//! `session`, so the session projection and its wire stay as they are, and
//! `attempts`/`last_error` survive a restart for the bounded retry (§1.4).
//!
//! A session with no row reads as `extracted_through = 0`: every session
//! created after migration 044 holds only post-044 messages, and 044 seeded
//! every older session at its newest message. Message ids are `AUTOINCREMENT`
//! and never reused, so a cursor above rows a clear deleted still reads
//! correctly. `delete_session` deletes the row with the session.

use crate::Database;
use anyhow::Result;
use rusqlite::OptionalExtension;

/// A session's cursor as stored, or the zero cursor when it has no row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LearningCursor {
    /// The last message id learning has read; `0` = nothing yet.
    pub extracted_through: i64,
    /// Consecutive failed attempts of the chunk after the cursor — a
    /// reachable model's bad output only (§1.4, ruling LR20).
    pub attempts: u32,
    /// The last such failure, for the WARN that skips the chunk.
    pub last_error: Option<String>,
}

pub struct LearningCursorRepository<'a> {
    db: &'a Database,
}

impl<'a> LearningCursorRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// The session's cursor; the zero cursor when it has no row.
    pub fn get(&self, session_id: &str) -> Result<LearningCursor> {
        self.db.with_connection(|conn| {
            let cursor = conn
                .query_row(
                    "SELECT extracted_through, attempts, last_error
                       FROM learning_cursor WHERE session_id = ?1",
                    [session_id],
                    |row| {
                        Ok(LearningCursor {
                            extracted_through: row.get(0)?,
                            attempts: row.get::<_, i64>(1)?.max(0) as u32,
                            last_error: row.get(2)?,
                        })
                    },
                )
                .optional()?;
            Ok(cursor.unwrap_or_default())
        })
    }

    /// Moves the cursor to `through` **only** when it still reads `expected`
    /// (compare-and-swap, design §1.4 step 4), and clears the attempt count.
    /// A session that no longer exists gets no row. `true` when it moved.
    pub fn advance_cas(&self, session_id: &str, through: i64, expected: i64) -> Result<bool> {
        self.db.with_connection(|conn| {
            let changed = conn.execute(
                "INSERT INTO learning_cursor(session_id, extracted_through, attempts, last_error, updated_at)
                 SELECT ?1, ?2, 0, NULL, datetime('now') WHERE EXISTS (SELECT 1 FROM session WHERE id = ?1)
                 ON CONFLICT(session_id) DO UPDATE SET extracted_through = excluded.extracted_through,
                     attempts = 0, last_error = NULL, updated_at = excluded.updated_at
                 WHERE learning_cursor.extracted_through = ?3",
                rusqlite::params![session_id, through, expected],
            )?;
            Ok(changed > 0)
        })
    }

    /// Counts one failed attempt of the chunk after `expected`, when the
    /// cursor still reads `expected`. The new attempt count, or `None` when
    /// the cursor moved or the session is gone.
    pub fn record_failure(
        &self,
        session_id: &str,
        expected: i64,
        error: &str,
    ) -> Result<Option<u32>> {
        self.db.with_connection(|conn| {
            let changed = conn.execute(
                "INSERT INTO learning_cursor(session_id, extracted_through, attempts, last_error, updated_at)
                 SELECT ?1, ?2, 1, ?3, datetime('now') WHERE EXISTS (SELECT 1 FROM session WHERE id = ?1)
                 ON CONFLICT(session_id) DO UPDATE SET attempts = learning_cursor.attempts + 1,
                     last_error = excluded.last_error, updated_at = excluded.updated_at
                 WHERE learning_cursor.extracted_through = ?2",
                rusqlite::params![session_id, expected, error],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            let attempts: i64 = conn.query_row(
                "SELECT attempts FROM learning_cursor WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )?;
            Ok(Some(attempts.max(0) as u32))
        })
    }

    /// Every session with a user-authored row past its cursor — the boot
    /// catch-up's one query (design §1.5): a user row a turn wrote
    /// (`request_id` set) that is not the assistant's own follow-up text
    /// (`source <> 'internal'`).
    pub fn sessions_with_unlearned_turns(&self) -> Result<Vec<String>> {
        self.db.with_connection(|conn| {
            let ids = conn
                .prepare(
                    "SELECT m.session_id FROM conversation_messages m
                     LEFT JOIN learning_cursor c ON c.session_id = m.session_id
                     WHERE m.session_id IS NOT NULL AND m.role = 'user'
                       AND m.request_id IS NOT NULL AND m.source <> 'internal'
                       AND m.id > COALESCE(c.extracted_through, 0)
                     GROUP BY m.session_id
                     ORDER BY MIN(m.id)",
                )?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_db;

    fn session(db: &Database, id: &str) {
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO session (id, lane_key, source) VALUES (?1, ?2, 'gui')",
                (id, format!("u-{id}:gui")),
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn message(
        db: &Database,
        session: &str,
        role: &str,
        source: &str,
        request_id: Option<&str>,
    ) -> i64 {
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO conversation_messages (lane_key, role, content, source, session_id, request_id)
                 VALUES ('l:gui', ?1, 'text', ?2, ?3, ?4)",
                (role, source, session, request_id),
            )?;
            Ok(c.last_insert_rowid())
        })
        .unwrap()
    }

    /// Design §1.4 step 4: the advance is a compare-and-swap. A writer that
    /// read a stale cursor moves nothing, so two flushes of one session can
    /// never skip a chunk between them.
    #[test]
    fn the_cursor_advances_only_from_the_expected_value() {
        let db = test_db();
        let repo = LearningCursorRepository::new(&db);
        session(&db, "s1");
        assert_eq!(repo.get("s1").unwrap().extracted_through, 0);

        assert!(
            repo.advance_cas("s1", 5, 0).unwrap(),
            "0 → 5 from the zero cursor"
        );
        assert_eq!(repo.get("s1").unwrap().extracted_through, 5);

        assert!(
            !repo.advance_cas("s1", 9, 0).unwrap(),
            "a stale expected value moves nothing"
        );
        assert_eq!(repo.get("s1").unwrap().extracted_through, 5);

        assert!(repo.advance_cas("s1", 9, 5).unwrap());
        assert_eq!(repo.get("s1").unwrap().extracted_through, 9);
    }

    /// A deleted session (LQ4) or one that never existed gets no cursor row.
    #[test]
    fn a_cursor_is_never_created_for_a_missing_session() {
        let db = test_db();
        let repo = LearningCursorRepository::new(&db);
        assert!(!repo.advance_cas("gone", 3, 0).unwrap());
        assert_eq!(repo.record_failure("gone", 0, "bad").unwrap(), None);
        let rows: i64 = db
            .with_connection(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM learning_cursor", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(rows, 0);
    }

    /// Ruling LR20: a failed attempt is counted against the chunk after the
    /// cursor, and an advance clears the count and the error.
    #[test]
    fn record_failure_counts_attempts_and_an_advance_clears_them() {
        let db = test_db();
        let repo = LearningCursorRepository::new(&db);
        session(&db, "s1");
        assert_eq!(repo.record_failure("s1", 0, "not json").unwrap(), Some(1));
        assert_eq!(repo.record_failure("s1", 0, "still not").unwrap(), Some(2));
        let cursor = repo.get("s1").unwrap();
        assert_eq!(cursor.attempts, 2);
        assert_eq!(cursor.last_error.as_deref(), Some("still not"));
        assert_eq!(
            repo.record_failure("s1", 7, "stale").unwrap(),
            None,
            "a failure against a cursor that moved counts nothing"
        );

        assert!(repo.advance_cas("s1", 4, 0).unwrap());
        let cursor = repo.get("s1").unwrap();
        assert_eq!(
            cursor,
            LearningCursor {
                extracted_through: 4,
                attempts: 0,
                last_error: None
            }
        );
    }

    /// The boot catch-up's query (design §1.5): a session is listed when a
    /// user row a turn wrote, not the assistant's own follow-up, lies past
    /// its cursor — and only then.
    #[test]
    fn sessions_with_unlearned_turns_lists_only_sessions_past_their_cursor() {
        let db = test_db();
        let repo = LearningCursorRepository::new(&db);
        for id in [
            "fresh",
            "learned",
            "replies_only",
            "followup_only",
            "no_turn",
        ] {
            session(&db, id);
        }
        message(&db, "fresh", "user", "gui", Some("r1"));
        let learned = message(&db, "learned", "user", "gui", Some("r2"));
        assert!(repo.advance_cas("learned", learned, 0).unwrap());
        message(&db, "replies_only", "assistant", "gui", Some("r3"));
        message(&db, "followup_only", "user", "internal", Some("r4"));
        message(&db, "no_turn", "user", "gui", None);

        assert_eq!(
            repo.sessions_with_unlearned_turns().unwrap(),
            vec!["fresh".to_string()]
        );
    }

    #[test]
    fn a_missing_row_reads_as_the_zero_cursor() {
        let db = test_db();
        let repo = LearningCursorRepository::new(&db);
        session(&db, "s1");
        assert_eq!(repo.get("s1").unwrap(), LearningCursor::default());
        let found: Option<i64> = db
            .with_connection(|c| {
                Ok(c.query_row(
                    "SELECT extracted_through FROM learning_cursor WHERE session_id = 's1'",
                    [],
                    |r| r.get(0),
                )
                .optional()?)
            })
            .unwrap();
        assert_eq!(found, None, "reading creates nothing");
    }
}
