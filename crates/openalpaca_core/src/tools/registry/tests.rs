use super::*;

struct MockBuiltIn {
    response: String,
}

#[async_trait]
impl BuiltInTool for MockBuiltIn {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Ok(self.response.clone())
    }
}

fn make_tool_with_caps(name: &str, caps: Vec<&str>) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: name.to_string(),
            description: format!("{} tool", name),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
        provides_capabilities: caps.into_iter().map(String::from).collect(),
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }
}

fn make_tool(name: &str, response: &str) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: name.to_string(),
            description: format!("{} tool", name),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: response.to_string(),
        })),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }
}

#[test]
fn test_register_and_lookup() {
    let registry = ToolRegistry::default();
    registry.register(make_tool("test_tool", "ok")).unwrap();

    assert!(registry.get("test_tool").is_some());
    assert!(registry.get("nonexistent").is_none());
    assert_eq!(registry.count(), 1);
}

#[tokio::test]
async fn test_execute_builtin() {
    let registry = ToolRegistry::default();
    registry.register(make_tool("test_tool", "hello")).unwrap();

    let result = registry.execute("test_tool", &serde_json::json!({})).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "hello");
}

#[tokio::test]
async fn test_execute_unknown_tool() {
    let registry = ToolRegistry::default();
    let result = registry
        .execute("nonexistent", &serde_json::json!({}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown tool"));
}

#[test]
fn test_registered_tool_names() {
    let registry = ToolRegistry::default();
    registry.register(make_tool("web_search", "results")).unwrap();
    registry.register(make_tool("summarize", "summary")).unwrap();

    let names = registry.registered_tool_names();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"web_search".to_string()));
    assert!(names.contains(&"summarize".to_string()));
}

#[test]
fn test_command_backend_tool_names_empty_for_builtins() {
    let registry = ToolRegistry::default();
    registry.register(make_tool("web_search", "results")).unwrap();
    registry.register(make_tool("summarize", "summary")).unwrap();

    let cmd_tools = registry.command_backend_tool_names();
    assert!(
        cmd_tools.is_empty(),
        "Built-in tools should not appear in command_backend_tool_names()"
    );
}

#[test]
fn test_command_backend_tool_names_returns_command_tools() {
    let registry = ToolRegistry::default();
    // Register a built-in tool
    registry.register(make_tool("web_search", "results")).unwrap();
    // Register a command-backend tool
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "git_log".to_string(),
            description: "Show git log".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Command {
            command: "git".to_string(),
            args_template: Some("log --oneline -n {count}".to_string()),
            timeout_secs: 15,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let cmd_tools = registry.command_backend_tool_names();
    assert_eq!(cmd_tools.len(), 1);
    assert!(cmd_tools.contains(&"git_log".to_string()));
}

#[tokio::test]
async fn test_execute_http_ssrf_blocks_private_ip() {
    // Verify that execute_http validates URLs (SSRF protection)
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "internal_api".to_string(),
            description: "Internal API".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            url: "http://169.254.169.254/latest/meta-data/".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let result = registry
        .execute("internal_api", &serde_json::json!({}))
        .await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("blocked"),
        "SSRF to cloud metadata should be blocked"
    );
}

#[tokio::test]
async fn test_execute_http_ssrf_blocks_localhost() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "local_api".to_string(),
            description: "Local API".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            url: "http://localhost/admin".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let result = registry.execute("local_api", &serde_json::json!({})).await;
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("blocked"),
        "SSRF to localhost should be blocked"
    );
}

// --- Issue 13: Unsubstituted placeholder detection ---

#[tokio::test]
async fn test_http_unsubstituted_placeholder_detected() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "weather".to_string(),
            description: "Get weather".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"}
                },
                "required": ["city"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            url: "https://api.example.com/weather?city={city}&units={units}".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    // Only provide "city" but not "units" — {units} should be detected
    let result = registry
        .execute("weather", &serde_json::json!({"city": "London"}))
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("unsubstituted placeholder"),
        "Should detect unsubstituted placeholder, got: {}",
        err
    );
    assert!(
        err.contains("{units}"),
        "Should mention the placeholder name"
    );
}

#[tokio::test]
async fn test_http_all_placeholders_substituted_passes() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "weather".to_string(),
            description: "Get weather".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"}
                },
                "required": ["city"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            // URL with placeholder that will be properly substituted
            url: "https://api.example.com/weather?city={city}".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    // This will pass placeholder check but fail on the actual HTTP request
    // (network error), which is expected — we're just checking that the
    // placeholder validation doesn't false-positive.
    let result = registry
        .execute("weather", &serde_json::json!({"city": "London"}))
        .await;
    // Should NOT fail with "unsubstituted placeholder"
    if let Err(ref e) = result {
        assert!(
            !e.contains("unsubstituted placeholder"),
            "Should not detect unsubstituted placeholder when all are filled"
        );
    }
}

// --- Issue 12: JSON Schema pre-validation ---

#[tokio::test]
async fn test_schema_missing_required_field() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let result = registry.execute("search", &serde_json::json!({})).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("missing required parameter 'query'"),
        "Should detect missing required field, got: {}",
        err
    );
}

#[tokio::test]
async fn test_schema_wrong_type() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer"}
                },
                "required": ["query"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    // limit should be integer, but we pass a string
    let result = registry
        .execute(
            "search",
            &serde_json::json!({"query": "test", "limit": "not_a_number"}),
        )
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("should be integer"),
        "Should detect type mismatch, got: {}",
        err
    );
}

#[tokio::test]
async fn test_schema_valid_args_pass() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer"}
                },
                "required": ["query"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let result = registry
        .execute("search", &serde_json::json!({"query": "test", "limit": 5}))
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "ok");
}

#[tokio::test]
async fn test_schema_non_object_args_rejected() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let result = registry
        .execute("search", &serde_json::json!("not an object"))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be a JSON object"));
}

// --- Issue 13: Command backend unsubstituted placeholder detection ---

#[tokio::test]
async fn test_command_unsubstituted_placeholder_detected() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "git_log".to_string(),
            description: "Show git log".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "count": {"type": "string"},
                    "branch": {"type": "string"}
                },
                "required": ["count"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Command {
            command: "git".to_string(),
            args_template: Some("log --oneline -n {count} {branch}".to_string()),
            timeout_secs: 15,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    // Only provide "count" but not "branch" — {branch} should be detected
    let result = registry
        .execute("git_log", &serde_json::json!({"count": "5"}))
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("unsubstituted placeholder"),
        "Should detect unsubstituted placeholder, got: {}",
        err
    );
    assert!(
        err.contains("{branch}"),
        "Should mention the placeholder name, got: {}",
        err
    );
}

#[tokio::test]
async fn test_command_all_placeholders_substituted_runs() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "echo_tool".to_string(),
            description: "Echo a message".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "msg": {"type": "string"}
                },
                "required": ["msg"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Command {
            command: "echo".to_string(),
            args_template: Some("{msg}".to_string()),
            timeout_secs: 5,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let result = registry
        .execute("echo_tool", &serde_json::json!({"msg": "hello"}))
        .await;
    // Should succeed — no unsubstituted placeholders
    assert!(result.is_ok(), "Expected success, got: {:?}", result);
    assert!(result.unwrap().contains("hello"));
}

// --- registry::execute_with_context ---

#[tokio::test]
async fn test_registry_execute_with_context_routes_to_builtin() {
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "mock".to_string(),
        })),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let ctx = super::ToolContext::default();
    let result = registry
        .execute_with_context("test_tool", &serde_json::json!({}), &ctx)
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "mock");
}

#[tokio::test]
async fn test_registry_execute_with_context_unknown_tool() {
    let registry = ToolRegistry::default();
    let ctx = super::ToolContext::default();
    let result = registry
        .execute_with_context("no_such_tool", &serde_json::json!({}), &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// --- ToolContext and execute_with_context ---

#[tokio::test]
async fn test_execute_with_context_defaults_to_execute() {
    // A tool that only implements execute() should work via execute_with_context()
    let tool = MockBuiltIn {
        response: "mock".to_string(),
    };
    let ctx = super::ToolContext {
        agent_id: Some("test-agent".to_string()),
        ..Default::default()
    };
    let args = serde_json::json!({"key": "value"});
    let result = tool.execute_with_context(&args, &ctx).await.unwrap();
    assert_eq!(result, "mock");
}

// --- memory_search reads from ToolContext ---

#[tokio::test]
async fn test_memory_search_reads_context_not_args() {
    use super::ToolContext;

    struct ContextCaptureTool;

    #[async_trait]
    impl super::BuiltInTool for ContextCaptureTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Err("should not be called directly".to_string())
        }
        async fn execute_with_context(
            &self,
            arguments: &serde_json::Value,
            ctx: &ToolContext,
        ) -> Result<String, String> {
            Ok(serde_json::json!({
                "ctx_owner": ctx.owner_id,
                "ctx_workspace": ctx.workspace_id,
                "args": arguments,
            })
            .to_string())
        }
    }

    let tool = ContextCaptureTool;
    let ctx = ToolContext {
        owner_id: Some("real-owner".to_string()),
        workspace_id: Some("ws-1".to_string()),
        ..Default::default()
    };
    let result = tool
        .execute_with_context(
            &serde_json::json!({"query": "test", "owner_id": "spoofed"}),
            &ctx,
        )
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["ctx_owner"], "real-owner");
    assert_eq!(parsed["ctx_workspace"], "ws-1");
}

// --- missing owner_id returns error ---

#[tokio::test]
async fn test_missing_owner_id_returns_error() {
    // A mock tool that mimics MemorySearchTool's execute_with_context:
    // requires ctx.owner_id and returns an error when it is None.
    struct OwnerRequiringTool;

    #[async_trait]
    impl super::BuiltInTool for OwnerRequiringTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Err("direct execute not supported".to_string())
        }

        async fn execute_with_context(
            &self,
            _arguments: &serde_json::Value,
            ctx: &super::ToolContext,
        ) -> Result<String, String> {
            let _owner_id = ctx.owner_id.as_deref().ok_or_else(|| {
                "Tool requires owner_id but none provided in execution context".to_string()
            })?;
            Ok("ok".to_string())
        }
    }

    let ctx = super::ToolContext {
        owner_id: None,
        ..Default::default()
    };
    let tool = OwnerRequiringTool;
    let result = tool
        .execute_with_context(&serde_json::json!({"query": "test"}), &ctx)
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("owner_id"),
        "Error should mention owner_id, got: {}",
        err
    );
}

// --- workspace tool requires task_id ---

#[tokio::test]
async fn test_workspace_read_requires_task_id() {
    // WorkspaceReadTool is private; test through the registry's execute_with_context.
    // When ctx.task_id is None the tool must return an error.
    use crate::tools::builtins::builtin_tools;

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();

    let registry = ToolRegistry::default();
    for tool in builtin_tools(Some(db), None, None, None, None) {
        registry.register(tool).unwrap();
    }

    let ctx = super::ToolContext {
        task_id: None,
        ..Default::default()
    };
    let result = registry
        .execute_with_context("workspace_read", &serde_json::json!({}), &ctx)
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("task") || err.contains("context"),
        "Error should mention missing task context, got: {}",
        err
    );
}

// --- tools_for_capabilities ---

#[test]
fn test_tools_for_capabilities_basic() {
    let registry = ToolRegistry::default();
    registry.register(make_tool_with_caps("file_read", vec!["file_read"])).unwrap();
    registry.register(make_tool_with_caps("web_search", vec!["web_access"])).unwrap();

    let result = registry.tools_for_capabilities(&["file_read".to_string()]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "file_read");
}

#[test]
fn test_tools_for_capabilities_multi_capability_tool() {
    let registry = ToolRegistry::default();
    registry.register(make_tool_with_caps("web_fetch", vec!["web_access"])).unwrap();

    let result = registry.tools_for_capabilities(&["web_access".to_string()]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "web_fetch");

    let result = registry.tools_for_capabilities(&["shell_execute".to_string()]);
    assert!(result.is_empty());
}

#[test]
fn test_tools_for_capabilities_empty_returns_empty() {
    let registry = ToolRegistry::default();
    registry.register(make_tool_with_caps("file_read", vec!["file_read"])).unwrap();

    let result = registry.tools_for_capabilities(&[]);
    assert!(result.is_empty());
}

#[test]
fn test_tools_for_capabilities_no_capability_tools_excluded() {
    let registry = ToolRegistry::default();
    registry.register(make_tool_with_caps("orphan_tool", vec![])).unwrap();

    let result = registry.tools_for_capabilities(&["file_read".to_string()]);
    assert!(result.is_empty());
}

#[test]
fn test_tools_for_capabilities_with_deny_basic() {
    let registry = ToolRegistry::default();
    registry.register(make_tool_with_caps("file_read", vec!["file_read"])).unwrap();
    registry.register(make_tool_with_caps("web_search", vec!["web_access"])).unwrap();
    registry.register(make_tool_with_caps("shell", vec!["shell_execute"])).unwrap();

    let result = registry.tools_for_capabilities_with_deny(
        &["file_read".to_string(), "web_access".to_string()],
        &["web_access".to_string()],
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "file_read");
}

#[test]
fn test_tools_for_capabilities_with_deny_excludes_any_denied() {
    let registry = ToolRegistry::default();
    registry.register(make_tool_with_caps("file_rw", vec!["file_read", "file_write"])).unwrap();
    registry.register(make_tool_with_caps("reader", vec!["file_read"])).unwrap();

    let result = registry.tools_for_capabilities_with_deny(
        &["file_read".to_string()],
        &["file_write".to_string()],
    );
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "reader");
}

// ===========================================================================
// Task 11: Concurrent access tests
// ===========================================================================

#[tokio::test]
async fn test_concurrent_register_and_execute() {
    let registry = Arc::new(ToolRegistry::default());
    let mut handles = Vec::new();

    // Spawn 10 tasks that register tools
    for i in 0..10 {
        let reg = registry.clone();
        handles.push(tokio::spawn(async move {
            let tool = RegisteredTool {
                definition: ToolDefinition {
                    name: format!("tool_{i}"),
                    description: format!("tool {i}"),
                    parameters: serde_json::json!({"type": "object", "properties": {}}),
                    strict: None,
                    input_examples: None,
                },
                backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
                    response: format!("ok_{i}"),
                })),
                provides_capabilities: vec![],
                exempt_from_timeout: false,
                annotations: None,
                version: "test-0.0.0".into(),
                author: "test".into(),
                created_at: chrono::Utc::now(),
            };
            let _ = reg.register(tool);
        }));
    }

    // Spawn 10 tasks that try to execute tools (may or may not exist yet)
    for i in 0..10 {
        let reg = registry.clone();
        handles.push(tokio::spawn(async move {
            let _ = reg.execute(&format!("tool_{i}"), &serde_json::json!({})).await;
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
    // Should not panic; exact count depends on race ordering
    assert!(registry.count() <= 10);
}

#[tokio::test]
async fn test_concurrent_register_and_remove() {
    let registry = Arc::new(ToolRegistry::default());

    // Pre-register 50 tools
    for i in 0..50 {
        registry
            .register(make_tool(&format!("conc_tool_{i}"), "ok"))
            .unwrap();
    }
    assert_eq!(registry.count(), 50);

    let mut handles = Vec::new();

    // Concurrently remove the first 25
    for i in 0..25 {
        let reg = registry.clone();
        handles.push(tokio::spawn(async move {
            reg.remove(&format!("conc_tool_{i}"));
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(registry.count(), 25);
}

// ===========================================================================
// ToolContext skill_stack tests
// ===========================================================================

#[test]
fn test_tool_context_default_empty_stack() {
    let ctx = ToolContext::default();
    assert!(ctx.skill_stack.is_empty());
}

#[test]
fn test_tool_context_push_skill() {
    let ctx = ToolContext {
        agent_id: Some("agent-1".into()),
        task_id: Some("task-1".into()),
        owner_id: Some("owner-1".into()),
        workspace_id: Some("ws-1".into()),
        ..Default::default()
    };
    let child = ctx.with_skill_pushed("skill-A");

    // Original unchanged
    assert!(ctx.skill_stack.is_empty());

    // Child has the pushed skill
    assert_eq!(child.skill_stack, vec!["skill-A".to_string()]);

    // All other fields preserved
    assert_eq!(child.agent_id, Some("agent-1".into()));
    assert_eq!(child.task_id, Some("task-1".into()));
    assert_eq!(child.owner_id, Some("owner-1".into()));
    assert_eq!(child.workspace_id, Some("ws-1".into()));
}

#[test]
fn test_tool_context_push_skill_chain() {
    let root = ToolContext::default();
    let level1 = root.with_skill_pushed("A");
    let level2 = level1.with_skill_pushed("B");
    let level3 = level2.with_skill_pushed("C");

    assert_eq!(level3.skill_stack, vec![
        "A".to_string(),
        "B".to_string(),
        "C".to_string(),
    ]);
}

// ===========================================================================
// Task 12: Plugin backend + validation edge case tests
// ===========================================================================

fn make_tool_with_name(name: &str) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: name.to_string(),
            description: "stub".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn test_plugin_backend_execution() {
    struct MockPluginExecutor;
    #[async_trait]
    impl openalpaca_api::plugin_traits::PluginToolExecutor for MockPluginExecutor {
        async fn execute(
            &self,
            tool_name: &str,
            _args: &serde_json::Value,
        ) -> Result<String, String> {
            Ok(format!("plugin: {tool_name}"))
        }
        fn plugin_id(&self) -> &str {
            "test"
        }
    }

    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: ToolDefinition {
                name: "test::echo".into(),
                description: "".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Plugin(Arc::new(MockPluginExecutor)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();

    let result = registry
        .execute("test::echo", &serde_json::json!({}))
        .await;
    assert_eq!(result.unwrap(), "plugin: test::echo");
}

#[test]
fn test_tool_name_validation() {
    let registry = ToolRegistry::default();
    // Empty name
    assert!(registry.register(make_tool_with_name("")).is_err());
    // Null byte
    assert!(registry.register(make_tool_with_name("bad\0name")).is_err());
    // Too long (300 chars exceeds 256 limit)
    assert!(registry.register(make_tool_with_name(&"x".repeat(300))).is_err());
    // Valid name
    assert!(registry.register(make_tool_with_name("good_tool")).is_ok());
}

#[tokio::test]
async fn test_unknown_type_rejected_via_validation() {
    // json_value_matches_type is private, so we test it indirectly through
    // validate_tool_arguments by using a schema with a typo in the type field.
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: ToolDefinition {
                name: "typo_schema".to_string(),
                description: "Tool with typo type".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "strign"}
                    }
                }),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
                response: "ok".to_string(),
            })),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();

    // "hello" is a valid string, but schema says type is "strign" (typo)
    // json_value_matches_type returns false for unknown types -> validation fails
    let result = registry
        .execute("typo_schema", &serde_json::json!({"name": "hello"}))
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("should be strign"),
        "Expected type mismatch error for typo type, got: {}",
        err
    );
}

#[tokio::test]
async fn test_enum_validation_valid_value() {
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: ToolDefinition {
                name: "color_picker".to_string(),
                description: "Pick a color".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "color": {"type": "string", "enum": ["red", "blue"]}
                    },
                    "required": ["color"]
                }),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
                response: "ok".to_string(),
            })),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();

    // Valid enum value
    let result = registry
        .execute("color_picker", &serde_json::json!({"color": "red"}))
        .await;
    assert!(result.is_ok(), "Valid enum value should pass, got: {:?}", result);
}

#[tokio::test]
async fn test_enum_validation_invalid_value() {
    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: ToolDefinition {
                name: "color_picker2".to_string(),
                description: "Pick a color".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "color": {"type": "string", "enum": ["red", "blue"]}
                    },
                    "required": ["color"]
                }),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
                response: "ok".to_string(),
            })),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();

    // Invalid enum value
    let result = registry
        .execute("color_picker2", &serde_json::json!({"color": "green"}))
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("not in allowed values"),
        "Expected enum validation error, got: {}",
        err
    );
}

// ===========================================================================
// P3: PermissionTier derivation tests
// ===========================================================================

#[test]
fn permission_tier_destructive_returns_admin() {
    let ann = openalpaca_mcp::ToolAnnotations {
        destructive_hint: Some(true),
        ..Default::default()
    };
    assert_eq!(permission_tier(Some(&ann)), PermissionTier::Admin);
}

#[test]
fn permission_tier_readonly_returns_readonly() {
    let ann = openalpaca_mcp::ToolAnnotations {
        read_only_hint: Some(true),
        destructive_hint: Some(false),
        ..Default::default()
    };
    assert_eq!(permission_tier(Some(&ann)), PermissionTier::ReadOnly);
}

#[test]
fn permission_tier_neither_returns_readwrite() {
    let ann = openalpaca_mcp::ToolAnnotations {
        destructive_hint: Some(false),
        read_only_hint: Some(false),
        ..Default::default()
    };
    assert_eq!(permission_tier(Some(&ann)), PermissionTier::ReadWrite);
}

#[test]
fn permission_tier_none_annotations_returns_readwrite() {
    assert_eq!(permission_tier(None), PermissionTier::ReadWrite);
}

#[test]
fn permission_tier_destructive_takes_precedence_over_readonly() {
    let ann = openalpaca_mcp::ToolAnnotations {
        destructive_hint: Some(true),
        read_only_hint: Some(true),
        ..Default::default()
    };
    assert_eq!(permission_tier(Some(&ann)), PermissionTier::Admin);
}

#[test]
fn iter_registered_tools_snapshots_state() {
    let registry = ToolRegistry::new().unwrap();
    let mut destructive = make_tool_with_caps("tool_a", vec![]);
    destructive.annotations = Some(openalpaca_mcp::ToolAnnotations {
        destructive_hint: Some(true),
        ..Default::default()
    });
    let plain = make_tool_with_caps("tool_b", vec![]);

    registry.register(destructive).unwrap();
    registry.register(plain).unwrap();

    let mut collected: Vec<_> = registry.iter_registered_tools().collect();
    collected.sort_by(|(a, _), (b, _)| a.cmp(b));

    assert_eq!(collected.len(), 2);
    assert_eq!(collected[0].0, "tool_a");
    assert_eq!(collected[1].0, "tool_b");

    // Destructive annotation preserved.
    let a_destructive = collected[0]
        .1
        .annotations
        .as_ref()
        .and_then(|a| a.destructive_hint)
        .unwrap_or(false);
    assert!(a_destructive);

    // None annotation preserved.
    assert!(collected[1].1.annotations.is_none());
}

#[test]
fn known_virtual_capabilities_default_includes_all_8() {
    let registry = ToolRegistry::new().unwrap();
    let known = registry.known_virtual_capabilities();
    assert_eq!(known.len(), 8);
    for name in ANNOTATION_CAPABILITY_NAMES {
        assert!(known.iter().any(|k| k == name), "{name} should be in known list");
    }
}

#[test]
fn register_capability_provider_extends_known_names() {
    use std::sync::Arc;
    use super::capabilities::CapabilityProvider;

    struct CustomProvider;
    impl CapabilityProvider for CustomProvider {
        fn derive_capabilities(&self, _: &RegisteredTool) -> Vec<String> {
            vec!["annotation:custom".into()]
        }
        fn known_capability_names(&self) -> Vec<String> {
            vec!["annotation:custom".to_string()]
        }
    }

    let registry = ToolRegistry::new().unwrap();
    registry.register_capability_provider(Arc::new(CustomProvider));
    let known = registry.known_virtual_capabilities();
    assert!(known.iter().any(|k| k == "annotation:custom"));
    assert_eq!(known.len(), 9);
}

#[test]
fn custom_provider_produces_virtual_caps_on_registered_tools() {
    use std::sync::Arc;
    use super::capabilities::CapabilityProvider;

    struct CustomProvider;
    impl CapabilityProvider for CustomProvider {
        fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String> {
            if tool.definition.name == "custom_target" {
                vec!["annotation:custom".into()]
            } else {
                vec![]
            }
        }
        fn known_capability_names(&self) -> Vec<String> {
            vec!["annotation:custom".to_string()]
        }
    }

    let registry = ToolRegistry::new().unwrap();
    registry.register_capability_provider(Arc::new(CustomProvider));

    let tool = make_tool_with_caps("custom_target", vec![]);
    registry.register(tool).unwrap();

    let tools = registry.tools_for_capabilities(&vec!["annotation:custom".to_string()]);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "custom_target");
}

// ===========================================================================
// Task 6: Integration tests for annotation resolution + deny semantics
// ===========================================================================

fn make_tool_with_annotations(
    name: &str,
    caps: Vec<&str>,
    annotations: Option<openalpaca_mcp::ToolAnnotations>,
) -> RegisteredTool {
    let mut tool = make_tool_with_caps(name, caps);
    tool.annotations = annotations;
    tool
}

fn readonly_annotations() -> openalpaca_mcp::ToolAnnotations {
    openalpaca_mcp::ToolAnnotations {
        read_only_hint: Some(true),
        destructive_hint: Some(false),
        idempotent_hint: Some(true),
        open_world_hint: Some(false),
        ..Default::default()
    }
}

fn destructive_annotations() -> openalpaca_mcp::ToolAnnotations {
    openalpaca_mcp::ToolAnnotations {
        read_only_hint: Some(false),
        destructive_hint: Some(true),
        idempotent_hint: Some(false),
        open_world_hint: Some(true),
        ..Default::default()
    }
}

#[test]
fn tools_for_capabilities_resolves_annotation_readonly() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations("readonly_a", vec![], Some(readonly_annotations()))).unwrap();
    registry.register(make_tool_with_annotations("readonly_b", vec![], Some(readonly_annotations()))).unwrap();
    registry.register(make_tool_with_annotations("destructive_a", vec![], Some(destructive_annotations()))).unwrap();

    let tools = registry.tools_for_capabilities(&vec!["annotation:readonly".to_string()]);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tools.len(), 2);
    assert!(names.contains(&"readonly_a"));
    assert!(names.contains(&"readonly_b"));
    assert!(!names.contains(&"destructive_a"));
}

#[test]
fn tools_for_capabilities_mixed_string_and_annotation() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations(
        "file_reader",
        vec!["file_read"],
        Some(readonly_annotations()),
    )).unwrap();
    registry.register(make_tool_with_annotations("other_reader", vec![], Some(readonly_annotations()))).unwrap();

    let tools = registry.tools_for_capabilities(&vec![
        "file_read".to_string(),
        "annotation:readonly".to_string(),
    ]);
    assert_eq!(tools.len(), 2);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"file_reader"));
    assert!(names.contains(&"other_reader"));
}

#[test]
fn tools_for_capabilities_with_deny_annotation_destructive() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations("readonly_a", vec![], Some(readonly_annotations()))).unwrap();
    registry.register(make_tool_with_annotations("destructive_a", vec![], Some(destructive_annotations()))).unwrap();

    let tools = registry.tools_for_capabilities_with_deny(
        &vec!["annotation:readonly".to_string(), "annotation:destructive".to_string()],
        &vec!["annotation:destructive".to_string()],
    );
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tools.len(), 1);
    assert!(names.contains(&"readonly_a"));
}

#[test]
fn deny_string_cap_excludes_annotation_matched_tool() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations(
        "file_reader",
        vec!["file_read"],
        Some(readonly_annotations()),
    )).unwrap();

    let tools = registry.tools_for_capabilities_with_deny(
        &vec!["annotation:readonly".to_string()],
        &vec!["file_read".to_string()],
    );
    assert_eq!(tools.len(), 0);
}

#[test]
fn deny_annotation_cap_excludes_string_matched_tool() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations(
        "web_fetcher",
        vec!["web_access"],
        Some(destructive_annotations()),
    )).unwrap();

    let tools = registry.tools_for_capabilities_with_deny(
        &vec!["web_access".to_string()],
        &vec!["annotation:open_world".to_string()],
    );
    assert_eq!(tools.len(), 0);
}

#[test]
fn non_destructive_inverse_matches_only_explicit_false() {
    let registry = ToolRegistry::new().unwrap();
    let ann_true = openalpaca_mcp::ToolAnnotations {
        destructive_hint: Some(true),
        ..Default::default()
    };
    let ann_false = openalpaca_mcp::ToolAnnotations {
        destructive_hint: Some(false),
        ..Default::default()
    };
    registry.register(make_tool_with_annotations("tool_a", vec![], Some(ann_false))).unwrap();
    registry.register(make_tool_with_annotations("tool_b", vec![], None)).unwrap();
    registry.register(make_tool_with_annotations("tool_c", vec![], Some(ann_true))).unwrap();

    let tools = registry.tools_for_capabilities(&vec!["annotation:non_destructive".to_string()]);
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tools.len(), 1);
    assert!(names.contains(&"tool_a"));
    assert!(!names.contains(&"tool_b"));
    assert!(!names.contains(&"tool_c"));
}

#[test]
fn register_populates_virtual_caps_in_index() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations("destructive_tool", vec![], Some(destructive_annotations()))).unwrap();

    let tools = registry.tools_for_capabilities(&vec!["annotation:destructive".to_string()]);
    assert_eq!(tools.len(), 1);
}

#[test]
fn remove_scrubs_virtual_caps_from_index() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations("destructive_tool", vec![], Some(destructive_annotations()))).unwrap();

    let before = registry.tools_for_capabilities(&vec!["annotation:destructive".to_string()]);
    assert_eq!(before.len(), 1);

    registry.remove("destructive_tool");

    let after = registry.tools_for_capabilities(&vec!["annotation:destructive".to_string()]);
    assert!(after.is_empty());
}

#[test]
fn register_with_no_annotations_skips_virtual_caps() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_annotations("plain_tool", vec!["some_cap"], None)).unwrap();

    let via_string = registry.tools_for_capabilities(&vec!["some_cap".to_string()]);
    assert_eq!(via_string.len(), 1);

    for name in ANNOTATION_CAPABILITY_NAMES {
        let via_ann = registry.tools_for_capabilities(&vec![name.to_string()]);
        assert!(via_ann.is_empty(), "{name} should not contain plain_tool");
    }
}

// ===========================================================================
// Task 4 (MCP P3d): Runtime provider lifecycle tests
// ===========================================================================

// Mock provider for lifecycle tests.
#[allow(dead_code)]
struct P3dMockProvider {
    capability_name: &'static str,
    target_tool_prefix: &'static str,
    known_names: &'static [&'static str],
}

impl super::capabilities::CapabilityProvider for P3dMockProvider {
    fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String> {
        if tool.definition.name.starts_with(self.target_tool_prefix) {
            vec![self.capability_name.to_string()]
        } else {
            vec![]
        }
    }
    fn known_capability_names(&self) -> Vec<String> {
        self.known_names.iter().map(|s| s.to_string()).collect()
    }
}

#[test]
fn register_capability_provider_returns_valid_handle() {
    let registry = ToolRegistry::new().unwrap();
    let before = registry.provider_handles().len();
    let handle = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_a",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_a"],
    }));
    let after = registry.provider_handles();
    assert_eq!(after.len(), before + 1);
    assert!(after.contains(&handle));
}

#[test]
fn register_provider_retroactively_indexes_existing_tools() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_caps("a_target", vec![])).unwrap();

    // No provider yet for annotation:custom_a.
    let before = registry.tools_for_capabilities(&vec!["annotation:custom_a".to_string()]);
    assert!(before.is_empty());

    // Register a provider AFTER the tool exists.
    let _handle = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_a",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_a"],
    }));

    // Retroactive rebuild — tool is now indexed.
    let after = registry.tools_for_capabilities(&vec!["annotation:custom_a".to_string()]);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name, "a_target");
}

#[test]
fn remove_capability_provider_unknown_handle_returns_false() {
    let registry = ToolRegistry::new().unwrap();
    assert!(!registry.remove_capability_provider(ProviderHandle(9999)));
}

#[test]
fn remove_capability_provider_known_handle_returns_true() {
    let registry = ToolRegistry::new().unwrap();
    let handle = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_a",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_a"],
    }));
    assert!(registry.remove_capability_provider(handle));
    assert!(!registry.provider_handles().contains(&handle));
}

#[test]
fn remove_provider_scrubs_its_only_virtual_caps() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_caps("a_target", vec![])).unwrap();
    let handle = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_a",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_a"],
    }));

    let before = registry.tools_for_capabilities(&vec!["annotation:custom_a".to_string()]);
    assert_eq!(before.len(), 1);

    registry.remove_capability_provider(handle);

    let after = registry.tools_for_capabilities(&vec!["annotation:custom_a".to_string()]);
    assert!(after.is_empty());
}

#[test]
fn remove_provider_preserves_caps_from_other_providers() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_caps("a_target", vec![])).unwrap();

    let h1 = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_shared",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_shared"],
    }));
    let h2 = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_shared",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_shared"],
    }));

    // Both providers emit the same cap for the same tool.
    let before = registry.tools_for_capabilities(&vec!["annotation:custom_shared".to_string()]);
    assert_eq!(before.len(), 1);

    registry.remove_capability_provider(h1);

    // Still resolves via remaining provider.
    let after = registry.tools_for_capabilities(&vec!["annotation:custom_shared".to_string()]);
    assert_eq!(after.len(), 1, "cap should still resolve via remaining provider");

    registry.remove_capability_provider(h2);

    let final_ = registry.tools_for_capabilities(&vec!["annotation:custom_shared".to_string()]);
    assert!(final_.is_empty());
}

#[test]
fn remove_default_annotation_provider_empties_known_list() {
    let registry = ToolRegistry::new().unwrap();
    let initial_handles = registry.provider_handles();
    assert_eq!(initial_handles.len(), 1, "default provider should be registered");

    let default_handle = initial_handles[0];
    let removed = registry.remove_capability_provider(default_handle);
    assert!(removed);

    assert!(registry.known_virtual_capabilities().is_empty());
}

#[test]
fn register_then_remove_then_register_cycle() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_caps("a_target", vec![])).unwrap();

    // Cycle 1: register, verify, remove.
    let h1 = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_a",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_a"],
    }));
    assert_eq!(
        registry.tools_for_capabilities(&vec!["annotation:custom_a".to_string()]).len(),
        1
    );
    registry.remove_capability_provider(h1);
    assert!(
        registry.tools_for_capabilities(&vec!["annotation:custom_a".to_string()]).is_empty()
    );

    // Cycle 2: register again (different handle), verify.
    let h2 = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_a",
        target_tool_prefix: "a_",
        known_names: &["annotation:custom_a"],
    }));
    assert_ne!(h1, h2, "new registration should issue fresh handle");
    assert_eq!(
        registry.tools_for_capabilities(&vec!["annotation:custom_a".to_string()]).len(),
        1
    );
}

#[test]
fn rebuild_preserves_string_capabilities() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_caps("tool_with_string_cap", vec!["file_read"])).unwrap();

    let before = registry.tools_for_capabilities(&vec!["file_read".to_string()]);
    assert_eq!(before.len(), 1);

    // Register then remove a dummy provider (triggers rebuild).
    let handle = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:dummy",
        target_tool_prefix: "nomatch_",
        known_names: &["annotation:dummy"],
    }));
    registry.remove_capability_provider(handle);

    let after = registry.tools_for_capabilities(&vec!["file_read".to_string()]);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].name, "tool_with_string_cap");
}

#[test]
fn concurrent_registers_return_distinct_handles() {
    use std::sync::Arc as StdArc;
    use std::thread;

    let registry = StdArc::new(ToolRegistry::new().unwrap());
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let reg = StdArc::clone(&registry);
            thread::spawn(move || {
                reg.register_capability_provider(Arc::new(P3dMockProvider {
                    capability_name: "annotation:concurrent",
                    target_tool_prefix: "c_",
                    known_names: &["annotation:concurrent"],
                }))
            })
        })
        .collect();

    let collected: Vec<ProviderHandle> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // All 10 handles should be distinct.
    use std::collections::HashSet;
    let as_set: HashSet<ProviderHandle> = collected.iter().copied().collect();
    assert_eq!(as_set.len(), 10);
}

#[test]
fn known_virtual_capabilities_reflects_current_providers() {
    let registry = ToolRegistry::new().unwrap();
    // Default provider contributes 8 names.
    assert_eq!(registry.known_virtual_capabilities().len(), 8);

    let handle = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:custom_x",
        target_tool_prefix: "x_",
        known_names: &["annotation:custom_x"],
    }));
    assert_eq!(registry.known_virtual_capabilities().len(), 9);

    registry.remove_capability_provider(handle);
    assert_eq!(registry.known_virtual_capabilities().len(), 8);
}

// ===========================================================================
// Task 4 (MCP P3d): Integration tests for provider lifecycle
// ===========================================================================

#[test]
fn plugin_hot_reload_scenario() {
    let registry = ToolRegistry::new().unwrap();
    registry.register(make_tool_with_caps("plugin_helper", vec![])).unwrap();

    // Phase 1: Register a "plugin provider."
    let handle = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:my_plugin",
        target_tool_prefix: "plugin_",
        known_names: &["annotation:my_plugin"],
    }));

    // Tool is under annotation:my_plugin.
    let phase1 = registry.tools_for_capabilities(&vec!["annotation:my_plugin".to_string()]);
    assert_eq!(phase1.len(), 1);

    // Phase 2: Unload plugin → remove provider.
    registry.remove_capability_provider(handle);

    // Tool no longer under annotation:my_plugin.
    let phase2 = registry.tools_for_capabilities(&vec!["annotation:my_plugin".to_string()]);
    assert!(phase2.is_empty());

    // Phase 3: Reload plugin → register provider again.
    let _handle2 = registry.register_capability_provider(Arc::new(P3dMockProvider {
        capability_name: "annotation:my_plugin",
        target_tool_prefix: "plugin_",
        known_names: &["annotation:my_plugin"],
    }));

    // Tool is back.
    let phase3 = registry.tools_for_capabilities(&vec!["annotation:my_plugin".to_string()]);
    assert_eq!(phase3.len(), 1);
}

#[test]
fn concurrent_tool_register_during_provider_rebuild() {
    use std::sync::Arc as StdArc;
    use std::thread;

    let registry = StdArc::new(ToolRegistry::new().unwrap());

    // Thread A registers a provider (triggers rebuild).
    let reg_a = StdArc::clone(&registry);
    let thread_a = thread::spawn(move || {
        reg_a.register_capability_provider(Arc::new(P3dMockProvider {
            capability_name: "annotation:concurrent_test",
            target_tool_prefix: "ct_",
            known_names: &["annotation:concurrent_test"],
        }))
    });

    // Thread B registers a tool concurrently.
    let reg_b = StdArc::clone(&registry);
    let thread_b = thread::spawn(move || {
        reg_b.register(make_tool_with_caps("ct_target", vec![])).unwrap();
    });

    let _handle = thread_a.join().unwrap();
    thread_b.join().unwrap();

    // After both complete, the new tool's virtual caps should be indexed.
    let result = registry.tools_for_capabilities(&vec!["annotation:concurrent_test".to_string()]);
    assert_eq!(result.len(), 1, "tool registered concurrently should be indexed");
    assert_eq!(result[0].name, "ct_target");
}

#[test]
fn lookup_during_rebuild_eventually_sees_full_result() {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    let registry = StdArc::new(ToolRegistry::new().unwrap());
    registry.register(make_tool_with_caps("lookup_target", vec![])).unwrap();

    let stop = StdArc::new(AtomicBool::new(false));
    let stop_clone = StdArc::clone(&stop);

    // Mutation thread: register/remove in a loop.
    let reg_m = StdArc::clone(&registry);
    let mutator = thread::spawn(move || {
        for _ in 0..20 {
            let h = reg_m.register_capability_provider(Arc::new(P3dMockProvider {
                capability_name: "annotation:flicker",
                target_tool_prefix: "lookup_",
                known_names: &["annotation:flicker"],
            }));
            thread::sleep(Duration::from_micros(100));
            reg_m.remove_capability_provider(h);
            thread::sleep(Duration::from_micros(100));
        }
        stop_clone.store(true, Ordering::Relaxed);
    });

    // Polling thread: lookup in a tight loop.
    let reg_p = StdArc::clone(&registry);
    let poller = thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let _ = reg_p.tools_for_capabilities(&vec!["annotation:flicker".to_string()]);
        }
    });

    mutator.join().unwrap();
    poller.join().unwrap();

    // After mutation ends, last remove leaves empty state.
    let final_ = registry.tools_for_capabilities(&vec!["annotation:flicker".to_string()]);
    assert!(final_.is_empty(), "after final remove, lookup must be empty");
}

// ── extension_tool_defs (tool/skill wiring, Chunk 2) ─────────────────────

struct ExtMockPluginExec;

#[async_trait]
impl openalpaca_api::plugin_traits::PluginToolExecutor for ExtMockPluginExec {
    async fn execute(
        &self,
        _tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<String, String> {
        Ok("plugin ok".to_string())
    }

    fn plugin_id(&self) -> &str {
        "plug"
    }
}

fn make_tool_with_backend(name: &str, backend: ToolBackend) -> RegisteredTool {
    let mut tool = make_tool(name, "ok");
    tool.backend = backend;
    tool
}

fn mcp_backend() -> ToolBackend {
    ToolBackend::Mcp {
        client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests("srv")),
        remote_name: "echo".to_string(),
        server_name: "srv".to_string(),
        generation: 0,
    }
}

#[test]
fn test_extension_tool_defs_filters_by_origin() {
    let registry = ToolRegistry::default();
    registry.register(make_tool("builtin_tool", "ok")).unwrap();
    registry
        .register(make_tool_with_backend("srv__echo", mcp_backend()))
        .unwrap();
    registry
        .register(make_tool_with_backend(
            "plug::do",
            ToolBackend::Plugin(Arc::new(ExtMockPluginExec)),
        ))
        .unwrap();
    registry
        .register(make_tool_with_backend("srv__blocked", mcp_backend()))
        .unwrap();
    registry
        .register(make_tool_with_backend(
            "custom_http",
            ToolBackend::Http {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                headers: HashMap::new(),
                timeout_secs: 5,
            },
        ))
        .unwrap();

    let defs = registry.extension_tool_defs();
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    // Only MCP/Plugin backends, sorted by name; builtins/custom excluded.
    assert_eq!(names, vec!["plug::do", "srv__blocked", "srv__echo"]);
}

// ═════════════════════════════════════════════════════════════════════════
// THE EXTENSION GATE (extension design §6.2 #1, §6.2a, §7.1)
// ═════════════════════════════════════════════════════════════════════════

use crate::tools::extensions::{
    Audience, ExtensionId, ExtensionLedger, ExtensionState, FailureReason, WithdrawalCause,
};

/// A plugin executor that reports which load it belongs to and, on call, how
/// many in-flight guards the ledger is holding — which is how "the gate ran
/// exactly once" is observed from below the gate.
struct GenPluginExec {
    generation: u64,
    ledger: Option<Arc<ExtensionLedger>>,
    ext: ExtensionId,
}

#[async_trait]
impl openalpaca_api::plugin_traits::PluginToolExecutor for GenPluginExec {
    async fn execute(
        &self,
        _tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<String, String> {
        match &self.ledger {
            Some(ledger) => Ok(format!("in_flight={}", ledger.in_flight(&self.ext))),
            None => Ok("plugin ok".to_string()),
        }
    }

    fn plugin_id(&self) -> &str {
        "plug"
    }

    fn generation(&self) -> u64 {
        self.generation
    }
}

fn mcp_tool(name: &str, server: &str, generation: u64) -> RegisteredTool {
    let mut tool = make_tool(name, "ok");
    tool.backend = ToolBackend::Mcp {
        client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests(server)),
        remote_name: "echo".to_string(),
        server_name: server.to_string(),
        generation,
    };
    tool.author = format!("mcp:{server}");
    tool
}

fn plugin_tool(name: &str, plugin: &str, executor: Arc<GenPluginExec>) -> RegisteredTool {
    let mut tool = make_tool(name, "ok");
    tool.backend = ToolBackend::Plugin(executor);
    tool.author = format!("plugin:{plugin}");
    tool
}

/// The disable half of a supervisor's T0–T5, run against the ledger alone.
fn ledger_disable(ledger: &ExtensionLedger, ext: &ExtensionId) {
    ledger.begin(ext, ExtensionState::Disabling, Some(WithdrawalCause::Disable));
    ledger.commit(ext, ExtensionState::Disabled);
}

fn enabled_record(ledger: &ExtensionLedger, ext: &ExtensionId, tools: &[&str]) -> u64 {
    let generation = match ledger.begin(ext, ExtensionState::Enabling, None) {
        crate::tools::extensions::Transition::Took(g) => g,
        other => panic!("E0 refused: {other:?}"),
    };
    ledger.restore(ext);
    ledger.record_tools(ext, tools.iter().map(|s| s.to_string()));
    assert!(ledger.commit(ext, ExtensionState::Enabled));
    generation
}

// ── (i) The snapshot arm ─────────────────────────────────────────────────

#[tokio::test]
async fn snapshot_refuses_after_a_ledger_disable_with_the_s4_string() {
    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::mcp("github");
    enabled_record(registry.extensions(), &ext, &["github__create_issue"]);
    registry
        .register(mcp_tool("github__create_issue", "github", 1))
        .unwrap();

    // A lead agent takes a deep snapshot and holds it for the whole run.
    let snapshot = (*registry).clone();
    assert!(snapshot.get("github__create_issue").is_some());

    ledger_disable(registry.extensions(), &ext);

    let err = snapshot
        .execute_with_context(
            "github__create_issue",
            &serde_json::json!({}),
            &ToolContext::default(),
        )
        .await
        .expect_err("a disabled extension must be refused in every snapshot");

    assert!(
        err.starts_with("tool 'github__create_issue' is unavailable: "),
        "{err}"
    );
    assert!(err.contains("MCP server 'github' is disabled by the owner"), "{err}");
    assert!(err.contains("Settings → Extensions"), "{err}");
    assert!(!err.contains("not found"), "{err}");
    assert!(!err.contains("transport"), "{err}");
}

/// The §6.3 guard rail, carried from C1's review: the snapshot test above
/// exercises `execute_with_context`, but `execute()` — the no-context path —
/// is the *same* gate through the same `dispatch`, and a refactor that
/// re-split them would be invisible without this.
#[tokio::test]
async fn the_no_context_execute_path_refuses_a_disabled_extension_too() {
    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::mcp("github");
    enabled_record(registry.extensions(), &ext, &["github__create_issue"]);
    registry
        .register(mcp_tool("github__create_issue", "github", 1))
        .unwrap();

    let snapshot = (*registry).clone();
    ledger_disable(registry.extensions(), &ext);

    let err = snapshot
        .execute("github__create_issue", &serde_json::json!({}))
        .await
        .expect_err("execute() is gated exactly like execute_with_context()");
    assert!(err.contains("MCP server 'github' is disabled by the owner"), "{err}");
    assert!(!err.contains("Unknown tool"), "{err}");
}

// ── (ii) The miss arm ────────────────────────────────────────────────────

#[tokio::test]
async fn live_registry_miss_on_withdrawn_tool_refuses_with_attribution() {
    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::mcp("github");
    enabled_record(registry.extensions(), &ext, &["github__create_issue"]);
    registry
        .register(mcp_tool("github__create_issue", "github", 1))
        .unwrap();

    // T0 then T1: the ordinary skill holds the LIVE registry and sees the
    // entry vanish, which without the miss arm is an unattributed "not found".
    ledger_disable(registry.extensions(), &ext);
    assert!(registry.remove("github__create_issue"));

    let ctx = ToolContext {
        task_id: Some("task-1".into()),
        ..Default::default()
    };
    let err = registry
        .execute_with_context("github__create_issue", &serde_json::json!({}), &ctx)
        .await
        .expect_err("the miss arm must refuse, not fall through");

    assert!(
        err.contains("MCP server 'github' is disabled by the owner"),
        "{err}"
    );
    assert!(!err.contains("not found in registry"), "{err}");

    // Exactly one announcement, however many times the model retries — while
    // every attempt still fails (design §7.1/§7.4). The dedup set is C1's
    // observable; the event variant lands in C4.
    for _ in 0..9 {
        assert!(
            registry
                .execute_with_context("github__create_issue", &serde_json::json!({}), &ctx)
                .await
                .is_err()
        );
    }
    assert_eq!(registry.extensions().warned_count(), 1);
}

// ── (iii) The generation compare ─────────────────────────────────────────

#[tokio::test]
async fn stale_snapshot_after_reenable_refuses_and_live_stays_enabled() {
    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::plugin("notion");
    let gen1 = enabled_record(registry.extensions(), &ext, &["notion::write"]);
    assert_eq!(gen1, 1);
    registry
        .register(plugin_tool(
            "notion::write",
            "notion",
            Arc::new(GenPluginExec {
                generation: 1,
                ledger: None,
                ext: ext.clone(),
            }),
        ))
        .unwrap();

    // A run in flight holds load 1's proxy.
    let snapshot = (*registry).clone();

    // Disable, then re-enable: E0 bumps the generation and E4 replaces the
    // registry entry with a handle stamped for load 2.
    ledger_disable(registry.extensions(), &ext);
    assert!(registry.remove("notion::write"));
    let gen2 = match registry
        .extensions()
        .begin(&ext, ExtensionState::Enabling, None)
    {
        crate::tools::extensions::Transition::Took(g) => g,
        other => panic!("E0 refused: {other:?}"),
    };
    assert_eq!(gen2, 2, "every load exceeds the last");
    registry
        .replace(plugin_tool(
            "notion::write",
            "notion",
            Arc::new(GenPluginExec {
                generation: gen2,
                ledger: None,
                ext: ext.clone(),
            }),
        ))
        .unwrap();
    registry.extensions().restore(&ext);
    registry.extensions().record_tools(&ext, ["notion::write"]);
    assert!(
        registry
            .extensions()
            .commit(&ext, ExtensionState::Enabled)
    );

    // The stale snapshot is refused BEFORE it can reach the dead channel.
    let err = snapshot
        .execute_with_context(
            "notion::write",
            &serde_json::json!({}),
            &ToolContext::default(),
        )
        .await
        .expect_err("a handle from a previous load must be refused");
    assert!(err.contains("belongs to a previous load of 'notion'"), "{err}");
    assert!(err.contains("available again on your next request"), "{err}");
    assert_eq!(registry.extensions().warned_count(), 1);

    // …while the live registry serves the new load.
    let ok = registry
        .execute_with_context(
            "notion::write",
            &serde_json::json!({}),
            &ToolContext::default(),
        )
        .await
        .expect("the current load still works");
    assert_eq!(ok, "plugin ok");

    // And the stale proxy's own crash report cannot flip the healthy row.
    assert!(!registry.extensions().mark_failed(&ext, gen1, FailureReason::Crashed, "channel closed"));
    assert_eq!(
        registry.extensions().state(&ext),
        Some(ExtensionState::Enabled)
    );
}

// ── The gate runs exactly once per call ──────────────────────────────────

#[tokio::test]
async fn the_gate_is_taken_exactly_once_for_a_plugin_backend() {
    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::plugin("plug");
    enabled_record(registry.extensions(), &ext, &["plug::do"]);
    registry
        .register(plugin_tool(
            "plug::do",
            "plug",
            Arc::new(GenPluginExec {
                generation: 1,
                ledger: Some(Arc::clone(registry.extensions())),
                ext: ext.clone(),
            }),
        ))
        .unwrap();

    // `execute_with_context`'s Plugin arm used to delegate to `execute`, which
    // would double-take the guard and double-count T3's drain.
    let out = registry
        .execute_with_context("plug::do", &serde_json::json!({}), &ToolContext::default())
        .await
        .unwrap();
    assert_eq!(out, "in_flight=1", "the guard must be taken exactly once");

    let out = registry
        .execute("plug::do", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(out, "in_flight=1");

    // The guard is released when the call finishes.
    assert_eq!(registry.extensions().in_flight(&ext), 0);
}

#[tokio::test]
async fn builtins_are_never_gated() {
    let registry = Arc::new(ToolRegistry::default());
    registry.register(make_tool("builtin_tool", "ok")).unwrap();
    // A ledger record for a *different* extension changes nothing.
    registry
        .extensions()
        .upsert(&ExtensionId::mcp("github"), false, ExtensionState::Disabled);

    assert_eq!(
        registry
            .execute_with_context("builtin_tool", &serde_json::json!({}), &ToolContext::default())
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        registry.execute("builtin_tool", &serde_json::json!({})).await.unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn an_unknown_name_with_no_ledger_owner_still_gets_the_plain_not_found_error() {
    let registry = Arc::new(ToolRegistry::default());
    // An owner that IS enabled must also fall through, not refuse.
    let ext = ExtensionId::mcp("github");
    enabled_record(registry.extensions(), &ext, &["github__create_issue"]);

    let err = registry
        .execute_with_context("nope", &serde_json::json!({}), &ToolContext::default())
        .await
        .unwrap_err();
    assert_eq!(err, "Tool 'nope' not found in registry");

    let err = registry.execute("nope", &serde_json::json!({})).await.unwrap_err();
    assert_eq!(err, "Unknown tool: 'nope'");

    // A name an ENABLED extension retains but the registry no longer holds is
    // also a fall-through — it is not withheld by anything.
    let err = registry
        .execute_with_context(
            "github__create_issue",
            &serde_json::json!({}),
            &ToolContext::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "Tool 'github__create_issue' not found in registry");
}

// ── §6.2a — fail-open, audited ───────────────────────────────────────────

#[tokio::test]
async fn unrecorded_extension_tool_executes() {
    let registry = Arc::new(ToolRegistry::default());
    // Exactly what `services/mcp.rs` and `manager.rs` do today: register
    // straight through, with no ledger record anywhere.
    registry.register(mcp_tool("srv__echo", "srv", 0)).unwrap();
    registry
        .register(plugin_tool(
            "plug::do",
            "plug",
            Arc::new(GenPluginExec {
                generation: 0,
                ledger: None,
                ext: ExtensionId::plugin("plug"),
            }),
        ))
        .unwrap();

    // Still listed on the default surfaces.
    let names: Vec<String> = registry
        .extension_tool_defs()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, vec!["plug::do".to_string(), "srv__echo".to_string()]);

    // Still executes: the plugin call returns its own result…
    assert_eq!(
        registry
            .execute_with_context("plug::do", &serde_json::json!({}), &ToolContext::default())
            .await
            .unwrap(),
        "plugin ok"
    );
    // …and the MCP call reaches its backend (the disconnected test client's own
    // transport error), rather than being stopped at the gate.
    let err = registry
        .execute_with_context("srv__echo", &serde_json::json!({}), &ToolContext::default())
        .await
        .unwrap_err();
    assert!(err.starts_with("MCP server 'srv' tool 'echo' failed:"), "{err}");

    // And the audit names both, so the fail-open path cannot hide.
    let audit = registry.extensions().audit(&registry);
    assert_eq!(audit.len(), 2, "{audit:?}");
    assert!(audit.iter().any(|line| line.contains("mcp:srv")), "{audit:?}");
    assert!(audit.iter().any(|line| line.contains("plugin:plug")), "{audit:?}");
}

// ── Registry hygiene (§6.2 #4, §3.2 T3, §3.3 E4) ─────────────────────────

#[test]
fn extension_tools_never_timeout_exempt() {
    let registry = ToolRegistry::default();
    let mut tool = mcp_tool("srv__echo", "srv", 0);
    tool.exempt_from_timeout = true;
    registry.register(tool).unwrap();
    assert!(!registry.is_exempt_from_timeout("srv__echo"));
    assert!(!registry.get("srv__echo").unwrap().exempt_from_timeout);

    // `replace` goes through the same guard.
    let mut tool = plugin_tool(
        "plug::do",
        "plug",
        Arc::new(GenPluginExec {
            generation: 0,
            ledger: None,
            ext: ExtensionId::plugin("plug"),
        }),
    );
    tool.exempt_from_timeout = true;
    registry.replace(tool).unwrap();
    assert!(!registry.is_exempt_from_timeout("plug::do"));

    // A coordination builtin keeps its exemption.
    let mut builtin = make_tool("wait_for_subagents", "ok");
    builtin.exempt_from_timeout = true;
    registry.register(builtin).unwrap();
    assert!(registry.is_exempt_from_timeout("wait_for_subagents"));
}

#[test]
fn remove_drops_the_capability_index_key_when_its_last_provider_leaves() {
    let registry = ToolRegistry::default();
    registry
        .register(make_tool_with_caps("a", vec!["shared_cap"]))
        .unwrap();
    registry
        .register(make_tool_with_caps("b", vec!["shared_cap"]))
        .unwrap();

    registry.remove("a");
    assert!(
        registry.capability_index.contains_key("shared_cap"),
        "a surviving provider keeps the key"
    );
    registry.remove("b");
    assert!(
        !registry.capability_index.contains_key("shared_cap"),
        "no phantom capabilities: the key goes with its last provider"
    );
}

#[test]
fn enable_disable_enable_leaves_no_duplicate_index_edges() {
    let registry = ToolRegistry::default();
    for _ in 0..3 {
        registry
            .replace(make_tool_with_caps("srv__echo", vec!["srv__echo"]))
            .unwrap();
    }
    let names = registry.capability_index.get("srv__echo").unwrap();
    assert_eq!(
        names.value().len(),
        1,
        "register appends without dedupe; only replace's remove scrubs"
    );
}

#[test]
fn two_assemblies_against_an_unchanged_ledger_are_byte_identical() {
    let registry = ToolRegistry::default();
    registry.register(mcp_tool("srv__b", "srv", 0)).unwrap();
    registry.register(mcp_tool("srv__a", "srv", 0)).unwrap();
    registry
        .register(plugin_tool(
            "plug::do",
            "plug",
            Arc::new(GenPluginExec {
                generation: 0,
                ledger: None,
                ext: ExtensionId::plugin("plug"),
            }),
        ))
        .unwrap();

    let first = serde_json::to_string(&registry.extension_tool_defs()).unwrap();
    let second = serde_json::to_string(&registry.extension_tool_defs()).unwrap();
    assert_eq!(first, second, "tool ordering feeds prompt-cache fingerprints");
}

#[test]
fn extension_tool_defs_drops_tools_whose_extension_is_not_enabled() {
    let registry = ToolRegistry::default();
    let ext = ExtensionId::mcp("srv");
    enabled_record(registry.extensions(), &ext, &["srv__echo"]);
    registry.register(mcp_tool("srv__echo", "srv", 1)).unwrap();
    registry.register(make_tool("builtin_tool", "ok")).unwrap();

    assert_eq!(registry.extension_tool_defs().len(), 1);
    registry
        .extensions()
        .begin(&ext, ExtensionState::Disabling, Some(WithdrawalCause::Disable));
    assert!(
        registry.extension_tool_defs().is_empty(),
        "hygiene: the T0→T1 window must not advertise a tool the gate refuses"
    );
}

// ── §7.2 classification ──────────────────────────────────────────────────

#[test]
fn partial_withdrawal_is_distinguished_from_total_loss_and_from_unknown() {
    let registry = ToolRegistry::default();
    let a = ExtensionId::mcp("a");
    let b = ExtensionId::plugin("b");
    let ledger = registry.extensions();
    enabled_record(ledger, &a, &["a__do"]);
    enabled_record(ledger, &b, &["b::do"]);
    registry
        .register(make_tool_with_caps("a__do", vec!["shared_cap"]))
        .unwrap();
    registry
        .register(make_tool_with_caps("b::do", vec!["shared_cap"]))
        .unwrap();

    let caps = vec!["shared_cap".to_string()];
    let clean = registry.resolve_capabilities(&caps, &[]);
    assert_eq!(clean.defs.len(), 2);
    assert!(clean.withheld.is_empty() && clean.partially_withheld.is_empty());

    // Disable A: B still serves the capability → partially withheld, attributed.
    ledger.withdraw(&a, ["shared_cap"]);
    registry.remove("a__do");
    ledger_disable(ledger, &a);

    let partial = registry.resolve_capabilities(&caps, &[]);
    assert_eq!(partial.defs.len(), 1);
    assert!(partial.withheld.is_empty());
    assert_eq!(partial.partially_withheld.len(), 1);
    assert_eq!(partial.partially_withheld[0].providers.len(), 1);
    assert_eq!(partial.partially_withheld[0].providers[0].extension, a);
    assert!(!partial.partially_withheld[0].providers[0].server_withdrawn);

    // Disable B too: every provider is gone → withheld, not "unknown".
    ledger.withdraw(&b, ["shared_cap"]);
    registry.remove("b::do");
    ledger_disable(ledger, &b);

    let total = registry.resolve_capabilities(&caps, &[]);
    assert!(total.defs.is_empty());
    assert_eq!(total.withheld.len(), 1);
    assert_eq!(total.withheld[0].providers.len(), 2);
    assert!(total.unknown.is_empty());

    // A capability nothing ever provided stays `unknown` — a typo and a
    // withdrawal must not become indistinguishable in the other direction.
    let unknown = registry.resolve_capabilities(&["never_provided".to_string()], &[]);
    assert_eq!(unknown.unknown, vec!["never_provided".to_string()]);
    assert!(unknown.withheld.is_empty());
}

// ── X-23: case ───────────────────────────────────────────────────────────

#[tokio::test]
async fn a_mixed_case_extension_tool_name_is_refused_on_both_arms() {
    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::mcp("srv");
    enabled_record(registry.extensions(), &ext, &["Srv__Echo"]);
    registry.register(mcp_tool("Srv__Echo", "srv", 1)).unwrap();
    ledger_disable(registry.extensions(), &ext);

    // Hit arm — the snapshot still holds the mixed-case entry.
    let err = registry
        .execute_with_context("Srv__Echo", &serde_json::json!({}), &ToolContext::default())
        .await
        .unwrap_err();
    assert!(err.contains("MCP server 'srv' is disabled by the owner"), "{err}");

    // Miss arm — the ledger retains "Srv__Echo"; the call names it differently.
    assert!(registry.remove("Srv__Echo"));
    assert_eq!(
        registry.extensions().owner_of("srv__echo"),
        Some(ext.clone())
    );
    let err = registry
        .execute_with_context("srv__echo", &serde_json::json!({}), &ToolContext::default())
        .await
        .unwrap_err();
    assert!(err.contains("MCP server 'srv' is disabled by the owner"), "{err}");
    assert!(!err.contains("not found"), "{err}");
}

// ── §3.7: the server withdrew the name itself ────────────────────────────

#[tokio::test]
async fn a_server_withdrawn_name_is_refused_on_both_arms_while_the_row_reads_enabled() {
    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::mcp("github");
    let generation = enabled_record(registry.extensions(), &ext, &["github__create_issue"]);
    registry
        .register(mcp_tool("github__create_issue", "github", generation))
        .unwrap();

    // §3.7 step 5: tombstone + remove, but keep the name flagged.
    registry
        .extensions()
        .flag_server_withdrawn(&ext, "github__create_issue");

    // Hit arm — a snapshot taken before the change still holds the entry, and
    // its state and generation both pass.
    let snapshot = (*registry).clone();
    let err = snapshot
        .execute_with_context(
            "github__create_issue",
            &serde_json::json!({}),
            &ToolContext::default(),
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("was withdrawn by 'github' itself, which is still enabled"),
        "{err}"
    );
    assert!(!err.contains("disabled by the owner"), "{err}");

    // Miss arm.
    assert!(registry.remove("github__create_issue"));
    let err = registry
        .execute_with_context(
            "github__create_issue",
            &serde_json::json!({}),
            &ToolContext::default(),
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("was withdrawn by 'github' itself, which is still enabled"),
        "{err}"
    );

    // The extension itself is untouched.
    assert_eq!(
        registry.extensions().state(&ext),
        Some(ExtensionState::Enabled)
    );
    assert_eq!(
        registry.extensions().record(&ext).unwrap().withdrawn_by_server,
        vec!["github__create_issue".to_string()]
    );
}

// ── X-21: precedence — the gate is the highest rung ──────────────────────

#[tokio::test]
async fn auto_approve_cannot_undo_the_extension_gate_on_either_arm() {
    use crate::bus::EventBus;
    use crate::daemon_config::DaemonConfig;
    use crate::security::capabilities::Allowlist;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};

    let registry = Arc::new(ToolRegistry::default());
    let ext = ExtensionId::mcp("github");
    enabled_record(registry.extensions(), &ext, &["github__create_issue"]);
    registry
        .register(mcp_tool("github__create_issue", "github", 1))
        .unwrap();

    let sandbox = SandboxManager::with_defaults(Arc::clone(&registry), EventBus::new(16));

    let mut config = DaemonConfig::default();
    config.security.auto_approve_confirmations = true;
    let policy = SandboxPolicy {
        agent_id: "lead".to_string(),
        allowed_capabilities: Allowlist::Unrestricted,
        denied_capabilities: vec![],
        // The tool would otherwise prompt; auto_approve skips the prompt.
        require_confirmation_for: vec!["github__create_issue".to_string()],
        max_tool_calls: None,
        max_tool_runtime_secs: 30,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: config.security.auto_approve_confirmations,
    };
    assert!(policy.auto_approve);

    let call = openalpaca_llm::ToolCall {
        id: "call-1".to_string(),
        name: "github__create_issue".to_string(),
        arguments: serde_json::json!({}),
    };

    // T0 — from this instant the gate refuses, whatever the policy says.
    registry
        .extensions()
        .begin(&ext, ExtensionState::Disabling, Some(WithdrawalCause::Disable));

    // Hit arm.
    let err = sandbox
        .execute_tool(&call, &policy, &ToolContext::default())
        .await
        .unwrap_err();
    assert!(err.contains("is being turned off right now"), "{err}");

    // Miss arm.
    assert!(registry.remove("github__create_issue"));
    let err = sandbox
        .execute_tool(&call, &policy, &ToolContext::default())
        .await
        .unwrap_err();
    assert!(err.contains("is being turned off right now"), "{err}");
    assert!(!err.contains("not found"), "{err}");
}

#[test]
fn the_refusal_is_rendered_from_the_same_table_as_the_row() {
    // One source, three renderings (X-18): the gate string and the human
    // secondary text come from `describe`, so they cannot disagree.
    let ext = ExtensionId::mcp("github");
    let model = ExtensionState::Disabled
        .describe(&ext, None, Audience::Model)
        .render_model(Some("github__create_issue"));
    let human = ExtensionState::Disabled
        .describe(&ext, None, Audience::Human)
        .render_human();
    assert!(model.contains("MCP server 'github' is disabled by the owner"));
    assert!(human.contains("MCP server 'github' is disabled by the owner"));
    assert!(human.contains("config/mcp.toml"));
}
