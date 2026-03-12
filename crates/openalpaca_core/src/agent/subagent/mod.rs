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
    pub capabilities: Vec<Capability>,
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

/// A capability the agent has been assigned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
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
    /// When true, skip interactive confirmations for this agent.
    #[serde(default)]
    pub auto_approve: bool,
    /// Sections to exclude from ContextPackage (e.g. ["conversation_summary", "user_context"]).
    #[serde(default)]
    pub denied_sections: Vec<String>,
    /// Maximum total context tokens for this agent. Overrides model default if set.
    #[serde(default)]
    pub max_context_tokens: Option<usize>,
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
        for s in &mut self.denied_sections {
            *s = s.to_lowercase();
        }
    }
}

impl SubAgent {
    /// Hydrate from a storage SubAgentConfig.
    pub fn from_config(config: &openalpaca_storage::SubAgentConfig) -> Self {
        let capabilities: Vec<Capability> = serde_json::from_str(&config.skills_json).unwrap_or_default();
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
            capabilities,
            preset,
            constraints,
            llm_config,
        }
    }
}

#[cfg(test)]
mod tests;
