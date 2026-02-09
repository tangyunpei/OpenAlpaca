//! Repository for LLM call logging and usage tracking

use crate::Database;
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::Row;
use serde::{Deserialize, Serialize};

/// A single LLM API call log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCallLog {
    pub id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub key_id: Option<String>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_usd: f64,
    pub status: String,
    pub latency_ms: Option<i64>,
    pub error_message: Option<String>,
}

/// Daily usage aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmUsageDaily {
    pub date: String,
    pub agent_id: String,
    pub model: String,
    pub total_requests: i32,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_usd: f64,
}

/// Repository for LLM usage operations.
pub struct LlmUsageRepository<'a> {
    db: &'a Database,
}

impl<'a> LlmUsageRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Insert a call log entry.
    pub fn insert_call_log(&self, log: &LlmCallLog) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO llm_call_log (timestamp, agent_id, task_id, provider, model, key_id, input_tokens, output_tokens, cost_usd, status, latency_ms, error_message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    log.timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
                    log.agent_id,
                    log.task_id,
                    log.provider,
                    log.model,
                    log.key_id,
                    log.input_tokens,
                    log.output_tokens,
                    log.cost_usd,
                    log.status,
                    log.latency_ms,
                    log.error_message,
                ],
            )
            .context("Failed to insert LLM call log")?;
            let id = conn.last_insert_rowid();
            Ok(id)
        })
    }

    /// Get usage for a specific agent (aggregated from call log).
    pub fn get_agent_usage(&self, agent_id: &str, limit: usize) -> Result<Vec<LlmCallLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, task_id, provider, model, key_id, input_tokens, output_tokens, cost_usd, status, latency_ms, error_message
                 FROM llm_call_log WHERE agent_id = ? ORDER BY timestamp DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![agent_id, limit as i64], |row| {
                Self::row_to_call_log(row)
            })?;
            let mut logs = Vec::new();
            for row in rows {
                logs.push(row?);
            }
            Ok(logs)
        })
    }

    /// Get usage for a specific task (aggregated from call log).
    pub fn get_task_usage(&self, task_id: &str, limit: usize) -> Result<Vec<LlmCallLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, task_id, provider, model, key_id, input_tokens, output_tokens, cost_usd, status, latency_ms, error_message
                 FROM llm_call_log WHERE task_id = ? ORDER BY timestamp DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![task_id, limit as i64], |row| {
                Self::row_to_call_log(row)
            })?;
            let mut logs = Vec::new();
            for row in rows {
                logs.push(row?);
            }
            Ok(logs)
        })
    }

    /// Upsert daily usage aggregate.
    pub fn upsert_daily_usage(&self, usage: &LlmUsageDaily) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO llm_usage_daily (date, agent_id, model, total_requests, total_input_tokens, total_output_tokens, total_cost_usd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(date, agent_id, model) DO UPDATE SET
                   total_requests = total_requests + excluded.total_requests,
                   total_input_tokens = total_input_tokens + excluded.total_input_tokens,
                   total_output_tokens = total_output_tokens + excluded.total_output_tokens,
                   total_cost_usd = total_cost_usd + excluded.total_cost_usd",
                rusqlite::params![
                    usage.date,
                    usage.agent_id,
                    usage.model,
                    usage.total_requests,
                    usage.total_input_tokens,
                    usage.total_output_tokens,
                    usage.total_cost_usd,
                ],
            )
            .context("Failed to upsert daily usage")?;
            Ok(())
        })
    }

    /// Get all usage (no agent filter).
    pub fn get_all_usage(&self, limit: usize) -> Result<Vec<LlmCallLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, task_id, provider, model, key_id, input_tokens, output_tokens, cost_usd, status, latency_ms, error_message
                 FROM llm_call_log ORDER BY timestamp DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Self::row_to_call_log(row)
            })?;
            let mut logs = Vec::new();
            for row in rows {
                logs.push(row?);
            }
            Ok(logs)
        })
    }

    /// Get all daily usage (no agent filter).
    pub fn get_all_daily_usage(&self, limit: usize) -> Result<Vec<LlmUsageDaily>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT date, agent_id, model, total_requests, total_input_tokens, total_output_tokens, total_cost_usd
                 FROM llm_usage_daily ORDER BY date DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
                Ok(LlmUsageDaily {
                    date: row.get(0)?,
                    agent_id: row.get(1)?,
                    model: row.get(2)?,
                    total_requests: row.get(3)?,
                    total_input_tokens: row.get(4)?,
                    total_output_tokens: row.get(5)?,
                    total_cost_usd: row.get(6)?,
                })
            })?;
            let mut usage = Vec::new();
            for row in rows {
                usage.push(row?);
            }
            Ok(usage)
        })
    }

    /// Get usage filtered by key_id.
    pub fn get_usage_by_key(&self, key_id: &str, limit: usize) -> Result<Vec<LlmCallLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, task_id, provider, model, key_id, input_tokens, output_tokens, cost_usd, status, latency_ms, error_message
                 FROM llm_call_log WHERE key_id = ? ORDER BY timestamp DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![key_id, limit as i64], |row| {
                Self::row_to_call_log(row)
            })?;
            let mut logs = Vec::new();
            for row in rows {
                logs.push(row?);
            }
            Ok(logs)
        })
    }

    /// Get usage filtered by provider.
    pub fn get_usage_by_provider(&self, provider: &str, limit: usize) -> Result<Vec<LlmCallLog>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, timestamp, agent_id, task_id, provider, model, key_id, input_tokens, output_tokens, cost_usd, status, latency_ms, error_message
                 FROM llm_call_log WHERE provider = ? ORDER BY timestamp DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![provider, limit as i64], |row| {
                Self::row_to_call_log(row)
            })?;
            let mut logs = Vec::new();
            for row in rows {
                logs.push(row?);
            }
            Ok(logs)
        })
    }

    /// Get daily usage for an agent.
    pub fn get_daily_usage(&self, agent_id: &str, limit: usize) -> Result<Vec<LlmUsageDaily>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT date, agent_id, model, total_requests, total_input_tokens, total_output_tokens, total_cost_usd
                 FROM llm_usage_daily WHERE agent_id = ? ORDER BY date DESC LIMIT ?",
            )?;
            let rows = stmt.query_map(rusqlite::params![agent_id, limit as i64], |row| {
                Ok(LlmUsageDaily {
                    date: row.get(0)?,
                    agent_id: row.get(1)?,
                    model: row.get(2)?,
                    total_requests: row.get(3)?,
                    total_input_tokens: row.get(4)?,
                    total_output_tokens: row.get(5)?,
                    total_cost_usd: row.get(6)?,
                })
            })?;
            let mut usage = Vec::new();
            for row in rows {
                usage.push(row?);
            }
            Ok(usage)
        })
    }

    /// Record a completed LLM call: inserts into call log and upserts daily aggregate.
    /// Use status "success" for successful calls, "error" or "cost_exceeded" for failures.
    pub fn record_and_log(
        &self,
        agent_id: &str,
        task_id: Option<&str>,
        provider: &str,
        model: &str,
        input_tokens: i32,
        output_tokens: i32,
        cost_usd: f64,
        latency_ms: i64,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now();
        let log = LlmCallLog {
            id: None,
            timestamp: now,
            agent_id: Some(agent_id.to_string()),
            task_id: task_id.map(|s| s.to_string()),
            provider: provider.to_string(),
            model: model.to_string(),
            key_id: None,
            input_tokens,
            output_tokens,
            cost_usd,
            status: status.to_string(),
            latency_ms: Some(latency_ms),
            error_message: error_message.map(|s| s.to_string()),
        };
        let id = self.insert_call_log(&log)?;

        let today = now.format("%Y-%m-%d").to_string();
        let daily = LlmUsageDaily {
            date: today,
            agent_id: agent_id.to_string(),
            model: model.to_string(),
            total_requests: 1,
            total_input_tokens: input_tokens as i64,
            total_output_tokens: output_tokens as i64,
            total_cost_usd: cost_usd,
        };
        let _ = self.upsert_daily_usage(&daily);

        Ok(id)
    }

    fn row_to_call_log(row: &Row) -> rusqlite::Result<LlmCallLog> {
        let ts_str: String = row.get(1)?;
        Ok(LlmCallLog {
            id: row.get(0)?,
            timestamp: parse_datetime(&ts_str),
            agent_id: row.get(2)?,
            task_id: row.get(3)?,
            provider: row.get(4)?,
            model: row.get(5)?,
            key_id: row.get(6)?,
            input_tokens: row.get(7)?,
            output_tokens: row.get(8)?,
            cost_usd: row.get(9)?,
            status: row.get(10)?,
            latency_ms: row.get(11)?,
            error_message: row.get(12)?,
        })
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| ndt.and_utc())
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_insert_and_get_call_log() {
        let db = setup_db();
        let repo = LlmUsageRepository::new(&db);

        let log = LlmCallLog {
            id: None,
            timestamp: Utc::now(),
            agent_id: Some("agent1".to_string()),
            task_id: Some("task1".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            key_id: Some("key1".to_string()),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.001,
            status: "success".to_string(),
            latency_ms: Some(250),
            error_message: None,
        };

        let id = repo.insert_call_log(&log).unwrap();
        assert!(id > 0);

        let logs = repo.get_agent_usage("agent1", 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].model, "claude-sonnet-4-5-20250929");
        assert_eq!(logs[0].input_tokens, 100);
        assert_eq!(logs[0].cost_usd, 0.001);
    }

    #[test]
    fn test_get_task_usage() {
        let db = setup_db();
        let repo = LlmUsageRepository::new(&db);

        let log = LlmCallLog {
            id: None,
            timestamp: Utc::now(),
            agent_id: Some("agent1".to_string()),
            task_id: Some("task1".to_string()),
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            key_id: None,
            input_tokens: 200,
            output_tokens: 100,
            cost_usd: 0.002,
            status: "success".to_string(),
            latency_ms: Some(500),
            error_message: None,
        };

        repo.insert_call_log(&log).unwrap();

        let logs = repo.get_task_usage("task1", 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].provider, "openai");
    }

    #[test]
    fn test_daily_usage_upsert() {
        let db = setup_db();
        let repo = LlmUsageRepository::new(&db);

        let usage = LlmUsageDaily {
            date: "2025-01-15".to_string(),
            agent_id: "agent1".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            total_requests: 5,
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_cost_usd: 0.01,
        };

        repo.upsert_daily_usage(&usage).unwrap();

        // Upsert again — should accumulate
        repo.upsert_daily_usage(&usage).unwrap();

        let daily = repo.get_daily_usage("agent1", 10).unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].total_requests, 10); // 5 + 5
        assert_eq!(daily[0].total_input_tokens, 2000); // 1000 + 1000
    }

    #[test]
    fn test_empty_results() {
        let db = setup_db();
        let repo = LlmUsageRepository::new(&db);

        let logs = repo.get_agent_usage("nonexistent", 10).unwrap();
        assert!(logs.is_empty());

        let daily = repo.get_daily_usage("nonexistent", 10).unwrap();
        assert!(daily.is_empty());
    }

    #[test]
    fn test_schema_version() {
        let db = setup_db();
        assert_eq!(db.schema_version().unwrap(), 13);
    }
}
