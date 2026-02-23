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
