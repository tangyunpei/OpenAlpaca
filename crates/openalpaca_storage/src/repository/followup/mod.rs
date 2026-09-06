//! Repository for the lane follow-up queue (Routing V2)
//!
//! Stores `queue_followup` items and unprocessed steering leftovers per lane.
//! Queued `followup` items are claimed one at a time (queued → running) when a
//! workflow finalizes; `unprocessed_steering` items are never auto-claimed —
//! they are surfaced on the lane's next user turn.

use crate::Database;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// Follow-up kind: an explicit `queue_followup` item.
pub const FOLLOWUP_KIND_FOLLOWUP: &str = "followup";
/// Follow-up kind: a steering message left undelivered at workflow exit.
pub const FOLLOWUP_KIND_UNPROCESSED_STEERING: &str = "unprocessed_steering";

/// A single lane follow-up row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowupRecord {
    pub id: i64,
    pub lane_key: String,
    /// "followup" | "unprocessed_steering"
    pub kind: String,
    pub content: String,
    /// Serialized `Principal` of the originating request (for re-entry).
    pub principal_json: String,
    pub workspace_path: Option<String>,
    /// Task the item was queued from, if any.
    pub source_task_id: Option<String>,
    /// The session the item was queued *in* (§5.3). A follow-up is a promise
    /// to continue that conversation, so the turn it later runs as belongs
    /// there — not in whatever session the lane happens to be showing by then.
    /// `None` for pre-039 rows, which fall back to the lane's active session.
    pub session_id: Option<String>,
    /// "queued" | "running" | "done" | "cancelled"
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

const SELECT_COLUMNS: &str = "id, lane_key, kind, content, principal_json, \
     workspace_path, source_task_id, status, created_at, updated_at, session_id";

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FollowupRecord> {
    Ok(FollowupRecord {
        id: row.get(0)?,
        lane_key: row.get(1)?,
        kind: row.get(2)?,
        content: row.get(3)?,
        principal_json: row.get(4)?,
        workspace_path: row.get(5)?,
        source_task_id: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        session_id: row.get(10)?,
    })
}

/// One interjection the boot sweep recovered out of a crashed run's session
/// log (§5.6b) — everything [`FollowupRepository::recover_unprocessed_steering`]
/// needs to write the row the graceful path would have written.
///
/// Deliberately not a `SteeringMsg`: this crate cannot see one, and the row's
/// columns are the whole contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredSteering {
    pub content: String,
    pub principal_json: String,
    pub workspace_path: Option<String>,
}

/// Repository for lane follow-up operations.
pub struct FollowupRepository<'a> {
    db: &'a Database,
}

impl<'a> FollowupRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Queue a new follow-up item. Returns the row ID.
    ///
    /// The row is pinned to the lane's active session as it is written (§5.3):
    /// the conversation the promise was made in is knowable now and not later.
    pub fn queue(
        &self,
        lane_key: &str,
        kind: &str,
        content: &str,
        principal_json: &str,
        workspace_path: Option<&str>,
        source_task_id: Option<&str>,
    ) -> Result<i64> {
        self.db.with_connection(|conn| {
            let session_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM session WHERE lane_key = ?1 AND status = 'active'",
                    rusqlite::params![lane_key],
                    |row| row.get(0),
                )
                .optional()
                .context("Failed to resolve the lane's active session")?;
            conn.execute(
                "INSERT INTO lane_followups \
                 (lane_key, kind, content, principal_json, workspace_path, source_task_id, session_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    lane_key,
                    kind,
                    content,
                    principal_json,
                    workspace_path,
                    source_task_id,
                    session_id,
                ],
            )
            .context("Failed to insert lane followup")?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Resolve the lane's currently active session, if it has one. The same
    /// lookup [`queue`](Self::queue) does internally — exposed here so a
    /// caller that wants "pin this row to whatever the lane is showing right
    /// now" (the dropped-record fallback and the graceful-exit conversion
    /// both want this, same as `queue`) can resolve it before calling
    /// [`queue_unprocessed_steering_once`](Self::queue_unprocessed_steering_once),
    /// which — unlike `queue` — takes the session id as a plain value rather
    /// than resolving it itself: the boot-time crash recovery wants the
    /// crashed run's *own* session instead of whatever the lane happens to
    /// be showing by the time the crash is noticed.
    pub fn active_session_id(&self, lane_key: &str) -> Result<Option<String>> {
        self.db.with_connection(|conn| {
            conn.query_row(
                "SELECT id FROM session WHERE lane_key = ?1 AND status = 'active'",
                rusqlite::params![lane_key],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to resolve the lane's active session")
        })
    }

    /// Queue a single `unprocessed_steering` row, unless a row for the same
    /// `(source_task_id, kind, content)` triple has already been filed.
    ///
    /// R56: the one guarded insert every writer of an `unprocessed_steering`
    /// row goes through — the dropped-record fallback (`runner/steering.rs`),
    /// the graceful-exit leftover conversion
    /// (`orchestrator/dispatcher/lead_agent.rs`), and
    /// [`recover_unprocessed_steering`](Self::recover_unprocessed_steering)
    /// below — so the same interjection can never be filed twice by two
    /// different call sites, even when both fire for the same message at
    /// workflow detach (the boot recovery previously carried its own
    /// multiset guard while the other two writers carried none at all).
    ///
    /// The guard is a single `INSERT … WHERE NOT EXISTS`, run under the
    /// process's one `Mutex<Connection>` like every other write here: there
    /// is no window between a check and an insert for two in-process writers
    /// to race through.
    ///
    /// This narrows the historical **multiset** guard
    /// (`recover_unprocessed_steering`'s own doc comment below) to a **set**:
    /// two calls naming the same task, kind, and literal text collapse to
    /// one row even when they really are two distinct interjections — the
    /// schema has no column that could tell them apart. That is an accepted
    /// trade against the harder guarantee this exists to close: the *same*
    /// interjection must never be filed twice by two call sites. Content
    /// that actually differs is unaffected — still one row per distinct
    /// message.
    ///
    /// Returns `Ok(Some(id))` when a new row was inserted, `Ok(None)` when a
    /// matching row already existed and nothing changed.
    pub fn queue_unprocessed_steering_once(
        &self,
        lane_key: &str,
        content: &str,
        principal_json: &str,
        workspace_path: Option<&str>,
        source_task_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<i64>> {
        self.db.with_connection(|conn| {
            let changed = conn
                .execute(
                    "INSERT INTO lane_followups \
                     (lane_key, kind, content, principal_json, workspace_path, source_task_id, session_id) \
                     SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7 \
                     WHERE NOT EXISTS ( \
                         SELECT 1 FROM lane_followups \
                         WHERE source_task_id = ?6 AND kind = ?2 AND content = ?3 \
                     )",
                    rusqlite::params![
                        lane_key,
                        FOLLOWUP_KIND_UNPROCESSED_STEERING,
                        content,
                        principal_json,
                        workspace_path,
                        source_task_id,
                        session_id,
                    ],
                )
                .context("Failed to insert an unprocessed_steering follow-up")?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(Some(conn.last_insert_rowid()))
        })
    }

    /// Fetch a single follow-up row by id.
    pub fn get(&self, id: i64) -> Result<Option<FollowupRecord>> {
        self.db.with_connection(|conn| {
            let record = conn
                .query_row(
                    &format!("SELECT {SELECT_COLUMNS} FROM lane_followups WHERE id = ?1"),
                    rusqlite::params![id],
                    row_to_record,
                )
                .optional()
                .context("Failed to fetch followup")?;
            Ok(record)
        })
    }

    /// File a crashed run's undelivered interjections as the rows the graceful
    /// path would have written (§5.6b). Returns the ids actually inserted.
    ///
    /// Built on [`queue_unprocessed_steering_once`](Self::queue_unprocessed_steering_once)
    /// (R56): each item is its own guarded `INSERT … WHERE NOT EXISTS`
    /// against `(source_task_id, kind, content)`, so a crash between this
    /// pass and the status flip is still safe the same way it always was —
    /// the run is non-terminal until the flip, the next boot scans it again,
    /// and the guard finds nothing left to add — and a run whose leftovers
    /// were partly filed by the graceful path before it crashed gains only
    /// the ones still missing.
    ///
    /// **This guard is a set, not the historical multiset.** R56 narrowed it
    /// to close a duplicate-insert bug shared with the other two writers of
    /// this row: two recovered interjections with identical text now
    /// collapse to one row, even within the same call — the schema has no
    /// column that would let the guard tell them apart. Content that
    /// actually differs is unaffected, and a second pass over the same log
    /// (or one that only partly landed before a crash) still adds nothing it
    /// already filed.
    ///
    /// `session_id` is the run's own (`task.session_id`), not the lane's
    /// current active session: the promise was made in that conversation
    /// (§5.3), and by the time a crash is noticed the lane may be showing
    /// another.
    pub fn recover_unprocessed_steering(
        &self,
        lane_key: &str,
        source_task_id: &str,
        session_id: Option<&str>,
        items: &[RecoveredSteering],
    ) -> Result<Vec<i64>> {
        let mut inserted = Vec::new();
        for item in items {
            if let Some(id) = self.queue_unprocessed_steering_once(
                lane_key,
                &item.content,
                &item.principal_json,
                item.workspace_path.as_deref(),
                source_task_id,
                session_id,
            )? {
                inserted.push(id);
            }
        }
        Ok(inserted)
    }

    /// List all queued items for a lane (any kind), oldest first.
    pub fn list_queued_by_lane(&self, lane_key: &str) -> Result<Vec<FollowupRecord>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM lane_followups \
                 WHERE lane_key = ?1 AND status = 'queued' ORDER BY id"
            ))?;
            let rows = stmt
                .query_map(rusqlite::params![lane_key], row_to_record)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Claim the oldest queued `followup` item for a lane, atomically moving it
    /// queued → running (CAS on status). Returns `None` when nothing is queued
    /// or a concurrent claimer won. `unprocessed_steering` items are never
    /// claimed — they must not auto-execute.
    ///
    /// The claim also **re-activates the item's session** (§5.3): a follow-up
    /// is a promise to continue *that* conversation, and the turn it is about
    /// to run as resolves its session from the lane. Without this, a follow-up
    /// queued in conversation A silently appends to whatever conversation B the
    /// user opened since. The re-activation shares the claim's transaction
    /// because the two must not be separable: a claimed row whose session was
    /// not re-activated runs in the wrong place, and a re-activated session
    /// whose claim then lost would have moved the user's chat window for
    /// nothing. Pre-039 rows (`session_id IS NULL`) fall back to whatever the
    /// lane's active session already is.
    pub fn claim_next(&self, lane_key: &str) -> Result<Option<FollowupRecord>> {
        self.db.with_connection_mut(|conn| {
            let tx = conn
                .transaction()
                .context("Failed to begin followup claim transaction")?;

            let candidate: Option<i64> = tx
                .query_row(
                    "SELECT id FROM lane_followups \
                     WHERE lane_key = ?1 AND status = 'queued' AND kind = 'followup' \
                     ORDER BY id LIMIT 1",
                    rusqlite::params![lane_key],
                    |row| row.get(0),
                )
                .optional()
                .context("Failed to select next queued followup")?;
            let Some(id) = candidate else {
                return Ok(None);
            };

            // CAS: only wins if the row is still queued.
            let changed = tx
                .execute(
                    "UPDATE lane_followups \
                     SET status = 'running', updated_at = datetime('now') \
                     WHERE id = ?1 AND status = 'queued'",
                    rusqlite::params![id],
                )
                .context("Failed to claim followup")?;
            if changed == 0 {
                return Ok(None);
            }

            let record = tx.query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM lane_followups WHERE id = ?1"),
                rusqlite::params![id],
                row_to_record,
            )?;

            // Re-home the turn: archive the usurper, then re-activate the
            // session this item was queued in. Order matters — the partial
            // unique index allows only one active session per lane.
            if let Some(ref session_id) = record.session_id {
                tx.execute(
                    "UPDATE session SET status = 'archived', ended_at = datetime('now'), \
                     updated_at = datetime('now') \
                     WHERE lane_key = ?1 AND status = 'active' AND id <> ?2",
                    rusqlite::params![record.lane_key, session_id],
                )
                .context("Failed to archive the usurping session")?;
                tx.execute(
                    "UPDATE session SET status = 'active', ended_at = NULL, \
                     updated_at = datetime('now') WHERE id = ?1",
                    rusqlite::params![session_id],
                )
                .context("Failed to re-activate the follow-up's session")?;
            }

            tx.commit()
                .context("Failed to commit followup claim transaction")?;
            Ok(Some(record))
        })
    }

    /// Cancel a follow-up item, but only while it is still queued **and**
    /// still belongs to `lane_key`. Returns whether the cancel won.
    ///
    /// One `UPDATE` with the whole predicate in its `WHERE`, for the same
    /// reason [`claim_next`](Self::claim_next) has one: the autostart claim
    /// races this. `mark_cancelled` is unconditional, so a caller that read
    /// the row, saw `queued`, and then cancelled would overwrite a row the
    /// claim had already moved to `running` — the turn keeps running while
    /// the ledger says it was cancelled. With both sides CAS-ing on
    /// `status = 'queued'`, exactly one wins and the loser is told so.
    ///
    /// The lane is part of the predicate rather than a separate check for the
    /// same reason: an id belonging to another lane must not be cancellable,
    /// and re-reading the row to decide that would reopen the window.
    pub fn cancel_if_queued(&self, id: i64, lane_key: &str) -> Result<bool> {
        self.db.with_connection(|conn| {
            let changed = conn
                .execute(
                    "UPDATE lane_followups \
                     SET status = 'cancelled', updated_at = datetime('now') \
                     WHERE id = ?1 AND lane_key = ?2 AND status = 'queued'",
                    rusqlite::params![id, lane_key],
                )
                .context("Failed to cancel followup")?;
            Ok(changed > 0)
        })
    }

    /// Consume a still-queued follow-up item, moving it queued → done.
    /// Returns whether the claim won.
    ///
    /// The third CAS on `status = 'queued'`, and it exists for the same reason
    /// as the other two. The lazy `unprocessed_steering` injection lists a
    /// lane's leftovers, renders them into the turn's context block, and marks
    /// them done so they surface exactly once — and a `DELETE` on the follow-up
    /// route can land in that gap, because every repository call takes and
    /// releases the connection separately. Unconditional [`mark_done`] would
    /// then overwrite a `cancelled` row with `done` *after* the client was told
    /// the item was dropped. CAS-ing instead lets the injector claim each row
    /// before it renders it, and skip the ones it lost.
    ///
    /// [`mark_done`](Self::mark_done) stays for the follow-up runner, which
    /// closes out a row it already claimed (`running`, not `queued`).
    pub fn mark_done_if_queued(&self, id: i64) -> Result<bool> {
        self.db.with_connection(|conn| {
            let changed = conn
                .execute(
                    "UPDATE lane_followups \
                     SET status = 'done', updated_at = datetime('now') \
                     WHERE id = ?1 AND status = 'queued'",
                    rusqlite::params![id],
                )
                .context("Failed to claim followup")?;
            Ok(changed > 0)
        })
    }

    /// Mark a follow-up item done, whatever its current status.
    ///
    /// For a row this caller already owns — the follow-up runner closing out
    /// the `running` row `claim_next` handed it. A caller that is racing
    /// another transition off `queued` wants
    /// [`mark_done_if_queued`](Self::mark_done_if_queued) instead.
    pub fn mark_done(&self, id: i64) -> Result<()> {
        self.set_status(id, "done")
    }

    /// Mark a follow-up item cancelled.
    pub fn mark_cancelled(&self, id: i64) -> Result<()> {
        self.set_status(id, "cancelled")
    }

    fn set_status(&self, id: i64, status: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE lane_followups \
                 SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![status, id],
            )
            .context("Failed to update followup status")?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
