use super::*;
use crate::bus::EventBus;

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
