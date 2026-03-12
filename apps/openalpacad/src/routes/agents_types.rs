//! Request/response types for agent management endpoints.

use openalpaca_core::agent::AgentConfigFile;
use openalpaca_storage::{AgentMetrics, SubAgentConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ListAgentsQuery {
    pub status: Option<String>,
    pub skill: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AgentActionRequest {
    pub action: String, // "pause", "resume"
}

#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub agent: SubAgentConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<AgentMetrics>,
}

#[derive(Debug, Serialize)]
pub struct AgentConfigResponse {
    pub config: AgentConfigFile,
    pub config_version: u64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentConfigRequest {
    pub config: AgentConfigFile,
    pub config_version: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentFromTomlRequest {
    pub toml_content: String,
}

/// JSON representation of an agent template for the REST API.
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub singleton: bool,
    pub capabilities: Vec<String>,
    pub denied_capabilities: Vec<String>,
    pub temperature: f32,
    pub verbosity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub fallback_models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost_per_task: Option<f64>,
    pub require_confirmation_for: Vec<String>,
    pub persona: String,
    pub body: String,
}

impl TemplateResponse {
    pub fn from_template(t: &openalpaca_core::agent::AgentTemplate) -> Self {
        let persona = openalpaca_core::agent::template::extract_persona(t);
        let fm = &t.frontmatter;
        Self {
            id: fm.id.clone(),
            name: fm.name.clone(),
            description: fm.description.clone(),
            icon: fm.icon.clone(),
            singleton: fm.singleton,
            capabilities: fm.capabilities.clone(),
            denied_capabilities: fm.denied_capabilities.clone(),
            temperature: fm.temperature,
            verbosity: fm.verbosity.clone(),
            model: fm.model.clone(),
            fallback_models: fm.fallback_models.clone(),
            max_tool_calls: fm.max_tool_calls,
            timeout_seconds: fm.timeout_seconds,
            max_cost_per_task: fm.max_cost_per_task,
            require_confirmation_for: fm.require_confirmation_for.clone(),
            persona,
            body: t.body.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTemplateRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct CreateTemplateFromMarkdownRequest {
    pub markdown: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTemplateRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct SpawnInstanceRequest {
    pub task_id: String,
}

/// JSON representation of an active agent instance.
#[derive(Debug, Serialize)]
pub struct InstanceResponse {
    pub id: String,
    pub template_id: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
}
