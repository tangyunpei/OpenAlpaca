//! In-memory SubAgent representation with rich typed model

use serde::{Deserialize, Serialize};

/// In-memory SubAgent representation.
///
/// In the template+instance model, each `SubAgent` is a runtime **instance**
/// spawned from an `AgentTemplate`. The `template_id` links back to the
/// originating template, while `id` is a unique instance identifier
/// (e.g. `"code_agent::a1b2c3d4"` for non-singletons, or `"lead_agent"` for singletons).
#[derive(Debug, Clone)]
pub struct SubAgent {
    pub id: String,
    /// Template this instance was spawned from (e.g. "code_agent").
    /// For backward compatibility, defaults to the same value as `id`.
    pub template_id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub status: AgentStatus,
    pub current_task: Option<String>,
    pub skills: Vec<Skill>,
    pub preset: AgentPreset,
    pub constraints: AgentConstraints,
    pub llm_config: AgentLlmConfig,
}

/// Runtime status of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Busy { task_id: String },
    Waiting { waiting_for: String },
    Error { message: String },
}

impl AgentStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::Busy { .. } => "busy",
            Self::Waiting { .. } => "waiting",
            Self::Error { .. } => "error",
        }
    }

    pub fn is_available(&self) -> bool {
        matches!(self, Self::Idle)
    }
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Busy { task_id } => write!(f, "busy (task: {})", task_id),
            Self::Waiting { waiting_for } => write!(f, "waiting (for: {})", waiting_for),
            Self::Error { message } => write!(f, "error: {}", message),
        }
    }
}

/// A skill the agent has been assigned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub category: String,
    pub proficiency: f32,
}

/// Preset configuration for agent behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPreset {
    pub persona: String,
    pub temperature: f32,
    pub verbosity: String,
}

impl Default for AgentPreset {
    fn default() -> Self {
        Self {
            persona: "You are a helpful assistant.".to_string(),
            temperature: 0.5,
            verbosity: "normal".to_string(),
        }
    }
}

/// Per-agent LLM configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentLlmConfig {
    /// Override model for this agent (None = inherit from orchestrator).
    #[serde(default)]
    pub model: Option<String>,
    /// Fallback models if the primary is unavailable.
    #[serde(default)]
    pub fallback_models: Vec<String>,
    /// Additional provider-specific overrides (e.g. temperature, max_tokens).
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, String>,
}

/// Constraints on agent behavior.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentConstraints {
    pub max_tool_calls: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub max_cost_per_task: Option<f64>,
    /// Maximum agentic loop rounds (overrides daemon default when set).
    #[serde(default)]
    pub max_rounds: Option<usize>,
    #[serde(default)]
    pub require_confirmation_for: Vec<String>,
    #[serde(default)]
    pub allowed_capabilities: Vec<String>,
    #[serde(default)]
    pub denied_capabilities: Vec<String>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub denied_models: Vec<String>,
}

impl AgentConstraints {
    /// Pre-lowercase all constraint list entries so that capability/model
    /// checks can compare against an already-normalized set.
    pub fn normalize(&mut self) {
        for s in &mut self.allowed_capabilities {
            *s = s.to_lowercase();
        }
        for s in &mut self.denied_capabilities {
            *s = s.to_lowercase();
        }
        for s in &mut self.allowed_models {
            *s = s.to_lowercase();
        }
        for s in &mut self.denied_models {
            *s = s.to_lowercase();
        }
    }
}

impl SubAgent {
    /// Hydrate from a storage SubAgentConfig.
    pub fn from_config(config: &openalpaca_storage::SubAgentConfig) -> Self {
        let skills: Vec<Skill> = serde_json::from_str(&config.skills_json).unwrap_or_default();
        let preset: AgentPreset = serde_json::from_str(&config.preset_json).unwrap_or_default();
        let mut constraints: AgentConstraints = config
            .constraints_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        constraints.normalize();
        let llm_config: AgentLlmConfig = config
            .llm_config_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let status = match config.status.as_str() {
            "busy" => AgentStatus::Busy {
                task_id: config.current_task_id.clone().unwrap_or_default(),
            },
            "waiting" => AgentStatus::Waiting {
                waiting_for: "unknown".to_string(),
            },
            "error" => AgentStatus::Error {
                message: "unknown".to_string(),
            },
            _ => AgentStatus::Idle,
        };

        Self {
            id: config.id.clone(),
            template_id: config.id.clone(), // backward compat: template_id = id
            name: config.name.clone(),
            description: config.description.clone(),
            icon: config.icon.clone(),
            status,
            current_task: config.current_task_id.clone(),
            skills,
            preset,
            constraints,
            llm_config,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_status_as_str() {
        assert_eq!(AgentStatus::Idle.as_str(), "idle");
        assert_eq!(
            AgentStatus::Busy {
                task_id: "t1".into()
            }
            .as_str(),
            "busy"
        );
        assert_eq!(
            AgentStatus::Waiting {
                waiting_for: "x".into()
            }
            .as_str(),
            "waiting"
        );
        assert_eq!(
            AgentStatus::Error {
                message: "fail".into()
            }
            .as_str(),
            "error"
        );
    }

    #[test]
    fn test_agent_status_is_available() {
        assert!(AgentStatus::Idle.is_available());
        assert!(
            !AgentStatus::Busy {
                task_id: "t1".into()
            }
            .is_available()
        );
    }

    #[test]
    fn test_agent_status_display() {
        assert_eq!(format!("{}", AgentStatus::Idle), "idle");
        assert_eq!(
            format!(
                "{}",
                AgentStatus::Busy {
                    task_id: "t1".into()
                }
            ),
            "busy (task: t1)"
        );
    }

    #[test]
    fn test_default_preset() {
        let p = AgentPreset::default();
        assert_eq!(p.temperature, 0.5);
        assert_eq!(p.verbosity, "normal");
    }

    #[test]
    fn test_default_constraints() {
        let c = AgentConstraints::default();
        assert!(c.max_tool_calls.is_none());
        assert!(c.require_confirmation_for.is_empty());
        assert!(c.allowed_capabilities.is_empty());
        assert!(c.denied_capabilities.is_empty());
        assert!(c.allowed_models.is_empty());
        assert!(c.denied_models.is_empty());
    }

    #[test]
    fn test_constraints_deserialize_without_capabilities() {
        // Existing JSON without new fields should deserialize fine via #[serde(default)]
        let json = r#"{"max_tool_calls":10,"require_confirmation_for":[]}"#;
        let c: AgentConstraints = serde_json::from_str(json).unwrap();
        assert_eq!(c.max_tool_calls, Some(10));
        assert!(c.allowed_capabilities.is_empty());
        assert!(c.denied_capabilities.is_empty());
    }

    #[test]
    fn test_constraints_with_capabilities() {
        let json = r#"{
            "max_tool_calls": 5,
            "require_confirmation_for": [],
            "allowed_capabilities": ["web_search", "summarize"],
            "denied_capabilities": ["shell_execute"]
        }"#;
        let c: AgentConstraints = serde_json::from_str(json).unwrap();
        assert_eq!(c.allowed_capabilities, vec!["web_search", "summarize"]);
        assert_eq!(c.denied_capabilities, vec!["shell_execute"]);
    }
}
