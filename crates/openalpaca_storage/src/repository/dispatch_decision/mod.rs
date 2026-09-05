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
                 (request_id, task_id, mode, reason, agent_count, dag_node_count, predictability_score, error_message) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    record.request_id,
                    record.task_id,
                    record.mode,
                    record.reason,
                    record.agent_count as i64,
                    record.dag_node_count.map(|v| v as i64),
                    record.predictability_score,
                    record.error_message,
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
                "SELECT id, request_id, task_id, mode, reason, agent_count, dag_node_count, \
                 predictability_score, error_message, timestamp \
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
                        dag_node_count: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                        predictability_score: row.get(7)?,
                        error_message: row.get(8)?,
                        timestamp: row.get(9)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}

#[cfg(test)]
mod tests;
