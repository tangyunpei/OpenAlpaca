//! A call the registry would refuse at dispatch raises no confirmation card
//! (N-TOOL-2, audit Z1-04).
//!
//! The sandbox used to raise the card first and leave the argument check and
//! the extension gate to dispatch, so a person could approve a call that was
//! refused the moment after the click. Each test here answers yes to every
//! card that appears, so on that build the click happens and the refusal
//! follows it; now the refusal comes first and no card is raised.

use super::*;
use crate::security::confirmation::{ConfirmationBroker, ConfirmationResponse};
use crate::tools::extensions::{ExtensionId, ExtensionState, Transition, WithdrawalCause};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A builtin that counts the times it ran.
#[derive(Default)]
struct Counted {
    runs: AtomicUsize,
}

#[async_trait]
impl BuiltInTool for Counted {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok("ran".to_string())
    }
}

fn registered(name: &str, parameters: serde_json::Value, backend: ToolBackend) -> RegisteredTool {
    RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            parameters,
            strict: None,
            input_examples: None,
        },
        backend,
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }
}

/// `write_note`: a string `path` it requires, an integer `limit` it allows.
fn note_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "limit": {"type": "integer"}
        },
        "required": ["path"]
    })
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("tc_{name}"),
        name: name.to_string(),
        arguments,
    }
}

/// A policy whose explicit list asks before `tool`, with a short timeout so a
/// card nobody answers cannot hold a test for five minutes.
fn asking_for(tool: &str) -> SandboxPolicy {
    SandboxPolicy {
        agent_id: "agent-1".to_string(),
        allowed_capabilities: Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: vec![tool.to_string()],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: Some(2),
        auto_approve: false,
        unattended: false,
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        agent_id: Some("agent-1".to_string()),
        task_id: Some("task-1".to_string()),
        ..Default::default()
    }
}

struct Harness {
    sandbox: SandboxManager,
    broker: Arc<ConfirmationBroker>,
    events: tokio::sync::broadcast::Receiver<SystemEvent>,
}

fn harness(registry: ToolRegistry) -> Harness {
    let bus = EventBus::default();
    let events = bus.subscribe();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox =
        SandboxManager::new(Arc::new(registry), bus, &CircuitBreakerConfig::default());
    sandbox.set_confirmation_broker(broker.clone());
    Harness {
        sandbox,
        broker,
        events,
    }
}

/// Says yes to every card that appears until aborted, counting them.
fn approve_every_card(
    broker: &Arc<ConfirmationBroker>,
) -> (Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let answered = Arc::new(AtomicUsize::new(0));
    let (broker, count) = (broker.clone(), answered.clone());
    let task = tokio::spawn(async move {
        loop {
            for request in broker.pending_requests() {
                let yes = ConfirmationResponse {
                    approved: true,
                    approval_scope: None,
                };
                if broker.respond(&request.request_id, yes).is_ok() {
                    count.fetch_add(1, Ordering::SeqCst);
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    (answered, task)
}

/// What the bus said about `tool` during the call: the cards raised for it,
/// and the `success` of each `ToolExecuted` it got.
fn drain(
    events: &mut tokio::sync::broadcast::Receiver<SystemEvent>,
    tool: &str,
) -> (usize, Vec<bool>) {
    let (mut cards, mut executed) = (0, Vec::new());
    while let Ok(event) = events.try_recv() {
        match event {
            SystemEvent::ToolConfirmationRequested { tool_name, .. } if tool_name == tool => {
                cards += 1;
            }
            SystemEvent::ToolExecuted {
                tool_name, success, ..
            } if tool_name == tool => executed.push(success),
            _ => {}
        }
    }
    (cards, executed)
}

/// The gate's first test: an argument the schema refuses is refused before
/// the card, with the error dispatch would have given after it.
#[tokio::test]
async fn a_call_its_schema_refuses_raises_no_card() {
    let tool = Arc::new(Counted::default());
    let registry = ToolRegistry::default();
    registry
        .register(registered(
            "write_note",
            note_schema(),
            ToolBackend::BuiltIn(tool.clone()),
        ))
        .unwrap();
    let mut h = harness(registry);
    let (answered, approver) = approve_every_card(&h.broker);

    let err = h
        .sandbox
        .execute_tool(
            &call("write_note", serde_json::json!({"limit": 3})),
            &asking_for("write_note"),
            &ctx(),
        )
        .await
        .expect_err("a call missing a required parameter is refused");
    approver.abort();

    assert_eq!(err, "Tool 'write_note': missing required parameter 'path'");
    let (cards, executed) = drain(&mut h.events, "write_note");
    assert_eq!(
        cards, 0,
        "no card is raised for a call that would be refused"
    );
    assert_eq!(answered.load(Ordering::SeqCst), 0, "nobody was asked");
    assert_eq!(h.broker.pending_count(), 0);
    assert_eq!(tool.runs.load(Ordering::SeqCst), 0, "the tool never ran");
    assert_eq!(
        executed,
        vec![false],
        "the refusal is recorded as the failed call it is, as dispatch records it"
    );
}

/// The gate's second test: a tool of an extension the owner disabled is
/// refused before the card, with the withheld-capability refusal.
#[tokio::test]
async fn a_disabled_extensions_tool_raises_no_card() {
    let registry = ToolRegistry::default();
    let ext = ExtensionId::mcp("github");
    let ledger = registry.extensions().clone();
    let generation = match ledger.begin(&ext, ExtensionState::Enabling, None) {
        Transition::Took(g) => g,
        other => panic!("E0 refused: {other:?}"),
    };
    ledger.restore(&ext);
    ledger.record_tools(&ext, ["github__create_issue".to_string()]);
    assert!(ledger.commit(&ext, ExtensionState::Enabled));
    registry
        .register(registered(
            "github__create_issue",
            serde_json::json!({"type": "object"}),
            ToolBackend::Mcp {
                client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests("github")),
                remote_name: "create_issue".to_string(),
                server_name: "github".to_string(),
                generation,
            },
        ))
        .unwrap();
    // The owner switches the server off; a snapshot still holds the tool.
    ledger.begin(
        &ext,
        ExtensionState::Disabling,
        Some(WithdrawalCause::Disable),
    );
    assert!(ledger.commit(&ext, ExtensionState::Disabled));

    let mut h = harness(registry);
    let (answered, approver) = approve_every_card(&h.broker);
    let err = h
        .sandbox
        .execute_tool(
            &call("github__create_issue", serde_json::json!({"title": "x"})),
            &asking_for("github__create_issue"),
            &ctx(),
        )
        .await
        .expect_err("a disabled extension's tool is withheld");
    approver.abort();

    assert!(crate::tools::extensions::is_withheld_refusal(&err), "{err}");
    assert!(
        err.contains("MCP server 'github' is disabled by the owner"),
        "{err}"
    );
    let (cards, _) = drain(&mut h.events, "github__create_issue");
    assert_eq!(cards, 0, "no card is raised for a withheld tool");
    assert_eq!(answered.load(Ordering::SeqCst), 0, "nobody was asked");
    assert_eq!(h.broker.pending_count(), 0);
    assert_eq!(
        ledger.in_flight(&ext),
        0,
        "the check before the card holds no call open"
    );
}

/// The gate's third test: a call that passes both checks still asks first.
#[tokio::test]
async fn a_valid_call_still_raises_its_card() {
    let tool = Arc::new(Counted::default());
    let registry = ToolRegistry::default();
    registry
        .register(registered(
            "write_note",
            note_schema(),
            ToolBackend::BuiltIn(tool.clone()),
        ))
        .unwrap();
    let mut h = harness(registry);
    let (answered, approver) = approve_every_card(&h.broker);

    let out = h
        .sandbox
        .execute_tool(
            &call(
                "write_note",
                serde_json::json!({"path": "notes/a.md", "limit": 3}),
            ),
            &asking_for("write_note"),
            &ctx(),
        )
        .await;
    approver.abort();

    assert_eq!(out.as_deref(), Ok("ran"));
    let (cards, executed) = drain(&mut h.events, "write_note");
    assert_eq!(cards, 1, "the card is raised");
    assert_eq!(answered.load(Ordering::SeqCst), 1);
    assert_eq!(
        tool.runs.load(Ordering::SeqCst),
        1,
        "and the approved call ran"
    );
    assert_eq!(executed, vec![true]);
}

/// The check runs before every answer a confirm-listed call could get, not
/// only before a card: an unattended run is told what is wrong with the call,
/// and is not filed as needing an approval it could never have used.
#[tokio::test]
async fn an_unattended_run_hears_the_arguments_problem_not_the_missing_approver() {
    let tool = Arc::new(Counted::default());
    let registry = ToolRegistry::default();
    registry
        .register(registered(
            "write_note",
            note_schema(),
            ToolBackend::BuiltIn(tool.clone()),
        ))
        .unwrap();
    let h = harness(registry);
    let mut policy = asking_for("write_note");
    policy.unattended = true;

    let err = h
        .sandbox
        .execute_tool(
            &call("write_note", serde_json::json!({"limit": 3})),
            &policy,
            &ctx(),
        )
        .await
        .expect_err("refused either way");

    assert_eq!(err, "Tool 'write_note': missing required parameter 'path'");
    assert_eq!(tool.runs.load(Ordering::SeqCst), 0);
}

/// A tool that asks for no approval keeps today's path: dispatch refuses it,
/// once, and nothing about it changed.
#[tokio::test]
async fn a_call_that_asks_for_no_approval_is_refused_at_dispatch_as_before() {
    let tool = Arc::new(Counted::default());
    let registry = ToolRegistry::default();
    registry
        .register(registered(
            "write_note",
            note_schema(),
            ToolBackend::BuiltIn(tool.clone()),
        ))
        .unwrap();
    let mut h = harness(registry);
    let mut policy = asking_for("write_note");
    policy.require_confirmation_for = vec!["something_else".to_string()];

    let err = h
        .sandbox
        .execute_tool(
            &call("write_note", serde_json::json!({"limit": 3})),
            &policy,
            &ctx(),
        )
        .await
        .expect_err("dispatch refuses it");

    assert_eq!(err, "Tool 'write_note': missing required parameter 'path'");
    let (cards, executed) = drain(&mut h.events, "write_note");
    assert_eq!(cards, 0);
    assert_eq!(executed, vec![false]);
    assert_eq!(tool.runs.load(Ordering::SeqCst), 0);
}
