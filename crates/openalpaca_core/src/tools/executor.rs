use super::ToolRegistry;
use crate::security::sandbox::ToolExecutor;
use async_trait::async_trait;
use std::sync::Arc;

/// Tool executor backed by a ToolRegistry.
/// Replaces StubToolExecutor with real tool dispatch.
pub struct RegistryToolExecutor {
    registry: Arc<ToolRegistry>,
}

impl RegistryToolExecutor {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolExecutor for RegistryToolExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
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

    struct EchoTool;

    #[async_trait]
    impl BuiltInTool for EchoTool {
        async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
            Ok(format!("echo: {}", arguments))
        }
    }

    #[tokio::test]
    async fn test_registry_executor_delegates() {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredTool {
            definition: ToolDefinition {
                name: "echo".to_string(),
                description: "Echo tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
        });

        let executor = RegistryToolExecutor::new(Arc::new(registry));

        let result = executor
            .execute("echo", &serde_json::json!({"msg": "hi"}))
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("echo:"));
    }

    #[test]
    fn test_registered_tools_lists_names() {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredTool {
            definition: ToolDefinition {
                name: "tool_a".to_string(),
                description: "A".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
        });

        let executor = RegistryToolExecutor::new(Arc::new(registry));
        let names = executor.registered_tools();
        assert_eq!(names, vec!["tool_a".to_string()]);
    }
}
