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
            conn.execute(
                "INSERT INTO event_log (event_type, agent_id, detail, result) VALUES (?1, ?2, ?3, ?4)",
                (
                    event_type,
                    agent_id,
                    detail.map(|v| v.to_string()),
                    result.map(|v| v.to_string()),
                ),
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Log an event with string result (convenience method)
    #[deprecated(note = "Use log() with result parameter instead")]
    pub fn log_with_result(
        &self,
        event_type: &str,
        agent_id: Option<&str>,
        detail: Option<&serde_json::Value>,
        result: &str,
    ) -> Result<i64> {
        let result_json = serde_json::Value::String(result.to_string());
        self.log(event_type, agent_id, detail, Some(&result_json))
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

    /// Get events in a time range
    pub fn range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Vec<EventLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, event_type, detail, result 
                 FROM event_log WHERE timestamp >= ?1 AND timestamp <= ?2 ORDER BY timestamp DESC",
            )?;

            let mut events = Vec::new();
            let mut rows = stmt.query([from.to_rfc3339(), to.to_rfc3339()])?;

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

        let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
            .map(|dt| dt.with_timezone(&Utc))
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
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_event_log() {
        let db = test_db();
        let repo = EventLogRepository::new(&db);

        // Log event (now with 4 args)
        let id = repo.log("test_event", None, None, None).unwrap();
        assert!(id > 0);

        // Get recent
        let events = repo.recent(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "test_event");
    }

    #[test]
    fn test_event_log_result_roundtrip() {
        let db = test_db();
        let repo = EventLogRepository::new(&db);

        let detail = serde_json::json!({"key": "value"});
        let result = serde_json::json!({"ok": true, "code": 200});

        let _id = repo
            .log("test", Some("agent-1"), Some(&detail), Some(&result))
            .unwrap();

        let rows = repo.recent(1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail.as_ref().unwrap()["key"], "value");
        assert_eq!(rows[0].result.as_ref().unwrap()["ok"], true);
        assert_eq!(rows[0].result.as_ref().unwrap()["code"], 200);
    }
}
