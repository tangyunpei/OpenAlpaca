use crate::agent::subagent::{
    AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Capability, SubAgent,
};
use crate::agent::template::{AgentTemplate, AgentTemplateFrontmatter};
use std::collections::HashMap;

pub(crate) fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: format!("Agent {}", id),
        description: Some(format!("{} agent", id)),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: skills
            .into_iter()
            .map(|s| Capability {
                name: s.to_string(),
                category: "test".to_string(),
                proficiency: 1.0,
            })
            .collect(),
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

/// Create a minimal AgentTemplate from a SubAgent (for test setup).
/// Templates with "lead_orchestration" skill are marked singleton
/// (matching production behavior where the lead agent is the singleton).
pub(crate) fn template_from_agent(agent: &SubAgent) -> AgentTemplate {
    let is_lead = agent.capabilities.iter().any(|s| s.name == "lead_orchestration");
    AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: agent.template_id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone().unwrap_or_default(),
            icon: agent.icon.clone(),
            singleton: is_lead,
            skills: agent.capabilities.iter().map(|s| s.name.clone()).collect(),
            denied_skills: vec![],
            temperature: agent.preset.temperature,
            verbosity: agent.preset.verbosity.clone(),
            model: agent.llm_config.model.clone(),
            fallback_models: agent.llm_config.fallback_models.clone(),
            max_tool_calls: agent.constraints.max_tool_calls,
            timeout_seconds: agent.constraints.timeout_seconds,
            max_cost_per_task: agent.constraints.max_cost_per_task,
            max_rounds: agent.constraints.max_rounds,
            require_confirmation_for: agent.constraints.require_confirmation_for.clone(),
        },
        body: String::new(),
        sections: HashMap::new(),
    }
}
