//! Data models for OpenAlpaca storage layer
//!
//! Defines the core entities: Agent, EventLog, Memory

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub persona: Option<String>,
    pub config: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Event log entry for auditing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLog {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub agent_id: Option<String>,
    pub event_type: String,
    pub detail: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
}

/// Memory entry (conversation message)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub agent_id: String,
    pub role: MemoryRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: Option<serde_json::Value>,
}

/// Role in a conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRole {
    User,
    Agent,
    System,
}

impl MemoryRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::System => "system",
        }
    }
}

impl std::str::FromStr for MemoryRole {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            "system" => Ok(Self::System),
            _ => anyhow::bail!("Invalid memory role: {}", s),
        }
    }
}
