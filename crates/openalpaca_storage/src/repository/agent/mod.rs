//! Agent repository - CRUD operations for agents

use crate::Database;
use crate::models::Agent;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

/// Repository for Agent operations
pub struct AgentRepository<'a> {
    db: &'a Database,
}

impl<'a> AgentRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Create a new agent
    pub fn create(&self, agent: &Agent) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO agent (id, name, persona, config, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    &agent.id,
                    &agent.name,
                    &agent.persona,
                    agent.config.as_ref().map(|v| v.to_string()),
                    agent.created_at.to_rfc3339(),
                ),
            )?;
            Ok(())
        })
    }

    /// Get an agent by ID
    pub fn get(&self, id: &str) -> Result<Option<Agent>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, name, persona, config, created_at FROM agent WHERE id = ?1")?;

            let mut rows = stmt.query([id])?;

            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_agent(row)?)),
                None => Ok(None),
            }
        })
    }

    /// List all agents
    pub fn list(&self) -> Result<Vec<Agent>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, persona, config, created_at FROM agent ORDER BY created_at DESC",
            )?;

            let mut agents = Vec::new();
            let mut rows = stmt.query([])?;

            while let Some(row) = rows.next()? {
                agents.push(Self::row_to_agent(row)?);
            }

            Ok(agents)
        })
    }

    /// Update an agent
    pub fn update(&self, agent: &Agent) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE agent SET name = ?1, persona = ?2, config = ?3 WHERE id = ?4",
                (
                    &agent.name,
                    &agent.persona,
                    agent.config.as_ref().map(|v| v.to_string()),
                    &agent.id,
                ),
            )?;
            Ok(())
        })
    }

    /// Delete an agent
    pub fn delete(&self, id: &str) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute("DELETE FROM agent WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    fn row_to_agent(row: &rusqlite::Row<'_>) -> Result<Agent> {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let persona: Option<String> = row.get(2)?;
        let config_str: Option<String> = row.get(3)?;
        let created_at_str: String = row.get(4)?;

        let config = config_str
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .context("Failed to parse agent config JSON")?;

        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(Agent {
            id,
            name,
            persona,
            config,
            created_at,
        })
    }
}

#[cfg(test)]
mod tests;
