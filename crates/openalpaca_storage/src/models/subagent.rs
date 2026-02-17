//! SubAgent data models for the storage layer

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Persistent agent configuration (maps to full agent table with SubAgent columns)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    pub id: String,
    /// Template this agent was created from (for the template+instance model).
    /// For legacy agents and templates themselves, this equals `id`.
    #[serde(default)]
    pub template_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub status: String,
    pub current_task_id: Option<String>,
    pub skills_json: String,
    pub preset_json: String,
    pub constraints_json: Option<String>,
    pub llm_config_json: Option<String>,
    pub persona: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Agent performance metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetrics {
    pub agent_id: String,
    pub tasks_completed: i32,
    pub tasks_failed: i32,
    pub total_runtime_seconds: i64,
    pub average_runtime_seconds: f64,
    pub success_rate: f64,
    pub updated_at: DateTime<Utc>,
}

impl AgentMetrics {
    /// Create empty metrics for a new agent.
    pub fn new_empty(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            tasks_completed: 0,
            tasks_failed: 0,
            total_runtime_seconds: 0,
            average_runtime_seconds: 0.0,
            success_rate: 1.0,
            updated_at: Utc::now(),
        }
    }
}

/// Historical record of agent-task participation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTaskHistory {
    pub id: String,
    pub agent_id: String,
    pub task_id: String,
    pub role: String,
    pub status: String,
    pub runtime_seconds: Option<i64>,
    pub completed_at: DateTime<Utc>,
}
