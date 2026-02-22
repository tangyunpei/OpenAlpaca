use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::collections::HashMap;
use std::sync::Arc;

/// Backend that executes a tool's logic.
pub enum ToolBackend {
    BuiltIn(Arc<dyn BuiltInTool>),
    Http {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        timeout_secs: u64,
    },
    Command {
        command: String,
        args_template: Option<String>,
        timeout_secs: u64,
    },
}

/// Trait for built-in tool implementations.
#[async_trait]
pub trait BuiltInTool: Send + Sync {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String>;
}

/// A tool registered in the registry: its LLM-facing definition + execution backend.
pub struct RegisteredTool {
    pub definition: ToolDefinition,
    pub backend: ToolBackend,
}

/// Central registry mapping tool names to definitions and execution backends.
///
/// Populated at startup (built-ins + user TOML), then shared as `Arc<ToolRegistry>`.
/// All read methods are `&self` — no locking needed after init.
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Called only during startup before wrapping in Arc.
    pub fn register(&mut self, tool: RegisteredTool) {
        self.tools.insert(tool.definition.name.clone(), tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    /// Return tool definitions filtered by skill names.
    /// If `skill_names` is empty, returns an empty list (least-privilege:
    /// agents without explicit skills get no tools from the registry).
    /// The caller (`resolve_agent_tools`) adds workspace tools separately.
    /// If non-empty but no names match, also returns empty.
    pub fn definitions_for_skills(&self, skill_names: &[String]) -> Vec<ToolDefinition> {
        if skill_names.is_empty() {
            tracing::debug!(
                "definitions_for_skills called with empty skills — returning empty (least-privilege)"
            );
            return Vec::new();
        }

        self.tools
            .values()
            .filter(|t| skill_names.contains(&t.definition.name))
            .map(|t| t.definition.clone())
            .collect()
    }

    /// Execute a tool by name.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| format!("Unknown tool: '{}'", tool_name))?;

        match &tool.backend {
            ToolBackend::BuiltIn(handler) => handler.execute(arguments).await,
            ToolBackend::Http {
                method,
                url,
                headers,
                timeout_secs,
            } => execute_http(method, url, headers, *timeout_secs, arguments).await,
            ToolBackend::Command {
                command,
                args_template,
                timeout_secs,
            } => execute_command(command, args_template.as_deref(), *timeout_secs, arguments).await,
        }
    }

    /// List registered tool names (used by InputSanitizer via ToolExecutor trait).
    pub fn registered_tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Number of registered tools.
    pub fn count(&self) -> usize {
        self.tools.len()
    }
}

/// Execute an HTTP backend tool call.
async fn execute_http(
    method: &str,
    url_template: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    // Replace {param_name} placeholders in URL with URL-encoded argument values
    let mut url = url_template.to_string();
    if let Some(obj) = arguments.as_object() {
        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let encoded = urlencoding::encode(&replacement);
            url = url.replace(&format!("{{{}}}", key), &encoded);
        }
    }

    let client = reqwest::Client::new();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    let mut request = match method.to_uppercase().as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    };

    for (k, v) in headers {
        request = request.header(k.as_str(), v.as_str());
    }

    request = request.timeout(timeout);

    let response = request
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    if status.is_success() {
        // Truncate to 8KB
        Ok(body.chars().take(8192).collect())
    } else {
        Err(format!(
            "HTTP {} — {}",
            status,
            body.chars().take(1024).collect::<String>()
        ))
    }
}

/// Execute a command backend tool call.
async fn execute_command(
    command: &str,
    args_template: Option<&str>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    let mut full_args = args_template.unwrap_or("").to_string();
    if let Some(obj) = arguments.as_object() {
        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let escaped = shell_escape::escape(replacement.into());
            full_args = full_args.replace(&format!("{{{}}}", key), &escaped);
        }
    }

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let full_command = format!("{} {}", command, full_args);

    let output = tokio::time::timeout(timeout, {
        crate::tools::platform::shell_command(&full_command).output()
    })
    .await
    .map_err(|_| format!("Command timed out after {}s", timeout_secs))?
    .map_err(|e| format!("Failed to execute command: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        Ok(stdout.to_string())
    } else {
        Err(format!(
            "Command failed (exit {}): {}{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        ))
    }
}

#[cfg(test)]
mod tests {
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
}
