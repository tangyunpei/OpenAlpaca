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
        max_tool_runtime_ceiling_secs: 600,
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
            annotations: Some(openalpaca_mcp::ToolAnnotations::new().destructive(true)),
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
            annotations: Some(openalpaca_mcp::ToolAnnotations::new().destructive(false)),
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

// ── Design 09 §4.2: the approval fact and per-call approvals (MD-12, G2) ──

/// A stand-in for `memory_confirm`. The per-call rule is keyed on the tool's
/// **name** (`approval_is_per_call`), so a recording double registered under
/// that name drives every branch of the sandbox without a memory store; the
/// real tool's own tests (`memory_tools/tests.rs`) run the same path end to
/// end.
#[derive(Default)]
struct Recorder {
    ran: std::sync::Mutex<Vec<ToolContext>>,
    denied: std::sync::Mutex<Vec<ToolContext>>,
    refuse_preview: Option<String>,
}

#[async_trait]
impl BuiltInTool for Recorder {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("needs a context".to_string())
    }

    async fn execute_with_context(
        &self,
        _arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        self.ran.lock().unwrap().push(ctx.clone());
        Ok("ran".to_string())
    }

    async fn confirmation_preview(
        &self,
        _arguments: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        match &self.refuse_preview {
            Some(reason) => Err(reason.clone()),
            None => Ok(Some("Replace your saved note “A” with “B”?".to_string())),
        }
    }

    async fn on_confirmation_denied(
        &self,
        _arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Option<String> {
        self.denied.lock().unwrap().push(ctx.clone());
        Some("The hook heard the No.".to_string())
    }
}

/// A sandbox whose registry holds `memory_confirm` (per call) and `recorder`
/// (an ordinary tool), both recording doubles, with a broker and a bus.
fn per_call_sandbox(
    confirm: Arc<Recorder>,
    recorder: Arc<Recorder>,
) -> (
    SandboxManager,
    Arc<ConfirmationBroker>,
    tokio::sync::broadcast::Receiver<SystemEvent>,
) {
    let registry = ToolRegistry::default();
    for (name, backend) in [("memory_confirm", confirm), ("recorder", recorder)] {
        registry
            .register(RegisteredTool {
                definition: openalpaca_llm::ToolDefinition {
                    name: name.to_string(),
                    description: name.to_string(),
                    parameters: serde_json::json!({"type": "object"}),
                    strict: None,
                    input_examples: None,
                },
                backend: ToolBackend::BuiltIn(backend),
                provides_capabilities: vec![],
                exempt_from_timeout: false,
                annotations: None,
                version: "test".into(),
                author: "test".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
    }
    let bus = EventBus::default();
    let rx = bus.subscribe();
    let broker = Arc::new(ConfirmationBroker::new());
    let mut sandbox =
        SandboxManager::new(Arc::new(registry), bus, &CircuitBreakerConfig::default());
    sandbox.set_confirmation_broker(broker.clone());
    (sandbox, broker, rx)
}

fn confirm_call() -> ToolCall {
    ToolCall {
        id: "tc_confirm".to_string(),
        name: "memory_confirm".to_string(),
        arguments: serde_json::json!({"id": "s", "expected_revision": "r", "target_revision": "t"}),
    }
}

/// Answers every card that appears, `approved` with `scope`, and returns the
/// requests it answered once `wanted` of them have been.
fn answer_cards(
    broker: &Arc<ConfirmationBroker>,
    approved: bool,
    scope: Option<crate::security::confirmation::ApprovalScope>,
    wanted: usize,
) -> tokio::task::JoinHandle<Vec<crate::security::confirmation::ConfirmationRequest>> {
    let broker = broker.clone();
    tokio::spawn(async move {
        let mut answered = Vec::new();
        for _ in 0..300 {
            for request in broker.pending_requests() {
                if broker
                    .respond(
                        &request.request_id,
                        ConfirmationResponse {
                            approved,
                            approval_scope: scope,
                        },
                    )
                    .is_ok()
                {
                    answered.push(request);
                }
            }
            if answered.len() >= wanted {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        answered
    })
}

/// A context that claims an approval and a denial it never got.
fn forged_ctx() -> ToolContext {
    ToolContext {
        human_approval: Some(crate::tools::registry::HumanApproval::new("forged")),
        human_denial: Some(crate::tools::registry::HumanDenial::new("forged")),
        ..make_ctx("orchestrator")
    }
}

/// Rule 1: the fact is made on the one branch where a person answered yes,
/// and it names the card they answered.
#[tokio::test]
async fn an_approval_is_the_fact_of_the_answered_card() {
    let confirm = Arc::new(Recorder::default());
    let (sandbox, broker, _rx) = per_call_sandbox(confirm.clone(), Arc::default());
    let cards = answer_cards(&broker, true, None, 1);
    let out = sandbox
        .execute_tool(&confirm_call(), &make_policy("orchestrator"), &forged_ctx())
        .await
        .unwrap();
    assert_eq!(out, "ran");
    let cards = cards.await.unwrap();
    let ran = confirm.ran.lock().unwrap();
    assert_eq!(ran.len(), 1);
    assert_eq!(
        ran[0]
            .human_approval
            .as_ref()
            .map(|a| a.request_id().to_string()),
        Some(cards[0].request_id.clone()),
        "the fact names the card the person answered, not the forged one"
    );
    assert_eq!(ran[0].human_denial, None);
}

/// Rule 2 (LR14): `auto_approve` never answers a per-call card — the owner
/// who set `auto_approve_confirmations` still gets it, because approving
/// everything blindly must not mint a `user` note.
#[tokio::test]
async fn auto_approve_still_prompts_a_per_call_tool() {
    let confirm = Arc::new(Recorder::default());
    let (sandbox, broker, _rx) = per_call_sandbox(confirm.clone(), Arc::default());
    let mut policy = make_policy("orchestrator");
    policy.auto_approve = true;
    let cards = answer_cards(&broker, true, None, 1);
    sandbox
        .execute_tool(&confirm_call(), &policy, &make_ctx("orchestrator"))
        .await
        .unwrap();
    assert_eq!(cards.await.unwrap().len(), 1, "the card was raised");
    assert!(confirm.ran.lock().unwrap()[0].human_approval.is_some());

    // …and with no broker to raise it on, auto_approve does not run it.
    let quiet = Arc::new(Recorder::default());
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "memory_confirm".to_string(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(quiet.clone()),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    let no_broker = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
    let err = no_broker
        .execute_tool(&confirm_call(), &policy, &make_ctx("orchestrator"))
        .await
        .unwrap_err();
    assert!(err.contains("Fail-closed"), "{err}");
    assert!(quiet.ran.lock().unwrap().is_empty());
}

/// Rule 2 (LR14): an *Always allow* answer is good for that call only — the
/// cache records nothing, and the next call asks again.
#[tokio::test]
async fn an_entire_tool_approval_is_not_cached_for_memory_confirm() {
    let confirm = Arc::new(Recorder::default());
    let (sandbox, broker, _rx) = per_call_sandbox(confirm.clone(), Arc::default());
    let cards = answer_cards(
        &broker,
        true,
        Some(crate::security::confirmation::ApprovalScope::EntireTool),
        2,
    );
    for _ in 0..2 {
        sandbox
            .execute_tool(
                &confirm_call(),
                &make_policy("orchestrator"),
                &make_ctx("orchestrator"),
            )
            .await
            .unwrap();
    }
    assert_eq!(cards.await.unwrap().len(), 2, "both calls asked");
    let args_hash = crate::security::confirmation::hash_canonical_args(&confirm_call().arguments);
    assert!(
        !sandbox
            .approval_cache()
            .is_approved("memory_confirm", args_hash),
        "nothing was recorded"
    );
}

/// Rule 1: a cache hit is not a person's answer. An ordinary tool approved
/// earlier runs without a card and **without** the fact; and a cache entry
/// for `memory_confirm` itself is not consulted.
#[tokio::test]
async fn a_cache_hit_never_carries_an_approval() {
    let confirm = Arc::new(Recorder::default());
    let recorder = Arc::new(Recorder::default());
    let (sandbox, broker, _rx) = per_call_sandbox(confirm.clone(), recorder.clone());
    let mut policy = make_policy("orchestrator");
    policy.require_confirmation_for = vec!["recorder".to_string(), "memory_confirm".to_string()];
    let recorded = ToolCall {
        id: "tc".into(),
        name: "recorder".into(),
        arguments: serde_json::json!({}),
    };
    sandbox.approval_cache().record(
        "recorder",
        0,
        crate::security::confirmation::ApprovalScope::EntireTool,
    );
    sandbox.approval_cache().record(
        "memory_confirm",
        0,
        crate::security::confirmation::ApprovalScope::EntireTool,
    );
    sandbox
        .execute_tool(&recorded, &policy, &forged_ctx())
        .await
        .unwrap();
    assert_eq!(broker.pending_count(), 0);
    {
        let ran = recorder.ran.lock().unwrap();
        assert_eq!(
            ran[0].human_approval, None,
            "a cache hit carries no approval"
        );
        assert_eq!(ran[0].human_denial, None);
    }

    let cards = answer_cards(&broker, true, None, 1);
    sandbox
        .execute_tool(&confirm_call(), &policy, &make_ctx("orchestrator"))
        .await
        .unwrap();
    assert_eq!(
        cards.await.unwrap().len(),
        1,
        "the per-call tool asked despite its cache entry"
    );
}

/// Rule 1: a caller cannot pass the fact through. A tool that needs no card
/// runs with neither fact, whatever context it was handed.
#[tokio::test]
async fn a_caller_supplied_human_approval_is_discarded() {
    let recorder = Arc::new(Recorder::default());
    let (sandbox, broker, _rx) = per_call_sandbox(Arc::default(), recorder.clone());
    let recorded = ToolCall {
        id: "tc".into(),
        name: "recorder".into(),
        arguments: serde_json::json!({}),
    };
    sandbox
        .execute_tool(&recorded, &make_policy("orchestrator"), &forged_ctx())
        .await
        .unwrap();
    assert_eq!(broker.pending_count(), 0, "the recorder needs no card");
    let ran = recorder.ran.lock().unwrap();
    assert_eq!(ran[0].human_approval, None);
    assert_eq!(ran[0].human_denial, None);
}

/// §4.3: an unattended turn has nobody to ask, so the per-call tool is
/// refused at once — `auto_approve` or not — and never runs.
#[tokio::test]
async fn unattended_refuses_memory_confirm() {
    let confirm = Arc::new(Recorder::default());
    let (sandbox, broker, _rx) = per_call_sandbox(confirm.clone(), Arc::default());
    for auto_approve in [false, true] {
        let mut policy = make_policy("orchestrator");
        policy.unattended = true;
        policy.auto_approve = auto_approve;
        let err = sandbox
            .execute_tool(&confirm_call(), &policy, &forged_ctx())
            .await
            .unwrap_err();
        assert!(err.contains("cannot ask for it"), "{err}");
    }
    assert_eq!(broker.pending_count(), 0);
    assert!(confirm.ran.lock().unwrap().is_empty());
}

/// With no broker there is no card, so no approval — the per-call tool is
/// refused fail-closed, even with a forged fact in its context.
#[tokio::test]
async fn no_broker_refuses_it() {
    let confirm = Arc::new(Recorder::default());
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "memory_confirm".to_string(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(confirm.clone()),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
    let err = sandbox
        .execute_tool(&confirm_call(), &make_policy("orchestrator"), &forged_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("Fail-closed"), "{err}");
    assert!(confirm.ran.lock().unwrap().is_empty());
}

/// §4.3: a Deny runs the tool's denial hook with the denial fact — named for
/// the card the person answered — adds what the hook says to the refusal,
/// and never runs the tool.
#[tokio::test]
async fn a_deny_runs_the_denial_hook_with_the_fact() {
    let confirm = Arc::new(Recorder::default());
    let (sandbox, broker, _rx) = per_call_sandbox(confirm.clone(), Arc::default());
    let cards = answer_cards(&broker, false, None, 1);
    let err = sandbox
        .execute_tool(&confirm_call(), &make_policy("orchestrator"), &forged_ctx())
        .await
        .unwrap_err();
    let cards = cards.await.unwrap();
    assert!(err.contains("denied by user"), "{err}");
    assert!(err.ends_with("The hook heard the No."), "{err}");
    assert!(confirm.ran.lock().unwrap().is_empty());
    let denied = confirm.denied.lock().unwrap();
    assert_eq!(denied.len(), 1);
    assert_eq!(
        denied[0]
            .human_denial
            .as_ref()
            .map(|d| d.request_id().to_string()),
        Some(cards[0].request_id.clone())
    );
    assert_eq!(denied[0].human_approval, None);
}

/// §4.4: a preview that refuses raises no card — no pending request, no
/// event — and the call fails with the preview's own reason.
#[tokio::test]
async fn a_preview_refusal_raises_no_card() {
    let confirm = Arc::new(Recorder {
        refuse_preview: Some("revision_conflict: the note moved".to_string()),
        ..Default::default()
    });
    let (sandbox, broker, mut rx) = per_call_sandbox(confirm.clone(), Arc::default());
    let err = sandbox
        .execute_tool(
            &confirm_call(),
            &make_policy("orchestrator"),
            &make_ctx("orchestrator"),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "revision_conflict: the note moved");
    assert_eq!(broker.pending_count(), 0);
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|e| matches!(e, SystemEvent::ToolConfirmationRequested { .. })),
        "no card went out"
    );
    assert!(confirm.ran.lock().unwrap().is_empty());
}

/// §4.4: the card carries the daemon's preview and says it is per call —
/// on the broker's pending request (what `GET /v1/chat/confirmations`
/// serves) and on the bus event every client hears.
#[tokio::test]
async fn the_card_carries_the_preview_and_per_call() {
    let (sandbox, broker, mut rx) = per_call_sandbox(Arc::default(), Arc::default());
    let cards = answer_cards(&broker, true, None, 1);
    sandbox
        .execute_tool(
            &confirm_call(),
            &make_policy("orchestrator"),
            &make_ctx("orchestrator"),
        )
        .await
        .unwrap();
    let card = cards.await.unwrap().remove(0);
    assert!(card.per_call);
    assert_eq!(
        card.preview.as_deref(),
        Some("Replace your saved note “A” with “B”?")
    );
    match rx.try_recv().unwrap() {
        SystemEvent::ToolConfirmationRequested {
            preview, per_call, ..
        } => {
            assert!(per_call);
            assert_eq!(preview, card.preview);
        }
        other => panic!("Expected ToolConfirmationRequested, got {other:?}"),
    }
}

// ── R29: the `../` rule follows the backend (storage design §11) ────────

/// A builtin, a command tool, an MCP tool and a skill script side by side.
fn traversal_sandbox(skill_dir: &std::path::Path) -> SandboxManager {
    let tool = |name: &str, backend: ToolBackend| RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend,
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    };
    let registry = ToolRegistry::default();
    registry
        .register(tool("note_tool", ToolBackend::BuiltIn(Arc::new(MockTool))))
        .unwrap();
    registry
        .register(tool(
            "cmd_tool",
            ToolBackend::Command {
                command: "true".to_string(),
                args_template: Some("{path}".to_string()),
                timeout_secs: 5,
            },
        ))
        .unwrap();
    registry
        .register(tool(
            "srv__echo",
            ToolBackend::Mcp {
                client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests("srv")),
                remote_name: "echo".to_string(),
                server_name: "srv".to_string(),
                generation: 0,
            },
        ))
        .unwrap();
    std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
    std::fs::write(skill_dir.join("scripts/noop.sh"), "exit 0\n").unwrap();
    let script = crate::tools::builtins::ScriptToolBuiltIn::new(
        skill_dir,
        &crate::middleware::skill::ScriptConfig {
            file: "noop.sh".to_string(),
            name: "noop".to_string(),
            description: "Does nothing".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            interpreter: Some("sh".to_string()),
            timeout_secs: 5,
        },
        crate::tools::builtins::SkillRoots::default(),
    )
    .unwrap();
    registry
        .register(tool("skill_script:noop", ToolBackend::BuiltIn(Arc::new(script))))
        .unwrap();
    SandboxManager::new(
        Arc::new(registry),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    )
}

fn traversal_call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "tc_r29".to_string(),
        name: name.to_string(),
        arguments,
    }
}

#[tokio::test]
async fn a_builtins_content_may_hold_a_relative_path() {
    let skill = tempfile::tempdir().unwrap();
    let sandbox = traversal_sandbox(skill.path());
    let out = sandbox
        .execute_tool(
            &traversal_call(
                "note_tool",
                serde_json::json!({"content": "import { x } from '../lib/x';"}),
            ),
            &make_policy("agent1"),
            &make_ctx("agent1"),
        )
        .await;
    assert_eq!(out.as_deref(), Ok("search results"));
}

#[tokio::test]
async fn a_command_tool_an_mcp_tool_and_a_skill_script_keep_the_traversal_rule() {
    let skill = tempfile::tempdir().unwrap();
    let sandbox = traversal_sandbox(skill.path());
    for (name, arguments) in [
        ("cmd_tool", serde_json::json!({"path": "../secret"})),
        ("srv__echo", serde_json::json!({"text": "../secret"})),
        ("skill_script:noop", serde_json::json!({"id": "../../escape"})),
    ] {
        let err = sandbox
            .execute_tool(
                &traversal_call(name, arguments),
                &make_policy("agent1"),
                &make_ctx("agent1"),
            )
            .await
            .unwrap_err();
        assert_eq!(err, "Input blocked: Path traversal pattern detected", "{name}");
    }
}

#[tokio::test]
async fn a_builtins_null_byte_is_still_refused() {
    let skill = tempfile::tempdir().unwrap();
    let sandbox = traversal_sandbox(skill.path());
    let err = sandbox
        .execute_tool(
            &traversal_call("note_tool", serde_json::json!({"content": "a\u{0}b"})),
            &make_policy("agent1"),
            &make_ctx("agent1"),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "Input blocked: Argument contains null bytes");
}

// ── R-T35: an explicit confirmation list naming `file_write` asks for `file_edit` ──

/// The lead's ruling R-T35: `file_edit` provides `file_write`'s capability
/// (R42), so whoever holds `file_write` holds `file_edit` beside it — a
/// template granted the capability, a skill allowed `file_write` by name
/// (`constraints::paired_tools`). An explicit list that names `file_write`
/// asks for `file_edit` too, so a project file is never changed with no card
/// where the author asked writes to prompt (T34's shape). Only that pair: a
/// list naming a reader, or naming neither half, is returned as written, and
/// one naming both is not doubled.
#[test]
fn an_explicit_confirmation_list_naming_file_write_also_asks_for_file_edit() {
    let registry = make_registry();
    let set = |list: &[&str]| {
        let mut policy = make_policy("agent1");
        policy.require_confirmation_for = list.iter().map(|name| name.to_string()).collect();
        effective_confirmation_set(&policy, &registry)
    };

    assert_eq!(set(&["file_write"]), ["file_write", "file_edit"]);
    assert_eq!(
        set(&["shell_execute", "file_write"]),
        ["shell_execute", "file_write", "file_edit"]
    );
    assert_eq!(
        set(&["file_edit", "file_write"]),
        ["file_edit", "file_write"]
    );
    assert_eq!(set(&["file_read"]), ["file_read"]);
    assert_eq!(set(&["shell_execute"]), ["shell_execute"]);
}

// ── N-TOOL-1b (B-11): one per-tool bound, a model-settable timeout_secs ──

fn bounded(max_tool_runtime_secs: u64, max_tool_runtime_ceiling_secs: u64) -> SandboxPolicy {
    SandboxPolicy {
        max_tool_runtime_secs,
        max_tool_runtime_ceiling_secs,
        ..make_policy("agent1")
    }
}

/// A template's `timeout_seconds` is the task's budget, not a tool's: the
/// policy built from a template's constraints takes the daemon's bound and
/// ceiling, whatever the template says.
#[test]
fn a_policy_from_a_template_takes_the_daemons_bound_not_its_timeout_seconds() {
    let constraints = AgentConstraints {
        timeout_seconds: Some(600),
        ..Default::default()
    };
    let policy = SandboxPolicy::from_constraints(
        "builder-1",
        &constraints,
        &crate::daemon_config::AgentDefaults::default(),
    );
    assert_eq!(policy.max_tool_runtime_secs, 120);
    assert_eq!(policy.max_tool_runtime_ceiling_secs, 600);
}

/// A command's `timeout_secs` is clamped to the ceiling, never refused; a
/// smaller one is its own bound; nothing usable asks for nothing; a tool
/// that runs no command keeps the bound whatever its arguments hold.
#[test]
fn a_commands_timeout_secs_is_its_bound_up_to_the_ceiling() {
    let policy = bounded(120, 600);
    let asks = |arguments: serde_json::Value| policy.runtime_secs_for(&arguments, true, None);
    assert_eq!(asks(serde_json::json!({"command": "make"})), 120);
    assert_eq!(asks(serde_json::json!({"timeout_secs": 3600})), 600);
    assert_eq!(asks(serde_json::json!({"timeout_secs": 600})), 600);
    assert_eq!(asks(serde_json::json!({"timeout_secs": 300})), 300);
    assert_eq!(asks(serde_json::json!({"timeout_secs": 5})), 5);
    for nothing in [
        serde_json::json!({"timeout_secs": 0}),
        serde_json::json!({"timeout_secs": -5}),
        serde_json::json!({"timeout_secs": 2.5}),
        serde_json::json!({"timeout_secs": "300"}),
        serde_json::json!({"timeout_secs": null}),
        serde_json::json!("not an object"),
    ] {
        assert_eq!(asks(nothing.clone()), 120, "{nothing}");
    }
    assert_eq!(
        policy.runtime_secs_for(&serde_json::json!({"timeout_secs": 300}), false, None),
        120,
        "a tool that runs no command keeps the bound"
    );
    // What was asked, apart from the bound: what a `Command`-backend tool's
    // own figure gives way to (CMD-TIMEOUT).
    let asked =
        |arguments: serde_json::Value, runs: bool| policy.asked_runtime_secs(&arguments, runs);
    assert_eq!(asked(serde_json::json!({"command": "make"}), true), None);
    assert_eq!(asked(serde_json::json!({"timeout_secs": 0}), true), None);
    assert_eq!(asked(serde_json::json!({"timeout_secs": 5}), true), Some(5));
    assert_eq!(
        asked(serde_json::json!({"timeout_secs": 3600}), true),
        Some(600)
    );
    assert_eq!(asked(serde_json::json!({"timeout_secs": 300}), false), None);
}

/// CMD-TIMEOUT-2: a call that asks for nothing runs under the bound, or
/// under a `Command`-backend tool's own figure above it clamped to the
/// ceiling — never below the bound; what a call asks for still comes first.
#[test]
fn a_command_tools_own_figure_above_the_bound_is_its_unasked_cut() {
    let policy = bounded(120, 600);
    let unasked = serde_json::json!({});
    let cut = |own: Option<u64>| policy.runtime_secs_for(&unasked, true, own);
    assert_eq!(cut(None), 120);
    assert_eq!(cut(Some(30)), 120, "below the bound: its own clock first");
    assert_eq!(cut(Some(120)), 120);
    assert_eq!(cut(Some(300)), 300);
    assert_eq!(cut(Some(900)), 600, "clamped to the ceiling");
    assert_eq!(
        policy.runtime_secs_for(&serde_json::json!({"timeout_secs": 5}), true, Some(300)),
        5,
        "what a call asks for comes first"
    );
    assert_eq!(
        bounded(120, 200).default_runtime_secs(Some(300)),
        200,
        "a ceiling below the author's figure clamps it"
    );
    assert_eq!(
        bounded(5, 2).default_runtime_secs(Some(300)),
        5,
        "never below the bound, even under a ceiling below it"
    );
}

/// Through the sandbox: a tool that runs no command is cut at the bound,
/// whatever `timeout_secs` it was sent, and is not told about one.
#[tokio::test]
async fn a_tool_that_runs_no_command_is_cut_at_the_bound_whatever_it_asks() {
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "slow_tool".to_string(),
                description: "slow_tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(crate::test_util::Sleeps)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
    let call = ToolCall {
        id: "tc_slow".to_string(),
        name: "slow_tool".to_string(),
        arguments: serde_json::json!({"timeout_secs": 2}),
    };
    let err = sandbox
        .execute_tool(&call, &bounded(1, 600), &ToolContext::default())
        .await
        .unwrap_err();
    assert_eq!(err, "Tool 'slow_tool' timed out after 1s");
}

/// A `Command`-backend tool running `sleep <seconds>`, its author's own
/// `timeout_secs` (its TOML's) `own_secs`.
fn sleeping_command_tool(seconds: &str, own_secs: u64) -> ToolRegistry {
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "slow_cmd".to_string(),
                description: "slow_cmd".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Command {
                command: "sleep".to_string(),
                args_template: Some(seconds.to_string()),
                timeout_secs: own_secs,
            },
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    registry
}

/// CMD-TIMEOUT-2: a `Command`-backend tool whose author's `timeout_secs`
/// is above the per-tool bound runs a call that asks for nothing to that
/// figure — its author's 3 s, under a 1 s bound, runs a 2-second command
/// to its end.
#[tokio::test]
async fn an_unasked_command_tool_above_the_bound_runs_to_its_own_figure() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("2", 3)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({}),
    };
    let out = sandbox
        .execute_tool(&call, &bounded(1, 10), &gp.ctx())
        .await;
    assert_eq!(out, Ok(String::new()));
}

/// CMD-TIMEOUT-2: such a call is cut by the sandbox at its author's figure
/// — one clock, never its backend's at the same instant — and the hint
/// names the ceiling, as `shell_execute`'s does, not that figure (a call
/// may lift it up to the ceiling, CMD-TIMEOUT). Was
/// `a_command_tool_cut_below_the_ceiling_is_told_the_ceiling_not_its_own_figure`,
/// cut at the 1 s bound.
#[tokio::test]
async fn an_unasked_command_tool_above_the_bound_is_cut_at_its_own_figure() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("5", 3)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({}),
    };
    let err = sandbox
        .execute_tool(&call, &bounded(1, 10), &gp.ctx())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Tool 'slow_cmd' timed out after 3s\nHint: a command may run up to 10 seconds here — \
         call it again with a larger timeout_secs, or split the work into smaller commands \
         that each finish sooner."
    );
}

/// CMD-TIMEOUT-2: an author's figure above the ceiling is clamped to it —
/// its author's 5 s under a 2 s ceiling is cut at 2 s, and told that is the
/// most a command may run here.
#[tokio::test]
async fn an_unasked_command_tool_above_the_ceiling_is_cut_at_the_ceiling() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("4", 5)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({}),
    };
    let err = sandbox
        .execute_tool(&call, &bounded(1, 2), &gp.ctx())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!(
            "Tool 'slow_cmd' timed out after 2s\n{}",
            crate::tools::platform::COMMAND_TIMED_OUT_HINT
        )
    );
}

/// CMD-TIMEOUT-2, one clock: an author's figure equal to the bound is the
/// sandbox's cut, so the backend's clock stands back — the call is cut by
/// the sandbox, with its hint, never by its backend at the same instant.
#[tokio::test]
async fn an_unasked_command_tool_at_the_bound_is_cut_by_the_sandbox() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("4", 2)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({}),
    };
    let err = sandbox
        .execute_tool(&call, &bounded(2, 10), &gp.ctx())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Tool 'slow_cmd' timed out after 2s\nHint: a command may run up to 10 seconds here — \
         call it again with a larger timeout_secs, or split the work into smaller commands \
         that each finish sooner."
    );
}

/// CMD-TIMEOUT: a `Command`-backend tool's own `timeout_secs` is the call's
/// default, and a call's `timeout_secs` lifts it — one whose author gave it
/// 1 s, asked for 3, runs a 2-second command to its end. Was
/// `a_command_tools_own_timeout_still_stops_it_whatever_a_call_asks`, the
/// no-change half (killed at its author's figure whatever a call asked).
#[tokio::test]
async fn a_calls_timeout_secs_lifts_a_command_tools_own_figure() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("2", 1)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({"timeout_secs": 3}),
    };
    let out = sandbox
        .execute_tool(&call, &bounded(1, 10), &gp.ctx())
        .await;
    assert_eq!(out, Ok(String::new()));
}

/// The default half: a call that asks for nothing runs under its author's
/// figure, killed by its backend at 1 s though the sandbox would allow 5 —
/// and since CMD-TIMEOUT-2 told, as the sandbox's cut is, that it may ask
/// for up to the 10 s ceiling.
#[tokio::test]
async fn a_command_tool_asked_for_nothing_stops_at_its_own_figure() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("3", 1)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({}),
    };
    let err = sandbox
        .execute_tool(&call, &bounded(5, 10), &gp.ctx())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Command timed out after 1s\nHint: a command may run up to 10 seconds here — \
         call it again with a larger timeout_secs, or split the work into smaller commands \
         that each finish sooner."
    );
}

/// A call asking above the ceiling runs under the ceiling — cut by the
/// sandbox at 2 s, not by its backend at its author's 1 s — and is told that
/// is the most a command may run here.
#[tokio::test]
async fn a_command_tool_asking_above_the_ceiling_is_cut_at_the_ceiling() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("4", 1)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({"timeout_secs": 10}),
    };
    let err = sandbox
        .execute_tool(&call, &bounded(1, 2), &gp.ctx())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!(
            "Tool 'slow_cmd' timed out after 2s\n{}",
            crate::tools::platform::COMMAND_TIMED_OUT_HINT
        )
    );
}

/// One clock: a call that asked for 2 s, below the 10 s ceiling, is cut by
/// the sandbox at its figure — never by its backend at the same instant,
/// with the backend's words — and is told it may ask for up to the ceiling.
#[tokio::test]
async fn a_command_tool_cut_at_its_asked_figure_is_cut_by_the_sandbox() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("4", 1)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({"timeout_secs": 2}),
    };
    let err = sandbox
        .execute_tool(&call, &bounded(1, 10), &gp.ctx())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Tool 'slow_cmd' timed out after 2s\nHint: a command may run up to 10 seconds here — \
         call it again with a larger timeout_secs, or split the work into smaller commands \
         that each finish sooner."
    );
}

/// The figure is the sandbox's to set: a context a caller built with one is
/// handed on without it, so a call that asked for nothing is still killed at
/// its author's 1 s, with the backend's words — and so is a ceiling
/// (CMD-TIMEOUT-2): the hint names the policy's 10 s, not the caller's 99.
#[tokio::test]
async fn a_caller_supplied_asked_figure_is_discarded() {
    let gp = crate::test_util::GuardedProject::new();
    let sandbox =
        SandboxManager::with_defaults(Arc::new(sleeping_command_tool("3", 1)), EventBus::default());
    let call = ToolCall {
        id: "tc_cmd".to_string(),
        name: "slow_cmd".to_string(),
        arguments: serde_json::json!({}),
    };
    let ctx = ToolContext {
        asked_runtime_secs: Some(10),
        runtime_ceiling_secs: Some(99),
        ..gp.ctx()
    };
    let err = sandbox
        .execute_tool(&call, &bounded(5, 10), &ctx)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Command timed out after 1s\nHint: a command may run up to 10 seconds here — \
         call it again with a larger timeout_secs, or split the work into smaller commands \
         that each finish sooner."
    );
}

// ── PLUGIN-WAIT: a plugin tool waits the sandbox's figure ───────────

/// A plugin's tool, `slow-plugin::slow`, whose executor answers after 2 s.
fn slow_plugin_tool() -> ToolRegistry {
    struct AnswersLate;
    #[async_trait]
    impl openalpaca_api::plugin_traits::PluginToolExecutor for AnswersLate {
        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok("late".to_string())
        }
        fn plugin_id(&self) -> &str {
            "slow-plugin"
        }
    }
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "slow-plugin::slow".to_string(),
                description: "slow".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Plugin(Arc::new(AnswersLate)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "plugin:slow-plugin".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    registry
}

/// A plugin tool's call may ask for its own figure, as a command's may: its
/// `timeout_secs` lifts the bound for that call, clamped to the ceiling; one
/// that asks nothing is cut at the bound.
#[tokio::test]
async fn a_plugin_tools_call_may_ask_for_its_own_figure() {
    let sandbox = SandboxManager::with_defaults(Arc::new(slow_plugin_tool()), EventBus::default());
    let call = |arguments: serde_json::Value| ToolCall {
        id: "tc_plugin".to_string(),
        name: "slow-plugin::slow".to_string(),
        arguments,
    };
    let asked = sandbox
        .execute_tool(
            &call(serde_json::json!({"timeout_secs": 3})),
            &bounded(1, 600),
            &ToolContext::default(),
        )
        .await;
    assert_eq!(asked.as_deref(), Ok("late"));
    let clamped = sandbox
        .execute_tool(
            &call(serde_json::json!({"timeout_secs": 3600})),
            &bounded(1, 3),
            &ToolContext::default(),
        )
        .await;
    assert_eq!(clamped.as_deref(), Ok("late"));
    let cut = sandbox
        .execute_tool(
            &call(serde_json::json!({})),
            &bounded(1, 600),
            &ToolContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(cut, "Tool 'slow-plugin::slow' timed out after 1s");
}

/// What a plugin tool's executor was handed: the sandbox's cut, or `None`
/// when called through `execute`.
#[derive(Default)]
struct RecordsTheCut(std::sync::Mutex<Vec<Option<Duration>>>);

#[async_trait]
impl openalpaca_api::plugin_traits::PluginToolExecutor for RecordsTheCut {
    async fn execute(
        &self,
        _tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<String, String> {
        self.0.lock().unwrap().push(None);
        Ok("plain".to_string())
    }
    async fn execute_within(
        &self,
        _tool_name: &str,
        _arguments: &serde_json::Value,
        sandbox_cut: Duration,
    ) -> Result<String, String> {
        self.0.lock().unwrap().push(Some(sandbox_cut));
        Ok("within".to_string())
    }
    fn plugin_id(&self) -> &str {
        "rec"
    }
}

/// The sandbox hands a plugin tool the figure it cuts the call at — the
/// bound, or what the call asked for clamped to the ceiling — and a figure a
/// caller put on the context is not passed through.
#[tokio::test]
async fn the_sandbox_hands_a_plugin_tool_the_figure_it_cuts_at() {
    let recorder = Arc::new(RecordsTheCut::default());
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "rec::tool".to_string(),
                description: "records".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Plugin(recorder.clone()),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "plugin:rec".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
    let call = |arguments: serde_json::Value| ToolCall {
        id: "tc_rec".to_string(),
        name: "rec::tool".to_string(),
        arguments,
    };
    let forged = ToolContext {
        sandbox_cut_secs: Some(99),
        ..Default::default()
    };
    for (arguments, ctx) in [
        (serde_json::json!({}), ToolContext::default()),
        (
            serde_json::json!({"timeout_secs": 7}),
            ToolContext::default(),
        ),
        (
            serde_json::json!({"timeout_secs": 9999}),
            ToolContext::default(),
        ),
        (serde_json::json!({}), forged),
    ] {
        let answer = sandbox
            .execute_tool(&call(arguments), &bounded(3, 600), &ctx)
            .await;
        assert_eq!(answer.as_deref(), Ok("within"));
    }
    let secs = |s| Some(Duration::from_secs(s));
    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![secs(3), secs(7), secs(600), secs(3)]
    );
}
