//! Read-only query helpers for tool invocation statistics backed by
//! `tool_execution_log` (migration 030).
//!
//! `ToolStats` is a thin wrapper over
//! [`openalpaca_storage::SkillExecutionRepository::tool_stats`] and
//! [`openalpaca_storage::SkillExecutionRepository::tool_stats_all`], which
//! keeps the raw SQL and `rusqlite` usage confined to the storage crate.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use openalpaca_storage::{Database, SkillExecutionRepository, ToolInvocationStats};

/// Aggregate stats for a single tool, computed from `tool_execution_log`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolStats {
    /// Timestamp of the most recent invocation (UTC). `None` if the tool has
    /// never been invoked.
    pub last_invoked_at: Option<DateTime<Utc>>,
    /// Total number of invocations recorded.
    pub invocation_count: u64,
    /// Number of invocations where `success = 0`.
    pub error_count: u64,
}

impl ToolStats {
    /// Query stats for a single tool by name. If the tool has never been
    /// logged, returns a zero-valued `ToolStats` with `last_invoked_at = None`.
    pub fn for_tool(db: &Database, tool_name: &str) -> anyhow::Result<Self> {
        Ok(SkillExecutionRepository::new(db).tool_stats(tool_name)?.into())
    }

    /// Query stats for every tool that has appeared in `tool_execution_log`,
    /// keyed by tool name. Tools that have never been invoked are not
    /// present in the returned map.
    pub fn for_all_tools(db: &Database) -> anyhow::Result<HashMap<String, Self>> {
        let rows = SkillExecutionRepository::new(db).tool_stats_all()?;
        let mut out = HashMap::with_capacity(rows.len());
        for (name, stats) in rows {
            out.insert(name, stats.into());
        }
        Ok(out)
    }
}

impl From<ToolInvocationStats> for ToolStats {
    fn from(s: ToolInvocationStats) -> Self {
        Self {
            last_invoked_at: s.last_invoked_at,
            invocation_count: s.invocation_count,
            error_count: s.error_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::ToolExecutionEntry;
    use tempfile::TempDir;

    /// Open a fresh on-disk DB in a tempdir; return the DB and the TempDir so
    /// the caller keeps ownership (dropping it removes the directory).
    fn mk_db() -> (Database, TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (db, dir)
    }

    fn insert_log(db: &Database, agent_id: &str, tool_name: &str, success: bool) {
        let entry = ToolExecutionEntry {
            id: None,
            request_id: None,
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            success,
            duration_ms: 100,
            error_message: None,
            timestamp: None,
        };
        SkillExecutionRepository::new(db)
            .record_tool(&entry)
            .expect("record tool entry");
    }

    #[test]
    fn stats_for_tool_never_called_returns_zeros() {
        let (db, _dir) = mk_db();
        let stats = ToolStats::for_tool(&db, "never_called").unwrap();
        assert_eq!(stats.invocation_count, 0);
        assert_eq!(stats.error_count, 0);
        assert!(stats.last_invoked_at.is_none());
    }

    #[test]
    fn stats_for_tool_aggregates_rows() {
        let (db, _dir) = mk_db();
        insert_log(&db, "agent1", "my_tool", true);
        insert_log(&db, "agent1", "my_tool", true);
        insert_log(&db, "agent1", "my_tool", false);

        let stats = ToolStats::for_tool(&db, "my_tool").unwrap();
        assert_eq!(stats.invocation_count, 3);
        assert_eq!(stats.error_count, 1);
        assert!(stats.last_invoked_at.is_some());
    }

    #[test]
    fn stats_for_all_tools_groups_by_name() {
        let (db, _dir) = mk_db();
        insert_log(&db, "a1", "tool_a", true);
        insert_log(&db, "a1", "tool_a", true);
        insert_log(&db, "a1", "tool_b", true);

        let all = ToolStats::for_all_tools(&db).unwrap();
        assert_eq!(all.get("tool_a").unwrap().invocation_count, 2);
        assert_eq!(all.get("tool_b").unwrap().invocation_count, 1);
        assert_eq!(all.len(), 2);
    }
}
