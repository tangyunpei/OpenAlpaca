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

/// A provider that answers from a canned script and keeps every request it
/// was handed — the offline seam the M2 utility-call tests assert against.
///
/// It never opens a socket: the router hands a `ChatRequest` to whatever
/// provider is registered for the model's type, so registering this one is
/// enough to see exactly what an internal call asked the model for
/// (`thinking`, `max_tokens`, the messages) without a mock HTTP server, a
/// key, or the real Ollama.
pub(crate) struct RecordingProvider {
    /// Every request the router routed here, in arrival order.
    seen: std::sync::Mutex<Vec<openalpaca_llm::ChatRequest>>,
    /// What to answer with — one body for every call.
    reply: String,
    /// How the answer ended. `MaxTokens` with an empty `reply` is the shape a
    /// thinking model on a small budget produces: the budget went into
    /// reasoning and nothing was written (M2).
    finish_reason: openalpaca_llm::FinishReason,
}

impl RecordingProvider {
    pub(crate) fn new(reply: impl Into<String>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            seen: std::sync::Mutex::new(Vec::new()),
            reply: reply.into(),
            finish_reason: openalpaca_llm::FinishReason::Stop,
        })
    }

    /// A provider whose answer is nothing at all, because the output budget
    /// was spent before a character was written.
    pub(crate) fn spent_budget() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            seen: std::sync::Mutex::new(Vec::new()),
            reply: String::new(),
            finish_reason: openalpaca_llm::FinishReason::MaxTokens,
        })
    }

    /// The first request this provider saw, panicking when it saw none —
    /// "the call never reached a provider" is the failure worth reporting.
    pub(crate) fn first_request(&self) -> openalpaca_llm::ChatRequest {
        self.first_request_opt()
            .expect("the call under test never reached a provider")
    }

    /// The same, for a test whose point is that **no** call was made — a tier
    /// that answers without a model (A1).
    pub(crate) fn first_request_opt(&self) -> Option<openalpaca_llm::ChatRequest> {
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        seen.first().cloned()
    }
}

#[async_trait::async_trait]
impl openalpaca_llm::LlmProvider for RecordingProvider {
    fn name(&self) -> &str {
        "recording"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(
        &self,
        request: openalpaca_llm::ChatRequest,
    ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
        let model = request.model.clone().unwrap_or_else(|| "recorded".into());
        self.seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(request);
        Ok(openalpaca_llm::ChatResponse {
            content: self.reply.clone(),
            tool_calls: Vec::new(),
            model,
            usage: openalpaca_llm::Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
            finish_reason: self.finish_reason.clone(),
            thinking: None,
            parts: None,
        })
    }
}

/// A router whose every call lands on `provider`, under a model id the
/// compiled-in registry already knows (so nothing has to be registered).
pub(crate) fn router_recording(
    provider: std::sync::Arc<RecordingProvider>,
) -> std::sync::Arc<openalpaca_llm::LlmRouter> {
    std::sync::Arc::new(openalpaca_llm::LlmRouter::single_provider(
        provider,
        openalpaca_llm::ProviderType::Anthropic,
        RECORDED_MODEL.to_string(),
    ))
}

/// The model the recording router defaults to: a compiled-in Anthropic id, so
/// `ModelRegistry::with_defaults()` resolves it to the registered provider.
pub(crate) const RECORDED_MODEL: &str = "claude-sonnet-4-6";
