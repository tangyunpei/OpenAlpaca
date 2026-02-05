//! Repository for SubAgent configuration, metrics, and task history

use crate::models::subagent::{AgentMetrics, AgentTaskHistory, SubAgentConfig};
use crate::Database;
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{OptionalExtension, Row};

/// Repository for SubAgent operations.
pub struct SubAgentRepository<'a> {
    db: &'a Database,
}

impl<'a> SubAgentRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    // ── SubAgentConfig CRUD ─────────────────────────────────────

    /// Insert or replace a SubAgent configuration.
    pub fn upsert(&self, config: &SubAgentConfig) -> Result<()> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            conn.execute(
                "INSERT INTO agent (id, name, persona, description, icon, status, current_task_id, skills_json, preset_json, constraints_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                   name = excluded.name,
                   persona = excluded.persona,
                   description = excluded.description,
                   icon = excluded.icon,
                   skills_json = excluded.skills_json,
                   preset_json = excluded.preset_json,
                   constraints_json = excluded.constraints_json,
                   updated_at = excluded.updated_at",
                rusqlite::params![
                    config.id,
                    config.name,
                    config.persona,
                    config.description,
                    config.icon,
                    config.status,
                    config.current_task_id,
                    config.skills_json,
                    config.preset_json,
                    config.constraints_json,
                    config.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    now,
                ],
            )
            .context("Failed to upsert SubAgentConfig")?;
            Ok(())
        })
    }

    /// Get a SubAgent config by ID.
    pub fn get(&self, id: &str) -> Result<Option<SubAgentConfig>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, persona, description, icon, status, current_task_id, skills_json, preset_json, constraints_json, created_at, updated_at
                 FROM agent WHERE id = ?",
            )?;
            let config = stmt
                .query_row([id], |row| Self::row_to_config(row))
                .optional()
                .context("Failed to get SubAgentConfig")?;
            Ok(config)
        })
    }

    /// List all SubAgent configs.
    pub fn list(&self, limit: usize) -> Result<Vec<SubAgentConfig>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, persona, description, icon, status, current_task_id, skills_json, preset_json, constraints_json, created_at, updated_at
                 FROM agent ORDER BY created_at DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Self::row_to_config(row)
            })?;
            let mut configs = Vec::new();
            for row in rows {
                configs.push(row?);
            }
            Ok(configs)
        })
    }

    /// List SubAgent configs by status.
    pub fn list_by_status(&self, status: &str, limit: usize) -> Result<Vec<SubAgentConfig>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, persona, description, icon, status, current_task_id, skills_json, preset_json, constraints_json, created_at, updated_at
                 FROM agent WHERE status = ? ORDER BY created_at DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![status, limit as i64], |row| {
                Self::row_to_config(row)
            })?;
            let mut configs = Vec::new();
            for row in rows {
                configs.push(row?);
            }
            Ok(configs)
        })
    }

    /// Update an agent's status and optionally its current task.
    /// Returns true if a row was updated.
    pub fn update_status(&self, id: &str, status: &str, task_id: Option<&str>) -> Result<bool> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let rows = conn.execute(
                "UPDATE agent SET status = ?1, current_task_id = ?2, updated_at = ?3 WHERE id = ?4",
                rusqlite::params![status, task_id, now, id],
            )
            .context("Failed to update agent status")?;
            Ok(rows > 0)
        })
    }

    /// Delete a SubAgent config by ID.
    pub fn delete(&self, id: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute("DELETE FROM agent WHERE id = ?", [id])
                .context("Failed to delete SubAgentConfig")?;
            Ok(())
        })
    }

    // ── Metrics CRUD ────────────────────────────────────────────

    /// Get metrics for an agent.
    pub fn get_metrics(&self, agent_id: &str) -> Result<Option<AgentMetrics>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT agent_id, tasks_completed, tasks_failed, total_runtime_seconds, average_runtime_seconds, success_rate, updated_at
                 FROM agent_metrics WHERE agent_id = ?",
            )?;
            let metrics = stmt
                .query_row([agent_id], |row| Self::row_to_metrics(row))
                .optional()
                .context("Failed to get AgentMetrics")?;
            Ok(metrics)
        })
    }

    /// Insert or replace agent metrics.
    pub fn upsert_metrics(&self, metrics: &AgentMetrics) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO agent_metrics (agent_id, tasks_completed, tasks_failed, total_runtime_seconds, average_runtime_seconds, success_rate, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(agent_id) DO UPDATE SET
                   tasks_completed = excluded.tasks_completed,
                   tasks_failed = excluded.tasks_failed,
                   total_runtime_seconds = excluded.total_runtime_seconds,
                   average_runtime_seconds = excluded.average_runtime_seconds,
                   success_rate = excluded.success_rate,
                   updated_at = excluded.updated_at",
                rusqlite::params![
                    metrics.agent_id,
                    metrics.tasks_completed,
                    metrics.tasks_failed,
                    metrics.total_runtime_seconds,
                    metrics.average_runtime_seconds,
                    metrics.success_rate,
                    metrics.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                ],
            )
            .context("Failed to upsert AgentMetrics")?;
            Ok(())
        })
    }

    /// Increment completed task count and update runtime stats.
    pub fn increment_completed(&self, agent_id: &str, runtime_secs: i64) -> Result<()> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            conn.execute(
                "UPDATE agent_metrics SET
                   tasks_completed = tasks_completed + 1,
                   total_runtime_seconds = total_runtime_seconds + ?1,
                   average_runtime_seconds = CAST((total_runtime_seconds + ?1) AS REAL) / (tasks_completed + 1),
                   success_rate = CAST((tasks_completed + 1) AS REAL) / (tasks_completed + 1 + tasks_failed),
                   updated_at = ?2
                 WHERE agent_id = ?3",
                rusqlite::params![runtime_secs, now, agent_id],
            )
            .context("Failed to increment completed")?;
            Ok(())
        })
    }

    /// Increment failed task count.
    pub fn increment_failed(&self, agent_id: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            conn.execute(
                "UPDATE agent_metrics SET
                   tasks_failed = tasks_failed + 1,
                   success_rate = CAST(tasks_completed AS REAL) / (tasks_completed + tasks_failed + 1),
                   updated_at = ?1
                 WHERE agent_id = ?2",
                rusqlite::params![now, agent_id],
            )
            .context("Failed to increment failed")?;
            Ok(())
        })
    }

    // ── Task History ────────────────────────────────────────────

    /// Add an entry to agent task history.
    pub fn add_history(&self, entry: &AgentTaskHistory) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO agent_task_history (id, agent_id, task_id, role, status, runtime_seconds, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    entry.id,
                    entry.agent_id,
                    entry.task_id,
                    entry.role,
                    entry.status,
                    entry.runtime_seconds,
                    entry.completed_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                ],
            )
            .context("Failed to add agent task history")?;
            Ok(())
        })
    }

    /// Get task history for an agent, ordered by most recent first.
    pub fn get_history(&self, agent_id: &str, limit: usize) -> Result<Vec<AgentTaskHistory>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, task_id, role, status, runtime_seconds, completed_at
                 FROM agent_task_history WHERE agent_id = ? ORDER BY completed_at DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![agent_id, limit as i64], |row| {
                Self::row_to_history(row)
            })?;
            let mut history = Vec::new();
            for row in rows {
                history.push(row?);
            }
            Ok(history)
        })
    }

    // ── Row Mappers ─────────────────────────────────────────────

    fn row_to_config(row: &Row) -> rusqlite::Result<SubAgentConfig> {
        let created_str: String = row.get(10)?;
        let updated_str: Option<String> = row.get(11)?;

        Ok(SubAgentConfig {
            id: row.get(0)?,
            name: row.get(1)?,
            persona: row.get(2)?,
            description: row.get(3)?,
            icon: row.get(4)?,
            status: row.get::<_, Option<String>>(5)?.unwrap_or_else(|| "idle".to_string()),
            current_task_id: row.get(6)?,
            skills_json: row.get::<_, Option<String>>(7)?.unwrap_or_else(|| "[]".to_string()),
            preset_json: row.get::<_, Option<String>>(8)?.unwrap_or_else(|| "{}".to_string()),
            constraints_json: row.get(9)?,
            created_at: parse_datetime(&created_str),
            updated_at: updated_str.as_deref().map(parse_datetime),
        })
    }

    fn row_to_metrics(row: &Row) -> rusqlite::Result<AgentMetrics> {
        let updated_str: String = row.get(6)?;

        Ok(AgentMetrics {
            agent_id: row.get(0)?,
            tasks_completed: row.get(1)?,
            tasks_failed: row.get(2)?,
            total_runtime_seconds: row.get(3)?,
            average_runtime_seconds: row.get(4)?,
            success_rate: row.get(5)?,
            updated_at: parse_datetime(&updated_str),
        })
    }

    fn row_to_history(row: &Row) -> rusqlite::Result<AgentTaskHistory> {
        let completed_str: String = row.get(6)?;

        Ok(AgentTaskHistory {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            task_id: row.get(2)?,
            role: row.get(3)?,
            status: row.get(4)?,
            runtime_seconds: row.get(5)?,
            completed_at: parse_datetime(&completed_str),
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
mod tests {
    use super::*;
    use crate::models::task::{Task, TaskStatus};
    use crate::repository::task::TaskRepository;
    use tempfile::tempdir;

    fn setup_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    fn make_config(id: &str, name: &str) -> SubAgentConfig {
        SubAgentConfig {
            id: id.to_string(),
            name: name.to_string(),
            description: Some("A test agent".to_string()),
            icon: None,
            status: "idle".to_string(),
            current_task_id: None,
            skills_json: r#"[{"name":"web_search","category":"research","proficiency":0.9}]"#
                .to_string(),
            preset_json: r#"{"persona":"test","temperature":0.5,"verbosity":"normal"}"#.to_string(),
            constraints_json: None,
            persona: Some("Test persona".to_string()),
            created_at: Utc::now(),
            updated_at: None,
        }
    }

    fn make_task(id: &str) -> Task {
        let now = Utc::now();
        Task {
            id: id.to_string(),
            title: "Test Task".to_string(),
            description: None,
            status: TaskStatus::Queued,
            priority: 0,
            progress_current: None,
            progress_total: None,
            result_summary: None,
            created_by: "test".to_string(),
            source_lane: "cli".to_string(),
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        let config = make_config("sa1", "Research Agent");
        repo.upsert(&config).unwrap();

        let fetched = repo.get("sa1").unwrap().unwrap();
        assert_eq!(fetched.id, "sa1");
        assert_eq!(fetched.name, "Research Agent");
        assert_eq!(fetched.status, "idle");
        assert_eq!(fetched.description.as_deref(), Some("A test agent"));
    }

    #[test]
    fn test_upsert_updates_existing() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        let mut config = make_config("sa1", "V1");
        repo.upsert(&config).unwrap();

        config.name = "V2".to_string();
        config.description = Some("Updated".to_string());
        repo.upsert(&config).unwrap();

        let fetched = repo.get("sa1").unwrap().unwrap();
        assert_eq!(fetched.name, "V2");
        assert_eq!(fetched.description.as_deref(), Some("Updated"));
    }

    #[test]
    fn test_get_nonexistent() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);
        assert!(repo.get("nope").unwrap().is_none());
    }

    #[test]
    fn test_list() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent 1")).unwrap();
        repo.upsert(&make_config("sa2", "Agent 2")).unwrap();

        let all = repo.list(10).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_by_status() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent 1")).unwrap();
        repo.upsert(&make_config("sa2", "Agent 2")).unwrap();
        repo.update_status("sa2", "busy", Some("task-1")).unwrap();

        let idle = repo.list_by_status("idle", 10).unwrap();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].id, "sa1");

        let busy = repo.list_by_status("busy", 10).unwrap();
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].id, "sa2");
    }

    #[test]
    fn test_update_status() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent")).unwrap();

        assert!(repo.update_status("sa1", "busy", Some("task-1")).unwrap());
        let fetched = repo.get("sa1").unwrap().unwrap();
        assert_eq!(fetched.status, "busy");
        assert_eq!(fetched.current_task_id.as_deref(), Some("task-1"));

        assert!(!repo.update_status("nope", "idle", None).unwrap());
    }

    #[test]
    fn test_delete() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent")).unwrap();
        repo.delete("sa1").unwrap();
        assert!(repo.get("sa1").unwrap().is_none());
    }

    #[test]
    fn test_metrics_upsert_and_get() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent")).unwrap();

        let metrics = AgentMetrics::new_empty("sa1");
        repo.upsert_metrics(&metrics).unwrap();

        let fetched = repo.get_metrics("sa1").unwrap().unwrap();
        assert_eq!(fetched.agent_id, "sa1");
        assert_eq!(fetched.tasks_completed, 0);
        assert_eq!(fetched.success_rate, 1.0);
    }

    #[test]
    fn test_metrics_increment_completed() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent")).unwrap();
        repo.upsert_metrics(&AgentMetrics::new_empty("sa1")).unwrap();

        repo.increment_completed("sa1", 120).unwrap();

        let fetched = repo.get_metrics("sa1").unwrap().unwrap();
        assert_eq!(fetched.tasks_completed, 1);
        assert_eq!(fetched.total_runtime_seconds, 120);
        assert_eq!(fetched.average_runtime_seconds, 120.0);
        assert_eq!(fetched.success_rate, 1.0);
    }

    #[test]
    fn test_metrics_increment_failed() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent")).unwrap();
        repo.upsert_metrics(&AgentMetrics::new_empty("sa1")).unwrap();

        // Complete one, fail one
        repo.increment_completed("sa1", 60).unwrap();
        repo.increment_failed("sa1").unwrap();

        let fetched = repo.get_metrics("sa1").unwrap().unwrap();
        assert_eq!(fetched.tasks_completed, 1);
        assert_eq!(fetched.tasks_failed, 1);
        assert_eq!(fetched.success_rate, 0.5);
    }

    #[test]
    fn test_history_add_and_get() {
        let db = setup_db();
        let repo = SubAgentRepository::new(&db);
        let task_repo = TaskRepository::new(&db);

        repo.upsert(&make_config("sa1", "Agent")).unwrap();
        task_repo.create(&make_task("t1")).unwrap();

        let entry = AgentTaskHistory {
            id: "h1".to_string(),
            agent_id: "sa1".to_string(),
            task_id: "t1".to_string(),
            role: "executor".to_string(),
            status: "completed".to_string(),
            runtime_seconds: Some(45),
            completed_at: Utc::now(),
        };
        repo.add_history(&entry).unwrap();

        let history = repo.get_history("sa1", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "executor");
        assert_eq!(history[0].runtime_seconds, Some(45));
    }
}
