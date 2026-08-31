//! EventLog repository - Audit logging operations

use crate::Database;
use crate::models::EventLog;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

/// Repository for EventLog operations
pub struct EventLogRepository<'a> {
    db: &'a Database,
}

impl<'a> EventLogRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Log an event with optional result
    pub fn log(
        &self,
        event_type: &str,
        agent_id: Option<&str>,
        detail: Option<&serde_json::Value>,
        result: Option<&serde_json::Value>,
    ) -> Result<i64> {
        self.db.with_connection(|conn| {
            // Store an explicit RFC3339 timestamp so it round-trips through
            // `row_to_event` (which parses RFC3339) — the column DEFAULT
            // `datetime('now')` produces a space-separated form that fails
            // to parse as RFC3339.
            conn.execute(
                "INSERT INTO event_log (event_type, agent_id, detail, result, timestamp) VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    event_type,
                    agent_id,
                    detail.map(|v| v.to_string()),
                    result.map(|v| v.to_string()),
                    Utc::now().to_rfc3339(),
                ),
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Get recent events
    pub fn recent(&self, limit: usize) -> Result<Vec<EventLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, event_type, detail, result 
                 FROM event_log ORDER BY timestamp DESC LIMIT ?1",
            )?;

            let mut events = Vec::new();
            let mut rows = stmt.query([limit as i64])?;

            while let Some(row) = rows.next()? {
                events.push(Self::row_to_event(row)?);
            }

            Ok(events)
        })
    }

    /// Get events by agent
    pub fn by_agent(&self, agent_id: &str, limit: usize) -> Result<Vec<EventLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, event_type, detail, result 
                 FROM event_log WHERE agent_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
            )?;

            let mut events = Vec::new();
            let mut rows = stmt.query([agent_id, &limit.to_string()])?;

            while let Some(row) = rows.next()? {
                events.push(Self::row_to_event(row)?);
            }

            Ok(events)
        })
    }

    fn row_to_event(row: &rusqlite::Row<'_>) -> Result<EventLog> {
        let id: i64 = row.get(0)?;
        let timestamp_str: String = row.get(1)?;
        let agent_id: Option<String> = row.get(2)?;
        let event_type: String = row.get(3)?;
        let detail_str: Option<String> = row.get(4)?;
        let result_str: Option<String> = row.get(5)?;

        let detail = detail_str
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .context("Failed to parse event detail JSON")?;

        // Accept RFC3339 (new rows) and the legacy SQLite `datetime('now')`
        // format (existing rows) before giving up to read-time.
        let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(&timestamp_str, "%Y-%m-%d %H:%M:%S")
                    .map(|ndt| ndt.and_utc())
            })
            .unwrap_or_else(|_| Utc::now());

        let result = result_str
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .context("Failed to parse event result JSON")?;

        Ok(EventLog {
            id,
            timestamp,
            agent_id,
            event_type,
            detail,
            result,
        })
    }
}

#[cfg(test)]
mod tests;
