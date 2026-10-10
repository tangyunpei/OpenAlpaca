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

/// A populated `USER.md`, the shape the persona layer renders as a
/// `### USER PROFILE ###` block (plan 3.3: injected exactly once).
pub(crate) const PROFILE_DOC: &str = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

# USER.md -- About Your Human

## Identity

* Name: Alex
* Timezone: PST (UTC-8)

## Communication Style

Prefers concise, technical responses. No filler or sycophancy.
"#;

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

/// A guarded temp home store and one temp project folder registered under
/// it (storage design, Stage 2): every test that writes, edits, lists or
/// runs a command does it here — never in the repository, never in the
/// process's current directory, never in `~/.openalpaca`. `_guard` is
/// declared first so it drops first: the variable is restored before the
/// folders go.
pub(crate) struct GuardedProject {
    _guard: HomeStoreGuard,
    pub(crate) home: tempfile::TempDir,
    pub(crate) project: tempfile::TempDir,
    /// The project's canonical root, as `projects::register` recorded it.
    pub(crate) root: std::path::PathBuf,
}

impl GuardedProject {
    /// Two tempdirs: the home store pointed at the first, the second
    /// registered as a project under it.
    pub(crate) fn new() -> Self {
        let home = tempfile::tempdir().expect("a temp home store");
        let project = tempfile::tempdir().expect("a temp project folder");
        let guard =
            HomeStoreGuard::set(&home.path().canonicalize().expect("the temp home resolves"));
        let root = openalpaca_storage::store::projects::register(
            &project
                .path()
                .canonicalize()
                .expect("the temp project resolves"),
        )
        .expect("the temp project registers under the temp home")
        .root;
        Self {
            _guard: guard,
            home,
            project,
            root,
        }
    }

    /// A project turn: its owner, the project as the request's root (and as
    /// memory's), and a conversation.
    pub(crate) fn ctx(&self) -> crate::tools::registry::ToolContext {
        let root = self.root.to_string_lossy().into_owned();
        crate::tools::registry::ToolContext {
            owner_id: Some("owner-test".to_string()),
            workspace_id: Some(root.clone()),
            request_workspace_root: Some(root),
            conversation_id: Some("conv-test".to_string()),
            ..Default::default()
        }
    }

    /// A turn with no project: its conversation, and nothing else. A test
    /// that needs an owner sets `owner_id` over it.
    pub(crate) fn scratch_ctx(&self) -> crate::tools::registry::ToolContext {
        crate::tools::registry::ToolContext {
            conversation_id: Some("conv-test".to_string()),
            ..Default::default()
        }
    }

    /// The home store's root, canonical: what `store::home_root()` answers
    /// while the guard holds.
    pub(crate) fn home_root(&self) -> std::path::PathBuf {
        self.home
            .path()
            .canonicalize()
            .expect("the temp home resolves")
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

    /// The same, every answer stopped at its output ceiling
    /// (`FinishReason::MaxTokens`): a model cut off mid-answer.
    pub(crate) fn cut(reply: impl Into<String>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            seen: std::sync::Mutex::new(Vec::new()),
            reply: reply.into(),
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

    /// How many requests reached this provider — for a test whose point is
    /// how many calls were made.
    pub(crate) fn request_count(&self) -> usize {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Every request that reached this provider, in arrival order.
    pub(crate) fn requests(&self) -> Vec<openalpaca_llm::ChatRequest> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
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

/// Storage's `store::projects::register` refuses an unguarded home store only
/// in a build that carries its `test-home-guard` feature, which this crate
/// turns on through `[dev-dependencies]` (storage design, Stage 1, S1.9).
#[test]
fn the_storage_test_home_guard_is_armed_in_this_test_build() {
    assert!(openalpaca_storage::store::projects::test_home_guard_armed());
}

/// R45 tests (storage design §11): a process a test's own command started,
/// sent SIGKILL when this guard drops, so an assertion that fails before the
/// code under test killed it leaves no orphan behind.
#[cfg(unix)]
pub(crate) struct KillOnFail(pub(crate) i32);

#[cfg(unix)]
impl Drop for KillOnFail {
    fn drop(&mut self) {
        // SAFETY: `kill` only sends a signal, to the pid this test read from
        // the file its own command wrote; one already gone answers ESRCH.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
        }
    }
}

/// How long an R45 test waits for a killed process to be gone. The kill is
/// immediate, but the system reaps an orphan asynchronously, and a loaded CI
/// machine is slow.
#[cfg(unix)]
pub(crate) const GONE_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

/// The pid a test's command wrote to `path`, waiting up to `within` for it.
#[cfg(unix)]
pub(crate) async fn pid_from_file(path: &std::path::Path, within: std::time::Duration) -> i32 {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<i32>()
        {
            return pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no pid in {} after {within:?}",
            path.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Whether `pid` has ended: `kill(pid, 0)` answers ESRCH or, on Linux, the
/// process is a zombie nobody has reaped yet (`kill` still finds one).
#[cfg(unix)]
pub(crate) fn process_is_gone(pid: i32) -> bool {
    // SAFETY: signal 0 sends nothing; it only asks whether `pid` exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // `pid (comm) S …`: the state follows the last `)`.
            return stat
                .rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'));
        }
    }
    false
}

/// Polls [`process_is_gone`] for up to `within`.
#[cfg(unix)]
pub(crate) async fn wait_until_gone(pid: i32, within: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if process_is_gone(pid) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    process_is_gone(pid)
}
