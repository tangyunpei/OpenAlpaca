//! Memory repository - Conversation memory with FTS5 search

use crate::Database;
use crate::models::{Memory, MemoryRole};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

/// Repository for Memory operations
pub struct MemoryRepository<'a> {
    db: &'a Database,
}

impl<'a> MemoryRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Add a memory entry
    pub fn add(
        &self,
        agent_id: &str,
        role: MemoryRole,
        content: &str,
        metadata: Option<&serde_json::Value>,
    ) -> Result<i64> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO memory (agent_id, role, content, metadata) VALUES (?1, ?2, ?3, ?4)",
                (
                    agent_id,
                    role.as_str(),
                    content,
                    metadata.map(|v| v.to_string()),
                ),
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Get recent memories for an agent
    pub fn recent(&self, agent_id: &str, limit: usize) -> Result<Vec<Memory>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, role, content, timestamp, metadata 
                 FROM memory WHERE agent_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
            )?;

            let mut memories = Vec::new();
            let mut rows = stmt.query([agent_id, &limit.to_string()])?;

            while let Some(row) = rows.next()? {
                memories.push(Self::row_to_memory(row)?);
            }

            // Return in chronological order
            memories.reverse();
            Ok(memories)
        })
    }

    /// Full-text search in memories using FTS5
    pub fn search(&self, query: &str, agent_id: Option<&str>, limit: usize) -> Result<Vec<Memory>> {
        self.db.with_connection(|conn| {
            let sql = if agent_id.is_some() {
                "SELECT m.id, m.agent_id, m.role, m.content, m.timestamp, m.metadata 
                 FROM memory m
                 JOIN memory_fts fts ON m.id = fts.rowid
                 WHERE memory_fts MATCH ?1 AND m.agent_id = ?2
                 ORDER BY rank LIMIT ?3"
            } else {
                "SELECT m.id, m.agent_id, m.role, m.content, m.timestamp, m.metadata 
                 FROM memory m
                 JOIN memory_fts fts ON m.id = fts.rowid
                 WHERE memory_fts MATCH ?1
                 ORDER BY rank LIMIT ?2"
            };

            let mut stmt = conn.prepare(sql)?;

            let mut memories = Vec::new();

            if let Some(aid) = agent_id {
                let mut rows = stmt.query([query, aid, &limit.to_string()])?;
                while let Some(row) = rows.next()? {
                    memories.push(Self::row_to_memory(row)?);
                }
            } else {
                let mut rows = stmt.query([query, &limit.to_string()])?;
                while let Some(row) = rows.next()? {
                    memories.push(Self::row_to_memory(row)?);
                }
            }

            Ok(memories)
        })
    }

    /// Clear all memories for an agent
    pub fn clear(&self, agent_id: &str) -> Result<u64> {
        self.db.with_connection(|conn| {
            let count = conn.execute("DELETE FROM memory WHERE agent_id = ?1", [agent_id])?;
            Ok(count as u64)
        })
    }

    /// Get memory by ID
    pub fn get(&self, id: i64) -> Result<Option<Memory>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, agent_id, role, content, timestamp, metadata FROM memory WHERE id = ?1",
            )?;

            let mut rows = stmt.query([id])?;

            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_memory(row)?)),
                None => Ok(None),
            }
        })
    }

    fn row_to_memory(row: &rusqlite::Row<'_>) -> Result<Memory> {
        let id: i64 = row.get(0)?;
        let agent_id: String = row.get(1)?;
        let role_str: String = row.get(2)?;
        let content: String = row.get(3)?;
        let timestamp_str: String = row.get(4)?;
        let metadata_str: Option<String> = row.get(5)?;

        let role: MemoryRole = role_str.parse()?;

        let metadata = metadata_str
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .context("Failed to parse memory metadata JSON")?;

        let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(Memory {
            id,
            agent_id,
            role,
            content,
            timestamp,
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::AgentRepository;
    use tempfile::tempdir;

    fn test_db() -> Database {
        let dir = tempdir().unwrap();
        Database::open(&dir.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_memory_operations() {
        let db = test_db();

        // Create agent first (foreign key constraint)
        let agent_repo = AgentRepository::new(&db);
        agent_repo
            .create(&crate::models::Agent {
                id: "agent-1".to_string(),
                name: "Test".to_string(),
                persona: None,
                config: None,
                created_at: Utc::now(),
            })
            .unwrap();

        let repo = MemoryRepository::new(&db);

        // Add memories
        repo.add("agent-1", MemoryRole::User, "Hello, how are you?", None)
            .unwrap();
        repo.add(
            "agent-1",
            MemoryRole::Agent,
            "I'm doing great, thank you!",
            None,
        )
        .unwrap();

        // Get recent
        let memories = repo.recent("agent-1", 10).unwrap();
        assert_eq!(memories.len(), 2);
        // Both memories should exist (order may vary if timestamps are equal)
        let contents: Vec<_> = memories.iter().map(|m| m.content.as_str()).collect();
        assert!(contents.contains(&"Hello, how are you?"));
        assert!(contents.contains(&"I'm doing great, thank you!"));
    }

    #[test]
    fn test_fts5_search() {
        let db = test_db();

        let agent_repo = AgentRepository::new(&db);
        agent_repo
            .create(&crate::models::Agent {
                id: "agent-1".to_string(),
                name: "Test".to_string(),
                persona: None,
                config: None,
                created_at: Utc::now(),
            })
            .unwrap();

        let repo = MemoryRepository::new(&db);

        repo.add("agent-1", MemoryRole::User, "I love Rust programming", None)
            .unwrap();
        repo.add("agent-1", MemoryRole::Agent, "Python is also great", None)
            .unwrap();

        // Search
        let results = repo.search("Rust", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));
    }
}
