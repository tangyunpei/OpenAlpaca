//! Repository for task operations

use std::collections::HashMap;

use crate::Database;
use crate::models::task::{OutcomeKind, Task, TaskStatus};
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{OptionalExtension, Row};

/// How many ids [`TaskRepository::titles_for`] puts in one `IN (…)`. Well under
/// SQLite's default variable limit (999), and one statement covers a full
/// artifact page, whose own limit is smaller than this.
const TITLES_FOR_CHUNK: usize = 500;

/// Every column [`TaskRepository::row_to_task`] reads, in the order it reads
/// them. Written once: the list used to be copied into each `SELECT`, and a
/// column added to the table but to only some of the copies is invisible until
/// something asks for it (migration 037's `source_task_id` was exactly that).
const TASK_COLUMNS: &str = "id, title, description, status, priority, progress_current, \
     progress_total, result_summary, created_by, source_lane, created_at, updated_at, \
     completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count, \
     workspace_id, source_task_id";

/// The placeholder tuple matching [`TASK_COLUMNS`].
const TASK_VALUES: &str = "(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
     ?16, ?17, ?18, ?19, ?20)";

/// [`TaskRepository::upsert_queued`]'s conflict tail — the row is reset to a
/// fresh queued run, keeping only what makes it *this* row (`id`, `created_at`,
/// `priority`).
const RELAUNCH_ON_CONFLICT: &str = "ON CONFLICT(id) DO UPDATE SET \
     title = excluded.title, \
     description = excluded.description, \
     status = excluded.status, \
     progress_current = NULL, \
     progress_total = NULL, \
     result_summary = NULL, \
     created_by = excluded.created_by, \
     source_lane = excluded.source_lane, \
     updated_at = excluded.updated_at, \
     completed_at = NULL, \
     state_json = NULL, \
     state_version = 0, \
     outcome_json = NULL, \
     outcome_kind = NULL, \
     artifact_count = 0, \
     workspace_id = excluded.workspace_id, \
     source_task_id = excluded.source_task_id";

/// Repository for task CRUD operations.
pub struct TaskRepository<'a> {
    db: &'a Database,
}

impl<'a> TaskRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    // ── Task CRUD ───────────────────────────────────────────────

    /// Create a new task.
    pub fn create(&self, task: &Task) -> Result<()> {
        self.insert_row(task, "", "Failed to create task")
    }

    /// Create the row, or re-launch the one already at this id (D5).
    ///
    /// The plan calls this the least clean code it asks for, and it is: every
    /// other dispatch mints a fresh id and `INSERT`s. `POST /v1/tasks/{id}/action
    /// {"action":"start"}` does not — D5 settled that `start` keeps the task id
    /// the client is already holding — so its persist step has to be a
    /// create-or-update. It lives here, alone, rather than as a branch inside
    /// the dispatcher, so there is exactly one place to read to know what a
    /// re-launch does to a stored row.
    ///
    /// On conflict the row is reset to a **fresh queued run**: the previous
    /// attempt's status, progress, summary, outcome and state are cleared,
    /// because a row that keeps them describes a run that is no longer the one
    /// it names. `state_version` goes back to `0` for the dispatcher's state
    /// init, which writes against that version.
    ///
    /// What survives is identity, not history: `id`, `created_at` and
    /// `priority` are the row's own and are never re-minted. Note the
    /// consequence, which is why the caller in front of this refuses both a
    /// live run and a finished one (R43): re-launching a *finished* row would
    /// discard that run's result. `rerun` is the verb that keeps it — it copies
    /// the goal onto a new id and leaves the original untouched.
    pub fn upsert_queued(&self, task: &Task) -> Result<()> {
        self.insert_row(task, RELAUNCH_ON_CONFLICT, "Failed to upsert queued task")
    }

    /// The one `INSERT INTO task`, with an optional `ON CONFLICT` tail.
    ///
    /// Both writers bind [`TASK_COLUMNS`] in the same order from the same
    /// place, so a column can never reach one statement and miss the other.
    fn insert_row(&self, task: &Task, on_conflict: &str, context: &'static str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                &format!("INSERT INTO task ({TASK_COLUMNS}) VALUES {TASK_VALUES} {on_conflict}"),
                rusqlite::params![
                    task.id,
                    task.title,
                    task.description,
                    task.status.as_str(),
                    task.priority,
                    task.progress_current,
                    task.progress_total,
                    task.result_summary,
                    task.created_by,
                    task.source_lane,
                    task.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    task.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    task.completed_at
                        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    task.state_json,
                    task.state_version,
                    task.outcome_json,
                    task.outcome_kind.map(|k| k.as_str().to_string()),
                    task.artifact_count,
                    task.workspace_id,
                    task.source_task_id,
                ],
            )
            .context(context)?;
            Ok(())
        })
    }

    /// Get a task by ID.
    pub fn get(&self, id: &str) -> Result<Option<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM task WHERE id = ?"
            ))?;
            let task = stmt
                .query_row([id], Self::row_to_task)
                .optional()
                .context("Failed to get task")?;
            Ok(task)
        })
    }

    /// `id -> title` for the ids that exist — the list routes' join (R26).
    ///
    /// Two columns, one statement per [`TITLES_FOR_CHUNK`] ids, all inside a
    /// single connection acquisition. The alternative a caller reaches for —
    /// [`Self::get`] per id — materializes a whole [`Task`] each time, blobs
    /// (`state_json`, `outcome_json`) included, and takes the global connection
    /// mutex once per row, to read one short string.
    ///
    /// Ids with no row are simply absent from the map (a run whose task was
    /// deleted has no title, which is not an error); an empty slice queries
    /// nothing.
    pub fn titles_for(&self, ids: &[String]) -> Result<HashMap<String, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.db
            .with_connection(|conn| {
                let mut titles = HashMap::with_capacity(ids.len());
                for chunk in ids.chunks(TITLES_FOR_CHUNK) {
                    let placeholders = vec!["?"; chunk.len()].join(", ");
                    let mut stmt = conn.prepare(&format!(
                        "SELECT id, title FROM task WHERE id IN ({placeholders})"
                    ))?;
                    let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?;
                    for row in rows {
                        let (id, title) = row?;
                        titles.insert(id, title);
                    }
                }
                Ok(titles)
            })
            .context("Failed to load task titles")
    }

    /// List tasks by creator.
    pub fn list_by_creator(&self, created_by: &str, limit: usize) -> Result<Vec<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM task WHERE created_by = ? ORDER BY created_at DESC LIMIT ?"
            ))?;
            let rows = stmt.query_map(rusqlite::params![created_by, limit as i64], |row| {
                Self::row_to_task(row)
            })?;
            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(row?);
            }
            Ok(tasks)
        })
    }

    /// List tasks by status.
    pub fn list_by_status(&self, status: TaskStatus, limit: usize) -> Result<Vec<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM task WHERE status = ? ORDER BY created_at DESC LIMIT ?"
            ))?;
            let rows = stmt.query_map(rusqlite::params![status.as_str(), limit as i64], |row| {
                Self::row_to_task(row)
            })?;
            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(row?);
            }
            Ok(tasks)
        })
    }

    /// List active tasks (queued, running, or paused).
    pub fn list_active(&self, limit: usize) -> Result<Vec<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM task
                 WHERE status IN ('queued', 'running', 'paused')
                 ORDER BY priority DESC, created_at ASC LIMIT ?"
            ))?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Self::row_to_task(row)
            })?;
            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(row?);
            }
            Ok(tasks)
        })
    }

    /// List active tasks (queued/running/paused) filtered by creator.
    pub fn list_active_by_creator(&self, created_by: &str, limit: usize) -> Result<Vec<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM task
                 WHERE created_by = ? AND status IN ('queued', 'running', 'paused')
                 ORDER BY priority DESC, created_at ASC LIMIT ?"
            ))?;
            let rows = stmt.query_map(rusqlite::params![created_by, limit as i64], |row| {
                Self::row_to_task(row)
            })?;
            let mut tasks = Vec::new();
            for row in rows { tasks.push(row?); }
            Ok(tasks)
        })
    }

    /// List recent tasks of all statuses (most recent first).
    pub fn list_recent(&self, limit: usize) -> Result<Vec<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {TASK_COLUMNS} FROM task ORDER BY created_at DESC LIMIT ?"
            ))?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Self::row_to_task(row)
            })?;
            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(row?);
            }
            Ok(tasks)
        })
    }

    /// Update a task's status. Sets completed_at if the new status is terminal.
    /// Returns true if a row was updated.
    pub fn update_status(&self, id: &str, status: TaskStatus) -> Result<bool> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let completed_at = if status.is_terminal() {
                Some(now.clone())
            } else {
                None
            };

            let rows = conn.execute(
                "UPDATE task SET status = ?1, updated_at = ?2, completed_at = COALESCE(?3, completed_at) WHERE id = ?4",
                rusqlite::params![status.as_str(), now, completed_at, id],
            )
            .context("Failed to update task status")?;
            Ok(rows > 0)
        })
    }

    /// Mark every non-terminal task (queued / running / paused) as failed
    /// with the given reason, preserving any result summary already present.
    /// Returns the number of tasks swept.
    ///
    /// Used by the daemon's startup orphan sweep (Routing V2 Phase 3):
    /// in-flight execution does not survive a restart, so any task left
    /// non-terminal in the DB is an orphan.
    pub fn fail_all_non_terminal(&self, reason: &str) -> Result<usize> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let rows = conn
                .execute(
                    "UPDATE task SET status = 'failed',
                            result_summary = COALESCE(result_summary, ?1),
                            updated_at = ?2,
                            completed_at = COALESCE(completed_at, ?2)
                     WHERE status IN ('queued', 'running', 'paused')",
                    rusqlite::params![reason, now],
                )
                .context("Failed to sweep non-terminal tasks")?;
            Ok(rows)
        })
    }

    /// Set a task's result summary.
    /// Returns true if a row was updated.
    pub fn set_result(&self, id: &str, result_summary: &str) -> Result<bool> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let rows = conn
                .execute(
                    "UPDATE task SET result_summary = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![result_summary, now, id],
                )
                .context("Failed to set task result")?;
            Ok(rows > 0)
        })
    }

    /// Persist the structured task outcome fields.
    /// Sets outcome_json, outcome_kind, and artifact_count.
    /// Does NOT update result_summary (caller should use set_result or finalize_task).
    /// Returns true if a row was updated.
    pub fn set_outcome(
        &self,
        id: &str,
        outcome_json: &str,
        outcome_kind: OutcomeKind,
        artifact_count: i32,
    ) -> Result<bool> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let rows = conn
                .execute(
                    "UPDATE task SET outcome_json = ?1, outcome_kind = ?2,
                            artifact_count = ?3, updated_at = ?4
                     WHERE id = ?5",
                    rusqlite::params![
                        outcome_json,
                        outcome_kind.as_str(),
                        artifact_count,
                        now,
                        id
                    ],
                )
                .context("Failed to set task outcome")?;
            Ok(rows > 0)
        })
    }

    /// Update state_json with optimistic locking on state_version.
    /// Returns true if updated, false if version mismatch.
    pub fn update_state(&self, id: &str, state_json: &str, expected_version: i32) -> Result<bool> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let rows = conn
                .execute(
                    "UPDATE task SET state_json = ?1, state_version = ?2, updated_at = ?3
                 WHERE id = ?4 AND state_version = ?5",
                    rusqlite::params![state_json, expected_version + 1, now, id, expected_version],
                )
                .context("Failed to update task state")?;
            Ok(rows > 0)
        })
    }

    /// Delete a task by ID.
    pub fn delete(&self, id: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute("DELETE FROM task WHERE id = ?", [id])
                .context("Failed to delete task")?;
            Ok(())
        })
    }

    // ── Row Mappers ─────────────────────────────────────────────

    fn row_to_task(row: &Row) -> rusqlite::Result<Task> {
        let status_str: String = row.get(3)?;
        let created_str: String = row.get(10)?;
        let updated_str: String = row.get(11)?;
        let completed_str: Option<String> = row.get(12)?;
        let outcome_kind_str: Option<String> = row.get(16)?;

        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            description: row.get(2)?,
            status: status_str.parse().unwrap_or(TaskStatus::Queued),
            priority: row.get(4)?,
            progress_current: row.get(5)?,
            progress_total: row.get(6)?,
            result_summary: row.get(7)?,
            created_by: row.get(8)?,
            source_lane: row.get(9)?,
            created_at: parse_datetime(&created_str),
            updated_at: parse_datetime(&updated_str),
            completed_at: completed_str.as_deref().map(parse_datetime),
            state_json: row.get(13)?,
            state_version: row.get(14)?,
            outcome_json: row.get(15)?,
            outcome_kind: outcome_kind_str
                .as_deref()
                .and_then(|s| s.parse().ok()),
            artifact_count: row.get(17)?,
            workspace_id: row.get(18)?,
            source_task_id: row.get(19)?,
        })
    }

}

/// Parse a SQLite datetime string into DateTime<Utc>.
fn parse_datetime(s: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| ndt.and_utc())
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests;
