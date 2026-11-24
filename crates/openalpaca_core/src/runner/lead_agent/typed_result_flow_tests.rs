//! SA-4's integration layer (design §5.3.7, §9's C9 tests; the owner's
//! second layer, an in-crate module of its own): a lead's
//! `spawn_subagent` with a `result_schema`, through the production spawn
//! path, the child's own loop, its `submit_result`, the lane's close, the
//! tracker, `check_subagent_status` and `wait_for_subagents`.
//!
//! Every child here answers from a scripted provider of this module's own
//! (one reply popped per request, each request kept), under a guarded temp
//! home and project, a temp session log and a temp database. No real home,
//! credential, model or network.

use super::typed_result::{ResultShape, register_submit_result};
use super::*;
use crate::session_log::{SessionLogHandle, SessionLogLimits, SessionLogService, read_records};
use crate::test_util::GuardedProject;
use openalpaca_llm::{
    ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, ProviderType, Role, ToolCall,
    Usage,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

const MODEL: &str = "claude-sonnet-5-5";

/// One reply per request, popped in order; every request kept. A request
/// past the script hangs when `hang` is set (so a test can cancel a run
/// mid-call), else is refused.
struct Script {
    replies: Mutex<VecDeque<ChatResponse>>,
    seen: Mutex<Vec<ChatRequest>>,
    hang: bool,
}

impl Script {
    fn new(replies: Vec<ChatResponse>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
            hang: false,
        })
    }

    fn hanging(replies: Vec<ChatResponse>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
            hang: true,
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.seen.lock().unwrap().clone()
    }

    /// The requests whose objective — the last user turn the spawn handed
    /// the child — contains `key`.
    fn requests_for(&self, key: &str) -> Vec<ChatRequest> {
        self.requests()
            .into_iter()
            .filter(|r| {
                r.messages
                    .iter()
                    .any(|m| m.role == Role::User && m.content.contains(key))
            })
            .collect()
    }
}

impl LlmProvider for Script {
    fn name(&self) -> &str {
        "script"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen.lock().unwrap().push(request);
        let next = self.replies.lock().unwrap().pop_front();
        match next {
            Some(reply) => Ok(reply),
            None if self.hang => std::future::pending().await,
            None => Err(LlmError::Transport {
                kind: openalpaca_llm::error::TransportKind::TlsOrOther,
                message: "script exhausted".to_string(),
            }),
        }
    }
}

fn reply(content: &str, tool_calls: Vec<ToolCall>, finish: FinishReason) -> ChatResponse {
    ChatResponse {
        content: content.to_string(),
        tool_calls,
        model: MODEL.to_string(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        finish_reason: finish,
        thinking: None,
        echo: None,
        parts: None,
    }
}

/// A round that ends with text and no call.
fn text(content: &str) -> ChatResponse {
    reply(content, vec![], FinishReason::Stop)
}

/// A round of calls, with `content` beside them.
fn calls(content: &str, tool_calls: Vec<ToolCall>) -> ChatResponse {
    reply(content, tool_calls, FinishReason::ToolUse)
}

fn call(id: &str, name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    }
}

fn submit(id: &str, result: Value) -> ToolCall {
    call(id, "submit_result", json!({ "result": result }))
}

/// A tool a template can be granted (`notes`): answers `noted`.
struct Note;

impl crate::tools::registry::BuiltInTool for Note {
    async fn execute(&self, _arguments: &Value) -> Result<String, String> {
        Ok("noted".to_string())
    }
}

/// The base registry every child resolves from: `note` under `notes`.
fn base_registry() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::default();
    registry
        .register(crate::tools::registry::RegisteredTool {
            definition: ToolDefinition {
                name: "note".to_string(),
                description: "Write a note.".to_string(),
                parameters: json!({"type": "object", "properties": {}}),
                strict: None,
                input_examples: None,
            },
            backend: crate::tools::registry::ToolBackend::built_in(Note),
            provides_capabilities: vec!["notes".to_string()],
            exempt_from_timeout: false,
            annotations: None,
            version: "test".to_string(),
            author: "builtin".to_string(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    Arc::new(registry)
}

/// A template, as the tests need it.
#[derive(Clone)]
struct Template {
    id: &'static str,
    capabilities: Vec<&'static str>,
    max_tool_calls: Option<u32>,
    max_rounds: Option<usize>,
    max_cost: Option<f64>,
}

fn worker() -> Template {
    Template {
        id: "worker",
        capabilities: vec!["notes"],
        max_tool_calls: None,
        max_rounds: None,
        max_cost: None,
    }
}

/// One lead's spawn tool over a scripted child, with its tracker, its
/// session log and its database, under a guarded home and project.
struct Lead {
    _project: GuardedProject,
    _dir: tempfile::TempDir,
    log_dir: std::path::PathBuf,
    handle: SessionLogHandle,
    db: openalpaca_storage::Database,
    shared: Arc<SharedContext>,
    tracker: Arc<SubagentTracker>,
    spawn: Arc<SpawnSubagentTool>,
    registry: Arc<ToolRegistry>,
    token: tokio_util::sync::CancellationToken,
}

fn start(script: Arc<Script>, templates: &[Template], config: DaemonConfig) -> Lead {
    let project = GuardedProject::new();
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    // The run's row and each template's, which the history's foreign keys
    // point at — what the dispatcher and the daemon's boot sync write.
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO task (id, title, created_by, source_lane)
             VALUES ('task-1', 'A run', 'user-1', 'user-1:cli')",
            [],
        )?;
        for t in templates.iter().map(|t| t.id).chain(["plugin_worker"]) {
            conn.execute(
                "INSERT INTO agent (id, name, template_id) VALUES (?1, ?1, ?1)",
                [t],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let service = SessionLogService::new(
        dir.path().join("sessions"),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-lead");
    let shared = Arc::new(SharedContext::new());
    shared.register_task_session_log("task-1", handle.clone());
    for t in templates {
        let agent = crate::test_util::make_agent(t.id, t.capabilities.clone());
        let mut template = crate::test_util::template_from_agent(&agent);
        template.frontmatter.max_tool_calls = t.max_tool_calls;
        template.frontmatter.max_rounds = t.max_rounds;
        template.frontmatter.max_cost_per_task = t.max_cost;
        assert!(shared.agent_registry.register_template(template));
    }
    let router = Arc::new(LlmRouter::single_provider(
        script,
        ProviderType::Anthropic,
        MODEL.to_string(),
    ));
    let tracker = Arc::new(SubagentTracker::new());
    let registry = base_registry();
    let token = tokio_util::sync::CancellationToken::new();
    let spawn = Arc::new(SpawnSubagentTool::new(
        router,
        registry.clone(),
        shared.clone(),
        EventBus::new(256),
        Some(db.clone()),
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(config)),
        Some(token.clone()),
        tracker.clone(),
        0,
        4,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
        None,
    ));
    Lead {
        _project: project,
        log_dir: dir.path().join("sessions").join("sess-lead"),
        _dir: dir,
        handle,
        db,
        shared,
        tracker,
        spawn,
        registry,
        token,
    }
}

/// The run id a spawn's reply names.
fn run_id_of(reply: &str) -> String {
    let start = reply.find("run_id: '").expect("a run id") + "run_id: '".len();
    let end = start + reply[start..].find('\'').unwrap();
    reply[start..end].to_string()
}

impl Lead {
    async fn spawn(&self, arguments: Value) -> Result<String, String> {
        self.spawn.execute(&arguments).await
    }

    /// Spawn and wait for every child to finish; the run id.
    async fn run(&self, arguments: Value) -> String {
        let reply = self.spawn(arguments).await.expect("spawned");
        self.finished().await;
        run_id_of(&reply)
    }

    async fn finished(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !self.tracker.all_done() {
            assert!(
                std::time::Instant::now() < deadline,
                "a subagent never finished"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn check(&self, run_id: &str) -> Result<String, String> {
        CheckSubagentStatusTool {
            tracker: self.tracker.clone(),
        }
        .execute(&json!({ "subagent_run_id": run_id }))
        .await
    }

    async fn wait(&self) -> String {
        WaitForSubagentsTool {
            tracker: self.tracker.clone(),
            steering: None,
        }
        .execute(&json!({}))
        .await
        .unwrap()
    }

    async fn records(&self) -> Vec<crate::session_log::LoggedRecord> {
        assert!(self.handle.flush().await);
        read_records(&self.log_dir).unwrap()
    }

    /// The data of every record of `kind`.
    async fn data_of(&self, kind: &str) -> Vec<Value> {
        self.records()
            .await
            .into_iter()
            .filter(|r| r.kind == kind)
            .map(|r| r.data)
            .collect()
    }

    fn usage_statuses(&self) -> Vec<String> {
        openalpaca_storage::repository::LlmUsageRepository::new(&self.db)
            .get_task_usage("task-1", 50)
            .unwrap()
            .into_iter()
            .map(|row| row.status)
            .collect()
    }

    fn history_statuses(&self) -> Vec<String> {
        openalpaca_storage::SubAgentRepository::new(&self.db)
            .get_history_for_task("task-1")
            .unwrap()
            .into_iter()
            .map(|row| row.status)
            .collect()
    }

    /// The tracker a resumed run rebuilds from this run's log.
    async fn restored(&self) -> SubagentTracker {
        assert!(self.handle.flush().await);
        let plan = crate::session_log::replay::rebuild(&self.log_dir, "task-1", 4).unwrap();
        let tracker = SubagentTracker::new();
        tracker.restore(&plan.subagents);
        tracker
    }
}

/// `summary` (`wait_for_subagents`'s answer) holds the entry `line`, whole,
/// after its header.
fn assert_line(summary: &str, line: String) {
    assert!(
        summary.contains(&format!("\n{line}")),
        "{summary}\n--- expected:\n{line}"
    );
}

/// The schema most tests use.
fn counted() -> Value {
    json!({
        "type": "object",
        "properties": {"n": {"type": "integer"}},
        "required": ["n"],
        "additionalProperties": false
    })
}

/// The child's tool results in its `request`-th request, with whether each
/// is an error.
fn tool_results(request: &ChatRequest) -> Vec<(String, bool)> {
    request
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| (m.content.clone(), m.is_error))
        .collect()
}

fn system_prompt(request: &ChatRequest) -> String {
    request
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_names(request: &ChatRequest) -> Vec<String> {
    request.tools.iter().map(|t| t.name.clone()).collect()
}

const PLAIN_SENTENCE: &str =
    "Start with a brief summary of what you accomplished, followed by the detailed output.";
const TYPED_SENTENCE: &str =
    "Deliver it by calling submit_result: your final text is not the result.";

// ── A typed child, end to end ────────────────────────────────────────

#[tokio::test]
async fn a_valid_submission_is_returned_typed() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!({"n": 3}))])]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;

    assert_eq!(
        lead.check(&run_id).await.unwrap(),
        format!(
            "Subagent '{run_id}' completed successfully: result (matches result_schema): {{\"n\":3}}"
        )
    );
    let done = lead.data_of("workflow_done").await;
    assert_eq!(done[0]["finish_reason"], "result_submitted");
    assert_eq!(script.requests().len(), 1, "an accepted call ends the run");
    assert_eq!(lead.usage_statuses(), ["success"]);
    assert_eq!(lead.history_statuses(), ["completed"]);
}

#[tokio::test]
async fn an_invalid_submission_is_answered_with_its_violations_as_the_tools_error() {
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": "three"}))]),
        calls("", vec![submit("s2", json!({"n": 3}))]),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;

    let second = &script.requests()[1];
    assert_eq!(
        tool_results(second),
        [(
            "[tool_error] The result does not match result_schema:\n/n: expected integer, got string"
                .to_string(),
            true
        )]
    );
    // No user turn is added: the round ends on the tool's result.
    assert_eq!(second.messages.last().unwrap().role, Role::Tool);
}

#[tokio::test]
async fn a_fourth_rejection_ends_the_child_with_the_typed_failure() {
    let script = Script::new(
        (1..=5)
            .map(|i| calls("", vec![submit(&format!("s{i}"), json!({"n": "x"}))]))
            .collect(),
    );
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;

    assert_eq!(script.requests().len(), 4, "four submissions in all");
    let typed = format!(
        "ended without a result matching its result_schema (4 rejected; at most 4): /n: \
         expected integer, got string. Its last answer is kept: check_subagent_status {run_id}."
    );
    // The wait first: a check reports the entry, and a later wait names it.
    let summary = lead.wait().await;
    assert_line(
        &summary,
        format!("- **{run_id}**: failed — {typed} Last output: {{\"n\":\"x\"}}"),
    );
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!("Subagent '{run_id}' failed: {typed}\nIts last output follows:\n{{\"n\":\"x\"}}")
    );
    let done = lead.data_of("workflow_done").await;
    assert_eq!(done[0]["finish_reason"], "result_rejected");
    assert!(
        lead.data_of("error").await.is_empty(),
        "a rejection is not a loop error"
    );
}

#[tokio::test]
async fn a_json_string_result_is_validated_as_parsed_once() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!("{\"n\": 3}"))])]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        lead.check(&run_id).await.unwrap(),
        format!(
            "Subagent '{run_id}' completed successfully: result (matches result_schema): {{\"n\":3}}"
        )
    );
}

#[tokio::test]
async fn a_child_ending_without_a_submission_is_nudged_once_then_fails() {
    let script = Script::new(vec![text("here it is: 3"), text("still just text")]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;

    let requests = script.requests();
    assert_eq!(requests.len(), 2, "one nudge, no more");
    let nudge = requests[1].messages.last().unwrap();
    assert_eq!(nudge.role, Role::User);
    assert_eq!(
        nudge.content,
        "You ended without calling submit_result. Call it now with a result matching the schema."
    );
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!(
            "Subagent '{run_id}' failed: ended without submitting a result: still just \
             text.\nIts last output follows:\nstill just text"
        )
    );
    let done = lead.data_of("workflow_done").await;
    assert_eq!(
        done[0]["rounds_used"], 2,
        "the nudge's round is an ordinary one"
    );
}

#[tokio::test]
async fn a_typed_child_at_max_rounds_fails_without_a_nudge() {
    let script = Script::new(vec![text("done: 3"), text("never asked")]);
    let template = Template {
        max_rounds: Some(1),
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(script.requests().len(), 1, "no round left, so no nudge");
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!(
            "Subagent '{run_id}' failed: ended without submitting a result: done: 3.\nIts last \
             output follows:\ndone: 3"
        )
    );
}

#[tokio::test]
async fn the_submission_is_never_forced_by_tool_choice() {
    let script = Script::new(vec![
        text("first"),
        calls("", vec![submit("s1", json!({"n": 1}))]),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let requests = script.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.tool_choice.is_none()));
}

#[tokio::test]
async fn submit_result_is_offered_only_to_a_child_spawned_with_a_schema() {
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": 1}))]),
        text("plain answer"),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.spawn(json!({"agent_id": "worker", "objective": "TYPED job", "result_schema": counted()}))
        .await
        .unwrap();
    lead.finished().await;
    lead.spawn(json!({"agent_id": "worker", "objective": "PLAIN job"}))
        .await
        .unwrap();
    lead.finished().await;
    let typed = &script.requests_for("TYPED job")[0];
    let plain = &script.requests_for("PLAIN job")[0];
    assert!(tool_names(typed).contains(&"submit_result".to_string()));
    assert!(!tool_names(plain).contains(&"submit_result".to_string()));
}

#[tokio::test]
async fn a_schemad_child_runs_on_a_clone_that_holds_submit_result_and_the_base_does_not() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!({"n": 1}))])]);
    let lead = start(script, &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    // The child's call reached its own `submit_result` — on its clone.
    assert!(
        lead.check(&run_id)
            .await
            .unwrap()
            .contains("result (matches result_schema)")
    );
    // The shared registry never held it.
    assert!(lead.registry.get("submit_result").is_none());
}

#[tokio::test]
async fn a_schemad_childs_surface_holds_submit_result_though_no_capability_resolves_it() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!({"n": 1}))])]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let sent = script.requests()[0]
        .tools
        .iter()
        .find(|t| t.name == "submit_result")
        .cloned()
        .expect("on the child's surface");
    assert_eq!(
        sent.parameters,
        json!({
            "type": "object",
            "properties": {"result": counted()},
            "required": ["result"],
            "additionalProperties": false
        })
    );
    assert_eq!(sent.strict, Some(true));
    // The registered entry is permissive, and no capability resolves it.
    let registry = ToolRegistry::default();
    register_submit_result(
        &registry,
        Arc::new(ResultShape::admit(&counted()).unwrap()),
        Arc::new(TypedResultSlot::default()),
    );
    let registered = registry.get("submit_result").unwrap();
    assert_eq!(registered.definition.parameters, json!({"type": "object"}));
    assert!(registered.provides_capabilities.is_empty());
    assert!(
        registry
            .resolve_capabilities(&["submit_result".to_string()], &[])
            .defs
            .is_empty()
    );
}

#[tokio::test]
async fn an_empty_allow_list_child_still_submits() {
    use crate::security::capabilities::Allowlist;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    // No spawn reaches an empty allow list today (a template always gains
    // the workspace tools); the admission step is driven directly, then
    // the sandbox is asked to run the call.
    let mut policy = SandboxPolicy::from_constraints(
        "worker::1",
        &crate::agent::subagent::AgentConstraints::default(),
        &crate::daemon_config::AgentDefaults::default(),
    );
    policy.allowed_capabilities = Allowlist::Only(vec![]);
    policy.admit_tool_surface(&[super::typed_result::submit_result_tool_definition(
        &counted(),
    )]);
    assert_eq!(policy.allowed_capabilities, Allowlist::Only(vec![]));
    super::typed_result::admit_submit_result(&mut policy);
    assert_eq!(
        policy.allowed_capabilities,
        Allowlist::Only(vec!["submit_result".to_string()])
    );

    let registry = Arc::new(ToolRegistry::default());
    let slot = Arc::new(TypedResultSlot::default());
    register_submit_result(
        &registry,
        Arc::new(ResultShape::admit(&counted()).unwrap()),
        slot.clone(),
    );
    let sandbox = SandboxManager::new(
        registry,
        EventBus::new(16),
        &crate::daemon_config::CircuitBreakerConfig::default(),
    );
    let answer = sandbox
        .execute_tool(
            &submit("s1", json!({"n": 2})),
            &policy,
            &crate::tools::registry::ToolContext::default(),
        )
        .await;
    assert_eq!(answer.as_deref(), Ok("Result accepted."));
    assert_eq!(slot.verdict(), super::typed_result::SlotVerdict::Accepted);

    // A deny list naming it keeps it out.
    let mut denied = SandboxPolicy::from_constraints(
        "worker::1",
        &crate::agent::subagent::AgentConstraints {
            denied_capabilities: vec!["submit_result".to_string()],
            ..Default::default()
        },
        &crate::daemon_config::AgentDefaults::default(),
    );
    denied.allowed_capabilities = Allowlist::Only(vec![]);
    super::typed_result::admit_submit_result(&mut denied);
    assert_eq!(denied.allowed_capabilities, Allowlist::Only(vec![]));
}

/// A closed object nested `levels` deep, every member required.
fn closed_tower(levels: usize) -> Value {
    let mut node =
        json!({"type": "object", "properties": {}, "required": [], "additionalProperties": false});
    for _ in 1..levels {
        node = json!({
            "type": "object",
            "properties": {"a": node},
            "required": ["a"],
            "additionalProperties": false
        });
    }
    node
}

fn goes_strict(definition: &ToolDefinition) -> bool {
    openalpaca_llm::providers::openai_tools::openai_function_tool(definition)["function"]
        .get("strict")
        == Some(&json!(true))
}

#[tokio::test]
async fn submit_result_goes_strict_only_where_strict_mode_accepts_it() {
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": 1}))]),
        calls("", vec![submit("s2", json!({"n": 1}))]),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.spawn(json!({"agent_id": "worker", "objective": "CLOSED", "result_schema": counted()}))
        .await
        .unwrap();
    lead.finished().await;
    lead.spawn(json!({
        "agent_id": "worker",
        "objective": "OPEN",
        "result_schema": {"type": "object", "properties": {"n": {"type": "integer"}}}
    }))
    .await
    .unwrap();
    lead.finished().await;
    let sent = |key: &str| {
        script.requests_for(key)[0]
            .tools
            .iter()
            .find(|t| t.name == "submit_result")
            .cloned()
            .unwrap()
    };
    assert!(goes_strict(&sent("CLOSED")));
    assert!(
        !goes_strict(&sent("OPEN")),
        "an optional member goes non-strict"
    );
}

#[tokio::test]
async fn a_schema_nine_objects_deep_goes_strict_and_ten_goes_non_strict() {
    let nine = super::typed_result::submit_result_tool_definition(&closed_tower(9));
    let ten = super::typed_result::submit_result_tool_definition(&closed_tower(10));
    assert!(
        goes_strict(&nine),
        "the wrapper's level and nine of the lead's"
    );
    assert!(!goes_strict(&ten), "eleven with the wrapper's");
    // Both are admitted: ten object levels are within the checker's cap.
    let script = Script::new(vec![]);
    let lead = start(script, &[worker()], DaemonConfig::default());
    let reply = lead
        .spawn(json!({"agent_id": "worker", "objective": "ten", "result_schema": closed_tower(10)}))
        .await;
    assert!(reply.is_ok(), "{reply:?}");
    lead.finished().await;
}

#[tokio::test]
async fn the_schema_is_never_in_the_brief() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!({"n": 1}))])]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let mut schema = counted();
    schema["properties"]["n"]["description"] = json!("ZEBRA-MARKER");
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": schema}))
        .await;
    let request = &script.requests()[0];
    assert!(
        request
            .messages
            .iter()
            .all(|m| !m.content.contains("ZEBRA-MARKER")),
        "no schema text in any message"
    );
    let sent = request
        .tools
        .iter()
        .find(|t| t.name == "submit_result")
        .unwrap();
    assert!(sent.parameters.to_string().contains("ZEBRA-MARKER"));
    assert!(!sent.description.contains("ZEBRA-MARKER"));
}

#[tokio::test]
async fn the_output_format_block_swaps_one_sentence_for_a_typed_child() {
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": 1}))]),
        text("plain"),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.spawn(json!({"agent_id": "worker", "objective": "TYPED", "result_schema": counted()}))
        .await
        .unwrap();
    lead.finished().await;
    lead.spawn(json!({"agent_id": "worker", "objective": "PLAIN"}))
        .await
        .unwrap();
    lead.finished().await;
    let typed = system_prompt(&script.requests_for("TYPED")[0]);
    let plain = system_prompt(&script.requests_for("PLAIN")[0]);
    assert!(typed.contains(&format!(
        "<output-format>\nProvide a clear, complete result. {TYPED_SENTENCE} The lead agent will \
         use your result to synthesize a final response, so be thorough and specific.\n\
         </output-format>"
    )), "{typed}");
    assert!(!typed.contains(PLAIN_SENTENCE));
    assert!(plain.contains(PLAIN_SENTENCE));
    assert!(!plain.contains("submit_result"));
    // The tool guidance names the tool and its description, nothing more.
    assert!(
        typed.contains("- submit_result: Deliver your result"),
        "{typed}"
    );
}

#[tokio::test]
async fn the_typed_brief_states_the_tool_budget() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!({"n": 1}))])]);
    let template = Template {
        max_tool_calls: Some(7),
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let prompt = system_prompt(&script.requests()[0]);
    assert!(prompt.contains(&format!(
        "{TYPED_SENTENCE} You have 7 tool calls in all, and submit_result is one of them: keep at \
         least one for it (two, if it may need a repair). A call past 7 is refused, and then no \
         result can be delivered. The lead agent"
    )), "{prompt}");
}

// ── The spawn's refusals ─────────────────────────────────────────────

#[tokio::test]
async fn a_schema_off_the_list_too_large_or_too_deep_refuses_the_spawn() {
    let lead = start(Script::new(vec![]), &[worker()], DaemonConfig::default());
    let refused = |schema: Value| {
        let lead = &lead;
        async move {
            lead.spawn(json!({"agent_id": "worker", "objective": "o", "result_schema": schema}))
                .await
                .unwrap_err()
        }
    };
    assert_eq!(
        refused(json!({"type": "string", "pattern": "^a"})).await,
        "result_schema is not usable: it uses what this checker does not support: pattern at \
         (root); supported: type, enum, const, anyOf, properties, required, \
         additionalProperties (true or false), items (one schema), minItems, maxItems, \
         minimum, maximum, exclusiveMinimum, exclusiveMaximum; not checked: format and the \
         annotations; say a pattern in \"description\" instead."
    );
    assert_eq!(
        refused(json!({"type": "string", "description": "x".repeat(16_385 - 34)})).await,
        "result_schema is not usable: it is more than 16,384 bytes as compact JSON."
    );
    let mut deep = json!({});
    for _ in 1..65 {
        deep = json!({ "items": deep });
    }
    assert_eq!(
        refused(json!({ "items": deep })).await,
        "result_schema is not usable: it nests more than 64 levels."
    );
    assert_eq!(
        refused(json!("{\"type\":\"object\"}")).await,
        "result_schema is not usable: it is a string, not a JSON object."
    );
}

/// The deepest schema the admission takes (64 levels of `items`), admitted
/// inline through the real spawn path on a runtime worker thread — where a
/// lead's tool runs in the daemon — and a value as deep accepted by its
/// child: the inline admission's stack at the cap (design §5.3.8 item 4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_schema_at_the_nesting_cap_is_admitted_through_the_spawn_on_a_worker() {
    let mut schema = json!({});
    for _ in 1..64 {
        schema = json!({ "items": schema });
    }
    let mut value = json!(0);
    for _ in 0..63 {
        value = json!([value]);
    }
    let script = Script::new(vec![calls("", vec![submit("s1", value.clone())])]);
    let lead = start(script, &[worker()], DaemonConfig::default());
    let spawn = lead.spawn.clone();
    let reply = tokio::spawn(async move {
        spawn
            .execute(&json!({"agent_id": "worker", "objective": "nest", "result_schema": schema}))
            .await
    })
    .await
    .expect("the spawn ran on a worker")
    .expect("64 levels are admitted");
    let run_id = run_id_of(&reply);
    lead.finished().await;
    assert_eq!(
        lead.check(&run_id).await.unwrap(),
        format!(
            "Subagent '{run_id}' completed successfully: result (matches result_schema): {}",
            serde_json::to_string(&value).unwrap()
        )
    );
}

/// A plugin-backed template, `plugin_worker` of `test_plugin`.
struct PluginAgent;

impl openalpaca_api::plugin_traits::PluginAgentExecutor for PluginAgent {
    async fn spawn(
        &self,
        _instance_id: &str,
        _task_id: &str,
        _instructions: &str,
        _context: &Value,
    ) -> Result<bool, String> {
        Ok(true)
    }

    async fn step(
        &self,
        _instance_id: &str,
        _tool_results: Option<&Value>,
    ) -> Result<(String, String, Vec<Value>), String> {
        Ok(("complete".to_string(), "plugin result".to_string(), vec![]))
    }

    async fn stop(&self, _instance_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn plugin_id(&self) -> &str {
        "test_plugin"
    }

    fn agent_id(&self) -> &str {
        "plugin_worker"
    }
}

fn register_plugin_worker(shared: &SharedContext) {
    let mut template = crate::test_util::template_from_agent(&crate::test_util::make_agent(
        "plugin_worker",
        vec![],
    ));
    template.source = crate::agent::template::AgentSource::Plugin {
        plugin_id: "test_plugin".to_string(),
        executor: openalpaca_api::plugin_traits::DynPluginAgentExecutor::new_arc(PluginAgent),
    };
    assert!(shared.agent_registry.register_template(template));
}

#[tokio::test]
async fn a_refused_result_schema_spends_no_spawn() {
    let lead = start(Script::new(vec![]), &[worker()], DaemonConfig::default());
    register_plugin_worker(&lead.shared);
    for (agent, schema) in [
        ("worker", json!(true)),
        (
            "worker",
            json!({"type": "string", "description": "x".repeat(16_385 - 34)}),
        ),
        (
            "worker",
            json!({"const": (0..64).fold(json!(0), |v, _| json!([v]))}),
        ),
        ("worker", json!({"oneOf": []})),
        ("plugin_worker", counted()),
    ] {
        let refused = lead
            .spawn(json!({"agent_id": agent, "objective": "o", "result_schema": schema}))
            .await;
        assert!(refused.is_err(), "{agent}: {refused:?}");
    }
    assert_eq!(lead.spawn.spawn_count(), 0, "no slot taken");
    assert_eq!(lead.tracker.status_counts(), (0, 0, 0, 0), "no lane");
    assert!(
        lead.shared.agent_registry.list_instances().is_empty(),
        "no instance"
    );
    assert!(lead.data_of("subagent_open").await.is_empty());
}

#[tokio::test]
async fn result_schema_on_a_plugin_template_is_refused() {
    let lead = start(Script::new(vec![]), &[], DaemonConfig::default());
    register_plugin_worker(&lead.shared);
    assert_eq!(
        lead.spawn(
            json!({"agent_id": "plugin_worker", "objective": "o", "result_schema": counted()})
        )
        .await
        .unwrap_err(),
        "agent 'plugin_worker' is provided by plugin 'test_plugin', which returns its own \
         result: omit result_schema."
    );
}

#[tokio::test]
async fn a_batch_items_result_schema_reaches_its_child() {
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": 1}))]),
        text("plain answer"),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let reply = SpawnSubagentsBatchTool::new(lead.spawn.clone())
        .execute(&json!({"subagents": [
            {"agent_id": "worker", "objective": "TYPED item", "result_schema": counted()},
            {"agent_id": "worker", "objective": "PLAIN item"},
        ]}))
        .await
        .unwrap();
    assert!(
        reply.starts_with("Batch spawn: 2/2 subagents spawned successfully."),
        "{reply}"
    );
    lead.finished().await;
    assert!(
        tool_names(&script.requests_for("TYPED item")[0]).contains(&"submit_result".to_string())
    );
    assert!(
        !tool_names(&script.requests_for("PLAIN item")[0]).contains(&"submit_result".to_string())
    );
}

// ── The caps (O-20 (b): `submit_result` counts) ──────────────────────

#[tokio::test]
async fn repair_rounds_count_against_the_childs_cap() {
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": "x"}))]),
        calls("", vec![submit("s2", json!({"n": "y"}))]),
        calls("", vec![submit("s3", json!({"n": 3}))]),
    ]);
    let template = Template {
        max_rounds: Some(2),
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        script.requests().len(),
        2,
        "each repair is a round under max_rounds"
    );
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!(
            "Subagent '{run_id}' failed: ended without a result matching its result_schema (2 \
             rejected; at most 4): /n: expected integer, got string. Its last answer is kept: \
             check_subagent_status {run_id}.\nIts last output follows:\n{{\"n\":\"y\"}}"
        )
    );
}

#[tokio::test]
async fn a_child_stopped_by_its_cap_mid_repair_fails_typed() {
    // The cost cap ends the run after one rejection.
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": "x"}))]),
        calls("", vec![submit("s2", json!({"n": 3}))]),
    ]);
    let template = Template {
        max_cost: Some(1e-12),
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let done = lead.data_of("workflow_done").await;
    assert_eq!(done[0]["finish_reason"], "cost_exceeded");
    assert!(lead.check(&run_id).await.unwrap_err().starts_with(&format!(
        "Subagent '{run_id}' failed: ended without a result matching its result_schema (1 \
         rejected; at most 4): /n: expected integer, got string."
    )));
    assert_eq!(lead.usage_statuses(), ["result_rejected"]);
}

#[tokio::test]
async fn a_typed_failure_closes_its_lane_failed_and_restores_failed() {
    let truncated = || reply("partial", vec![], FinishReason::MaxTokens);
    let script = Script::new(vec![
        // NUDGED: text, the nudge, text again.
        text("first"),
        text("second"),
        // ROUNDS: one round, then the cap.
        calls("", vec![submit("r1", json!({"n": "x"}))]),
        // TRUNCATED: three cut replies.
        truncated(),
        truncated(),
        truncated(),
        // REJECTED: four rejections.
        calls("", vec![submit("j1", json!({}))]),
        calls("", vec![submit("j2", json!({}))]),
        calls("", vec![submit("j3", json!({}))]),
        calls("", vec![submit("j4", json!({}))]),
        // DONE: accepted.
        calls("", vec![submit("d1", json!({"n": 5}))]),
    ]);
    let short = Template {
        id: "short",
        max_rounds: Some(1),
        ..worker()
    };
    let lead = start(script, &[worker(), short], DaemonConfig::default());
    let mut runs = Vec::new();
    for (agent, objective) in [
        ("worker", "NUDGED"),
        ("short", "ROUNDS"),
        ("worker", "TRUNCATED"),
        ("worker", "REJECTED"),
        ("worker", "DONE"),
    ] {
        let run_id = lead
            .run(json!({"agent_id": agent, "objective": objective, "result_schema": counted()}))
            .await;
        runs.push(run_id);
    }
    let finishes: Vec<String> = lead
        .data_of("workflow_done")
        .await
        .iter()
        .map(|d| d["finish_reason"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        finishes,
        [
            "complete",
            "max_rounds",
            "truncated",
            "result_rejected",
            "result_submitted"
        ]
    );
    let closes: Vec<String> = lead
        .data_of("subagent_close")
        .await
        .iter()
        .map(|d| d["state"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(closes, ["failed", "failed", "failed", "failed", "done"]);
    assert_eq!(
        lead.usage_statuses()
            .iter()
            .filter(|s| *s == "result_rejected")
            .count(),
        4
    );
    let mut history = lead.history_statuses();
    history.sort();
    assert_eq!(
        history,
        ["completed", "failed", "failed", "failed", "failed"]
    );
    // Every typed open says so.
    assert!(
        lead.data_of("subagent_open")
            .await
            .iter()
            .all(|d| d["typed"] == true)
    );

    let restored = lead.restored().await;
    for run_id in &runs[..4] {
        match (restored.get(run_id), lead.tracker.get(run_id)) {
            (
                Some(SubagentStatus::Failed { error, .. }),
                Some(SubagentStatus::Failed { error: live, .. }),
            ) => assert_eq!(error, live, "{run_id}"),
            other => panic!("{run_id}: {other:?}"),
        }
    }
    match restored.get(&runs[4]) {
        Some(SubagentStatus::Completed { content, success }) => {
            assert!(success);
            assert_eq!(content, "result (matches result_schema): {\"n\":5}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_typed_failure_keeps_its_fixed_sentences_under_the_512_byte_cut() {
    let schema = json!({"items": {"type": "string"}});
    let many = json!(vec![1; 30]);
    let mut script: Vec<ChatResponse> = (1..=4)
        .map(|i| calls("", vec![submit(&format!("s{i}"), many.clone())]))
        .collect();
    // A second child, capped at two calls, ends with the cap's clause.
    script.extend([
        calls("", vec![submit("c1", many.clone())]),
        calls("", vec![submit("c2", many.clone())]),
        text("out of calls"),
    ]);
    let script = Script::new(script);
    let capped = Template {
        id: "capped",
        max_tool_calls: Some(2),
        ..worker()
    };
    let lead = start(script, &[worker(), capped], DaemonConfig::default());
    let first = lead
        .run(json!({"agent_id": "worker", "objective": "many", "result_schema": schema}))
        .await;
    let second = lead
        .run(json!({"agent_id": "capped", "objective": "many", "result_schema": schema}))
        .await;
    let summary = lead.wait().await;
    let line = |run_id: &str| {
        summary
            .lines()
            .find(|l| l.starts_with(&format!("- **{run_id}**")))
            .map(str::to_string)
            .unwrap_or_else(|| panic!("{summary}"))
    };
    let first_line = line(&first);
    assert!(first_line.starts_with(&format!(
        "- **{first}**: failed — ended without a result matching its result_schema (4 rejected; \
         at most 4): /0: expected string, got integer; "
    )), "{first_line}");
    assert!(
        summary.contains(&format!(
            ". Its last answer is kept: check_subagent_status {first}."
        )),
        "{summary}"
    );
    assert!(
        summary.contains("elided"),
        "the violations are cut: {summary}"
    );
    assert!(summary.contains(&format!(
        "- **{second}**: failed — ended without a result matching its result_schema (2 rejected; \
         it used all 2 of its tool calls (max_tool_calls) before a fourth): /0:"
    )), "{summary}");
    assert!(
        summary.contains(&format!(
            ". Its last answer is kept: check_subagent_status {second}."
        )),
        "{summary}"
    );
}

#[tokio::test]
async fn a_cancelled_typed_child_closes_cancelled_with_no_typed_failure_text() {
    // `/cancel` of the run: the run's token.
    let script = Script::hanging(vec![calls("", vec![submit("s1", json!({"n": "x"}))])]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let reply = lead
        .spawn(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await
        .unwrap();
    let run_id = run_id_of(&reply);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while script.requests().len() < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "the second round never asked"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    lead.token.cancel();
    lead.finished().await;
    // Today's cancelled sentence (Y-19), its last tool error after it as
    // for any cancelled subagent: no typed failure text.
    let error = lead.check(&run_id).await.unwrap_err();
    assert!(
        error.starts_with(&format!(
            "Subagent '{run_id}' failed: Agent 'worker' was cancelled before it finished."
        )),
        "{error}"
    );
    assert!(!error.contains("ended without"), "{error}");
    let closes = lead.data_of("subagent_close").await;
    assert_eq!(closes[0]["state"], "cancelled");
    assert_eq!(closes[0]["detail"], "cancelled");
    assert_eq!(lead.usage_statuses(), ["cancelled"]);
    match lead.restored().await.get(&run_id) {
        Some(SubagentStatus::Failed { error, .. }) => assert_eq!(error, "cancelled"),
        other => panic!("{other:?}"),
    }

    // The lead's run ending: the subagents' token. (One harness at a time:
    // each holds the home store's lock.)
    drop(lead);
    let script = Script::hanging(vec![]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let reply = lead
        .spawn(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await
        .unwrap();
    let run_id = run_id_of(&reply);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while script.requests().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the child never asked"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    lead.spawn.stop_subagents().await;
    match lead.tracker.get(&run_id) {
        Some(SubagentStatus::Failed { error, .. }) => {
            assert_eq!(error, "Agent 'worker' was cancelled before it finished.");
        }
        other => panic!("{other:?}"),
    }
    let closes = lead.data_of("subagent_close").await;
    assert_eq!(closes[0]["state"], "cancelled");
    assert_eq!(closes[0]["detail"], SUBAGENT_RUN_ENDED_DETAIL);
}

// ── What counts as a rejection ───────────────────────────────────────

#[tokio::test]
async fn a_missing_result_counts_as_one_rejection() {
    let script = Script::new(
        (1..=4)
            .map(|i| calls("", vec![call(&format!("s{i}"), "submit_result", json!({}))]))
            .collect(),
    );
    let lead = start(script, &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!(
            "Subagent '{run_id}' failed: ended without a result matching its result_schema (4 \
             rejected; at most 4): (root): \"result\" is a required property. Its last answer \
             is kept: check_subagent_status {run_id}."
        )
    );
}

#[tokio::test]
async fn a_missing_result_is_executes_rejection_not_the_pre_checks() {
    let script = Script::new(vec![
        calls("", vec![call("s1", "submit_result", json!({}))]),
        calls("", vec![submit("s2", json!({"n": 1}))]),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        tool_results(&script.requests()[1]),
        [(
            "[tool_error] The result does not match result_schema:\n(root): \"result\" is a \
             required property"
                .to_string(),
            true
        )]
    );
}

#[tokio::test]
async fn a_string_result_for_an_object_schema_reaches_execute() {
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!("not json at all"))]),
        calls("", vec![submit("s2", json!({"n": 1}))]),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        tool_results(&script.requests()[1]),
        [(
            "[tool_error] The result does not match result_schema:\n(root): expected object, got \
             string"
                .to_string(),
            true
        )]
    );
}

#[tokio::test]
async fn unreadable_arguments_do_not_count() {
    let mut replies: Vec<ChatResponse> = (1..=4)
        .map(|i| {
            calls(
                "",
                vec![call(&format!("u{i}"), "submit_result", json!("{not json"))],
            )
        })
        .collect();
    replies.push(calls("", vec![submit("s5", json!({"n": 1}))]));
    let script = Script::new(replies);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    // Four unreadable calls are spent calls, not rejections: the fifth is
    // still checked, and accepted.
    assert!(
        lead.check(&run_id)
            .await
            .unwrap()
            .contains("result (matches result_schema)")
    );
    let second = tool_results(&script.requests()[1]);
    assert!(second[0].1);
    assert!(
        !second[0].0.contains("The result does not match"),
        "{second:?}"
    );
}

#[tokio::test]
async fn the_first_accepted_call_in_a_round_wins() {
    let script = Script::new(vec![
        calls(
            "",
            vec![
                submit("s1", json!({"n": 1})),
                submit("s2", json!({"n": 2})),
                call("n1", "note", json!({})),
            ],
        ),
        text("never asked"),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        lead.check(&run_id).await.unwrap(),
        format!(
            "Subagent '{run_id}' completed successfully: result (matches result_schema): {{\"n\":1}}"
        )
    );
    let results: Vec<Value> = lead
        .data_of("tool_result")
        .await
        .into_iter()
        .map(|d| d["result"].clone())
        .collect();
    assert_eq!(
        results,
        [
            json!("Result accepted."),
            json!("a result was already accepted"),
            json!("noted"),
        ],
        "the round's sibling calls still run and are recorded"
    );
    assert_eq!(script.requests().len(), 1);
}

#[tokio::test]
async fn submit_result_runs_alone_in_its_group() {
    let registry = ToolRegistry::default();
    register_submit_result(
        &registry,
        Arc::new(ResultShape::admit(&counted()).unwrap()),
        Arc::new(TypedResultSlot::default()),
    );
    assert!(!registry.runs_concurrently("submit_result"));
}

#[tokio::test]
async fn a_result_holding_a_relative_path_reaches_execute() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!("../src/main.rs"))])]);
    let lead = start(script, &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(
            json!({"agent_id": "worker", "objective": "path", "result_schema": {"type": "string"}}),
        )
        .await;
    assert_eq!(
        lead.check(&run_id).await.unwrap(),
        format!(
            "Subagent '{run_id}' completed successfully: result (matches result_schema): \
             \"../src/main.rs\""
        )
    );
}

#[tokio::test]
async fn a_result_holding_a_nul_is_refused_by_the_sanitizer_and_is_not_a_rejection() {
    let mut replies: Vec<ChatResponse> = (1..=4)
        .map(|i| calls("", vec![submit(&format!("z{i}"), json!("a\u{0}b"))]))
        .collect();
    replies.push(calls("", vec![submit("s5", json!("ab"))]));
    let script = Script::new(replies);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "nul", "result_schema": {"type": "string"}}))
        .await;
    let refused = tool_results(&script.requests()[1]);
    assert!(refused[0].1);
    assert!(
        refused[0].0.contains("Argument contains null bytes"),
        "{refused:?}"
    );
    assert!(
        lead.check(&run_id)
            .await
            .unwrap()
            .ends_with("result (matches result_schema): \"ab\"")
    );
}

#[tokio::test]
async fn a_result_over_64_kib_counts_as_a_rejection() {
    let big = "x".repeat(70_000);
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!(big))]),
        calls("", vec![submit("s2", json!("small"))]),
    ]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "big", "result_schema": {"type": "string"}}))
        .await;
    assert_eq!(
        tool_results(&script.requests()[1]),
        [(
            "[tool_error] The result does not match result_schema:\n(root): the result is more \
             than 65,536 bytes as compact JSON; send a smaller one"
                .to_string(),
            true
        )]
    );
    assert!(lead.check(&run_id).await.unwrap().ends_with("\"small\""));
}

#[tokio::test]
async fn a_failure_after_an_oversize_or_missing_result_keeps_only_the_final_text() {
    let big = json!("x".repeat(70_000));
    let script = Script::new(vec![
        calls("try 1", vec![submit("s1", json!({"n": "x"}))]),
        calls("try 2", vec![submit("s2", json!({"n": "y"}))]),
        calls("try 3", vec![submit("s3", json!({"n": "z"}))]),
        calls("try 4", vec![submit("s4", big.clone())]),
        calls("miss 1", vec![submit("m1", json!({"n": "x"}))]),
        calls("miss 2", vec![submit("m2", json!({"n": "y"}))]),
        calls("miss 3", vec![submit("m3", json!({"n": "z"}))]),
        calls("miss 4", vec![call("m4", "submit_result", json!({}))]),
    ]);
    let lead = start(script, &[worker()], DaemonConfig::default());
    let oversize = lead
        .run(json!({"agent_id": "worker", "objective": "a", "result_schema": counted()}))
        .await;
    let missing = lead
        .run(json!({"agent_id": "worker", "objective": "b", "result_schema": counted()}))
        .await;
    for (run_id, last) in [(oversize, "try 4"), (missing, "miss 4")] {
        match lead.tracker.get(&run_id) {
            Some(SubagentStatus::Failed { output, .. }) => {
                assert_eq!(output, last, "nothing serialised beside the final text");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn submit_result_counts_against_max_tool_calls() {
    // Every executed call counts — unreadable, sanitizer-refused, rejected,
    // accepted and answered from the slot alike: five.
    let script = Script::new(vec![
        calls("", vec![call("u1", "submit_result", json!("{not json"))]),
        calls("", vec![submit("z1", json!({"n": "a\u{0}b"}))]),
        calls("", vec![submit("s1", json!({"n": "x"}))]),
        calls(
            "",
            vec![submit("s2", json!({"n": 2})), submit("s3", json!({"n": 3}))],
        ),
    ]);
    let lead = start(script, &[worker()], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let done = lead.data_of("workflow_done").await;
    assert_eq!(done[0]["finish_reason"], "result_submitted");
    assert_eq!(done[0]["tool_calls_made"], 5);

    // A call the cap refuses is not executed and raises nothing.
    drop(lead);
    let script = Script::new(vec![
        calls("", vec![submit("s1", json!({"n": "x"}))]),
        calls("", vec![submit("s2", json!({"n": "y"}))]),
        calls("", vec![submit("s3", json!({"n": 3}))]),
        text("I could not submit"),
    ]);
    let template = Template {
        max_tool_calls: Some(2),
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let fourth = tool_results(&script.requests()[3]);
    assert_eq!(
        fourth.last().unwrap().0,
        "[tool_error] max_tool_calls limit reached — no more tool calls allowed"
    );
    let done = lead.data_of("workflow_done").await;
    assert_eq!(
        done[0]["tool_calls_made"], 2,
        "both executed submissions counted, the refused one not"
    );
}

#[tokio::test]
async fn a_child_at_its_tool_cap_fails_typed_and_the_line_says_so() {
    // Three calls: two of work, a rejected submit; then a submit the cap
    // refuses, and a text-only end with no call left — no nudge.
    let script = Script::new(vec![
        calls(
            "",
            vec![call("n1", "note", json!({})), call("n2", "note", json!({}))],
        ),
        calls("", vec![submit("s1", json!({"n": "x"}))]),
        calls("", vec![submit("s2", json!({"n": 3}))]),
        text("I could not submit"),
        text("never asked"),
    ]);
    let template = Template {
        max_tool_calls: Some(3),
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(script.requests().len(), 4, "no nudge was sent");
    let typed = format!(
        "ended without a result matching its result_schema (1 rejected; it used all 3 of its \
         tool calls (max_tool_calls) before a fourth): /n: expected integer, got string. Its \
         last answer is kept: check_subagent_status {run_id}."
    );
    let output = "{\"n\":\"x\"}\n\nI could not submit";
    // The wait first: a check reports the entry, and a later wait names it.
    let summary = lead.wait().await;
    assert_line(
        &summary,
        format!("- **{run_id}**: failed — {typed} Last output: {output}"),
    );
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!("Subagent '{run_id}' failed: {typed}\nIts last output follows:\n{output}")
    );
    let closes = lead.data_of("subagent_close").await;
    assert_eq!(closes[0]["state"], "failed");
}

/// A config whose subagents run one call per round.
fn one_call_per_round() -> DaemonConfig {
    let mut config = DaemonConfig::default();
    config.execution.agent_defaults.max_tools_per_round = 1;
    config
}

#[tokio::test]
async fn a_submit_past_max_tools_per_round_is_refused_for_that_round_and_can_be_sent_again() {
    let script = Script::new(vec![
        calls(
            "",
            vec![call("n1", "note", json!({})), submit("s1", json!({"n": 1}))],
        ),
        calls("", vec![submit("s2", json!({"n": 2}))]),
    ]);
    let lead = start(script.clone(), &[worker()], one_call_per_round());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let second = tool_results(&script.requests()[1]);
    assert_eq!(
        second,
        [
            ("noted".to_string(), false),
            (
                "[tool_error] max tools per round exceeded".to_string(),
                true
            )
        ]
    );
    assert!(lead.check(&run_id).await.unwrap().ends_with("{\"n\":2}"));
}

#[tokio::test]
async fn a_submit_refused_by_the_round_cap_in_the_last_round_is_named_in_the_line() {
    let script = Script::new(vec![calls(
        "",
        vec![call("n1", "note", json!({})), submit("s1", json!({"n": 1}))],
    )]);
    let template = Template {
        max_rounds: Some(1),
        ..worker()
    };
    let lead = start(script, &[template], one_call_per_round());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!(
            "Subagent '{run_id}' failed: ended without submitting a result: its submit_result \
             call was refused by the per-round tool limit (max_tools_per_round) in its last \
             round."
        )
    );
}

#[tokio::test]
async fn a_round_cap_refusal_before_the_last_round_is_not_said_to_be_in_it() {
    // Round 1's submit_result is refused by the per-round cap; the child
    // then answers in text, is nudged, and answers in text again: its last
    // round refused nothing, so its failure is the plain one.
    let script = Script::new(vec![
        calls(
            "",
            vec![call("n1", "note", json!({})), submit("s1", json!({"n": 1}))],
        ),
        text("I stopped"),
        text("still no"),
    ]);
    let lead = start(script.clone(), &[worker()], one_call_per_round());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(script.requests().len(), 3);
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!(
            "Subagent '{run_id}' failed: ended without submitting a result: still no.\nIts last \
             output follows:\nstill no"
        )
    );
}

#[tokio::test]
async fn no_nudge_when_the_tool_cap_is_spent() {
    let script = Script::new(vec![
        calls("", vec![call("n1", "note", json!({}))]),
        text("all done"),
        text("never asked"),
    ]);
    let template = Template {
        max_tool_calls: Some(1),
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        script.requests().len(),
        2,
        "no nudge for a call the loop would refuse"
    );
    assert_eq!(
        lead.check(&run_id).await.unwrap_err(),
        format!(
            "Subagent '{run_id}' failed: ended without submitting a result: it used all 1 of its \
             tool calls (max_tool_calls), and submit_result counts against them. all done.\nIts \
             last output follows:\nall done"
        )
    );
}

#[tokio::test]
async fn an_accepted_submit_that_spends_the_last_call_is_a_success() {
    let script = Script::new(vec![calls("", vec![submit("s1", json!({"n": 1}))])]);
    let template = Template {
        max_tool_calls: Some(1),
        ..worker()
    };
    let lead = start(script, &[template], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    assert_eq!(
        lead.check(&run_id).await.unwrap(),
        format!(
            "Subagent '{run_id}' completed successfully: result (matches result_schema): {{\"n\":1}}"
        )
    );
}

#[tokio::test]
async fn the_fourth_rejection_that_spends_the_cap_decides() {
    let script = Script::new(
        (1..=4)
            .map(|i| calls("", vec![submit(&format!("s{i}"), json!({"n": "x"}))]))
            .collect(),
    );
    let template = Template {
        max_tool_calls: Some(4),
        ..worker()
    };
    let lead = start(script, &[template], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "count", "result_schema": counted()}))
        .await;
    let error = lead.check(&run_id).await.unwrap_err();
    assert!(
        error.starts_with(&format!(
            "Subagent '{run_id}' failed: ended without a result matching its result_schema (4 \
             rejected; at most 4): "
        )),
        "{error}"
    );
    assert!(!error.contains("max_tool_calls"), "no cap clause: {error}");
    let done = lead.data_of("workflow_done").await;
    assert_eq!(done[0]["finish_reason"], "result_rejected");
}

#[tokio::test]
async fn a_template_capability_named_submit_result_admits_nothing() {
    let script = Script::new(vec![text("plain")]);
    let template = Template {
        capabilities: vec!["notes", "submit_result"],
        ..worker()
    };
    let lead = start(script.clone(), &[template], DaemonConfig::default());
    lead.run(json!({"agent_id": "worker", "objective": "plain"}))
        .await;
    assert_eq!(tool_names(&script.requests()[0]), ["note"]);
    assert!(lead.registry.get("submit_result").is_none());
}

// ── A schema that asks for nothing (L5 (i)) ──────────────────────────

#[tokio::test]
async fn a_schema_that_asks_for_nothing_is_an_ordinary_spawn_with_one_clause() {
    const CLAUSE: &str = " (result_schema constrains nothing, so the result comes back as text)";
    for schema in [
        json!({}),
        json!({"title": "t", "description": "d"}),
        json!({"format": "email"}),
    ] {
        let script = Script::new(vec![text("plain"), text("plain")]);
        let lead = start(script.clone(), &[worker()], DaemonConfig::default());
        register_plugin_worker(&lead.shared);
        let absent = lead
            .spawn(json!({"agent_id": "worker", "objective": "A"}))
            .await
            .unwrap();
        let nothing = lead
            .spawn(json!({"agent_id": "worker", "objective": "B", "result_schema": schema.clone()}))
            .await
            .unwrap();
        lead.finished().await;
        let (absent_id, nothing_id) = (run_id_of(&absent), run_id_of(&nothing));
        assert_eq!(
            nothing.replace(CLAUSE, "").replace(&nothing_id, "ID"),
            absent.replace(&absent_id, "ID"),
            "{schema}: the clause and nothing else"
        );
        assert!(nothing.contains(&format!("{CLAUSE}, ")), "{nothing}");
        let (a, b) = (&script.requests_for("A")[0], &script.requests_for("B")[0]);
        assert_eq!(tool_names(a), tool_names(b));
        assert!(!system_prompt(b).contains("submit_result"));
        // A plugin-backed template given it is not refused.
        assert!(
            lead.spawn(json!({"agent_id": "plugin_worker", "objective": "P", "result_schema": schema.clone()}))
                .await
                .unwrap()
                .contains(CLAUSE)
        );
        lead.finished().await;
        // A batch item's OK line carries it.
        let batch = SpawnSubagentsBatchTool::new(lead.spawn.clone())
            .execute(&json!({"subagents": [
                {"agent_id": "worker", "objective": "C", "result_schema": schema},
            ]}))
            .await
            .unwrap();
        assert!(
            batch.contains("[1] OK: Subagent 'worker' spawned (run_id: '"),
            "{batch}"
        );
        assert!(batch.contains(&format!("{CLAUSE}, ")), "{batch}");
        lead.finished().await;
    }
}

// ── The no-schema regression guards (green on the parent) ────────────

/// **Regression guard.** An absent and a `null` `result_schema` spawn the
/// same ordinary child: no admission, no `submit_result`, the same tools,
/// prompt, record and reply; a plugin-backed template given `null` is not
/// refused; a batch item likewise.
#[tokio::test]
async fn a_null_result_schema_is_an_ordinary_spawn() {
    let script = Script::new(vec![text("plain"), text("plain"), text("plain")]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    register_plugin_worker(&lead.shared);
    let absent = lead
        .spawn(json!({"agent_id": "worker", "objective": "A"}))
        .await
        .unwrap();
    let null = lead
        .spawn(json!({"agent_id": "worker", "objective": "B", "result_schema": null}))
        .await
        .unwrap();
    lead.finished().await;
    assert_eq!(
        null.replace(&run_id_of(&null), "ID"),
        absent.replace(&run_id_of(&absent), "ID")
    );
    let (a, b) = (&script.requests_for("A")[0], &script.requests_for("B")[0]);
    assert_eq!(tool_names(a), tool_names(b));
    assert!(!tool_names(b).contains(&"submit_result".to_string()));
    assert_eq!(
        system_prompt(a),
        system_prompt(b),
        "the same system prompt, the plain output block in both"
    );
    assert!(system_prompt(b).contains(PLAIN_SENTENCE));
    let opens = lead.data_of("subagent_open").await;
    assert!(opens.iter().all(|o| o.get("typed").is_none()), "{opens:?}");
    // A plugin-backed template given `null` is not refused.
    assert!(
        lead.spawn(json!({"agent_id": "plugin_worker", "objective": "P", "result_schema": null}))
            .await
            .is_ok()
    );
    lead.finished().await;
    let batch = SpawnSubagentsBatchTool::new(lead.spawn.clone())
        .execute(&json!({"subagents": [
            {"agent_id": "worker", "objective": "C", "result_schema": null},
        ]}))
        .await
        .unwrap();
    assert!(
        batch.starts_with("Batch spawn: 1/1 subagents spawned successfully."),
        "{batch}"
    );
    lead.finished().await;
    assert!(!tool_names(&script.requests_for("C")[0]).contains(&"submit_result".to_string()));
}

/// **Regression guard.** A child spawned without a schema answers as it
/// always has: its text is its answer, its lane closes `done`, its usage
/// row says `success`, and `submit_result` is nowhere.
#[tokio::test]
async fn a_child_without_a_schema_is_unchanged() {
    let script = Script::new(vec![text("the plain answer")]);
    let lead = start(script.clone(), &[worker()], DaemonConfig::default());
    let run_id = lead
        .run(json!({"agent_id": "worker", "objective": "plain"}))
        .await;
    assert_eq!(
        lead.check(&run_id).await.unwrap(),
        format!("Subagent '{run_id}' completed successfully: the plain answer")
    );
    assert_eq!(script.requests().len(), 1, "no nudge for an untyped child");
    assert_eq!(tool_names(&script.requests()[0]), ["note"]);
    let done = lead.data_of("workflow_done").await;
    assert_eq!(done[0]["finish_reason"], "complete");
    let closes = lead.data_of("subagent_close").await;
    assert_eq!(closes[0]["state"], "done");
    assert_eq!(lead.usage_statuses(), ["success"]);
    assert_eq!(lead.history_statuses(), ["completed"]);
}
