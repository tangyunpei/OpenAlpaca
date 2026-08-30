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
    /// "queued" | "running" | "done" | "cancelled"
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

const SELECT_COLUMNS: &str = "id, lane_key, kind, content, principal_json, \
     workspace_path, source_task_id, status, created_at, updated_at";

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
    })
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
            conn.execute(
                "INSERT INTO lane_followups \
                 (lane_key, kind, content, principal_json, workspace_path, source_task_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    lane_key,
                    kind,
                    content,
                    principal_json,
                    workspace_path,
                    source_task_id,
                ],
            )
            .context("Failed to insert lane followup")?;
            Ok(conn.last_insert_rowid())
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
    pub fn claim_next(&self, lane_key: &str) -> Result<Option<FollowupRecord>> {
        self.db.with_connection(|conn| {
            let candidate: Option<i64> = conn
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
            let changed = conn
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

            let record = conn.query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM lane_followups WHERE id = ?1"),
                rusqlite::params![id],
                row_to_record,
            )?;
            Ok(Some(record))
        })
    }

    /// Mark a follow-up item done.
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
