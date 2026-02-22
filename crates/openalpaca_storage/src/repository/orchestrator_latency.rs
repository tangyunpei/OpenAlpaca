//! Repository for orchestrator latency metrics

use crate::Database;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A single orchestrator latency record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorLatencyRecord {
    pub id: Option<i64>,
    pub request_id: String,
    pub mode: String,
    pub planner_ms: u64,
    pub dispatch_ms: u64,
    pub ack_ms: u64,
    pub fallback_reason: Option<String>,
    pub auto_promotion_reason: Option<String>,
    pub timestamp: Option<String>,
}

/// Repository for orchestrator latency operations.
pub struct OrchestratorLatencyRepository<'a> {
    db: &'a Database,
}

impl<'a> OrchestratorLatencyRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Record a latency measurement.
    pub fn record(&self, record: &OrchestratorLatencyRecord) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO orchestrator_latency \
                 (request_id, mode, planner_ms, dispatch_ms, ack_ms, fallback_reason, auto_promotion_reason) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    record.request_id,
                    record.mode,
                    record.planner_ms as i64,
                    record.dispatch_ms as i64,
                    record.ack_ms as i64,
                    record.fallback_reason,
                    record.auto_promotion_reason,
                ],
            )
            .context("Failed to insert orchestrator latency record")?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Query records filtered by mode and time range.
    pub fn query_by_mode(
        &self,
        mode: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        limit: usize,
    ) -> Result<Vec<OrchestratorLatencyRecord>> {
        self.db.with_connection(|conn| {
            let mut sql = String::from(
                "SELECT id, request_id, mode, planner_ms, dispatch_ms, ack_ms, \
                 fallback_reason, auto_promotion_reason, timestamp \
                 FROM orchestrator_latency WHERE 1=1",
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
                    Ok(OrchestratorLatencyRecord {
                        id: row.get(0)?,
                        request_id: row.get(1)?,
                        mode: row.get(2)?,
                        planner_ms: row.get::<_, i64>(3)? as u64,
                        dispatch_ms: row.get::<_, i64>(4)? as u64,
                        ack_ms: row.get::<_, i64>(5)? as u64,
                        fallback_reason: row.get(6)?,
                        auto_promotion_reason: row.get(7)?,
                        timestamp: row.get(8)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Query all records in a time range for aggregate computation.
    pub fn aggregate(
        &self,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<OrchestratorLatencyRecord>> {
        self.query_by_mode(None, from, to, 10_000)
    }
}
