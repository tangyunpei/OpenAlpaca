use crate::agent::subagent::{
    AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Capability, SubAgent,
};
use crate::agent::template::{AgentSource, AgentTemplate, AgentTemplateFrontmatter};
use std::collections::HashMap;

pub(crate) fn make_agent(id: &str, capabilities: Vec<&str>) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: format!("Agent {}", id),
        description: Some(format!("{} agent", id)),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: capabilities
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
/// Templates with "orchestration" capability are marked singleton
/// (matching production behavior where the lead agent is the singleton).
pub(crate) fn template_from_agent(agent: &SubAgent) -> AgentTemplate {
    let is_lead = agent.capabilities.iter().any(|c| c.name == "orchestration");
    AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: agent.template_id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone().unwrap_or_default(),
            icon: agent.icon.clone(),
            singleton: is_lead,
            capabilities: agent.capabilities.iter().map(|s| s.name.clone()).collect(),
            denied_capabilities: vec![],
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
        source: AgentSource::default(),
    }
}

/// Serializes every test in this crate that re-points `OPENALPACA_HOME_STORE`.
///
/// The variable is process-global and every store accessor reads it on each
/// call, so two modules holding *separate* locks would still race. This is the
/// crate's one lock; `config_io` and the artifact tools both take it.
static HOME_STORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Points `OPENALPACA_HOME_STORE` at a temp root for the guard's lifetime.
/// No test ever touches the real `~/.openalpaca`.
pub(crate) struct HomeStoreGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

impl HomeStoreGuard {
    pub(crate) fn set(path: &std::path::Path) -> Self {
        let lock = HOME_STORE_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(openalpaca_storage::store::HOME_STORE_ENV);
        // SAFETY: serialized by HOME_STORE_ENV_LOCK — the crate's only writer.
        unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, path) };
        Self { _lock: lock, prev }
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding the lock.
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, v) },
            None => unsafe { std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV) },
        }
    }
}
