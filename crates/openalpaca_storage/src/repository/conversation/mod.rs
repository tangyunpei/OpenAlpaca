//! ConversationRepository - Chat message persistence
//!
//! Migration 039 rebuilt `conversations` as `session`: the same transcript
//! container, minus 011's column-level `UNIQUE(lane_key)`, plus a workspace
//! binding and an `active`/`archived` lifecycle. **A lane now has many
//! sessions and at most one active one** — a partial unique index
//! (`idx_session_active_lane`) makes that a database invariant rather than a
//! convention this file has to remember.
//!
//! The Rust names did not move with the table (`Conversation`,
//! `ConversationRepository`, `ConversationMessage`): renaming them is churn
//! with no behaviour change. Read "conversation" as "session" throughout.

use crate::Database;
use crate::models::conversation::{Conversation, ConversationMessage};
use anyhow::Result;
use rusqlite::OptionalExtension;

/// Every column [`ConversationRepository::row_to_session`] reads, in order.
/// Written once so a column cannot reach one `SELECT` and miss another.
const SESSION_COLUMNS: &str = "id, lane_key, source, title, message_count, last_message_at, \
     created_at, updated_at, summary, summary_version, last_summarized_message_id, \
     summary_updated_at, workspace_id, status, ended_at";

/// Every column [`ConversationRepository::row_to_message`] reads, in order.
///
/// One projection, not the two that used to exist: a narrow read that omits a
/// column silently answers `None` for rows that have one (migration 038's
/// `task_id` was exactly that bug), and the older-context window that motivated
/// the narrow form discards everything but `(id, role, content)` anyway.
const MESSAGE_COLUMNS: &str = "id, lane_key, role, content, source, model, tokens_in, \
     tokens_out, duration_ms, created_at, content_json, display_text, task_id, session_id";

/// The page [`ConversationRepository::list_sessions`] serves when the caller
/// asks for a non-positive one. `LIMIT -1` means *no limit* in SQLite, so an
/// unclamped `?limit=-1` returned the whole table — and every id of it then fed
/// the unchunked `IN (…)` lists of `task_counts_by_session` and the artifact
/// link lookup.
const SESSION_PAGE_DEFAULT: i64 = 100;
/// The largest page it will serve, whatever the caller asks for. Chosen to match
/// `TITLES_FOR_CHUNK`, so one page's ids always fit in one statement's bound
/// variables.
const SESSION_PAGE_MAX: i64 = 500;

/// A session is live. Its lane may have exactly one of these.
pub const SESSION_ACTIVE: &str = "active";
/// A session is closed: fully readable, and re-activatable.
pub const SESSION_ARCHIVED: &str = "archived";

/// Filters for [`ConversationRepository::list_sessions`] (`GET /v1/sessions`).
#[derive(Debug, Clone, Default)]
pub struct SessionFilter<'a> {
    pub workspace_id: Option<&'a str>,
    pub source: Option<&'a str>,
    pub status: Option<&'a str>,
    /// Case-insensitive substring over title and lane key. Matched literally:
    /// `%` and `_` are the user's characters, not wildcards (see
    /// [`escape_like`]).
    pub q: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
}

/// Escapes a user-supplied `LIKE` needle so `%`, `_` and the escape character
/// itself match literally. Paired with `ESCAPE '\'` on every pattern built from
/// it — without both halves a search for `100%` matches every row.
fn escape_like(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len());
    for ch in needle.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Repository for conversation message CRUD operations
pub struct ConversationRepository<'a> {
    db: &'a Database,
}

impl<'a> ConversationRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Insert a conversation message and return its ID.
    ///
    /// The row is keyed by session as well as by lane: when the caller did not
    /// name one, the lane's active session is resolved here, in the same
    /// connection, so that no writer can leave a message unattached to the
    /// conversation it belongs to. A lane with no session row at all (nothing
    /// has been persisted through the gateway yet) stores `NULL`, exactly as
    /// every pre-039 row that predates the column.
    pub fn insert(&self, msg: &ConversationMessage) -> Result<i64> {
        self.db.with_connection(|conn| {
            let session_id = Self::resolve_session_id(conn, msg)?;
            conn.execute(
                "INSERT INTO conversation_messages (lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, task_id, session_id)
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
                    &msg.task_id,
                    &session_id,
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
            let session_id = Self::resolve_session_id(conn, msg)?;
            conn.execute(
                "INSERT INTO conversation_messages (lane_key, role, content, source, model, tokens_in, tokens_out, duration_ms, content_json, display_text, task_id, session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                    &msg.task_id,
                    &session_id,
                ),
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// The session a message belongs to: the one it names, else the lane's
    /// active one, else nothing.
    fn resolve_session_id(
        conn: &rusqlite::Connection,
        msg: &ConversationMessage,
    ) -> rusqlite::Result<Option<String>> {
        if msg.session_id.is_some() {
            return Ok(msg.session_id.clone());
        }
        conn.query_row(
            "SELECT id FROM session WHERE lane_key = ?1 AND status = 'active'",
            [&msg.lane_key],
            |row| row.get::<_, String>(0),
        )
        .optional()
    }

    /// List messages for a lane, ordered by creation time ascending.
    ///
    /// Lane-wide: every session's messages, oldest first. The transcript reads
    /// [`list_by_session`](Self::list_by_session) instead — this stays for the
    /// callers that genuinely mean the whole lane (identity re-keying, and the
    /// notice lane, which has one session by construction).
    pub fn list_by_lane(
        &self,
        lane_key: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS}
                 FROM conversation_messages
                 WHERE lane_key = ?1
                 ORDER BY created_at ASC, id ASC
                 LIMIT ?2 OFFSET ?3",
            ))?;

            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![lane_key, limit, offset])?;

            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
            }

            Ok(messages)
        })
    }

    /// List messages for one session, ordered by creation time ascending.
    pub fn list_by_session(
        &self,
        session_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS}
                 FROM conversation_messages
                 WHERE session_id = ?1
                 ORDER BY created_at ASC, id ASC
                 LIMIT ?2 OFFSET ?3",
            ))?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![session_id, limit, offset])?;
            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
            }
            Ok(messages)
        })
    }

    /// The page of a session's transcript that ends just before `before_id`:
    /// the newest `limit` messages older than the cursor, chronological.
    ///
    /// This is what "load older" means — [`list_by_session_id_range`] returns
    /// the *oldest* rows in a range, which walks a long transcript from the
    /// wrong end.
    ///
    /// [`list_by_session_id_range`]: Self::list_by_session_id_range
    pub fn list_by_session_before(
        &self,
        session_id: &str,
        before_id: i64,
        limit: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS}
                 FROM (
                     SELECT * FROM conversation_messages
                     WHERE session_id = ?1 AND id < ?2
                     ORDER BY id DESC
                     LIMIT ?3
                 )
                 ORDER BY id ASC",
            ))?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![session_id, before_id, limit])?;
            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
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
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS}
                 FROM (
                     SELECT * FROM conversation_messages
                     WHERE lane_key = ?1
                     ORDER BY created_at DESC, id DESC
                     LIMIT ?2
                 )
                 ORDER BY created_at ASC, id ASC",
            ))?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![lane_key, limit])?;
            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
            }
            Ok(messages)
        })
    }

    /// List the most recent N messages of one session, in chronological order.
    ///
    /// This is what a turn's context window reads: a new session on a lane
    /// starts from an empty transcript rather than inheriting the previous
    /// conversation's tail.
    pub fn list_recent_by_session(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS}
                 FROM (
                     SELECT * FROM conversation_messages
                     WHERE session_id = ?1
                     ORDER BY created_at DESC, id DESC
                     LIMIT ?2
                 )
                 ORDER BY created_at ASC, id ASC",
            ))?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![session_id, limit])?;
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

    /// Delete every message of one session. Returns the number of deleted rows.
    pub fn delete_by_session(&self, session_id: &str) -> Result<u64> {
        self.db.with_connection(|conn| {
            let count = conn.execute(
                "DELETE FROM conversation_messages WHERE session_id = ?1",
                [session_id],
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

    /// Count messages in one session.
    pub fn count_by_session(&self, session_id: &str) -> Result<i64> {
        self.db.with_connection(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )?;
            Ok(count)
        })
    }

    // ========== Sessions ==========

    /// Get the lane's active session, creating one if the lane has none.
    ///
    /// This is 039's evolution of `get_or_create_conversation`, at the same
    /// call sites. Two things it deliberately does **not** do: it never
    /// archives anything (implicit creation continues a lane, it does not end
    /// a conversation — only an explicit [`create_session`](Self::create_session)
    /// or [`activate_session`](Self::activate_session) does that, §5.1), and it
    /// never re-binds a workspace. `workspace_path` binds only when the session
    /// has none yet: the first `x-workspace-path` the session sees wins, and
    /// changing project means a new session, not a re-pointed one.
    ///
    /// Opening that new session is the caller's half of the rule, not this
    /// method's: `GatewayPersistence::resolve_turn_session` creates one on the
    /// turn whose project differs from the binding, and `PATCH
    /// /v1/sessions/{id}` answers `409 SESSION_WORKSPACE_BOUND` rather than
    /// moving a binding that runs are already pinned to.
    pub fn get_or_create_active_session(
        &self,
        lane_key: &str,
        source: &str,
        workspace_path: Option<&str>,
    ) -> Result<Conversation> {
        if let Some(existing) = self.get_active_session_for_lane(lane_key)? {
            if existing.workspace_id.is_none()
                && let Some(path) = workspace_path
            {
                self.db.with_connection(|conn| {
                    conn.execute(
                        "UPDATE session SET workspace_id = ?1, updated_at = datetime('now')
                         WHERE id = ?2 AND workspace_id IS NULL",
                        rusqlite::params![path, existing.id],
                    )?;
                    Ok(())
                })?;
                return self
                    .get_session(&existing.id)?
                    .ok_or_else(|| anyhow::anyhow!("session {} vanished", existing.id));
            }
            return Ok(existing);
        }

        // Two callers can race here (chat reply + task-completion persist).
        // `idx_session_active_lane` is unique over the active rows, so use
        // ON CONFLICT DO NOTHING and then re-read the canonical row (ours, or
        // the concurrent winner's) instead of failing the loser.
        let id = uuid::Uuid::new_v4().to_string();
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO session (id, lane_key, source, title, workspace_id, status, message_count)
                 VALUES (?1, ?2, ?3, '', ?4, 'active', 0)
                 ON CONFLICT DO NOTHING",
                rusqlite::params![id, lane_key, source, workspace_path],
            )?;
            Ok(())
        })?;

        self.get_active_session_for_lane(lane_key)?
            .ok_or_else(|| anyhow::anyhow!("session row missing after upsert for lane {lane_key}"))
    }

    /// Create a **new** session on a lane, archiving whatever was active there.
    ///
    /// The explicit path — `POST /v1/sessions`, the GUI's "New chat". The
    /// archive and the insert share one transaction because the partial unique
    /// index would otherwise reject the insert: at most one active row per lane
    /// is enforced by the database, so the old one must step down first.
    pub fn create_session(
        &self,
        lane_key: &str,
        source: &str,
        workspace_path: Option<&str>,
        title: Option<&str>,
    ) -> Result<Conversation> {
        let id = uuid::Uuid::new_v4().to_string();
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE session SET status = 'archived', ended_at = datetime('now'),
                 updated_at = datetime('now') WHERE lane_key = ?1 AND status = 'active'",
                [lane_key],
            )?;
            tx.execute(
                "INSERT INTO session (id, lane_key, source, title, workspace_id, status, message_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'active', 0)",
                rusqlite::params![id, lane_key, source, title.unwrap_or(""), workspace_path],
            )?;
            tx.commit()?;
            Ok(())
        })?;
        self.get_session(&id)?
            .ok_or_else(|| anyhow::anyhow!("session {id} missing after create"))
    }

    /// Make a session the active one on its lane, archiving the incumbent.
    ///
    /// Idempotent: activating the session that is already active is a no-op
    /// that still answers `true`. Returns `false` when the id is unknown.
    pub fn activate_session(&self, id: &str) -> Result<bool> {
        let Some(session) = self.get_session(id)? else {
            return Ok(false);
        };
        if session.status == SESSION_ACTIVE {
            return Ok(true);
        }
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE session SET status = 'archived', ended_at = datetime('now'),
                 updated_at = datetime('now')
                 WHERE lane_key = ?1 AND status = 'active' AND id <> ?2",
                rusqlite::params![session.lane_key, id],
            )?;
            tx.execute(
                "UPDATE session SET status = 'active', ended_at = NULL,
                 updated_at = datetime('now') WHERE id = ?1",
                [id],
            )?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(true)
    }

    /// Archive a session. Idempotent; `false` when the id is unknown.
    pub fn archive_session(&self, id: &str) -> Result<bool> {
        self.db.with_connection(|conn| {
            let exists: bool = conn
                .query_row("SELECT 1 FROM session WHERE id = ?1", [id], |_| Ok(true))
                .optional()?
                .unwrap_or(false);
            if !exists {
                return Ok(false);
            }
            conn.execute(
                "UPDATE session SET status = 'archived', ended_at = datetime('now'),
                 updated_at = datetime('now') WHERE id = ?1 AND status = 'active'",
                [id],
            )?;
            Ok(true)
        })
    }

    /// Rename a session and/or bind its workspace. `false` when unknown.
    ///
    /// `workspace_path` is `Some(Some(path))` to bind, `Some(None)` to unbind,
    /// `None` to leave alone — the three states `PATCH` can express.
    pub fn update_session(
        &self,
        id: &str,
        title: Option<&str>,
        workspace_path: Option<Option<&str>>,
    ) -> Result<bool> {
        self.db.with_connection(|conn| {
            let mut changed = 0usize;
            if let Some(title) = title {
                changed += conn.execute(
                    "UPDATE session SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
                    rusqlite::params![title, id],
                )?;
            }
            if let Some(workspace) = workspace_path {
                changed += conn.execute(
                    "UPDATE session SET workspace_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                    rusqlite::params![workspace, id],
                )?;
            }
            if changed > 0 {
                return Ok(true);
            }
            // Nothing to change (an empty PATCH) still has to distinguish a
            // known session from an unknown one.
            let exists: bool = conn
                .query_row("SELECT 1 FROM session WHERE id = ?1", [id], |_| Ok(true))
                .optional()?
                .unwrap_or(false);
            Ok(exists)
        })
    }

    /// Delete a session, its messages, and the run links that pointed at it.
    ///
    /// One transaction, not three statements: a half-deleted session leaves
    /// messages nothing can reach. `task.session_id` is nulled rather than
    /// cascaded — the runs happened, and their rows outlive the transcript
    /// they were started from (the same posture 038 took for `task_id`).
    /// Returns `false` when the id is unknown.
    ///
    /// `tool_execution_log` is de-indexed rather than deleted, the
    /// [`SkillExecutionRepository::clear_session_log_index`] posture (R51): that
    /// a tool ran is still true and `GET /v1/tools`' `invocations_today` must
    /// not change because a transcript went, but `session_id`, `log_seq` and
    /// `result_ref` addressed a session and a `sessions/<id>/log.jsonl` that no
    /// longer exist.
    ///
    /// [`SkillExecutionRepository::clear_session_log_index`]: crate::repository::SkillExecutionRepository::clear_session_log_index
    pub fn delete_session(&self, id: &str) -> Result<bool> {
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;
            let exists: bool = tx
                .query_row("SELECT 1 FROM session WHERE id = ?1", [id], |_| Ok(true))
                .optional()?
                .unwrap_or(false);
            if !exists {
                return Ok(false);
            }
            tx.execute(
                "DELETE FROM conversation_messages WHERE session_id = ?1",
                [id],
            )?;
            tx.execute(
                "UPDATE task SET session_id = NULL WHERE session_id = ?1",
                [id],
            )?;
            tx.execute(
                "UPDATE lane_followups SET session_id = NULL WHERE session_id = ?1",
                [id],
            )?;
            tx.execute(
                "UPDATE tool_execution_log
                    SET session_id = NULL, log_seq = NULL, result_ref = NULL
                  WHERE session_id = ?1",
                [id],
            )?;
            tx.execute("DELETE FROM session WHERE id = ?1", [id])?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// The lane's active session, if it has one.
    pub fn get_active_session_for_lane(&self, lane_key: &str) -> Result<Option<Conversation>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SESSION_COLUMNS} FROM session WHERE lane_key = ?1 AND status = 'active'",
            ))?;
            let mut rows = stmt.query(rusqlite::params![lane_key])?;
            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_session(row)?)),
                None => Ok(None),
            }
        })
    }

    /// The id of the lane's active session — the one lookup the persistence
    /// path does per turn.
    pub fn active_session_id(&self, lane_key: &str) -> Result<Option<String>> {
        self.db.with_connection(|conn| {
            let id = conn
                .query_row(
                    "SELECT id FROM session WHERE lane_key = ?1 AND status = 'active'",
                    [lane_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Ok(id)
        })
    }

    /// Every lane's active session id.
    ///
    /// The boot sweep's protected set (plan §5.4: "an active session's log is
    /// never evicted"). One query rather than one per lane, because the sweep
    /// runs before anything else knows which lanes exist.
    pub fn active_session_ids(&self) -> Result<Vec<String>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM session WHERE status = 'active'")?;
            let ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(ids)
        })
    }

    /// Get a session by ID.
    pub fn get_session(&self, id: &str) -> Result<Option<Conversation>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1",
            ))?;
            let mut rows = stmt.query(rusqlite::params![id])?;
            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_session(row)?)),
                None => Ok(None),
            }
        })
    }

    /// List sessions with the `GET /v1/sessions` filters, newest first, plus
    /// the total matching the same filters (before paging).
    ///
    /// The page is clamped here, not only at the route: a non-positive `limit`
    /// serves [`SESSION_PAGE_DEFAULT`] and anything larger than
    /// [`SESSION_PAGE_MAX`] serves that. `total` is the count before paging
    /// either way, so a caller can still see how much it did not get.
    pub fn list_sessions(&self, filter: &SessionFilter<'_>) -> Result<(Vec<Conversation>, i64)> {
        let mut where_sql = String::from(" WHERE 1 = 1");
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(workspace) = filter.workspace_id {
            params.push(Box::new(workspace.to_string()));
            where_sql.push_str(&format!(" AND workspace_id = ?{}", params.len()));
        }
        if let Some(source) = filter.source {
            params.push(Box::new(source.to_string()));
            where_sql.push_str(&format!(" AND source = ?{}", params.len()));
        }
        if let Some(status) = filter.status {
            params.push(Box::new(status.to_string()));
            where_sql.push_str(&format!(" AND status = ?{}", params.len()));
        }
        if let Some(q) = filter.q {
            // `to_ascii_lowercase`, not `to_lowercase`: the other side of the
            // comparison is SQLite's `LOWER()`, which folds ASCII only. Rust's
            // Unicode folding would lower-case a needle the column never
            // lowers — 'İ' against a stored 'İ' — and match nothing.
            params.push(Box::new(format!(
                "%{}%",
                escape_like(&q.to_ascii_lowercase())
            )));
            let idx = params.len();
            where_sql.push_str(&format!(
                " AND (LOWER(title) LIKE ?{idx} ESCAPE '\\' \
                    OR LOWER(lane_key) LIKE ?{idx} ESCAPE '\\')"
            ));
        }

        self.db.with_connection(|conn| {
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            let total: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM session{where_sql}"),
                params_refs.as_slice(),
                |row| row.get(0),
            )?;

            let mut paged = params_refs.clone();
            let limit = match filter.limit {
                n if n <= 0 => SESSION_PAGE_DEFAULT,
                n => n.min(SESSION_PAGE_MAX),
            };
            let offset = filter.offset.max(0);
            paged.push(&limit);
            paged.push(&offset);
            let sql = format!(
                "SELECT {SESSION_COLUMNS} FROM session{where_sql} \
                 ORDER BY updated_at DESC, id DESC LIMIT ?{} OFFSET ?{}",
                params.len() + 1,
                params.len() + 2,
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(paged.as_slice())?;
            let mut sessions = Vec::new();
            while let Some(row) = rows.next()? {
                sessions.push(Self::row_to_session(row)?);
            }
            Ok((sessions, total))
        })
    }

    /// List sessions for a specific owner (lane_key starts with "{owner_id}:").
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
                    format!("SELECT {SESSION_COLUMNS} FROM session WHERE lane_key LIKE ?1 AND source = ?2 ORDER BY updated_at DESC LIMIT ?3 OFFSET ?4"),
                    vec![Box::new(lane_pattern), Box::new(source.to_string()), Box::new(limit), Box::new(offset)],
                ),
                None => (
                    format!("SELECT {SESSION_COLUMNS} FROM session WHERE lane_key LIKE ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3"),
                    vec![Box::new(lane_pattern), Box::new(limit), Box::new(offset)],
                ),
            };

            let mut stmt = conn.prepare(&sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let mut rows = stmt.query(params_refs.as_slice())?;
            let mut conversations = Vec::new();
            while let Some(row) = rows.next()? {
                conversations.push(Self::row_to_session(row)?);
            }
            Ok(conversations)
        })
    }

    /// Increment the message count and update last_message_at for the lane's
    /// active session.
    pub fn increment_message_count(&self, lane_key: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE session
                 SET message_count = message_count + 1,
                     last_message_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE lane_key = ?1 AND status = 'active'",
                [lane_key],
            )?;
            Ok(())
        })
    }

    /// Increment the message count of one session, whatever its status.
    ///
    /// A workflow's completion report lands in the session that *started* it
    /// (§5.3), which may since have been archived; counting it on whatever
    /// session the lane happens to be showing now would be a lie in both rows.
    pub fn increment_message_count_for_session(&self, session_id: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE session
                 SET message_count = message_count + 1,
                     last_message_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?1",
                [session_id],
            )?;
            Ok(())
        })
    }

    /// Get summary data for the lane's active session.
    /// Returns (summary, summary_version, last_summarized_message_id).
    pub fn get_summary(&self, lane_key: &str) -> Result<(String, i64, i64)> {
        self.db.with_connection(|conn| {
            let result = conn.query_row(
                "SELECT summary, summary_version, last_summarized_message_id FROM session WHERE lane_key = ?1 AND status = 'active'",
                [lane_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
            ).optional()?;
            Ok(result.unwrap_or_else(|| (String::new(), 0, 0)))
        })
    }

    /// Update the active session's summary with optimistic locking.
    /// Returns true if the update succeeded.
    pub fn update_summary_optimistic(
        &self,
        lane_key: &str,
        expected_version: i64,
        summary: &str,
        last_id: i64,
    ) -> Result<bool> {
        self.db.with_connection(|conn| {
            let rows = conn.execute(
                "UPDATE session SET summary = ?1, summary_version = summary_version + 1,
                 last_summarized_message_id = ?2, summary_updated_at = datetime('now'),
                 updated_at = datetime('now')
                 WHERE lane_key = ?3 AND status = 'active' AND summary_version = ?4",
                rusqlite::params![summary, last_id, lane_key, expected_version],
            )?;
            Ok(rows > 0)
        })
    }

    /// Clear the summary and reset counters for the lane's active session.
    pub fn clear_summary(&self, lane_key: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE session SET summary = '', summary_version = 0,
                 last_summarized_message_id = 0, summary_updated_at = datetime('now'),
                 message_count = 0, last_message_at = NULL, updated_at = datetime('now')
                 WHERE lane_key = ?1 AND status = 'active'",
                [lane_key],
            )?;
            Ok(())
        })
    }

    /// Clear the summary and reset counters for one session by id.
    pub fn clear_summary_for_session(&self, session_id: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE session SET summary = '', summary_version = 0,
                 last_summarized_message_id = 0, summary_updated_at = datetime('now'),
                 message_count = 0, last_message_at = NULL, updated_at = datetime('now')
                 WHERE id = ?1",
                [session_id],
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
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS}
                 FROM conversation_messages
                 WHERE lane_key = ?1 AND id > ?2 AND id < ?3
                 ORDER BY id ASC
                 LIMIT ?4",
            ))?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![lane_key, after_id, before_id, limit])?;
            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
            }
            Ok(messages)
        })
    }

    /// List messages of one session in an ID range (exclusive bounds), id ASC.
    pub fn list_by_session_id_range(
        &self,
        session_id: &str,
        after_id: i64,
        before_id: i64,
        limit: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS}
                 FROM conversation_messages
                 WHERE session_id = ?1 AND id > ?2 AND id < ?3
                 ORDER BY id ASC
                 LIMIT ?4",
            ))?;
            let mut messages = Vec::new();
            let mut rows = stmt.query(rusqlite::params![session_id, after_id, before_id, limit])?;
            while let Some(row) = rows.next()? {
                messages.push(Self::row_to_message(row)?);
            }
            Ok(messages)
        })
    }

    /// How many runs of each `task.status` a set of sessions started — one
    /// grouped query for a whole page of `SessionView`s, never one per row.
    pub fn task_counts_by_session(
        &self,
        session_ids: &[String],
        status: &str,
    ) -> Result<std::collections::HashMap<String, i64>> {
        if session_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        self.db.with_connection(|conn| {
            let placeholders = std::iter::repeat_n("?", session_ids.len())
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = conn.prepare(&format!(
                "SELECT session_id, COUNT(*) FROM task \
                 WHERE session_id IN ({placeholders}) AND status = ?{} \
                 GROUP BY session_id",
                session_ids.len() + 1
            ))?;
            let mut params: Vec<&dyn rusqlite::types::ToSql> =
                session_ids.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
            params.push(&status);
            let mut rows = stmt.query(params.as_slice())?;
            let mut counts = std::collections::HashMap::new();
            while let Some(row) = rows.next()? {
                counts.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
            }
            Ok(counts)
        })
    }

    /// `source → COUNT(*)` over `conversation_messages` since `since_utc`, for
    /// `GET /v1/connectors`' `messages_7d` (GAP-17, T49).
    ///
    /// `since_utc` is **already UTC** in the table's own `%Y-%m-%d %H:%M:%S`
    /// text form: `created_at` defaults to `datetime('now')`, which is UTC, so
    /// a local cutoff would be off by the daemon's offset. The caller converts.
    ///
    /// The grouping column is `source` — the connector id the gateway stamped
    /// on the turn — not the lane key's suffix: a lane key is a routing
    /// address that a follow-up can override, while `source` is what actually
    /// attributed the message. A row with no source (nothing attributed it) is
    /// counted for no connector rather than for a guessed one.
    ///
    /// One grouped query for the whole list: the Connectors panel renders every
    /// connector from one call, and a per-connector count would be an N+1.
    pub fn message_counts_by_source_since(
        &self,
        since_utc: &str,
    ) -> Result<std::collections::HashMap<String, i64>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT source, COUNT(*) FROM conversation_messages \
                 WHERE source IS NOT NULL AND created_at >= ?1 \
                 GROUP BY source",
            )?;
            let mut rows = stmt.query(rusqlite::params![since_utc])?;
            let mut counts = std::collections::HashMap::new();
            while let Some(row) = rows.next()? {
                counts.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
            }
            Ok(counts)
        })
    }

    fn row_to_session(row: &rusqlite::Row<'_>) -> Result<Conversation> {
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
            workspace_id: row.get(12)?,
            status: row.get(13)?,
            ended_at: row.get(14)?,
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
            content_json: row.get(10)?,
            display_text: row.get(11)?,
            task_id: row.get(12)?,
            session_id: row.get(13)?,
        })
    }
}

#[cfg(test)]
mod tests;
