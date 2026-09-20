//! EventLog repository - Audit logging operations

use crate::Database;
use crate::models::EventLog;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

/// The columns `row_to_event` reads, in the order it reads them.
const COLUMNS: &str = "id, timestamp, agent_id, task_id, event_type, detail, result";

/// One page of the event log (GAP-10).
///
/// Every field is a filter except `before`/`limit`, which are the page window.
/// Ordering and paging are on the autoincrement `id`, never on `timestamp`:
/// the column holds two spellings (RFC 3339 for rows this repository wrote,
/// SQLite's space-separated `datetime('now')` for older ones), so it neither
/// sorts nor slices reliably.
#[derive(Debug, Default, Clone)]
pub struct EventLogQuery<'a> {
    /// The run whose log this is. Matches the indexed column only — a row that
    /// carries the id solely inside `detail` is not this run's row.
    pub task_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    /// Exact match on `event_type`; never a prefix or a LIKE.
    pub event_type: Option<&'a str>,
    /// Exclusive upper bound on `id` — the previous page's `next_before`.
    pub before: Option<i64>,
    /// Row cap. The route clamps it before calling; the repository takes it.
    pub limit: usize,
}

/// Repository for EventLog operations
pub struct EventLogRepository<'a> {
    db: &'a Database,
}

impl<'a> EventLogRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Log an event with optional result.
    ///
    /// Leaves `task_id` NULL — this is the writer for events that belong to no
    /// run. Anything that happened *inside* a run goes through
    /// [`Self::log_for_task`] instead, so `?task_id=` can find it.
    pub fn log(
        &self,
        event_type: &str,
        agent_id: Option<&str>,
        detail: Option<&serde_json::Value>,
        result: Option<&serde_json::Value>,
    ) -> Result<i64> {
        self.log_for_task(event_type, agent_id, None, detail, result)
    }

    /// Log an event against the run it belongs to (GAP-10).
    ///
    /// `task_id` lands in the indexed column *as well as* wherever the caller
    /// already put it in `detail`: the blob is what every reader written
    /// before migration 037 looks at, and dropping it there would break them
    /// for nothing.
    pub fn log_for_task(
        &self,
        event_type: &str,
        agent_id: Option<&str>,
        task_id: Option<&str>,
        detail: Option<&serde_json::Value>,
        result: Option<&serde_json::Value>,
    ) -> Result<i64> {
        self.db.with_connection(|conn| {
            // Store an explicit RFC3339 timestamp so it round-trips through
            // `row_to_event` (which parses RFC3339) — the column DEFAULT
            // `datetime('now')` produces a space-separated form that fails
            // to parse as RFC3339.
            conn.execute(
                "INSERT INTO event_log (event_type, agent_id, task_id, detail, result, timestamp) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (
                    event_type,
                    agent_id,
                    task_id,
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
        self.query(&EventLogQuery {
            limit,
            ..Default::default()
        })
    }

    /// Get events by agent
    pub fn by_agent(&self, agent_id: &str, limit: usize) -> Result<Vec<EventLog>> {
        self.query(&EventLogQuery {
            agent_id: Some(agent_id),
            limit,
            ..Default::default()
        })
    }

    /// One page of the log, newest first (GAP-10).
    ///
    /// Filters compose with AND; an absent filter constrains nothing.
    pub fn query(&self, query: &EventLogQuery<'_>) -> Result<Vec<EventLog>> {
        let mut sql = format!("SELECT {COLUMNS} FROM event_log");
        let mut clauses: Vec<&str> = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(task_id) = query.task_id {
            clauses.push("task_id = ?");
            params.push(Box::new(task_id.to_string()));
        }
        if let Some(agent_id) = query.agent_id {
            clauses.push("agent_id = ?");
            params.push(Box::new(agent_id.to_string()));
        }
        if let Some(event_type) = query.event_type {
            clauses.push("event_type = ?");
            params.push(Box::new(event_type.to_string()));
        }
        if let Some(before) = query.before {
            clauses.push("id < ?");
            params.push(Box::new(before));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        params.push(Box::new(query.limit as i64));

        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut events = Vec::new();
            let mut rows = stmt.query(rusqlite::params_from_iter(
                params.iter().map(|p| p.as_ref()),
            ))?;
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
        let task_id: Option<String> = row.get(3)?;
        let event_type: String = row.get(4)?;
        let detail_str: Option<String> = row.get(5)?;
        let result_str: Option<String> = row.get(6)?;

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
            task_id,
            event_type,
            detail,
            result,
        })
    }
}

#[cfg(test)]
mod tests;
