//! Repository for task and task assignment operations

use crate::Database;
use crate::models::task::{AssignmentStatus, OutcomeKind, Task, TaskAgentAssignment, TaskStatus};
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{OptionalExtension, Row};

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
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO task (id, title, description, status, priority, progress_current, progress_total, result_summary, created_by, source_lane, created_at, updated_at, completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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
                    task.completed_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    task.state_json,
                    task.state_version,
                    task.outcome_json,
                    task.outcome_kind.map(|k| k.as_str().to_string()),
                    task.artifact_count,
                ],
            )
            .context("Failed to create task")?;
            Ok(())
        })
    }

    /// Create a task and its assignments atomically in a single transaction.
    /// If any insert fails, the entire operation is rolled back.
    pub fn create_task_with_assignments(
        &self,
        task: &Task,
        assignments: &[TaskAgentAssignment],
    ) -> Result<()> {
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;

            tx.execute(
                "INSERT INTO task (id, title, description, status, priority, progress_current, progress_total, result_summary, created_by, source_lane, created_at, updated_at, completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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
                    task.completed_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    task.state_json,
                    task.state_version,
                    task.outcome_json,
                    task.outcome_kind.map(|k| k.as_str().to_string()),
                    task.artifact_count,
                ],
            )
            .context("Failed to create task")?;

            for assignment in assignments {
                tx.execute(
                    "INSERT INTO task_agent_assignment (id, task_id, agent_id, role, status, step_order, started_at, completed_at, result_output)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        assignment.id,
                        assignment.task_id,
                        assignment.agent_id,
                        assignment.role,
                        assignment.status.as_str(),
                        assignment.step_order,
                        assignment.started_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                        assignment.completed_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                        assignment.result_output,
                    ],
                )
                .context("Failed to create assignment")?;
            }

            tx.commit()?;
            Ok(())
        })
    }

    /// Get a task by ID.
    pub fn get(&self, id: &str) -> Result<Option<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, description, status, priority, progress_current, progress_total, result_summary, created_by, source_lane, created_at, updated_at, completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count
                 FROM task WHERE id = ?",
            )?;
            let task = stmt
                .query_row([id], Self::row_to_task)
                .optional()
                .context("Failed to get task")?;
            Ok(task)
        })
    }

    /// List tasks by creator.
    pub fn list_by_creator(&self, created_by: &str, limit: usize) -> Result<Vec<Task>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, description, status, priority, progress_current, progress_total, result_summary, created_by, source_lane, created_at, updated_at, completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count
                 FROM task WHERE created_by = ? ORDER BY created_at DESC LIMIT ?",
            )?;
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
            let mut stmt = conn.prepare(
                "SELECT id, title, description, status, priority, progress_current, progress_total, result_summary, created_by, source_lane, created_at, updated_at, completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count
                 FROM task WHERE status = ? ORDER BY created_at DESC LIMIT ?",
            )?;
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
            let mut stmt = conn.prepare(
                "SELECT id, title, description, status, priority, progress_current, progress_total, result_summary, created_by, source_lane, created_at, updated_at, completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count
                 FROM task WHERE status IN ('queued', 'running', 'paused') ORDER BY priority DESC, created_at ASC LIMIT ?",
            )?;
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
            let mut stmt = conn.prepare(
                "SELECT id, title, description, status, priority, progress_current, progress_total,
                        result_summary, created_by, source_lane, created_at, updated_at, completed_at,
                        state_json, state_version, outcome_json, outcome_kind, artifact_count
                 FROM task
                 WHERE created_by = ? AND status IN ('queued', 'running', 'paused')
                 ORDER BY priority DESC, created_at ASC LIMIT ?",
            )?;
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
            let mut stmt = conn.prepare(
                "SELECT id, title, description, status, priority, progress_current, progress_total, result_summary, created_by, source_lane, created_at, updated_at, completed_at, state_json, state_version, outcome_json, outcome_kind, artifact_count
                 FROM task ORDER BY created_at DESC LIMIT ?",
            )?;
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

    /// Update a task's progress.
    /// Returns true if a row was updated.
    pub fn update_progress(&self, id: &str, current: i32, total: i32) -> Result<bool> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let rows = conn.execute(
                "UPDATE task SET progress_current = ?1, progress_total = ?2, updated_at = ?3 WHERE id = ?4",
                rusqlite::params![current, total, now, id],
            )
            .context("Failed to update task progress")?;
            Ok(rows > 0)
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

    // ── Assignment CRUD ─────────────────────────────────────────

    /// Create a new agent assignment for a task.
    pub fn create_assignment(&self, assignment: &TaskAgentAssignment) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO task_agent_assignment (id, task_id, agent_id, role, status, step_order, started_at, completed_at, result_output)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    assignment.id,
                    assignment.task_id,
                    assignment.agent_id,
                    assignment.role,
                    assignment.status.as_str(),
                    assignment.step_order,
                    assignment.started_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    assignment.completed_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    assignment.result_output,
                ],
            )
            .context("Failed to create assignment")?;
            Ok(())
        })
    }

    /// Get all assignments for a task.
    pub fn get_assignments(&self, task_id: &str) -> Result<Vec<TaskAgentAssignment>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, agent_id, role, status, step_order, started_at, completed_at, result_output
                 FROM task_agent_assignment WHERE task_id = ? ORDER BY step_order ASC",
            )?;
            let rows = stmt.query_map([task_id], Self::row_to_assignment)?;
            let mut assignments = Vec::new();
            for row in rows {
                assignments.push(row?);
            }
            Ok(assignments)
        })
    }

    /// Update an assignment's status.
    /// Returns true if a row was updated.
    pub fn update_assignment_status(&self, id: &str, status: AssignmentStatus) -> Result<bool> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

            let (started_clause, completed_clause) = match status {
                AssignmentStatus::Running => (
                    "started_at = COALESCE(started_at, ?3)",
                    "completed_at = completed_at",
                ),
                AssignmentStatus::Completed | AssignmentStatus::Failed => {
                    ("started_at = started_at", "completed_at = ?3")
                }
                _ => ("started_at = started_at", "completed_at = completed_at"),
            };

            let sql = format!(
                "UPDATE task_agent_assignment SET status = ?1, {}, {} WHERE id = ?2",
                started_clause, completed_clause
            );

            let rows = conn
                .execute(&sql, rusqlite::params![status.as_str(), id, now])
                .context("Failed to update assignment status")?;
            Ok(rows > 0)
        })
    }

    /// Set an assignment's result output.
    /// Returns true if a row was updated.
    pub fn set_assignment_output(&self, id: &str, output: &str) -> Result<bool> {
        self.db.with_connection(|conn| {
            let rows = conn
                .execute(
                    "UPDATE task_agent_assignment SET result_output = ?1 WHERE id = ?2",
                    rusqlite::params![output, id],
                )
                .context("Failed to set assignment output")?;
            Ok(rows > 0)
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
        })
    }

    fn row_to_assignment(row: &Row) -> rusqlite::Result<TaskAgentAssignment> {
        let status_str: String = row.get(4)?;
        let started_str: Option<String> = row.get(6)?;
        let completed_str: Option<String> = row.get(7)?;

        Ok(TaskAgentAssignment {
            id: row.get(0)?,
            task_id: row.get(1)?,
            agent_id: row.get(2)?,
            role: row.get(3)?,
            status: status_str.parse().unwrap_or(AssignmentStatus::Pending),
            step_order: row.get(5)?,
            started_at: started_str.as_deref().map(parse_datetime),
            completed_at: completed_str.as_deref().map(parse_datetime),
            result_output: row.get(8)?,
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
