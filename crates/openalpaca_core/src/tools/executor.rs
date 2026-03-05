use super::ToolRegistry;
use crate::security::sandbox::ToolExecutor;
use async_trait::async_trait;
use std::sync::Arc;

/// Tools whose `owner_id` / `workspace_id` parameters must be injected by a
/// contextual executor (e.g., `ContextualToolExecutor`). When called through
/// `RegistryToolExecutor` directly, any LLM-supplied values for these fields
/// are stripped to prevent spoofing.
const OWNER_SCOPED_TOOLS: &[&str] = &[
    "memory_search",
    "update_persona",
];

/// Tool executor backed by a ToolRegistry.
/// Replaces StubToolExecutor with real tool dispatch.
///
/// Safety: for owner-scoped tools, strips any `owner_id` / `workspace_id`
/// from arguments to prevent LLM spoofing when used without
/// `ContextualToolExecutor`.
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
        // Strip LLM-supplied owner_id/workspace_id from owner-scoped tools.
        // These fields must be injected by ContextualToolExecutor, not the LLM.
        // Without this guard, a direct RegistryToolExecutor call would let the
        // LLM spoof another user's identity.
        if OWNER_SCOPED_TOOLS.contains(&tool_name) {
            let mut sanitized = arguments.clone();
            if let Some(obj) = sanitized.as_object_mut() {
                if obj.remove("owner_id").is_some() {
                    tracing::warn!(
                        tool = tool_name,
                        "Stripped LLM-supplied owner_id from tool arguments \
                         (must be injected by ContextualToolExecutor)"
                    );
                }
                if obj.remove("workspace_id").is_some() {
                    tracing::warn!(
                        tool = tool_name,
                        "Stripped LLM-supplied workspace_id from tool arguments"
                    );
                }
            }
            return self.registry.execute(tool_name, &sanitized).await;
        }

        self.registry.execute(tool_name, arguments).await
    }

    fn registered_tools(&self) -> Vec<String> {
        self.registry.registered_tool_names()
    }

    fn shell_like_tools(&self) -> Vec<String> {
        self.registry.command_backend_tool_names()
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
                strict: None,
                input_examples: None,
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
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
        });

        let executor = RegistryToolExecutor::new(Arc::new(registry));
        let names = executor.registered_tools();
        assert_eq!(names, vec!["tool_a".to_string()]);
    }

    /// Verify that RegistryToolExecutor strips LLM-supplied owner_id from
    /// owner-scoped tools to prevent spoofing.
    #[tokio::test]
    async fn test_strips_owner_id_from_memory_search() {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredTool {
            definition: ToolDefinition {
                name: "memory_search".to_string(),
                description: "Search memories".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
        });

        let executor = RegistryToolExecutor::new(Arc::new(registry));

        // LLM tries to supply owner_id — it should be stripped
        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "hello", "owner_id": "evil-owner"}),
            )
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result.replace("echo: ", ""))
            .unwrap_or_else(|_| {
                // EchoTool returns "echo: {json}", parse the json part
                let json_part = result.strip_prefix("echo: ").unwrap_or(&result);
                serde_json::from_str(json_part).unwrap()
            });
        // owner_id should have been stripped
        assert!(
            parsed.get("owner_id").is_none(),
            "owner_id should be stripped from owner-scoped tools via RegistryToolExecutor"
        );
        // query should pass through unchanged
        assert_eq!(parsed["query"], "hello");
    }

    /// Verify that RegistryToolExecutor strips workspace_id too.
    #[tokio::test]
    async fn test_strips_workspace_id_from_memory_search() {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredTool {
            definition: ToolDefinition {
                name: "memory_search".to_string(),
                description: "Search memories".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
        });

        let executor = RegistryToolExecutor::new(Arc::new(registry));

        let result = executor
            .execute(
                "memory_search",
                &serde_json::json!({"query": "hello", "workspace_id": "evil-ws"}),
            )
            .await
            .unwrap();

        let json_part = result.strip_prefix("echo: ").unwrap_or(&result);
        let parsed: serde_json::Value = serde_json::from_str(json_part).unwrap();
        assert!(
            parsed.get("workspace_id").is_none(),
            "workspace_id should be stripped from owner-scoped tools via RegistryToolExecutor"
        );
    }

    /// Verify that non-owner-scoped tools are NOT affected by stripping.
    #[tokio::test]
    async fn test_non_owner_scoped_tool_passes_through() {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredTool {
            definition: ToolDefinition {
                name: "echo".to_string(),
                description: "Echo tool".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
        });

        let executor = RegistryToolExecutor::new(Arc::new(registry));

        // A regular tool should pass all args through unchanged
        let result = executor
            .execute(
                "echo",
                &serde_json::json!({"msg": "hi", "owner_id": "should-pass"}),
            )
            .await
            .unwrap();

        let json_part = result.strip_prefix("echo: ").unwrap_or(&result);
        let parsed: serde_json::Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(parsed["owner_id"], "should-pass");
    }
}
