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

fn make_tool(name: &str, response: &str) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: name.to_string(),
            description: format!("{} tool", name),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: response.to_string(),
        })),
    }
}

#[test]
fn test_register_and_lookup() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("test_tool", "ok"));

    assert!(registry.get("test_tool").is_some());
    assert!(registry.get("nonexistent").is_none());
    assert_eq!(registry.count(), 1);
}

#[test]
fn test_definitions_for_skills() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("web_search", "search results"));
    registry.register(make_tool("summarize", "summary"));
    registry.register(make_tool("file_read", "file contents"));

    let skills = vec!["web_search".to_string(), "summarize".to_string()];
    let defs = registry.definitions_for_skills(&skills);
    assert_eq!(defs.len(), 2);
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"web_search"));
    assert!(names.contains(&"summarize"));
}

#[test]
fn test_definitions_for_skills_no_match_returns_empty() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("web_search", "results"));
    registry.register(make_tool("summarize", "summary"));

    let skills = vec!["nonexistent_skill".to_string()];
    let defs = registry.definitions_for_skills(&skills);
    assert_eq!(
        defs.len(),
        0,
        "Non-matching skills should return empty list, not all tools"
    );
}

#[test]
fn test_definitions_for_empty_skills_returns_empty() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("web_search", "results"));
    let defs = registry.definitions_for_skills(&[]);
    assert_eq!(
        defs.len(),
        0,
        "Empty skills should return empty list (least-privilege)"
    );
}

#[tokio::test]
async fn test_execute_builtin() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("test_tool", "hello"));

    let result = registry.execute("test_tool", &serde_json::json!({})).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "hello");
}

#[tokio::test]
async fn test_execute_unknown_tool() {
    let registry = ToolRegistry::new();
    let result = registry
        .execute("nonexistent", &serde_json::json!({}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown tool"));
}

#[test]
fn test_registered_tool_names() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("web_search", "results"));
    registry.register(make_tool("summarize", "summary"));

    let names = registry.registered_tool_names();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"web_search".to_string()));
    assert!(names.contains(&"summarize".to_string()));
}

#[test]
fn test_command_backend_tool_names_empty_for_builtins() {
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("web_search", "results"));
    registry.register(make_tool("summarize", "summary"));

    let cmd_tools = registry.command_backend_tool_names();
    assert!(
        cmd_tools.is_empty(),
        "Built-in tools should not appear in command_backend_tool_names()"
    );
}

#[test]
fn test_command_backend_tool_names_returns_command_tools() {
    let mut registry = ToolRegistry::new();
    // Register a built-in tool
    registry.register(make_tool("web_search", "results"));
    // Register a command-backend tool
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "git_log".to_string(),
            description: "Show git log".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
        backend: ToolBackend::Command {
            command: "git".to_string(),
            args_template: Some("log --oneline -n {count}".to_string()),
            timeout_secs: 15,
        },
    });

    let cmd_tools = registry.command_backend_tool_names();
    assert_eq!(cmd_tools.len(), 1);
    assert!(cmd_tools.contains(&"git_log".to_string()));
}

#[tokio::test]
async fn test_execute_http_ssrf_blocks_private_ip() {
    // Verify that execute_http validates URLs (SSRF protection)
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "internal_api".to_string(),
            description: "Internal API".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            url: "http://169.254.169.254/latest/meta-data/".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
    });

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
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "local_api".to_string(),
            description: "Local API".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            url: "http://localhost/admin".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
    });

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
    let mut registry = ToolRegistry::new();
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
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            url: "https://api.example.com/weather?city={city}&units={units}".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
    });

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
    let mut registry = ToolRegistry::new();
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
        },
        backend: ToolBackend::Http {
            method: "GET".to_string(),
            // URL with placeholder that will be properly substituted
            url: "https://api.example.com/weather?city={city}".to_string(),
            headers: HashMap::new(),
            timeout_secs: 10,
        },
    });

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
    let mut registry = ToolRegistry::new();
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
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
    });

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
    let mut registry = ToolRegistry::new();
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
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
    });

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
    let mut registry = ToolRegistry::new();
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
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
    });

    let result = registry
        .execute("search", &serde_json::json!({"query": "test", "limit": 5}))
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "ok");
}

#[tokio::test]
async fn test_schema_non_object_args_rejected() {
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
        backend: ToolBackend::BuiltIn(Arc::new(MockBuiltIn {
            response: "ok".to_string(),
        })),
    });

    let result = registry
        .execute("search", &serde_json::json!("not an object"))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be a JSON object"));
}

// --- Issue 13: Command backend unsubstituted placeholder detection ---

#[tokio::test]
async fn test_command_unsubstituted_placeholder_detected() {
    let mut registry = ToolRegistry::new();
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
        },
        backend: ToolBackend::Command {
            command: "git".to_string(),
            args_template: Some("log --oneline -n {count} {branch}".to_string()),
            timeout_secs: 15,
        },
    });

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
    let mut registry = ToolRegistry::new();
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
        },
        backend: ToolBackend::Command {
            command: "echo".to_string(),
            args_template: Some("{msg}".to_string()),
            timeout_secs: 5,
        },
    });

    let result = registry
        .execute("echo_tool", &serde_json::json!({"msg": "hello"}))
        .await;
    // Should succeed — no unsubstituted placeholders
    assert!(result.is_ok(), "Expected success, got: {:?}", result);
    assert!(result.unwrap().contains("hello"));
}

// --- Issue 8: Skill name mismatch warning ---

#[test]
fn test_definitions_for_skills_warns_on_mismatch() {
    // This test verifies that `definitions_for_skills` correctly returns
    // an empty list when skill names don't match any registered tool.
    // The warning is logged via `tracing::warn!` — we verify the observable
    // behavior (empty result set) rather than capturing log output.
    let mut registry = ToolRegistry::new();
    registry.register(make_tool("web_search", "results"));
    registry.register(make_tool("summarize", "summary"));

    let skills = vec![
        "web_search".to_string(),
        "typo_skill".to_string(),
        "nonexistent".to_string(),
    ];
    let defs = registry.definitions_for_skills(&skills);

    // Only "web_search" should match; the mismatched names produce warnings
    // but don't affect the result for the matching skill.
    assert_eq!(defs.len(), 1, "Only matching skill should be returned");
    assert_eq!(defs[0].name, "web_search");
}
