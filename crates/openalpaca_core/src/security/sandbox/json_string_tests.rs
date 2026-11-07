//! N-TOOL-8: a call whose arguments are a top-level JSON string holding an
//! object is read as that object at the sandbox's entry — once, before the
//! sanitizer — so the sanitizer, the registry's schema check, the card and the
//! tool all see the one decoded value. A string nested inside it is never
//! decoded, and nothing else is: the registry still decodes nothing
//! (`tools/registry/arguments_tests.rs`, N-TOOL-5).

use super::*;
use crate::security::confirmation::{ConfirmationBroker, ConfirmationResponse};
use crate::tools::ToolRegistry;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use async_trait::async_trait;
use std::sync::Mutex;

/// A builtin that keeps the arguments of every run.
#[derive(Default)]
struct Kept {
    runs: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl BuiltInTool for Kept {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        self.runs.lock().unwrap().push(arguments.clone());
        Ok("ran".to_string())
    }
}

/// A sandbox holding `lookup` (a `query` it requires, a `filter` it may
/// take), with a broker and a receiver on its bus.
fn sandbox_with(
    tool: Arc<Kept>,
) -> (
    SandboxManager,
    Arc<ConfirmationBroker>,
    tokio::sync::broadcast::Receiver<SystemEvent>,
) {
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "lookup".to_string(),
                description: "lookup".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "filter": {"type": "string"}
                    },
                    "required": ["query"]
                }),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(tool),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    let bus = EventBus::default();
    let rx = bus.subscribe();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox =
        SandboxManager::new(Arc::new(registry), bus, &CircuitBreakerConfig::default());
    sandbox.set_confirmation_broker(broker.clone());
    (sandbox, broker, rx)
}

fn policy(confirm: &[&str]) -> SandboxPolicy {
    SandboxPolicy {
        agent_id: "plugin_agent".to_string(),
        allowed_capabilities: Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: confirm.iter().map(|t| t.to_string()).collect(),
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        max_tool_runtime_ceiling_secs: 600,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: Some(5),
        auto_approve: false,
        unattended: false,
    }
}

fn lookup(arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "tc_json_string".to_string(),
        name: "lookup".to_string(),
        arguments,
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        agent_id: Some("plugin_agent".to_string()),
        ..Default::default()
    }
}

/// The JSON string of `object`, as a plugin that encodes its arguments sends
/// them.
fn as_string(object: &serde_json::Value) -> serde_json::Value {
    serde_json::Value::String(object.to_string())
}

/// Read once, and only the top: a field of the decoded object that itself
/// holds the JSON string of an object reaches the tool as that string.
#[tokio::test]
async fn a_nested_json_string_reaches_the_tool_undecoded() {
    let tool = Arc::new(Kept::default());
    let (sandbox, _broker, _rx) = sandbox_with(tool.clone());
    let object = serde_json::json!({
        "query": "q",
        "filter": serde_json::json!({"kind": "note"}).to_string()
    });

    let out = sandbox
        .execute_tool(&lookup(as_string(&object)), &policy(&[]), &ctx())
        .await;

    assert_eq!(out.as_deref(), Ok("ran"));
    assert_eq!(*tool.runs.lock().unwrap(), vec![object]);
}

/// The registry's schema check reads the decoded object: what it lacks is
/// named, not "got string".
#[tokio::test]
async fn the_schema_check_reads_the_decoded_object() {
    let tool = Arc::new(Kept::default());
    let (sandbox, _broker, _rx) = sandbox_with(tool.clone());

    let out = sandbox
        .execute_tool(
            &lookup(as_string(&serde_json::json!({"filter": "f"}))),
            &policy(&[]),
            &ctx(),
        )
        .await;

    assert_eq!(
        out,
        Err("Tool 'lookup': missing required parameter 'query'".to_string())
    );
    assert!(tool.runs.lock().unwrap().is_empty());
}

/// The card shows the decoded object — the request the person answers and
/// the event a client draws it from — and the tool runs on that object.
#[tokio::test]
async fn the_card_shows_the_decoded_object() {
    let tool = Arc::new(Kept::default());
    let (sandbox, broker, mut rx) = sandbox_with(tool.clone());
    let object = serde_json::json!({"query": "q", "filter": "f"});
    let answered = {
        let broker = broker.clone();
        tokio::spawn(async move {
            for _ in 0..300 {
                if let Some(request) = broker.pending_requests().pop() {
                    broker
                        .respond(
                            &request.request_id,
                            ConfirmationResponse {
                                approved: true,
                                approval_scope: None,
                            },
                        )
                        .unwrap();
                    return Some(request);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            None
        })
    };

    let out = sandbox
        .execute_tool(&lookup(as_string(&object)), &policy(&["lookup"]), &ctx())
        .await;

    let request = answered.await.unwrap();
    assert_eq!(out.as_deref(), Ok("ran"), "card answered: {request:?}");
    assert_eq!(
        request.map(|request| request.tool_arguments),
        Some(object.clone())
    );
    let mut drawn = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::ToolConfirmationRequested { tool_arguments, .. } = event {
            drawn.push(tool_arguments);
        }
    }
    assert_eq!(drawn, vec![object.clone()]);
    assert_eq!(*tool.runs.lock().unwrap(), vec![object]);
}

/// Green on the parent by design — what is not decoded: text that is not
/// JSON, JSON of another kind, and a string holding the JSON string of an
/// object (encoded twice, read at most once) each stay the string they are,
/// refused by the schema as before, with no WARN.
#[tokio::test]
#[tracing_test::traced_test]
async fn only_a_string_holding_an_object_is_read_and_only_once() {
    let tool = Arc::new(Kept::default());
    let (sandbox, _broker, _rx) = sandbox_with(tool.clone());
    let twice = serde_json::json!(serde_json::json!({"query": "q"}).to_string()).to_string();

    for text in ["query q", "[1, 2]", "42", "null", "\"q\"", twice.as_str()] {
        let out = sandbox
            .execute_tool(&lookup(serde_json::json!(text)), &policy(&[]), &ctx())
            .await;
        assert_eq!(
            out,
            Err("Tool 'lookup': arguments must be a JSON object, got string".to_string()),
            "{text}"
        );
    }

    assert!(tool.runs.lock().unwrap().is_empty());
    assert!(!logs_contain("read as that object"));
}
