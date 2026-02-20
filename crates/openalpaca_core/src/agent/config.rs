//! TOML-based agent configuration file structure

use super::subagent::{
    AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent,
};
use super::template::{AgentTemplate, AgentTemplateFrontmatter, extract_persona};
use chrono::Utc;
use openalpaca_storage::SubAgentConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// TOML config file structure for agent definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfigFile {
    pub agent: AgentMeta,
    pub skills: AgentSkillsConfig,
    pub preset: AgentPresetConfig,
    pub constraints: Option<AgentConstraintsConfig>,
    pub llm: Option<AgentLlmConfigFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkillsConfig {
    pub assigned: Vec<String>,
    pub denied: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPresetConfig {
    pub persona: String,
    pub temperature: Option<f32>,
    pub verbosity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLlmConfigFile {
    pub model: Option<String>,
    pub fallback_models: Option<Vec<String>>,
    pub overrides: Option<std::collections::HashMap<String, String>>,
}

impl AgentConfigFile {
    /// Reconstruct an AgentConfigFile from an in-memory SubAgent.
    /// Reverse of `into_subagent()`.
    pub fn from_subagent(agent: &SubAgent) -> Self {
        // Extract skills.denied from constraints.denied_capabilities
        // that are NOT in the assigned skills list
        let assigned: Vec<String> = agent.skills.iter().map(|s| s.name.clone()).collect();
        let denied: Vec<String> = agent
            .constraints
            .denied_capabilities
            .iter()
            .filter(|d| !assigned.contains(d))
            .cloned()
            .collect();

        let meta = AgentMeta {
            id: agent.id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone().unwrap_or_default(),
            icon: agent.icon.clone(),
        };

        let skills = AgentSkillsConfig {
            assigned,
            denied: if denied.is_empty() {
                None
            } else {
                Some(denied)
            },
        };

        let preset = AgentPresetConfig {
            persona: agent.preset.persona.clone(),
            temperature: Some(agent.preset.temperature),
            verbosity: Some(agent.preset.verbosity.clone()),
        };

        let constraints = {
            let c = &agent.constraints;
            let has_values = c.max_tool_calls.is_some()
                || c.timeout_seconds.is_some()
                || c.max_cost_per_task.is_some()
                || !c.require_confirmation_for.is_empty()
                || !c.allowed_capabilities.is_empty()
                || !c.denied_capabilities.is_empty()
                || !c.allowed_models.is_empty()
                || !c.denied_models.is_empty();

            if has_values {
                Some(AgentConstraintsConfig {
                    max_tool_calls: c.max_tool_calls,
                    timeout_seconds: c.timeout_seconds,
                    max_cost_per_task: c.max_cost_per_task,
                    require_confirmation_for: if c.require_confirmation_for.is_empty() {
                        None
                    } else {
                        Some(c.require_confirmation_for.clone())
                    },
                    allowed_capabilities: if c.allowed_capabilities.is_empty() {
                        None
                    } else {
                        Some(c.allowed_capabilities.clone())
                    },
                    denied_capabilities: if c.denied_capabilities.is_empty() {
                        None
                    } else {
                        Some(c.denied_capabilities.clone())
                    },
                    allowed_models: if c.allowed_models.is_empty() {
                        None
                    } else {
                        Some(c.allowed_models.clone())
                    },
                    denied_models: if c.denied_models.is_empty() {
                        None
                    } else {
                        Some(c.denied_models.clone())
                    },
                })
            } else {
                None
            }
        };

        let llm = {
            let l = &agent.llm_config;
            let has_values =
                l.model.is_some() || !l.fallback_models.is_empty() || !l.overrides.is_empty();
            if has_values {
                Some(AgentLlmConfigFile {
                    model: l.model.clone(),
                    fallback_models: if l.fallback_models.is_empty() {
                        None
                    } else {
                        Some(l.fallback_models.clone())
                    },
                    overrides: if l.overrides.is_empty() {
                        None
                    } else {
                        Some(l.overrides.clone())
                    },
                })
            } else {
                None
            }
        };

        Self {
            agent: meta,
            skills,
            preset,
            constraints,
            llm,
        }
    }

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
            verbosity: self
                .preset
                .verbosity
                .clone()
                .unwrap_or_else(|| "normal".to_string()),
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
                let mut constraints = AgentConstraints {
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
                };
                constraints.normalize();
                constraints
            })
            .unwrap_or_else(|| {
                let mut constraints = AgentConstraints {
                    denied_capabilities: skills_denied,
                    ..Default::default()
                };
                constraints.normalize();
                constraints
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

        let id = self.agent.id;
        SubAgent {
            template_id: id.clone(), // backward compat: template_id = id
            id,
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

    /// Create an `AgentConfigFile` from an `AgentTemplate`.
    ///
    /// This enables backward-compatible REST API responses: the template
    /// is converted to the TOML-style structure callers already expect.
    pub fn from_template(template: &AgentTemplate) -> Self {
        let fm = &template.frontmatter;
        let persona = extract_persona(template);

        let meta = AgentMeta {
            id: fm.id.clone(),
            name: fm.name.clone(),
            description: fm.description.clone(),
            icon: fm.icon.clone(),
        };

        let skills = AgentSkillsConfig {
            assigned: fm.skills.clone(),
            denied: if fm.denied_skills.is_empty() {
                None
            } else {
                Some(fm.denied_skills.clone())
            },
        };

        let preset = AgentPresetConfig {
            persona,
            temperature: Some(fm.temperature),
            verbosity: Some(fm.verbosity.clone()),
        };

        let constraints = {
            let has_values = fm.max_tool_calls.is_some()
                || fm.timeout_seconds.is_some()
                || fm.max_cost_per_task.is_some()
                || !fm.require_confirmation_for.is_empty()
                || !fm.skills.is_empty()
                || !fm.denied_skills.is_empty();

            if has_values {
                Some(AgentConstraintsConfig {
                    max_tool_calls: fm.max_tool_calls,
                    timeout_seconds: fm.timeout_seconds,
                    max_cost_per_task: fm.max_cost_per_task,
                    require_confirmation_for: if fm.require_confirmation_for.is_empty() {
                        None
                    } else {
                        Some(fm.require_confirmation_for.clone())
                    },
                    allowed_capabilities: if fm.skills.is_empty() {
                        None
                    } else {
                        Some(fm.skills.clone())
                    },
                    denied_capabilities: if fm.denied_skills.is_empty() {
                        None
                    } else {
                        Some(fm.denied_skills.clone())
                    },
                    allowed_models: None,
                    denied_models: None,
                })
            } else {
                None
            }
        };

        let llm = {
            let has_values = fm.model.is_some() || !fm.fallback_models.is_empty();
            if has_values {
                Some(AgentLlmConfigFile {
                    model: fm.model.clone(),
                    fallback_models: if fm.fallback_models.is_empty() {
                        None
                    } else {
                        Some(fm.fallback_models.clone())
                    },
                    overrides: None,
                })
            } else {
                None
            }
        };

        Self {
            agent: meta,
            skills,
            preset,
            constraints,
            llm,
        }
    }

    /// Convert this TOML config to an `AgentTemplate`.
    ///
    /// This is the reverse of `from_template()` and is used for
    /// TOML→Markdown migration or when the REST API creates a template
    /// from a TOML-format POST body.
    pub fn into_template(self) -> AgentTemplate {
        let skills_denied = self.skills.denied.clone().unwrap_or_default();

        let frontmatter = AgentTemplateFrontmatter {
            id: self.agent.id.clone(),
            name: self.agent.name.clone(),
            description: self.agent.description.clone(),
            icon: self.agent.icon.clone(),
            singleton: false, // default — only lead_agent overrides
            skills: self.skills.assigned.clone(),
            denied_skills: skills_denied,
            temperature: self.preset.temperature.unwrap_or(0.5),
            verbosity: self
                .preset
                .verbosity
                .clone()
                .unwrap_or_else(|| "normal".to_string()),
            model: self.llm.as_ref().and_then(|l| l.model.clone()),
            fallback_models: self
                .llm
                .as_ref()
                .and_then(|l| l.fallback_models.clone())
                .unwrap_or_default(),
            max_tool_calls: self.constraints.as_ref().and_then(|c| c.max_tool_calls),
            timeout_seconds: self.constraints.as_ref().and_then(|c| c.timeout_seconds),
            max_cost_per_task: self.constraints.as_ref().and_then(|c| c.max_cost_per_task),
            require_confirmation_for: self
                .constraints
                .as_ref()
                .and_then(|c| c.require_confirmation_for.clone())
                .unwrap_or_default(),
        };

        // Build body from persona
        let body = format!("## Persona\n\n{}", self.preset.persona);
        let mut sections = HashMap::new();
        sections.insert("Persona".to_string(), self.preset.persona.clone());

        AgentTemplate {
            frontmatter,
            body,
            sections,
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
            verbosity: self
                .preset
                .verbosity
                .clone()
                .unwrap_or_else(|| "normal".to_string()),
        };

        let skills_denied2 = self.skills.denied.clone().unwrap_or_default();

        let constraints = self.constraints.as_ref().map(|c| {
            let mut denied = c.denied_capabilities.clone().unwrap_or_default();
            for d in &skills_denied2 {
                if !denied.contains(d) {
                    denied.push(d.clone());
                }
            }
            let mut constraints = AgentConstraints {
                max_tool_calls: c.max_tool_calls,
                timeout_seconds: c.timeout_seconds,
                max_cost_per_task: c.max_cost_per_task,
                require_confirmation_for: c.require_confirmation_for.clone().unwrap_or_default(),
                allowed_capabilities: c.allowed_capabilities.clone().unwrap_or_default(),
                denied_capabilities: denied,
                allowed_models: c.allowed_models.clone().unwrap_or_default(),
                denied_models: c.denied_models.clone().unwrap_or_default(),
            };
            constraints.normalize();
            constraints
        });

        let llm_config = self.llm.as_ref().map(|l| AgentLlmConfig {
            model: l.model.clone(),
            fallback_models: l.fallback_models.clone().unwrap_or_default(),
            overrides: l.overrides.clone().unwrap_or_default(),
        });
        let llm_config_json = llm_config
            .as_ref()
            .and_then(|c| serde_json::to_string(c).ok());

        let now = Utc::now();

        let agent_id = self.agent.id.clone();
        SubAgentConfig {
            id: agent_id.clone(),
            template_id: agent_id,
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
        assert_eq!(
            config.constraints.as_ref().unwrap().max_tool_calls,
            Some(20)
        );
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
        assert!(
            agent
                .constraints
                .denied_capabilities
                .contains(&"shell_execute".to_string())
        );
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
        assert_eq!(
            agent.constraints.allowed_capabilities,
            vec!["web_search", "summarize"]
        );
        // "file_write" from denied_capabilities + "shell_execute" from skills.denied
        assert!(
            agent
                .constraints
                .denied_capabilities
                .contains(&"file_write".to_string())
        );
        assert!(
            agent
                .constraints
                .denied_capabilities
                .contains(&"shell_execute".to_string())
        );
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
        assert!(
            agent
                .constraints
                .denied_capabilities
                .contains(&"shell_execute".to_string())
        );
    }

    // ── Template bridge tests ──────────────────────────────────────

    fn sample_template() -> AgentTemplate {
        use crate::agent::template::parse_agent_markdown;
        parse_agent_markdown(
            r#"---
id: "bridge_agent"
name: "Bridge Agent"
description: "Agent for bridge testing"
icon: "bridge"
skills:
  - "web_search"
  - "summarize"
denied_skills:
  - "shell_execute"
temperature: 0.3
verbosity: "detailed"
model: "claude-sonnet-4-5-20250929"
fallback_models:
  - "claude-haiku-4-5-20251001"
max_tool_calls: 20
timeout_seconds: 300
max_cost_per_task: 0.50
require_confirmation_for:
  - "file_delete"
---

## Persona

You are a bridge testing assistant.
"#,
        )
        .unwrap()
    }

    #[test]
    fn test_from_template() {
        let template = sample_template();
        let config = AgentConfigFile::from_template(&template);

        assert_eq!(config.agent.id, "bridge_agent");
        assert_eq!(config.agent.name, "Bridge Agent");
        assert_eq!(config.agent.icon, Some("bridge".to_string()));
        assert_eq!(config.skills.assigned, vec!["web_search", "summarize"]);
        assert_eq!(
            config.skills.denied,
            Some(vec!["shell_execute".to_string()])
        );
        assert_eq!(config.preset.temperature, Some(0.3));
        assert_eq!(config.preset.verbosity, Some("detailed".to_string()));
        assert!(config.preset.persona.contains("bridge testing assistant"));
        assert_eq!(
            config.constraints.as_ref().unwrap().max_tool_calls,
            Some(20)
        );
        assert_eq!(
            config.llm.as_ref().unwrap().model,
            Some("claude-sonnet-4-5-20250929".to_string())
        );
        assert_eq!(
            config.llm.as_ref().unwrap().fallback_models,
            Some(vec!["claude-haiku-4-5-20251001".to_string()])
        );
    }

    #[test]
    fn test_from_template_roundtrip_to_subagent() {
        let template = sample_template();
        let config = AgentConfigFile::from_template(&template);
        let agent = config.into_subagent();

        assert_eq!(agent.id, "bridge_agent");
        assert_eq!(agent.template_id, "bridge_agent");
        assert_eq!(agent.skills.len(), 2);
        assert_eq!(agent.preset.temperature, 0.3);
        assert!(
            agent
                .constraints
                .denied_capabilities
                .contains(&"shell_execute".to_string())
        );
        assert!(agent.status.is_available());
    }

    #[test]
    fn test_into_template() {
        let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
        let template = config.into_template();

        assert_eq!(template.frontmatter.id, "test_agent");
        assert_eq!(template.frontmatter.name, "Test Agent");
        assert_eq!(template.frontmatter.skills, vec!["web_search", "summarize"]);
        assert_eq!(template.frontmatter.denied_skills, vec!["shell_execute"]);
        assert_eq!(template.frontmatter.temperature, 0.3);
        assert_eq!(template.frontmatter.max_tool_calls, Some(20));
        assert!(
            template
                .sections
                .get("Persona")
                .unwrap()
                .contains("test assistant")
        );
        assert!(!template.frontmatter.singleton);
    }

    #[test]
    fn test_into_template_roundtrip() {
        // TOML → AgentConfigFile → AgentTemplate → AgentConfigFile → SubAgent
        let config: AgentConfigFile = toml::from_str(sample_toml()).unwrap();
        let template = config.into_template();
        let config2 = AgentConfigFile::from_template(&template);
        let agent = config2.into_subagent();

        assert_eq!(agent.id, "test_agent");
        assert_eq!(agent.skills.len(), 2);
        assert_eq!(agent.preset.temperature, 0.3);
        assert!(
            agent
                .constraints
                .denied_capabilities
                .contains(&"shell_execute".to_string())
        );
    }
}
