//! TOML-based agent configuration file structure

use super::subagent::{AgentConstraints, AgentPreset, AgentStatus, Skill, SubAgent};
use chrono::Utc;
use openalpaca_storage::SubAgentConfig;
use serde::Deserialize;

/// TOML config file structure for agent definitions.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfigFile {
    pub agent: AgentMeta,
    pub skills: AgentSkillsConfig,
    pub preset: AgentPresetConfig,
    pub constraints: Option<AgentConstraintsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentSkillsConfig {
    pub assigned: Vec<String>,
    pub denied: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentPresetConfig {
    pub persona: String,
    pub temperature: Option<f32>,
    pub verbosity: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConstraintsConfig {
    pub max_tool_calls: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub max_cost_per_task: Option<f64>,
    pub require_confirmation_for: Option<Vec<String>>,
}

impl AgentConfigFile {
    /// Convert TOML config to in-memory SubAgent.
    pub fn into_subagent(self) -> SubAgent {
        let skills: Vec<Skill> = self
            .skills
            .assigned
            .iter()
            .map(|name| Skill {
                name: name.clone(),
                category: "assigned".to_string(),
                proficiency: 1.0,
            })
            .collect();

        let preset = AgentPreset {
            persona: self.preset.persona.clone(),
            temperature: self.preset.temperature.unwrap_or(0.5),
            verbosity: self.preset.verbosity.clone().unwrap_or_else(|| "normal".to_string()),
        };

        let constraints = self
            .constraints
            .as_ref()
            .map(|c| AgentConstraints {
                max_tool_calls: c.max_tool_calls,
                timeout_seconds: c.timeout_seconds,
                max_cost_per_task: c.max_cost_per_task,
                require_confirmation_for: c
                    .require_confirmation_for
                    .clone()
                    .unwrap_or_default(),
            })
            .unwrap_or_default();

        SubAgent {
            id: self.agent.id,
            name: self.agent.name,
            description: Some(self.agent.description),
            icon: self.agent.icon,
            status: AgentStatus::Idle,
            current_task: None,
            skills,
            preset,
            constraints,
        }
    }

    /// Convert TOML config to storage SubAgentConfig.
    pub fn into_storage_config(self) -> SubAgentConfig {
        let skills_json: Vec<Skill> = self
            .skills
            .assigned
            .iter()
            .map(|name| Skill {
                name: name.clone(),
                category: "assigned".to_string(),
                proficiency: 1.0,
            })
            .collect();

        let preset = AgentPreset {
            persona: self.preset.persona.clone(),
            temperature: self.preset.temperature.unwrap_or(0.5),
            verbosity: self.preset.verbosity.clone().unwrap_or_else(|| "normal".to_string()),
        };

        let constraints = self.constraints.as_ref().map(|c| AgentConstraints {
            max_tool_calls: c.max_tool_calls,
            timeout_seconds: c.timeout_seconds,
            max_cost_per_task: c.max_cost_per_task,
            require_confirmation_for: c.require_confirmation_for.clone().unwrap_or_default(),
        });

        let now = Utc::now();

        SubAgentConfig {
            id: self.agent.id,
            name: self.agent.name.clone(),
            description: Some(self.agent.description),
            icon: self.agent.icon,
            status: "idle".to_string(),
            current_task_id: None,
            skills_json: serde_json::to_string(&skills_json).unwrap_or_else(|_| "[]".to_string()),
            preset_json: serde_json::to_string(&preset).unwrap_or_else(|_| "{}".to_string()),
            constraints_json: constraints
                .as_ref()
                .map(|c| serde_json::to_string(c).unwrap_or_else(|_| "{}".to_string())),
            persona: Some(self.preset.persona),
            created_at: now,
            updated_at: Some(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_toml() -> &'static str {
        r#"
[agent]
id = "test_agent"
name = "Test Agent"
description = "A test agent"
icon = "star"

[skills]
assigned = ["web_search", "summarize"]
denied = ["shell_execute"]

[preset]
persona = "You are a test assistant."
temperature = 0.3
verbosity = "detailed"

[constraints]
max_tool_calls = 20
timeout_seconds = 300
max_cost_per_task = 0.50
require_confirmation_for = ["file_delete"]
"#
    }

    #[test]
    fn test_parse_toml() {
        let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
        assert_eq!(config.agent.id, "test_agent");
        assert_eq!(config.skills.assigned.len(), 2);
        assert_eq!(config.preset.temperature, Some(0.3));
        assert_eq!(config.constraints.as_ref().unwrap().max_tool_calls, Some(20));
    }

    #[test]
    fn test_into_subagent() {
        let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
        let agent = config.into_subagent();
        assert_eq!(agent.id, "test_agent");
        assert_eq!(agent.name, "Test Agent");
        assert_eq!(agent.skills.len(), 2);
        assert_eq!(agent.preset.temperature, 0.3);
        assert_eq!(agent.constraints.max_tool_calls, Some(20));
        assert!(agent.status.is_available());
    }

    #[test]
    fn test_into_storage_config() {
        let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
        let sc = config.into_storage_config();
        assert_eq!(sc.id, "test_agent");
        assert_eq!(sc.status, "idle");
        assert!(sc.constraints_json.is_some());
    }
}
