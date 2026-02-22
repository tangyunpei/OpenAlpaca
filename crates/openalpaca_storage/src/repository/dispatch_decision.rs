//! Repository for dispatch decision history

use crate::Database;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A single dispatch decision record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchDecisionRecord {
    pub id: Option<i64>,
    pub task_id: String,
    pub mode: String,
    pub reason: String,
    pub agent_count: usize,
    pub dag_node_count: Option<usize>,
    pub predictability_score: Option<f64>,
    pub planner_requested_mode: Option<String>,
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

    /// Record a dispatch decision.
    pub fn record(&self, record: &DispatchDecisionRecord) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO dispatch_decisions \
                 (task_id, mode, reason, agent_count, dag_node_count, predictability_score, planner_requested_mode) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    record.task_id,
                    record.mode,
                    record.reason,
                    record.agent_count as i64,
                    record.dag_node_count.map(|v| v as i64),
                    record.predictability_score,
                    record.planner_requested_mode,
                ],
            )
            .context("Failed to insert dispatch decision record")?;
            Ok(conn.last_insert_rowid())
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
                "SELECT id, task_id, mode, reason, agent_count, dag_node_count, \
                 predictability_score, planner_requested_mode, timestamp \
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
                        task_id: row.get(1)?,
                        mode: row.get(2)?,
                        reason: row.get(3)?,
                        agent_count: row.get::<_, i64>(4)? as usize,
                        dag_node_count: row
                            .get::<_, Option<i64>>(5)?
                            .map(|v| v as usize),
                        predictability_score: row.get(6)?,
                        planner_requested_mode: row.get(7)?,
                        timestamp: row.get(8)?,
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
                task_id: "task-1".to_string(),
                mode: "lead_agent".to_string(),
                reason: "planner_explicit".to_string(),
                agent_count: 3,
                dag_node_count: None,
                predictability_score: Some(0.85),
                planner_requested_mode: Some("lead_agent".to_string()),
                timestamp: None,
            })
            .unwrap();

        assert!(id > 0);

        let records = repo.query(None, None, None, 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].task_id, "task-1");
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
    fn test_dispatch_decision_mode_filter() {
        let db = setup_db();
        let repo = DispatchDecisionRepository::new(&db);

        for (mode, count) in [("lead_agent", 3), ("dag_parallel", 5), ("sequential_pipeline", 2)]
        {
            for i in 0..count {
                repo.record(&DispatchDecisionRecord {
                    id: None,
                    task_id: format!("task-{}-{}", mode, i),
                    mode: mode.to_string(),
                    reason: "test".to_string(),
                    agent_count: 1,
                    dag_node_count: None,
                    predictability_score: None,
                    planner_requested_mode: None,
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
    fn test_migration_023_creates_table_and_indexes() {
        let db = setup_db();
        assert_eq!(db.schema_version().unwrap(), 23);

        // Verify table exists by inserting + querying
        let repo = DispatchDecisionRepository::new(&db);
        repo.record(&DispatchDecisionRecord {
            id: None,
            task_id: "migration-test".to_string(),
            mode: "lead_agent".to_string(),
            reason: "test".to_string(),
            agent_count: 1,
            dag_node_count: Some(4),
            predictability_score: Some(0.5),
            planner_requested_mode: None,
            timestamp: None,
        })
        .unwrap();

        let records = repo.query(None, None, None, 1).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].dag_node_count, Some(4));
    }
}
