//! ContextualToolExecutor: per-request wrapper that injects owner_id
//! into owner-scoped tools (e.g. memory_search) before delegating to
//! the underlying ToolRegistry.

use crate::security::sandbox::ToolExecutor;
use crate::tools::ToolRegistry;
use async_trait::async_trait;
use std::sync::Arc;

/// Tools whose arguments need owner_id injection.
const OWNER_SCOPED_TOOLS: &[&str] = &["memory_search"];

/// Per-request execution context carrying the authenticated owner.
pub struct ToolExecutionContext {
    pub owner_id: Option<String>,
}

/// A ToolExecutor that injects contextual data (owner_id) into
/// owner-scoped tools before delegating to the inner ToolRegistry.
pub struct ContextualToolExecutor {
    registry: Arc<ToolRegistry>,
    context: ToolExecutionContext,
}

impl ContextualToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>, context: ToolExecutionContext) -> Self {
        Self { registry, context }
    }
}

#[async_trait]
impl ToolExecutor for ContextualToolExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        if OWNER_SCOPED_TOOLS.contains(&tool_name) {
            if let Some(ref owner_id) = self.context.owner_id {
                let mut args = arguments.clone();
                if let Some(obj) = args.as_object_mut() {
                    obj.insert("owner_id".to_string(), serde_json::Value::String(owner_id.clone()));
                }
                return self.registry.execute(tool_name, &args).await;
            }
        }
        self.registry.execute(tool_name, arguments).await
    }

    fn registered_tools(&self) -> Vec<String> {
        self.registry.registered_tool_names()
    }
}

#[cfg(test)]
mod tests {
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
}
