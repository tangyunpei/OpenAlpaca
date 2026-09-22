use super::*;
use async_trait::async_trait;
use crate::bus::EventBus;
use crate::security::confirmation::{ConfirmationBroker, ConfirmationResponse};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use crate::tools::ToolRegistry;

struct MockTool;

#[async_trait]
impl BuiltInTool for MockTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Check for the slow_tool marker in arguments
        if arguments.get("__slow").is_some() {
            tokio::time::sleep(Duration::from_secs(5)).await;
            return Ok("done".to_string());
        }
        Ok("search results".to_string())
    }
}

fn make_registry() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: "web_search".to_string(),
            description: "Web search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockTool)),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();
    registry.register(RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: "slow_tool".to_string(),
            description: "Slow tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockTool)),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();
    Arc::new(registry)
}

fn make_sandbox() -> SandboxManager {
    SandboxManager::new(
        make_registry(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    )
}

fn make_policy(agent_id: &str) -> SandboxPolicy {
    SandboxPolicy {
        agent_id: agent_id.to_string(),
        // These tests exercise sanitization, confirmation, the circuit breaker
        // and timeouts — not the allow axis.
        allowed_capabilities: Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
        unattended: false,
    }
}

fn make_tool_call(name: &str) -> ToolCall {
    ToolCall {
        id: "tc_1".to_string(),
        name: name.to_string(),
        arguments: serde_json::json!({"query": "test"}),
    }
}

fn make_ctx(agent_id: &str) -> ToolContext {
    ToolContext {
        agent_id: Some(agent_id.to_string()),
        ..Default::default()
    }
}

/// A context that belongs to a run — what every call inside a workflow has.
fn make_ctx_for_task(agent_id: &str, task_id: &str) -> ToolContext {
    ToolContext {
        agent_id: Some(agent_id.to_string()),
        task_id: Some(task_id.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn test_happy_path() {
    let sandbox = make_sandbox();
    let policy = make_policy("agent1");
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "search results");
}

#[tokio::test]
async fn test_denied_capability() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.denied_capabilities = vec!["web_search".to_string()];
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("denied"));
}

/// A0 (bug A): an allow list that resolved to nothing blocks execution — it is
/// never read as "unconstrained".
#[tokio::test]
async fn test_empty_allowlist_blocks_execution() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.allowed_capabilities = Allowlist::Only(vec![]);
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not in allow list"));
}

#[tokio::test]
async fn test_timeout() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.max_tool_runtime_secs = 1; // 1 second timeout
    let tc = ToolCall {
        id: "tc_1".to_string(),
        name: "slow_tool".to_string(),
        arguments: serde_json::json!({"query": "test", "__slow": true}),
    };
    let ctx = make_ctx("agent1");

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("timed out"));
}

#[tokio::test]
async fn test_security_event_emitted() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let sandbox = SandboxManager::new(
        make_registry(),
        bus,
        &CircuitBreakerConfig::default(),
    );
    let mut policy = make_policy("agent1");
    policy.denied_capabilities = vec!["web_search".to_string()];
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let _ = sandbox.execute_tool(&tc, &policy, &ctx).await;

    let event = rx.try_recv().unwrap();
    match event {
        SystemEvent::SecurityViolation {
            agent_id,
            tool_name,
            ..
        } => {
            assert_eq!(agent_id, "agent1");
            assert_eq!(tool_name, "web_search");
        }
        other => panic!("Expected SecurityViolation, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_tool_event_emitted() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let sandbox = SandboxManager::new(
        make_registry(),
        bus,
        &CircuitBreakerConfig::default(),
    );
    let policy = make_policy("agent1");
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let _ = sandbox.execute_tool(&tc, &policy, &ctx).await;

    let event = rx.try_recv().unwrap();
    match event {
        SystemEvent::ToolExecuted {
            agent_id,
            tool_name,
            success,
            ..
        } => {
            assert_eq!(agent_id, "agent1");
            assert_eq!(tool_name, "web_search");
            assert!(success);
        }
        other => panic!("Expected ToolExecuted, got: {:?}", other),
    }
}

// ── GAP-10: the run the call belonged to ────────────────────────────────────

/// `ctx.task_id` is already at the emit site; the frame now carries it, so the
/// event log can be filtered to one run instead of guessing from `agent_id`.
#[tokio::test]
async fn tool_executed_carries_the_run_it_belonged_to() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let sandbox = SandboxManager::new(make_registry(), bus, &CircuitBreakerConfig::default());
    let policy = make_policy("agent1");
    let tc = make_tool_call("web_search");
    let ctx = make_ctx_for_task("agent1", "t-1");

    let _ = sandbox.execute_tool(&tc, &policy, &ctx).await;

    match rx.try_recv().unwrap() {
        SystemEvent::ToolExecuted { task_id, .. } => {
            assert_eq!(task_id.as_deref(), Some("t-1"));
        }
        other => panic!("Expected ToolExecuted, got: {:?}", other),
    }
}

/// A refusal is attributed the same way — a run whose tool was denied shows the
/// denial in its own log.
#[tokio::test]
async fn security_violation_carries_the_run_it_belonged_to() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let sandbox = SandboxManager::new(make_registry(), bus, &CircuitBreakerConfig::default());
    let mut policy = make_policy("agent1");
    policy.denied_capabilities = vec!["web_search".to_string()];
    let tc = make_tool_call("web_search");
    let ctx = make_ctx_for_task("agent1", "t-1");

    let _ = sandbox.execute_tool(&tc, &policy, &ctx).await;

    match rx.try_recv().unwrap() {
        SystemEvent::SecurityViolation { task_id, .. } => {
            assert_eq!(task_id.as_deref(), Some("t-1"));
        }
        other => panic!("Expected SecurityViolation, got: {:?}", other),
    }
}

/// A call outside any run — a main-loop turn — carries no run, and says so
/// rather than borrowing one.
#[tokio::test]
async fn a_call_outside_a_run_carries_no_task_id() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let sandbox = SandboxManager::new(make_registry(), bus, &CircuitBreakerConfig::default());
    let policy = make_policy("agent1");
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let _ = sandbox.execute_tool(&tc, &policy, &ctx).await;

    match rx.try_recv().unwrap() {
        SystemEvent::ToolExecuted { task_id, .. } => assert_eq!(task_id, None),
        other => panic!("Expected ToolExecuted, got: {:?}", other),
    }
}

/// The confirmation prompt is a run event too — §4.4's `blocked` lane and the
/// run's own log both need to know which run is waiting.
#[tokio::test]
async fn tool_confirmation_requested_carries_the_run_it_belonged_to() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let mut sandbox =
        SandboxManager::new(make_registry(), bus, &CircuitBreakerConfig::default());
    sandbox.set_confirmation_broker(Arc::new(ConfirmationBroker::new()));
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.confirmation_timeout_secs = Some(1);
    let tc = make_tool_call("web_search");
    let ctx = make_ctx_for_task("agent1", "t-1");

    let _ = sandbox.execute_tool(&tc, &policy, &ctx).await;

    match rx.try_recv().unwrap() {
        SystemEvent::ToolConfirmationRequested { task_id, .. } => {
            assert_eq!(task_id.as_deref(), Some("t-1"));
        }
        other => panic!("Expected ToolConfirmationRequested, got: {:?}", other),
    }
}

// ── M6: a client that cannot answer is told at once ─────────────────

/// **M6.** A workflow started from a client that declared it cannot answer
/// confirmations used to raise the prompt anyway and sit on it for the whole
/// 300-second timeout — five and a half minutes per tool call, then a
/// failure. It is refused immediately instead, in words that name the fix,
/// and nothing is executed.
#[tokio::test]
async fn an_unattended_run_is_refused_at_once_instead_of_waiting() {
    let mut sandbox = make_sandbox();
    sandbox.set_confirmation_broker(Arc::new(ConfirmationBroker::new()));
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    // Long enough that a test which waited for it would hang the suite.
    policy.confirmation_timeout_secs = Some(300);
    policy.unattended = true;
    let tc = make_tool_call("web_search");
    let ctx = make_ctx_for_task("agent1", "t-1");

    let started = std::time::Instant::now();
    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    let elapsed = started.elapsed();

    let err = result.expect_err("fail-closed: the tool must not run");
    assert!(
        err.contains("needs your approval") && err.contains("cannot ask for it"),
        "the refusal must say what happened: {err}"
    );
    assert!(
        err.contains("GUI") && err.contains("openalpaca chat"),
        "…and where it can be approved: {err}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the refusal must be immediate, took {elapsed:?}"
    );
}

/// The declaration never approves anything: with `auto_approve` off it
/// refuses, and it also does not leave a prompt behind for someone to answer.
#[tokio::test]
async fn an_unattended_run_leaves_no_pending_confirmation() {
    let mut sandbox = make_sandbox();
    let broker = Arc::new(ConfirmationBroker::new());
    sandbox.set_confirmation_broker(broker.clone());
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.unattended = true;
    let ctx = make_ctx_for_task("agent1", "t-1");

    let _ = sandbox
        .execute_tool(&make_tool_call("web_search"), &policy, &ctx)
        .await;

    assert_eq!(
        broker.pending_count(),
        0,
        "nothing was raised, so nothing is waiting"
    );
}

/// `auto_approve` is the owner's own explicit decision and still wins: the
/// declaration is about who can answer a prompt, not about what is allowed.
#[tokio::test]
async fn auto_approve_still_wins_over_the_declaration() {
    let mut sandbox = make_sandbox();
    sandbox.set_confirmation_broker(Arc::new(ConfirmationBroker::new()));
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.unattended = true;
    policy.auto_approve = true;
    let ctx = make_ctx_for_task("agent1", "t-1");

    let result = sandbox
        .execute_tool(&make_tool_call("web_search"), &policy, &ctx)
        .await;
    assert!(result.is_ok(), "auto_approve runs the tool: {result:?}");
}

/// A tool that needs no confirmation is untouched by the declaration.
#[tokio::test]
async fn an_unattended_run_still_runs_tools_that_need_no_approval() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.unattended = true;
    let ctx = make_ctx_for_task("agent1", "t-1");

    let result = sandbox
        .execute_tool(&make_tool_call("web_search"), &policy, &ctx)
        .await;
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn test_unregistered_tool() {
    let sandbox = make_sandbox();
    let policy = make_policy("agent1");
    let tc = make_tool_call("unknown_tool");
    let ctx = make_ctx("agent1");

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("not in the allowed tools list")
    );
}

#[tokio::test]
async fn test_require_confirmation_fails_closed() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("requires human confirmation"),
        "Should fail-closed with confirmation message, got: {}",
        err
    );
}

#[tokio::test]
async fn test_confirmation_approved_allows_execution() {
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = make_sandbox();
    sandbox.set_confirmation_broker(broker.clone());

    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.confirmation_timeout_secs = Some(5);
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    // Spawn a task to approve the confirmation after a brief delay
    let broker_clone = broker.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The broker should have exactly 1 pending request
        assert_eq!(broker_clone.pending_count(), 1);
        // We need to find the request_id — iterate pending keys
        let keys: Vec<String> = broker_clone.pending_keys();
        assert_eq!(keys.len(), 1);
        broker_clone
            .respond(
                &keys[0],
                ConfirmationResponse {
                    approved: true,
                    approval_scope: None,
                },
            )
            .unwrap();
    });

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_ok(), "Tool should execute after approval: {:?}", result);
    assert_eq!(result.unwrap(), "search results");
}

#[tokio::test]
async fn test_confirmation_denied_blocks_execution() {
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = make_sandbox();
    sandbox.set_confirmation_broker(broker.clone());

    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.confirmation_timeout_secs = Some(5);
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    let broker_clone = broker.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let keys: Vec<String> = broker_clone.pending_keys();
        assert_eq!(keys.len(), 1);
        broker_clone
            .respond(
                &keys[0],
                ConfirmationResponse {
                    approved: false,
                    approval_scope: None,
                },
            )
            .unwrap();
    });

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("denied by user"));
}

#[tokio::test]
async fn test_auto_approve_skips_confirmation() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.auto_approve = true;
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    // Should succeed without broker, because auto_approve bypasses confirmation
    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_ok(), "Auto-approve should bypass confirmation: {:?}", result);
    assert_eq!(result.unwrap(), "search results");
}

#[tokio::test]
async fn test_confirmation_timeout() {
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = make_sandbox();
    sandbox.set_confirmation_broker(broker.clone());

    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.confirmation_timeout_secs = Some(1); // 1 second timeout
    let tc = make_tool_call("web_search");
    let ctx = make_ctx("agent1");

    // Don't respond — let it time out
    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("timed out"),
        "Should timeout, got: {}",
        err
    );
}

/// **T1.** A prompt nobody answered used to produce nothing at all: the log
/// line said "timed out", the tool call failed, and no client heard a word —
/// so the GUI kept the approval bar up and the composer paused until the
/// window was reloaded. The timeout is announced now, exactly as an answer is,
/// with the outcome that says which it was.
#[tokio::test]
async fn an_expired_confirmation_announces_itself_as_timed_out() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let mut sandbox = SandboxManager::new(make_registry(), bus, &CircuitBreakerConfig::default());
    sandbox.set_confirmation_broker(Arc::new(ConfirmationBroker::new()));
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.confirmation_timeout_secs = Some(1);
    policy.stream_id = Some("stream-1".to_string());
    let tc = make_tool_call("web_search");
    let ctx = make_ctx_for_task("agent1", "t-1");

    // Nobody answers.
    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(result.unwrap_err().contains("timed out"));

    // The request frame first, then its twin.
    assert!(matches!(
        rx.try_recv().unwrap(),
        SystemEvent::ToolConfirmationRequested { .. }
    ));
    match rx.try_recv().unwrap() {
        SystemEvent::ToolConfirmationResolved {
            outcome,
            tool_name,
            task_id,
            stream_id,
            ..
        } => {
            assert_eq!(outcome, crate::events::ConfirmationOutcome::TimedOut);
            assert_eq!(tool_name, "web_search");
            // Routable to the same places the prompt went.
            assert_eq!(task_id.as_deref(), Some("t-1"));
            assert_eq!(stream_id.as_deref(), Some("stream-1"));
        }
        other => panic!("Expected ToolConfirmationResolved, got: {other:?}"),
    }
}

/// …and an answer is announced with the outcome it was, so a second window
/// showing the same card settles it too.
#[tokio::test]
async fn an_answered_confirmation_announces_the_answer() {
    for (approved, expected) in [
        (true, crate::events::ConfirmationOutcome::Approved),
        (false, crate::events::ConfirmationOutcome::Denied),
    ] {
        let broker = Arc::new(ConfirmationBroker::new());
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let mut sandbox =
            SandboxManager::new(make_registry(), bus, &CircuitBreakerConfig::default());
        sandbox.set_confirmation_broker(broker.clone());

        let mut policy = make_policy("agent1");
        policy.require_confirmation_for = vec!["web_search".to_string()];
        policy.confirmation_timeout_secs = Some(5);
        let tc = make_tool_call("web_search");
        let ctx = make_ctx("agent1");

        let answering = broker.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let keys = answering.pending_keys();
            answering
                .respond(
                    &keys[0],
                    ConfirmationResponse {
                        approved,
                        approval_scope: None,
                    },
                )
                .unwrap();
        });

        let _ = sandbox.execute_tool(&tc, &policy, &ctx).await;

        assert!(matches!(
            rx.try_recv().unwrap(),
            SystemEvent::ToolConfirmationRequested { .. }
        ));
        match rx.try_recv().unwrap() {
            SystemEvent::ToolConfirmationResolved { outcome, .. } => {
                assert_eq!(outcome, expected);
            }
            other => panic!("Expected ToolConfirmationResolved, got: {other:?}"),
        }
    }
}

/// ADR-030's S4 refusal is a governance decision, not a failure of the tool —
/// and a `Failed` extension's refusal quotes the extension's own error detail,
/// which routinely says "timed out". Counted as a transient failure, a handful
/// of refused calls opened the breaker for this agent, and an open breaker
/// outlives the reload that fixes the extension.
#[tokio::test]
async fn a_withheld_capability_never_opens_the_circuit_breaker() {
    use crate::tools::extensions::{ExtensionId, ExtensionState, FailureReason, Transition};

    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::mcp("github");
    let generation = match registry
        .extensions()
        .begin(&ext, ExtensionState::Enabling, None)
    {
        Transition::Took(g) => g,
        other => panic!("E0 refused: {other:?}"),
    };
    registry.extensions().restore(&ext);
    registry
        .extensions()
        .record_tools(&ext, ["github__create_issue".to_string()]);
    assert!(
        registry
            .extensions()
            .commit(&ext, ExtensionState::Enabled)
    );
    // An MCP-backed tool: the gate is keyed on the backend's server name and
    // the generation stamped into this handle.
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "github__create_issue".to_string(),
                description: "Create an issue".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Mcp {
                client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests("github")),
                remote_name: "create_issue".to_string(),
                server_name: "github".to_string(),
                generation,
            },
            provides_capabilities: vec!["github__create_issue".to_string()],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "mcp:github".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();

    // The child died on a handshake that timed out — the detail the S4 refusal
    // quotes back to the model.
    assert!(registry.extensions().mark_failed(
        &ext,
        generation,
        FailureReason::Crashed,
        "stdio handshake timed out after 10s",
    ));

    let sandbox = SandboxManager::new(
        registry,
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    );
    let policy = make_policy("agent-1");
    let ctx = make_ctx_for_task("agent-1", "task-1");

    // Well past the default failure_threshold of 5.
    for attempt in 0..12 {
        let err = sandbox
            .execute_tool(&make_tool_call("github__create_issue"), &policy, &ctx)
            .await
            .expect_err("a failed extension's tool is withheld");
        assert!(
            crate::tools::extensions::is_withheld_refusal(&err),
            "attempt {attempt} must be the S4 refusal, not a tool error: {err}"
        );
        assert!(
            err.contains("timed out"),
            "and it quotes the detail the breaker used to key on: {err}"
        );
    }

    // The breaker is untouched: the next call is still refused by the gate, not
    // by a breaker that has to time out before the owner's reload can help.
    assert!(
        sandbox
            .circuit_breaker
            .check("agent-1", "github__create_issue")
            .is_ok(),
        "a withheld capability must never open the breaker"
    );
}

// ---------- Task 9: annotation-derived confirmation + approval cache ----------

/// Build a registry that also has a tool with `destructive_hint=true`.
fn make_registry_with_destructive() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "destructive_test".to_string(),
                description: "destructive test tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(MockTool)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: Some(openalpaca_mcp::ToolAnnotations {
                destructive_hint: Some(true),
                ..Default::default()
            }),
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    // A non-destructive peer so we can verify selectivity.
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "safe_peer".to_string(),
                description: "safe peer tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(MockTool)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: Some(openalpaca_mcp::ToolAnnotations {
                destructive_hint: Some(false),
                ..Default::default()
            }),
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    Arc::new(registry)
}

#[tokio::test]
async fn annotation_derived_confirmation_prompts() {
    use crate::security::confirmation::ApprovalScope;

    let registry = make_registry_with_destructive();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = SandboxManager::new(
        registry.clone(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    );
    sandbox.set_confirmation_broker(broker.clone());

    // Empty require_confirmation_for — should derive from annotations.
    let policy = make_policy("agent1");
    let tc = make_tool_call("destructive_test");
    let ctx = make_ctx("agent1");

    let sandbox = Arc::new(sandbox);
    let sandbox_task = Arc::clone(&sandbox);
    let exec_task = tokio::spawn(async move {
        sandbox_task.execute_tool(&tc, &policy, &ctx).await
    });

    // Wait for broker to have a pending request.
    let mut attempts = 0;
    while broker.pending_count() == 0 && attempts < 50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        attempts += 1;
    }
    assert!(
        broker.pending_count() > 0,
        "Broker should have received a request derived from destructive_hint annotation"
    );

    // Respond with approval + explicit TheseArgs scope.
    let keys: Vec<String> = broker.pending_keys();
    let req_id = keys.first().cloned().unwrap();
    broker
        .respond(
            &req_id,
            ConfirmationResponse {
                approved: true,
                approval_scope: Some(ApprovalScope::TheseArgs),
            },
        )
        .unwrap();

    let result = exec_task.await.unwrap();
    assert!(result.is_ok(), "Tool should have executed: {result:?}");
}

#[tokio::test]
async fn explicit_policy_overrides_annotations() {
    // Tool has destructive_hint=true, but policy lists an *unrelated* tool in
    // require_confirmation_for. Per Q1(C), the explicit policy wins — the
    // destructive tool must NOT prompt.
    let registry = make_registry_with_destructive();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = SandboxManager::new(
        registry.clone(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    );
    sandbox.set_confirmation_broker(broker.clone());

    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["unrelated_tool".to_string()];

    let tc = make_tool_call("destructive_test");
    let ctx = make_ctx("agent1");

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(
        result.is_ok(),
        "Execution should succeed without any prompt: {result:?}"
    );
    assert_eq!(
        broker.pending_count(),
        0,
        "Broker must not receive a request when explicit policy excludes this tool"
    );
}

#[tokio::test]
async fn sandbox_cached_approval_skips_broker() {
    use crate::security::confirmation::{hash_canonical_args, ApprovalScope};

    let registry = make_registry_with_destructive();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = SandboxManager::new(
        registry.clone(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    );
    sandbox.set_confirmation_broker(broker.clone());

    let tc = make_tool_call("destructive_test");
    let ctx = make_ctx("agent1");
    let policy = make_policy("agent1"); // empty require_confirmation_for → annotation-derived

    // Pre-populate the cache: these exact args under TheseArgs.
    let args_hash = hash_canonical_args(&tc.arguments);
    sandbox
        .approval_cache()
        .record("destructive_test", args_hash, ApprovalScope::TheseArgs);

    let result = sandbox.execute_tool(&tc, &policy, &ctx).await;
    assert!(
        result.is_ok(),
        "Cached approval should allow execution: {result:?}"
    );
    assert_eq!(
        broker.pending_count(),
        0,
        "Broker must not receive a request when the invocation is cached"
    );
}

#[tokio::test]
async fn sandbox_records_on_user_approval() {
    use crate::security::confirmation::{hash_canonical_args, ApprovalScope};

    let registry = make_registry_with_destructive();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = SandboxManager::new(
        registry.clone(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    );
    sandbox.set_confirmation_broker(broker.clone());

    let policy = make_policy("agent1");
    let tc = make_tool_call("destructive_test");
    let ctx = make_ctx("agent1");
    let args_hash = hash_canonical_args(&tc.arguments);

    let sandbox = Arc::new(sandbox);
    let sandbox_task = Arc::clone(&sandbox);
    let exec_task = tokio::spawn(async move {
        sandbox_task.execute_tool(&tc, &policy, &ctx).await
    });

    // Wait for broker, then approve with TheseArgs scope.
    let mut attempts = 0;
    while broker.pending_count() == 0 && attempts < 50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        attempts += 1;
    }
    let keys: Vec<String> = broker.pending_keys();
    let req_id = keys.first().cloned().unwrap();
    broker
        .respond(
            &req_id,
            ConfirmationResponse {
                approved: true,
                approval_scope: Some(ApprovalScope::TheseArgs),
            },
        )
        .unwrap();

    let result = exec_task.await.unwrap();
    assert!(result.is_ok(), "Tool should execute after approval: {result:?}");

    // Cache should now have the (tool, args_hash) entry.
    assert!(
        sandbox.approval_cache().is_approved("destructive_test", args_hash),
        "Approval cache should contain the approved invocation"
    );
}

#[tokio::test]
async fn sandbox_denial_does_not_record() {
    use crate::security::confirmation::hash_canonical_args;

    let registry = make_registry_with_destructive();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = SandboxManager::new(
        registry.clone(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    );
    sandbox.set_confirmation_broker(broker.clone());

    let policy = make_policy("agent1");
    let tc = make_tool_call("destructive_test");
    let ctx = make_ctx("agent1");
    let args_hash = hash_canonical_args(&tc.arguments);

    let sandbox = Arc::new(sandbox);
    let sandbox_task = Arc::clone(&sandbox);
    let exec_task = tokio::spawn(async move {
        sandbox_task.execute_tool(&tc, &policy, &ctx).await
    });

    let mut attempts = 0;
    while broker.pending_count() == 0 && attempts < 50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        attempts += 1;
    }
    let keys: Vec<String> = broker.pending_keys();
    let req_id = keys.first().cloned().unwrap();
    broker
        .respond(
            &req_id,
            ConfirmationResponse {
                approved: false,
                approval_scope: None,
            },
        )
        .unwrap();

    let result = exec_task.await.unwrap();
    assert!(result.is_err(), "Denied tool should fail");
    assert!(
        !sandbox.approval_cache().is_approved("destructive_test", args_hash),
        "Denial must not populate the approval cache"
    );
}

#[tokio::test]
async fn sandbox_default_scope_when_response_missing_scope() {
    use crate::security::confirmation::hash_canonical_args;

    let registry = make_registry_with_destructive();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox = SandboxManager::new(
        registry.clone(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    );
    sandbox.set_confirmation_broker(broker.clone());

    let policy = make_policy("agent1");
    let tc = make_tool_call("destructive_test");
    let ctx = make_ctx("agent1");
    let args_hash = hash_canonical_args(&tc.arguments);

    let sandbox = Arc::new(sandbox);
    let sandbox_task = Arc::clone(&sandbox);
    let exec_task = tokio::spawn(async move {
        sandbox_task.execute_tool(&tc, &policy, &ctx).await
    });

    let mut attempts = 0;
    while broker.pending_count() == 0 && attempts < 50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        attempts += 1;
    }
    let keys: Vec<String> = broker.pending_keys();
    let req_id = keys.first().cloned().unwrap();
    // approved=true but scope omitted — should default to TheseArgs (per-args entry).
    broker
        .respond(
            &req_id,
            ConfirmationResponse {
                approved: true,
                approval_scope: None,
            },
        )
        .unwrap();

    let result = exec_task.await.unwrap();
    assert!(result.is_ok(), "Approval should allow execution: {result:?}");

    // Exact args must be approved (TheseArgs default).
    assert!(
        sandbox.approval_cache().is_approved("destructive_test", args_hash),
        "Default scope should produce a TheseArgs cache entry"
    );
    // Different args must NOT match (proves TheseArgs was used, not EntireTool).
    assert!(
        !sandbox
            .approval_cache()
            .is_approved("destructive_test", args_hash.wrapping_add(1)),
        "Default TheseArgs scope must not behave like EntireTool"
    );
}

/// **S4.** The refusal is written to the run's audit log under its own event
/// type, so finalisation can ask "what did this run have to refuse?" without
/// parsing prose out of every `security_violation` the run produced.
#[tokio::test]
async fn an_unapprovable_tool_is_filed_under_its_own_event_type() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
    let mut sandbox = SandboxManager::with_db(
        make_registry(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
        db.clone(),
    );
    sandbox.set_confirmation_broker(Arc::new(ConfirmationBroker::new()));
    let mut policy = make_policy("agent1");
    policy.require_confirmation_for = vec!["web_search".to_string()];
    policy.unattended = true;
    let ctx = make_ctx_for_task("agent1", "t-1");

    let _ = sandbox
        .execute_tool(&make_tool_call("web_search"), &policy, &ctx)
        .await;

    let rows = openalpaca_storage::repository::EventLogRepository::new(&db)
        .query(&openalpaca_storage::repository::EventLogQuery {
            task_id: Some("t-1"),
            event_type: Some(UNAPPROVABLE_EVENT_TYPE),
            limit: 10,
            ..Default::default()
        })
        .expect("read the audit log");
    assert_eq!(rows.len(), 1, "one row per refusal");
    assert_eq!(
        rows[0].detail.as_ref().unwrap()["tool_name"],
        "web_search"
    );

    // …and it is not double-counted as an ordinary violation.
    let violations = openalpaca_storage::repository::EventLogRepository::new(&db)
        .query(&openalpaca_storage::repository::EventLogQuery {
            task_id: Some("t-1"),
            event_type: Some("security_violation"),
            limit: 10,
            ..Default::default()
        })
        .expect("read the audit log");
    assert!(violations.is_empty(), "{violations:?}");
}

/// Every other refusal keeps the word it has always had — a capability the
/// agent was never granted is not a missing approver.
#[tokio::test]
async fn an_ordinary_violation_keeps_its_own_event_type() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
    let sandbox = SandboxManager::with_db(
        make_registry(),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
        db.clone(),
    );
    let mut policy = make_policy("agent1");
    policy.allowed_capabilities = Allowlist::only(["nothing_at_all"]);
    let ctx = make_ctx_for_task("agent1", "t-1");

    let _ = sandbox
        .execute_tool(&make_tool_call("web_search"), &policy, &ctx)
        .await;

    let rows = openalpaca_storage::repository::EventLogRepository::new(&db)
        .query(&openalpaca_storage::repository::EventLogQuery {
            task_id: Some("t-1"),
            event_type: Some("security_violation"),
            limit: 10,
            ..Default::default()
        })
        .expect("read the audit log");
    assert_eq!(rows.len(), 1);
}

// ── `admit_tool_surface`: the allow list admits the tools the loop was handed ──

fn surface(names: &[&str]) -> Vec<openalpaca_llm::ToolDefinition> {
    names
        .iter()
        .map(|name| openalpaca_llm::ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        })
        .collect()
}

/// A capability name and the names of the tools that provide it are different
/// words; the sandbox is asked about the tool. Admitting the resolved surface
/// is what lets `web_access` mean `web_search`.
#[test]
fn a_granted_capability_admits_the_tools_it_resolved_to() {
    let mut policy = make_policy("researcher-1");
    policy.allowed_capabilities = Allowlist::only(["web_access", "workspace_read"]);

    policy.admit_tool_surface(&surface(&["web_search", "web_fetch", "workspace_read"]));

    assert_eq!(
        policy.allowed_capabilities,
        Allowlist::Only(vec![
            "web_access".to_string(),
            "workspace_read".to_string(),
            "web_search".to_string(),
            "web_fetch".to_string(),
        ]),
        "appended once each, nothing duplicated"
    );
    // Only the surface: a tool nobody resolved is still outside the list.
    assert!(!policy.allowed_capabilities.admits("shell_execute"));
}

/// The `Allowlist::Only` contract is pre-lowercased entries, because the check
/// lowercases the called name and compares verbatim.
#[test]
fn admitted_tool_names_are_lowercased() {
    let mut policy = make_policy("worker-1");
    policy.allowed_capabilities = Allowlist::only(["acme__search"]);

    policy.admit_tool_surface(&surface(&["Acme__Search", "Notion::Query"]));

    assert_eq!(
        policy.allowed_capabilities,
        Allowlist::Only(vec!["acme__search".to_string(), "notion::query".to_string()])
    );
}

/// **Empty stays empty.** A template that granted nothing yields an agent that
/// can call nothing; an assembled surface must never back-fill it (the lead's
/// surface carries tools no template capability resolved).
#[test]
fn an_empty_allow_list_is_not_back_filled_from_the_surface() {
    let mut policy = make_policy("worker-1");
    policy.allowed_capabilities = Allowlist::Only(vec![]);

    policy.admit_tool_surface(&surface(&["web_search", "invoke_skill"]));

    assert_eq!(policy.allowed_capabilities, Allowlist::Only(vec![]));
    assert!(!policy.allowed_capabilities.admits("web_search"));
}

#[test]
fn an_unrestricted_allow_list_is_left_alone() {
    let mut policy = make_policy("main-loop");
    policy.allowed_capabilities = Allowlist::Unrestricted;

    policy.admit_tool_surface(&surface(&["web_search"]));

    assert_eq!(policy.allowed_capabilities, Allowlist::Unrestricted);
}

/// Admission never touches the deny list, and the sandbox reads the deny list
/// first — so a denied name on the surface is refused all the same.
#[tokio::test]
async fn a_denial_wins_over_an_admitted_surface() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("researcher-1");
    policy.allowed_capabilities = Allowlist::only(["web_access"]);
    policy.denied_capabilities = vec!["web_search".to_string()];

    policy.admit_tool_surface(&surface(&["web_search"]));

    assert_eq!(policy.denied_capabilities, vec!["web_search".to_string()]);
    let err = sandbox
        .execute_tool(&make_tool_call("web_search"), &policy, &make_ctx("researcher-1"))
        .await
        .unwrap_err();
    assert!(err.contains("denied"), "{err}");
}
