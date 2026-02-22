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

    /// Compute aggregated latency statistics grouped by mode.
    pub fn aggregate_by_mode(
        &self,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<LatencyAggregate>> {
        let records = self.aggregate(from, to)?;
        if records.is_empty() {
            return Ok(vec![]);
        }

        // Group by mode
        let mut groups: std::collections::HashMap<String, Vec<&OrchestratorLatencyRecord>> =
            std::collections::HashMap::new();
        for r in &records {
            groups.entry(r.mode.clone()).or_default().push(r);
        }

        let mut aggregates = Vec::with_capacity(groups.len());
        for (mode, group) in &groups {
            let count = group.len();

            // Compute total_ms for percentiles
            let mut totals: Vec<u64> = group
                .iter()
                .map(|r| r.planner_ms + r.dispatch_ms + r.ack_ms)
                .collect();
            totals.sort_unstable();

            let planner_sum: f64 = group.iter().map(|r| r.planner_ms as f64).sum();
            let dispatch_sum: f64 = group.iter().map(|r| r.dispatch_ms as f64).sum();
            let ack_sum: f64 = group.iter().map(|r| r.ack_ms as f64).sum();

            let auto_promotion_count = group
                .iter()
                .filter(|r| r.auto_promotion_reason.is_some())
                .count();
            let fallback_count = group.iter().filter(|r| r.fallback_reason.is_some()).count();

            aggregates.push(LatencyAggregate {
                mode: mode.clone(),
                count,
                p50_total_ms: percentile(&totals, 50),
                p95_total_ms: percentile(&totals, 95),
                p99_total_ms: percentile(&totals, 99),
                mean_planner_ms: planner_sum / count as f64,
                mean_dispatch_ms: dispatch_sum / count as f64,
                mean_ack_ms: ack_sum / count as f64,
                auto_promotion_count,
                fallback_count,
            });
        }

        // Sort by mode name for stable output
        aggregates.sort_by(|a, b| a.mode.cmp(&b.mode));
        Ok(aggregates)
    }
}

/// Aggregated latency statistics for a single dispatch mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyAggregate {
    pub mode: String,
    pub count: usize,
    pub p50_total_ms: u64,
    pub p95_total_ms: u64,
    pub p99_total_ms: u64,
    pub mean_planner_ms: f64,
    pub mean_dispatch_ms: f64,
    pub mean_ack_ms: f64,
    pub auto_promotion_count: usize,
    pub fallback_count: usize,
}

/// Compute the p-th percentile from a sorted slice using nearest-rank method.
fn percentile(sorted: &[u64], pct: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (pct as f64 / 100.0 * sorted.len() as f64).ceil() as usize;
    let idx = idx.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> crate::Database {
        let dir = tempfile::tempdir().unwrap();
        crate::Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_percentile_single_record() {
        assert_eq!(percentile(&[42], 50), 42);
        assert_eq!(percentile(&[42], 95), 42);
        assert_eq!(percentile(&[42], 99), 42);
    }

    #[test]
    fn test_percentile_empty() {
        assert_eq!(percentile(&[], 50), 0);
    }

    #[test]
    fn test_percentile_multiple() {
        // 10 values: 1..=10
        let sorted: Vec<u64> = (1..=10).collect();
        assert_eq!(percentile(&sorted, 50), 5);
        assert_eq!(percentile(&sorted, 95), 10);
        assert_eq!(percentile(&sorted, 99), 10);
    }

    #[test]
    fn test_percentile_100_values() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&sorted, 50), 50);
        assert_eq!(percentile(&sorted, 95), 95);
        assert_eq!(percentile(&sorted, 99), 99);
    }

    #[test]
    fn test_aggregate_empty() {
        let db = setup_db();
        let repo = OrchestratorLatencyRepository::new(&db);
        let result = repo.aggregate_by_mode(None, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_aggregate_single_mode() {
        let db = setup_db();
        let repo = OrchestratorLatencyRepository::new(&db);

        for i in 0..10 {
            repo.record(&OrchestratorLatencyRecord {
                id: None,
                request_id: format!("req-{}", i),
                mode: "fast_path".to_string(),
                planner_ms: 10 + i,
                dispatch_ms: 5,
                ack_ms: 2,
                fallback_reason: None,
                auto_promotion_reason: None,
                timestamp: None,
            })
            .unwrap();
        }

        let aggs = repo.aggregate_by_mode(None, None).unwrap();
        assert_eq!(aggs.len(), 1);
        assert_eq!(aggs[0].mode, "fast_path");
        assert_eq!(aggs[0].count, 10);
        assert!(aggs[0].p50_total_ms > 0);
        assert!(aggs[0].p95_total_ms >= aggs[0].p50_total_ms);
        assert!(aggs[0].mean_planner_ms > 0.0);
    }

    #[test]
    fn test_aggregate_multiple_modes() {
        let db = setup_db();
        let repo = OrchestratorLatencyRepository::new(&db);

        for mode in &["fast_path", "planner_complex_task", "heuristic_simple_query"] {
            for i in 0..5 {
                repo.record(&OrchestratorLatencyRecord {
                    id: None,
                    request_id: format!("req-{}-{}", mode, i),
                    mode: mode.to_string(),
                    planner_ms: 10,
                    dispatch_ms: 5,
                    ack_ms: 2,
                    fallback_reason: if i == 0 { Some("test".to_string()) } else { None },
                    auto_promotion_reason: if i == 1 {
                        Some("auto".to_string())
                    } else {
                        None
                    },
                    timestamp: None,
                })
                .unwrap();
            }
        }

        let aggs = repo.aggregate_by_mode(None, None).unwrap();
        assert_eq!(aggs.len(), 3);
        // Sorted by mode name
        assert_eq!(aggs[0].mode, "fast_path");
        assert_eq!(aggs[1].mode, "heuristic_simple_query");
        assert_eq!(aggs[2].mode, "planner_complex_task");
        // Each has 5 records
        for agg in &aggs {
            assert_eq!(agg.count, 5);
            assert_eq!(agg.fallback_count, 1);
            assert_eq!(agg.auto_promotion_count, 1);
        }
    }
}
