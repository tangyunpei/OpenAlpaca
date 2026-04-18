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
        task_id: None,
        owner_id: None,
        workspace_id: None,
        skill_stack: vec![],
        effective_constraints: None,
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
        skill_stack: vec![],
        effective_constraints: None,
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
        assert!(known.contains(name), "{name} should be in known list");
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
        fn known_capability_names(&self) -> &'static [&'static str] {
            &["annotation:custom"]
        }
    }

    let mut registry = ToolRegistry::new().unwrap();
    registry.register_capability_provider(Arc::new(CustomProvider));
    let known = registry.known_virtual_capabilities();
    assert!(known.contains(&"annotation:custom"));
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
        fn known_capability_names(&self) -> &'static [&'static str] {
            &["annotation:custom"]
        }
    }

    let mut registry = ToolRegistry::new().unwrap();
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
