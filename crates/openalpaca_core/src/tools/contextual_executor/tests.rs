use super::*;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;

struct CaptureTool;

#[async_trait]
impl BuiltInTool for CaptureTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Return the arguments as a string so tests can inspect them
        Ok(arguments.to_string())
    }
}

fn make_registry_with_tools(names: &[&str]) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    for name in names {
        registry.register(RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: format!("{} tool", name),
                parameters: serde_json::json!({"type": "object"}),
            },
            backend: ToolBackend::BuiltIn(Arc::new(CaptureTool)),
        });
    }
    Arc::new(registry)
}

#[tokio::test]
async fn test_injects_owner_id_for_memory_search() {
    let registry = make_registry_with_tools(&["memory_search"]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: Some("user-42".to_string()),
            task_id: None,
            agent_id: None,
            db: None,
            workspace_id: None,
        },
    );

    let result = executor
        .execute("memory_search", &serde_json::json!({"query": "hello"}))
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["owner_id"], "user-42");
    assert_eq!(parsed["query"], "hello");
}

#[tokio::test]
async fn test_passes_through_non_memory_tools() {
    let registry = make_registry_with_tools(&["web_search"]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: Some("user-42".to_string()),
            task_id: None,
            agent_id: None,
            db: None,
            workspace_id: None,
        },
    );

    let result = executor
        .execute("web_search", &serde_json::json!({"query": "hello"}))
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    // owner_id should NOT be injected for non-memory tools
    assert!(parsed.get("owner_id").is_none());
}

#[tokio::test]
async fn test_overrides_spoofed_owner_id() {
    let registry = make_registry_with_tools(&["memory_search"]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: Some("real-owner".to_string()),
            task_id: None,
            agent_id: None,
            db: None,
            workspace_id: None,
        },
    );

    // LLM tries to spoof owner_id
    let result = executor
        .execute(
            "memory_search",
            &serde_json::json!({"query": "hello", "owner_id": "evil-owner"}),
        )
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    // Should be overridden with the real owner
    assert_eq!(parsed["owner_id"], "real-owner");
}

#[tokio::test]
async fn test_workspace_tools_listed_when_task_id_present() {
    let registry = make_registry_with_tools(&["web_search"]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: None,
            task_id: Some("task-1".to_string()),
            agent_id: Some("test-agent".to_string()),
            db: None,
            workspace_id: None,
        },
    );

    let tools = executor.registered_tools();
    assert!(tools.contains(&"workspace_read".to_string()));
    assert!(tools.contains(&"workspace_write".to_string()));
}

#[tokio::test]
async fn test_workspace_tools_not_listed_without_task_id() {
    let registry = make_registry_with_tools(&["web_search"]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: None,
            task_id: None,
            agent_id: None,
            db: None,
            workspace_id: None,
        },
    );

    let tools = executor.registered_tools();
    assert!(!tools.contains(&"workspace_read".to_string()));
    assert!(!tools.contains(&"workspace_write".to_string()));
}

#[tokio::test]
async fn test_workspace_read_without_task_context_errors() {
    let registry = make_registry_with_tools(&[]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: None,
            task_id: None,
            agent_id: None,
            db: None,
            workspace_id: None,
        },
    );

    let result = executor
        .execute("workspace_read", &serde_json::json!({"key": "test"}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("requires a task context"));
}

#[tokio::test]
async fn test_update_user_not_injected_workspace_id() {
    let registry = make_registry_with_tools(&["update_user"]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: Some("user-42".to_string()),
            task_id: None,
            agent_id: None,
            db: None,
            workspace_id: Some("ws-abc".to_string()),
        },
    );

    let result = executor
        .execute("update_user", &serde_json::json!({"mode": "replace"}))
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    // owner_id should be injected
    assert_eq!(parsed["owner_id"], "user-42");
    // workspace_id should NOT be injected for update_user
    assert!(parsed.get("workspace_id").is_none());
}

#[tokio::test]
async fn test_memory_search_gets_both_owner_and_workspace() {
    let registry = make_registry_with_tools(&["memory_search"]);
    let executor = ContextualToolExecutor::new(
        registry,
        ToolExecutionContext {
            owner_id: Some("user-42".to_string()),
            task_id: None,
            agent_id: None,
            db: None,
            workspace_id: Some("ws-abc".to_string()),
        },
    );

    let result = executor
        .execute("memory_search", &serde_json::json!({"query": "hello"}))
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["owner_id"], "user-42");
    assert_eq!(parsed["workspace_id"], "ws-abc");
}
