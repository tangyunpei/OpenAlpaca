use super::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

/// Return all built-in tool definitions and implementations.
/// When `db` is provided, memory-backed tools (memory_search) are included.
/// When `embedder` is provided, memory_search uses hybrid (FTS + vector) search.
pub fn builtin_tools(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
) -> Vec<RegisteredTool> {
    let mut tools = vec![
        web_search_tool(),
        web_fetch_tool(),
        summarize_tool(),
        text_generate_tool(),
        file_read_tool(),
        file_write_tool(),
        shell_execute_tool(),
    ];
    if let Some(db) = db {
        tools.push(memory_search_tool(db, embedder));
    }
    tools
}

// --- web_search ---

struct WebSearchTool;

#[async_trait]
impl BuiltInTool for WebSearchTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(format!(
            "Web search is not yet configured. Please set up a search provider API key. Query was: {}",
            query
        ))
    }
}

fn web_search_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web for information".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    }
                },
                "required": ["query"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(WebSearchTool)),
    }
}

// --- web_fetch ---

struct WebFetchTool;

#[async_trait]
impl BuiltInTool for WebFetchTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let url = arguments
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: url".to_string())?;

        let client = reqwest::Client::new();
        let response = client
            .get(url)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch URL: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP error: {}", status));
        }

        let body = response
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {}", e))?;

        // Truncate to 8KB
        Ok(body.chars().take(8192).collect())
    }
}

fn web_fetch_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch content from a URL".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(WebFetchTool)),
    }
}

// --- summarize ---

struct SummarizeTool;

#[async_trait]
impl BuiltInTool for SummarizeTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let input = arguments
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(format!("Summary request noted: {}", input))
    }
}

fn summarize_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "summarize".to_string(),
            description: "Condense text into a shorter summary".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to summarize"
                    }
                },
                "required": ["text"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(SummarizeTool)),
    }
}

// --- text_generate ---

struct TextGenerateTool;

#[async_trait]
impl BuiltInTool for TextGenerateTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let input = arguments
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(format!("Generation request noted: {}", input))
    }
}

fn text_generate_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "text_generate".to_string(),
            description: "Generate text from a prompt".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The text generation prompt"
                    }
                },
                "required": ["prompt"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(TextGenerateTool)),
    }
}

// --- file_read ---

struct FileReadTool;

#[async_trait]
impl BuiltInTool for FileReadTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        // Security: reject absolute paths and .. components
        validate_workspace_path(path)?;

        let full_path = std::env::current_dir()
            .map_err(|e| format!("Cannot determine working directory: {}", e))?
            .join(path);

        tokio::fs::read_to_string(&full_path)
            .await
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))
    }
}

fn file_read_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file from the workspace".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace"
                    }
                },
                "required": ["path"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileReadTool)),
    }
}

// --- file_write ---

struct FileWriteTool;

#[async_trait]
impl BuiltInTool for FileWriteTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: content".to_string())?;

        // Security: reject absolute paths and .. components
        validate_workspace_path(path)?;

        let full_path = std::env::current_dir()
            .map_err(|e| format!("Cannot determine working directory: {}", e))?
            .join(path);

        // Create parent directories if needed
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        tokio::fs::write(&full_path, content)
            .await
            .map_err(|e| format!("Failed to write file '{}': {}", path, e))?;

        Ok(format!("Successfully wrote {} bytes to {}", content.len(), path))
    }
}

fn file_write_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_write".to_string(),
            description: "Write content to a file in the workspace".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileWriteTool)),
    }
}

// --- shell_execute ---

struct ShellExecuteTool;

#[async_trait]
impl BuiltInTool for ShellExecuteTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: command".to_string())?;

        // Note: InputSanitizer already blocks ;, &&, ||, backticks, $( in arguments.
        // This timeout provides defense-in-depth.
        let timeout = std::time::Duration::from_secs(30);

        let output = tokio::time::timeout(timeout, {
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
        })
        .await
        .map_err(|_| "Command timed out after 30s".to_string())?
        .map_err(|e| format!("Failed to execute command: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(format!("{}{}", stdout, stderr))
        } else {
            Err(format!(
                "Command failed (exit {}): {}{}",
                output.status.code().unwrap_or(-1),
                stdout,
                stderr
            ))
        }
    }
}

fn shell_execute_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "shell_execute".to_string(),
            description: "Run a shell command. Note: command chaining (;, &&, ||) and command substitution (backticks, $()) are blocked by security filters.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    }
                },
                "required": ["command"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(ShellExecuteTool)),
    }
}

// --- memory_search ---

struct MemorySearchTool {
    db: openalpaca_storage::Database,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
}

#[async_trait]
impl BuiltInTool for MemorySearchTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: query".to_string())?;

        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        let owner_id = arguments
            .get("owner_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing owner_id (should be injected by executor)".to_string())?;

        // Generate query embedding if embedder available
        let query_embedding = if let Some(ref embedder) = self.embedder {
            embedder.embed(&[query]).await.ok().and_then(|v| v.into_iter().next())
        } else {
            None
        };

        let repo = openalpaca_storage::repository::MemoryRepository::new(&self.db);
        let memories = repo
            .search_hybrid(owner_id, query, query_embedding.as_deref(), limit, None, None, None)
            .map_err(|e| format!("Memory search failed: {}", e))?;

        let results: Vec<serde_json::Value> = memories
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "kind": m.kind.as_str(),
                    "scope": m.scope.as_str(),
                    "content": m.content,
                    "importance": m.importance,
                    "created_at": m.created_at,
                })
            })
            .collect();

        serde_json::to_string(&results).map_err(|e| format!("JSON serialization failed: {}", e))
    }
}

fn memory_search_tool(
    db: openalpaca_storage::Database,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "memory_search".to_string(),
            description: "Search the user's memory for relevant facts, preferences, and knowledge. Use this when you need to recall something the user told you previously.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query to find relevant memories"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(MemorySearchTool { db, embedder })),
    }
}

/// Validate that a path is safe for workspace-scoped file operations.
/// Rejects absolute paths and paths containing `..` components.
fn validate_workspace_path(path: &str) -> Result<(), String> {
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("Absolute paths are not allowed. Use relative paths within the workspace.".to_string());
    }
    if path.contains("..") {
        return Err("Path traversal ('..') is not allowed.".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_tools_count_without_db() {
        let tools = builtin_tools(None, None);
        assert_eq!(tools.len(), 7);
    }

    #[test]
    fn test_builtin_tools_count_with_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
        let tools = builtin_tools(Some(db), None);
        assert_eq!(tools.len(), 8);
    }

    #[test]
    fn test_all_tools_have_valid_definitions() {
        for tool in builtin_tools(None, None) {
            assert!(!tool.definition.name.is_empty());
            assert!(!tool.definition.description.is_empty());
            assert!(tool.definition.parameters.is_object());
        }
    }

    #[test]
    fn test_validate_workspace_path_rejects_absolute() {
        assert!(validate_workspace_path("/etc/passwd").is_err());
    }

    #[test]
    fn test_validate_workspace_path_rejects_traversal() {
        assert!(validate_workspace_path("../secret").is_err());
        assert!(validate_workspace_path("foo/../../bar").is_err());
    }

    #[test]
    fn test_validate_workspace_path_accepts_relative() {
        assert!(validate_workspace_path("src/main.rs").is_ok());
        assert!(validate_workspace_path("README.md").is_ok());
    }

    #[tokio::test]
    async fn test_summarize_tool() {
        let tool = SummarizeTool;
        let result = tool
            .execute(&serde_json::json!({"text": "Hello world"}))
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Summary request noted"));
    }

    #[tokio::test]
    async fn test_text_generate_tool() {
        let tool = TextGenerateTool;
        let result = tool
            .execute(&serde_json::json!({"prompt": "Write a poem"}))
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Generation request noted"));
    }

    #[tokio::test]
    async fn test_web_search_stub() {
        let tool = WebSearchTool;
        let result = tool
            .execute(&serde_json::json!({"query": "rust language"}))
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("not yet configured"));
    }
}
