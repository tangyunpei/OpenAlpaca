use super::*;
use crate::bus::EventBus;
use crate::security::confirmation::{ConfirmationBroker, ConfirmationResponse};

struct MockExecutor;

#[async_trait]
impl ToolExecutor for MockExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<String, String> {
        match tool_name {
            "web_search" => Ok("search results".to_string()),
            "slow_tool" => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok("done".to_string())
            }
            _ => Err(format!("Unknown tool: {}", tool_name)),
        }
    }

    fn registered_tools(&self) -> Vec<String> {
        vec!["web_search".to_string(), "slow_tool".to_string()]
    }
}

fn make_sandbox() -> SandboxManager {
    SandboxManager::new(
        Arc::new(MockExecutor),
        EventBus::default(),
        &CircuitBreakerConfig::default(),
    )
}

fn make_policy(agent_id: &str) -> SandboxPolicy {
    SandboxPolicy {
        agent_id: agent_id.to_string(),
        allowed_capabilities: vec![],
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
    }
}

fn make_tool_call(name: &str) -> ToolCall {
    ToolCall {
        id: "tc_1".to_string(),
        name: name.to_string(),
        arguments: serde_json::json!({"query": "test"}),
    }
}

#[tokio::test]
async fn test_happy_path() {
    let sandbox = make_sandbox();
    let policy = make_policy("agent1");
    let tc = make_tool_call("web_search");

    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "search results");
}

#[tokio::test]
async fn test_denied_capability() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.denied_capabilities = vec!["web_search".to_string()];
    let tc = make_tool_call("web_search");

    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("denied"));
}

#[tokio::test]
async fn test_timeout() {
    let sandbox = make_sandbox();
    let mut policy = make_policy("agent1");
    policy.max_tool_runtime_secs = 1; // 1 second timeout
    let tc = make_tool_call("slow_tool");

    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("timed out"));
}

#[tokio::test]
async fn test_security_event_emitted() {
    let bus = EventBus::default();
    let mut rx = bus.subscribe();
    let sandbox = SandboxManager::new(
        Arc::new(MockExecutor),
        bus,
        &CircuitBreakerConfig::default(),
    );
    let mut policy = make_policy("agent1");
    policy.denied_capabilities = vec!["web_search".to_string()];
    let tc = make_tool_call("web_search");

    let _ = sandbox.execute_tool("agent1", &tc, &policy).await;

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
        Arc::new(MockExecutor),
        bus,
        &CircuitBreakerConfig::default(),
    );
    let policy = make_policy("agent1");
    let tc = make_tool_call("web_search");

    let _ = sandbox.execute_tool("agent1", &tc, &policy).await;

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

#[tokio::test]
async fn test_unregistered_tool() {
    let sandbox = make_sandbox();
    let policy = make_policy("agent1");
    let tc = make_tool_call("unknown_tool");

    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
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

    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
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
            .respond(&keys[0], ConfirmationResponse { approved: true })
            .unwrap();
    });

    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
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

    let broker_clone = broker.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let keys: Vec<String> = broker_clone.pending_keys();
        assert_eq!(keys.len(), 1);
        broker_clone
            .respond(&keys[0], ConfirmationResponse { approved: false })
            .unwrap();
    });

    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
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

    // Should succeed without broker, because auto_approve bypasses confirmation
    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
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

    // Don't respond — let it time out
    let result = sandbox.execute_tool("agent1", &tc, &policy).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("timed out"),
        "Should timeout, got: {}",
        err
    );
}
