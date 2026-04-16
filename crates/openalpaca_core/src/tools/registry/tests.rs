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
