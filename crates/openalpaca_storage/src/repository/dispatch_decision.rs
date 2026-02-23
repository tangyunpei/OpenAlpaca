//! Repository for dispatch decision history

use crate::Database;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A single dispatch decision record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchDecisionRecord {
    pub id: Option<i64>,
    pub request_id: String,
    pub task_id: Option<String>,
    pub mode: String,
    pub reason: String,
    pub agent_count: usize,
    pub dag_node_count: Option<usize>,
    pub predictability_score: Option<f64>,
    pub planner_requested_mode: Option<String>,
    pub error_message: Option<String>,
    pub timestamp: Option<String>,
}

/// Repository for dispatch decision operations.
pub struct DispatchDecisionRepository<'a> {
    db: &'a Database,
}

impl<'a> DispatchDecisionRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Record a dispatch decision. Returns the row ID for later task_id backfill.
    pub fn record(&self, record: &DispatchDecisionRecord) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO dispatch_decisions \
                 (request_id, task_id, mode, reason, agent_count, dag_node_count, predictability_score, planner_requested_mode, error_message) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    record.request_id,
                    record.task_id,
                    record.mode,
                    record.reason,
                    record.agent_count as i64,
                    record.dag_node_count.map(|v| v as i64),
                    record.predictability_score,
                    record.planner_requested_mode,
                    record.error_message,
                ],
            )
            .context("Failed to insert dispatch decision record")?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Backfill the task_id after task creation.
    pub fn update_task_id(&self, decision_id: i64, task_id: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE dispatch_decisions SET task_id = ?1 WHERE id = ?2",
                rusqlite::params![task_id, decision_id],
            )
            .context("Failed to update dispatch decision task_id")?;
            Ok(())
        })
    }

    /// Query dispatch decisions with optional filters.
    pub fn query(
        &self,
        mode: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DispatchDecisionRecord>> {
        self.db.with_connection(|conn| {
            let mut sql = String::from(
                "SELECT id, request_id, task_id, mode, reason, agent_count, dag_node_count, \
                 predictability_score, planner_requested_mode, error_message, timestamp \
                 FROM dispatch_decisions WHERE 1=1",
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            if let Some(m) = mode {
                sql.push_str(" AND mode = ?");
                params.push(Box::new(m.to_string()));
            }
            if let Some(f) = from {
                sql.push_str(" AND timestamp >= ?");
                params.push(Box::new(f.to_string()));
            }
            if let Some(t) = to {
                sql.push_str(" AND timestamp <= ?");
                params.push(Box::new(t.to_string()));
            }
            sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
            params.push(Box::new(limit as i64));

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(param_refs.as_slice(), |row| {
                    Ok(DispatchDecisionRecord {
                        id: row.get(0)?,
                        request_id: row.get(1)?,
                        task_id: row.get(2)?,
                        mode: row.get(3)?,
                        reason: row.get(4)?,
                        agent_count: row.get::<_, i64>(5)? as usize,
                        dag_node_count: row
                            .get::<_, Option<i64>>(6)?
                            .map(|v| v as usize),
                        predictability_score: row.get(7)?,
                        planner_requested_mode: row.get(8)?,
                        error_message: row.get(9)?,
                        timestamp: row.get(10)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Database {
        let dir = tempfile::tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_dispatch_decision_record_roundtrip() {
        let db = setup_db();
        let repo = DispatchDecisionRepository::new(&db);

        let id = repo
            .record(&DispatchDecisionRecord {
                id: None,
                request_id: "req-abc-123".to_string(),
                task_id: None,
                mode: "lead_agent".to_string(),
                reason: "planner_explicit".to_string(),
                agent_count: 3,
                dag_node_count: None,
                predictability_score: Some(0.85),
                planner_requested_mode: Some("lead_agent".to_string()),
                error_message: None,
                timestamp: None,
            })
            .unwrap();

        assert!(id > 0);

        let records = repo.query(None, None, None, 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request_id, "req-abc-123");
        assert!(records[0].task_id.is_none());
        assert_eq!(records[0].mode, "lead_agent");
        assert_eq!(records[0].reason, "planner_explicit");
        assert_eq!(records[0].agent_count, 3);
        assert!(records[0].dag_node_count.is_none());
        assert_eq!(records[0].predictability_score, Some(0.85));
        assert_eq!(
            records[0].planner_requested_mode.as_deref(),
            Some("lead_agent")
        );
    }

    #[test]
    fn test_dispatch_decision_update_task_id() {
        let db = setup_db();
        let repo = DispatchDecisionRepository::new(&db);

        let row_id = repo
            .record(&DispatchDecisionRecord {
                id: None,
                request_id: "req-backfill".to_string(),
                task_id: None,
                mode: "dag_parallel".to_string(),
                reason: "execution_mode_field".to_string(),
                agent_count: 4,
                dag_node_count: Some(4),
                predictability_score: Some(0.9),
                planner_requested_mode: Some("dag".to_string()),
                error_message: None,
                timestamp: None,
            })
            .unwrap();

        // Initially task_id is NULL
        let records = repo.query(None, None, None, 10).unwrap();
        assert!(records[0].task_id.is_none());

        // Backfill task_id
        repo.update_task_id(row_id, "task-real-uuid-456").unwrap();

        let records = repo.query(None, None, None, 10).unwrap();
        assert_eq!(records[0].task_id.as_deref(), Some("task-real-uuid-456"));
        assert_eq!(records[0].request_id, "req-backfill");
    }

    #[test]
    fn test_dispatch_decision_null_task_id_query() {
        let db = setup_db();
        let repo = DispatchDecisionRepository::new(&db);

        // Record with task_id
        repo.record(&DispatchDecisionRecord {
            id: None,
            request_id: "req-1".to_string(),
            task_id: Some("task-1".to_string()),
            mode: "lead_agent".to_string(),
            reason: "heuristic_fallback".to_string(),
            agent_count: 1,
            dag_node_count: None,
            predictability_score: None,
            planner_requested_mode: None,
            error_message: None,
            timestamp: None,
        })
        .unwrap();

        // Record without task_id
        repo.record(&DispatchDecisionRecord {
            id: None,
            request_id: "req-2".to_string(),
            task_id: None,
            mode: "lead_agent".to_string(),
            reason: "heuristic_fallback".to_string(),
            agent_count: 0,
            dag_node_count: None,
            predictability_score: None,
            planner_requested_mode: None,
            error_message: None,
            timestamp: None,
        })
        .unwrap();

        let records = repo.query(None, None, None, 10).unwrap();
        assert_eq!(records.len(), 2);
        // One has task_id, one doesn't
        let with_task: Vec<_> = records.iter().filter(|r| r.task_id.is_some()).collect();
        let without_task: Vec<_> = records.iter().filter(|r| r.task_id.is_none()).collect();
        assert_eq!(with_task.len(), 1);
        assert_eq!(without_task.len(), 1);
    }

    #[test]
    fn test_dispatch_decision_mode_filter() {
        let db = setup_db();
        let repo = DispatchDecisionRepository::new(&db);

        for (mode, count) in [("lead_agent", 3), ("dag_parallel", 5), ("sequential_pipeline", 2)]
        {
            for i in 0..count {
                repo.record(&DispatchDecisionRecord {
                    id: None,
                    request_id: format!("req-{}-{}", mode, i),
                    task_id: None,
                    mode: mode.to_string(),
                    reason: "test".to_string(),
                    agent_count: 1,
                    dag_node_count: None,
                    predictability_score: None,
                    planner_requested_mode: None,
                    error_message: None,
                    timestamp: None,
                })
                .unwrap();
            }
        }

        // Filter by mode
        let dag_only = repo.query(Some("dag_parallel"), None, None, 100).unwrap();
        assert_eq!(dag_only.len(), 5);
        assert!(dag_only.iter().all(|r| r.mode == "dag_parallel"));

        // All modes
        let all = repo.query(None, None, None, 100).unwrap();
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn test_dispatch_decision_error_message_roundtrip() {
        let db = setup_db();
        let repo = DispatchDecisionRepository::new(&db);

        repo.record(&DispatchDecisionRecord {
            id: None,
            request_id: "req-err".to_string(),
            task_id: None,
            mode: "sequential_pipeline".to_string(),
            reason: "heuristic_match_failed".to_string(),
            agent_count: 0,
            dag_node_count: None,
            predictability_score: None,
            planner_requested_mode: None,
            error_message: Some("No agents match the required skills".to_string()),
            timestamp: None,
        })
        .unwrap();

        let records = repo.query(None, None, None, 1).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].reason, "heuristic_match_failed");
        assert_eq!(
            records[0].error_message.as_deref(),
            Some("No agents match the required skills")
        );

        // Also insert one without error_message and verify it's None
        repo.record(&DispatchDecisionRecord {
            id: None,
            request_id: "req-ok".to_string(),
            task_id: None,
            mode: "lead_agent".to_string(),
            reason: "planner_explicit".to_string(),
            agent_count: 1,
            dag_node_count: None,
            predictability_score: None,
            planner_requested_mode: None,
            error_message: None,
            timestamp: None,
        })
        .unwrap();

        let all = repo.query(None, None, None, 10).unwrap();
        assert_eq!(all.len(), 2);
        let ok_record = all.iter().find(|r| r.request_id == "req-ok").unwrap();
        assert!(ok_record.error_message.is_none());
    }

    #[test]
    fn test_migration_025_creates_error_message_schema() {
        let db = setup_db();
        assert_eq!(db.schema_version().unwrap(), 26);

        // Verify request_id column works
        let repo = DispatchDecisionRepository::new(&db);
        repo.record(&DispatchDecisionRecord {
            id: None,
            request_id: "migration-024-test".to_string(),
            task_id: Some("task-test".to_string()),
            mode: "lead_agent".to_string(),
            reason: "test".to_string(),
            agent_count: 1,
            dag_node_count: Some(4),
            predictability_score: Some(0.5),
            planner_requested_mode: None,
            error_message: None,
            timestamp: None,
        })
        .unwrap();

        let records = repo.query(None, None, None, 1).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request_id, "migration-024-test");
        assert_eq!(records[0].task_id.as_deref(), Some("task-test"));
    }
}
