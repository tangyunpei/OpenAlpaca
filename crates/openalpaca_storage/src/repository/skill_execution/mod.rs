//! Repository for skill and tool execution telemetry

use crate::models::skill_execution::{SkillExecutionEntry, ToolExecutionEntry};
use crate::models::skill_health::SkillHealthMetrics;
use crate::Database;
use anyhow::{Context, Result};
use chrono::{NaiveDateTime, Utc};

/// Repository for skill/tool execution log operations.
pub struct SkillExecutionRepository<'a> {
    db: &'a Database,
}

impl<'a> SkillExecutionRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Record a skill execution entry. Returns the row id.
    pub fn record(&self, entry: &SkillExecutionEntry) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO skill_execution_log (
                    request_id, skill_id, agent_id, status, finish_reason,
                    error_message, validation_failures, duration_ms,
                    rounds_used, tool_calls_made, input_tokens, output_tokens,
                    cost_usd, model_used, query_preview, route_score,
                    was_auto_selected, repair_attempted, repair_succeeded
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                rusqlite::params![
                    entry.request_id,
                    entry.skill_id,
                    entry.agent_id,
                    entry.status,
                    entry.finish_reason,
                    entry.error_message,
                    entry.validation_failures,
                    entry.duration_ms,
                    entry.rounds_used,
                    entry.tool_calls_made,
                    entry.input_tokens,
                    entry.output_tokens,
                    entry.cost_usd,
                    entry.model_used,
                    entry.query_preview,
                    entry.route_score,
                    entry.was_auto_selected as i32,
                    entry.repair_attempted as i32,
                    entry.repair_succeeded as i32,
                ],
            )
            .context("Failed to insert skill execution log")?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Record a tool execution entry. Returns the row id.
    pub fn record_tool(&self, entry: &ToolExecutionEntry) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO tool_execution_log (
                    request_id, agent_id, tool_name, success, duration_ms, error_message
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    entry.request_id,
                    entry.agent_id,
                    entry.tool_name,
                    entry.success as i32,
                    entry.duration_ms,
                    entry.error_message,
                ],
            )
            .context("Failed to insert tool execution log")?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Get recent skill execution entries for a given skill.
    pub fn get_by_skill(&self, skill_id: &str, limit: usize) -> Result<Vec<SkillExecutionEntry>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, request_id, skill_id, agent_id, status, finish_reason,
                        error_message, validation_failures, duration_ms,
                        rounds_used, tool_calls_made, input_tokens, output_tokens,
                        cost_usd, model_used, query_preview, route_score,
                        was_auto_selected, repair_attempted, repair_succeeded, timestamp
                 FROM skill_execution_log
                 WHERE skill_id = ?
                 ORDER BY timestamp DESC
                 LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![skill_id, limit as i64], |row| {
                let ts_str: Option<String> = row.get(20)?;
                Ok(SkillExecutionEntry {
                    id: row.get(0)?,
                    request_id: row.get(1)?,
                    skill_id: row.get(2)?,
                    agent_id: row.get(3)?,
                    status: row.get(4)?,
                    finish_reason: row.get(5)?,
                    error_message: row.get(6)?,
                    validation_failures: row.get(7)?,
                    duration_ms: row.get(8)?,
                    rounds_used: row.get(9)?,
                    tool_calls_made: row.get(10)?,
                    input_tokens: row.get(11)?,
                    output_tokens: row.get(12)?,
                    cost_usd: row.get(13)?,
                    model_used: row.get(14)?,
                    query_preview: row.get(15)?,
                    route_score: row.get(16)?,
                    was_auto_selected: row.get::<_, i32>(17)? != 0,
                    repair_attempted: row.get::<_, i32>(18)? != 0,
                    repair_succeeded: row.get::<_, i32>(19)? != 0,
                    timestamp: ts_str.and_then(|s| parse_datetime(&s)),
                })
            })?;
            let mut entries = Vec::new();
            for row in rows {
                entries.push(row?);
            }
            Ok(entries)
        })
    }

    /// Get health metrics for a specific skill.
    pub fn skill_health(&self, skill_id: &str) -> Result<Option<SkillHealthMetrics>> {
        let all = self.all_skill_health()?;
        Ok(all.into_iter().find(|m| m.skill_id == skill_id))
    }

    /// Get health metrics for all skills, enriched with user feedback data.
    pub fn all_skill_health(&self) -> Result<Vec<SkillHealthMetrics>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT
                    sel.skill_id,
                    COUNT(*) AS total,
                    CAST(SUM(CASE WHEN sel.finish_reason = 'complete' AND sel.repair_attempted = 0
                         THEN 1 ELSE 0 END) AS REAL) / COUNT(*) AS clean_success_rate,
                    CAST(SUM(CASE WHEN sel.finish_reason = 'complete' AND sel.repair_attempted = 0
                         AND sel.timestamp >= datetime('now', '-7 days') THEN 1 ELSE 0 END) AS REAL)
                    / NULLIF(SUM(CASE WHEN sel.timestamp >= datetime('now', '-7 days') THEN 1 ELSE 0 END), 0)
                        AS clean_success_rate_7d,
                    CAST(SUM(sel.repair_attempted) AS REAL) / COUNT(*) AS repair_rate,
                    CASE WHEN SUM(sel.repair_attempted) > 0
                         THEN CAST(SUM(sel.repair_succeeded) AS REAL) / SUM(sel.repair_attempted)
                         ELSE 0.0 END AS repair_effectiveness,
                    CAST(SUM(CASE WHEN sel.finish_reason IN ('max_rounds', 'truncated')
                         THEN 1 ELSE 0 END) AS REAL) / COUNT(*) AS degraded_rate,
                    AVG(sel.duration_ms) AS avg_duration_ms,
                    AVG(sel.cost_usd) AS avg_cost_usd,
                    AVG(sel.rounds_used) AS avg_rounds,
                    MAX(sel.timestamp) AS last_invoked_at,
                    fb.fb_count,
                    fb.satisfaction,
                    fb.coverage
                FROM skill_execution_log sel
                LEFT JOIN (
                    SELECT sel2.skill_id,
                        COUNT(mf.id) AS fb_count,
                        CAST(SUM(CASE WHEN mf.feedback = 'positive' THEN 1 ELSE 0 END) AS REAL)
                            / NULLIF(COUNT(mf.id), 0) AS satisfaction,
                        CAST(COUNT(mf.id) AS REAL) / COUNT(sel2.id) AS coverage
                    FROM skill_execution_log sel2
                    LEFT JOIN message_feedback mf ON mf.message_id = sel2.response_message_id
                    GROUP BY sel2.skill_id
                ) fb ON fb.skill_id = sel.skill_id
                GROUP BY sel.skill_id
                HAVING COUNT(*) >= 1",
            )?;
            let rows = stmt.query_map([], |row| {
                let clean_success_rate: f64 = row.get(2)?;
                let clean_success_rate_7d: Option<f64> = row.get(3)?;
                let last_ts: Option<String> = row.get(10)?;
                let fb_count: Option<i64> = row.get(11)?;
                let satisfaction: Option<f64> = row.get(12)?;
                let coverage: Option<f64> = row.get(13)?;
                Ok(SkillHealthMetrics {
                    skill_id: row.get(0)?,
                    total_invocations: row.get::<_, i64>(1)? as u64,
                    clean_success_rate,
                    clean_success_rate_7d: clean_success_rate_7d.unwrap_or(clean_success_rate),
                    repair_rate: row.get(4)?,
                    repair_effectiveness: row.get(5)?,
                    degraded_rate: row.get(6)?,
                    avg_duration_ms: row.get(7)?,
                    avg_cost_usd: row.get(8)?,
                    avg_rounds: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
                    last_invoked_at: last_ts.and_then(|s| parse_datetime(&s)),
                    user_satisfaction_rate: satisfaction,
                    feedback_count: fb_count.unwrap_or(0) as u64,
                    feedback_coverage: coverage.unwrap_or(0.0),
                })
            })?;
            let mut metrics = Vec::new();
            for row in rows {
                metrics.push(row?);
            }
            Ok(metrics)
        })
    }

    /// Link a skill execution to its response message (for feedback correlation).
    pub fn link_response(&self, request_id: &str, message_id: i64) -> Result<bool> {
        self.db.with_connection(|conn| {
            let changed = conn.execute(
                "UPDATE skill_execution_log SET response_message_id = ?1 WHERE request_id = ?2",
                rusqlite::params![message_id, request_id],
            )?;
            Ok(changed > 0)
        })
    }

    /// Delete old telemetry rows. Returns (skill_rows_deleted, tool_rows_deleted).
    pub fn cleanup_old(&self, skill_days: u32, tool_days: u32) -> Result<(usize, usize)> {
        self.db.with_connection(|conn| {
            let skill_deleted = conn.execute(
                "DELETE FROM skill_execution_log WHERE timestamp < datetime('now', ?)",
                rusqlite::params![format!("-{skill_days} days")],
            )?;
            let tool_deleted = conn.execute(
                "DELETE FROM tool_execution_log WHERE timestamp < datetime('now', ?)",
                rusqlite::params![format!("-{tool_days} days")],
            )?;
            Ok((skill_deleted, tool_deleted))
        })
    }
}

fn parse_datetime(s: &str) -> Option<chrono::DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|ndt| ndt.and_utc())
}

#[cfg(test)]
mod tests;
