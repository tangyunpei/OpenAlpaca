//! TOML-based agent configuration file structure

use super::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent};
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
    pub llm: Option<AgentLlmConfigFile>,
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
    pub allowed_capabilities: Option<Vec<String>>,
    pub denied_capabilities: Option<Vec<String>>,
    pub allowed_models: Option<Vec<String>>,
    pub denied_models: Option<Vec<String>>,
}

/// TOML structure for per-agent LLM config.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentLlmConfigFile {
    pub model: Option<String>,
    pub fallback_models: Option<Vec<String>>,
    pub overrides: Option<std::collections::HashMap<String, String>>,
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

        // Merge skills.denied into denied_capabilities
        let skills_denied = self.skills.denied.clone().unwrap_or_default();

        let constraints = self
            .constraints
            .as_ref()
            .map(|c| {
                let mut denied = c.denied_capabilities.clone().unwrap_or_default();
                for d in &skills_denied {
                    if !denied.contains(d) {
                        denied.push(d.clone());
                    }
                }
                AgentConstraints {
                    max_tool_calls: c.max_tool_calls,
                    timeout_seconds: c.timeout_seconds,
                    max_cost_per_task: c.max_cost_per_task,
                    require_confirmation_for: c
                        .require_confirmation_for
                        .clone()
                        .unwrap_or_default(),
                    allowed_capabilities: c.allowed_capabilities.clone().unwrap_or_default(),
                    denied_capabilities: denied,
                    allowed_models: c.allowed_models.clone().unwrap_or_default(),
                    denied_models: c.denied_models.clone().unwrap_or_default(),
                }
            })
            .unwrap_or_else(|| AgentConstraints {
                denied_capabilities: skills_denied,
                ..Default::default()
            });

        let llm_config = self
            .llm
            .as_ref()
            .map(|l| AgentLlmConfig {
                model: l.model.clone(),
                fallback_models: l.fallback_models.clone().unwrap_or_default(),
                overrides: l.overrides.clone().unwrap_or_default(),
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
            llm_config,
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

        let skills_denied2 = self.skills.denied.clone().unwrap_or_default();

        let constraints = self.constraints.as_ref().map(|c| {
            let mut denied = c.denied_capabilities.clone().unwrap_or_default();
            for d in &skills_denied2 {
                if !denied.contains(d) {
                    denied.push(d.clone());
                }
            }
            AgentConstraints {
                max_tool_calls: c.max_tool_calls,
                timeout_seconds: c.timeout_seconds,
                max_cost_per_task: c.max_cost_per_task,
                require_confirmation_for: c.require_confirmation_for.clone().unwrap_or_default(),
                allowed_capabilities: c.allowed_capabilities.clone().unwrap_or_default(),
                denied_capabilities: denied,
                allowed_models: c.allowed_models.clone().unwrap_or_default(),
                denied_models: c.denied_models.clone().unwrap_or_default(),
            }
        });

        let llm_config = self.llm.as_ref().map(|l| {
            AgentLlmConfig {
                model: l.model.clone(),
                fallback_models: l.fallback_models.clone().unwrap_or_default(),
                overrides: l.overrides.clone().unwrap_or_default(),
            }
        });
        let llm_config_json = llm_config
            .as_ref()
            .and_then(|c| serde_json::to_string(c).ok());

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
            llm_config_json,
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

    #[test]
    fn test_skills_denied_merged_into_denied_capabilities() {
        let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
        let agent = config.into_subagent();
        // skills.denied = ["shell_execute"] should be merged
        assert!(agent.constraints.denied_capabilities.contains(&"shell_execute".to_string()));
    }

    #[test]
    fn test_toml_with_capabilities() {
        let toml_str = r#"
[agent]
id = "cap_agent"
name = "Cap Agent"
description = "Agent with capabilities"

[skills]
assigned = ["web_search"]
denied = ["shell_execute"]

[preset]
persona = "test"

[constraints]
max_tool_calls = 5
allowed_capabilities = ["web_search", "summarize"]
denied_capabilities = ["file_write"]
"#;
        let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
        let agent = config.into_subagent();
        assert_eq!(agent.constraints.allowed_capabilities, vec!["web_search", "summarize"]);
        // "file_write" from denied_capabilities + "shell_execute" from skills.denied
        assert!(agent.constraints.denied_capabilities.contains(&"file_write".to_string()));
        assert!(agent.constraints.denied_capabilities.contains(&"shell_execute".to_string()));
    }

    #[test]
    fn test_toml_without_constraints_but_with_denied_skills() {
        let toml_str = r#"
[agent]
id = "no_constraints"
name = "No Constraints"
description = "Agent without constraints section"

[skills]
assigned = ["web_search"]
denied = ["shell_execute"]

[preset]
persona = "test"
"#;
        let config: AgentConfigFile = toml::from_str(toml_str).unwrap();
        let agent = config.into_subagent();
        // skills.denied should still be captured even without [constraints]
        assert!(agent.constraints.denied_capabilities.contains(&"shell_execute".to_string()));
    }
}
