use super::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use crate::bus::EventBus;
use crate::middleware::soul::{parse_soul_markdown, render_soul_markdown};
use async_trait::async_trait;
use base64::Engine as _;
use openalpaca_llm::ToolDefinition;
use std::path::PathBuf;
use std::sync::Arc;

/// Context required by the `update_soul` tool at runtime.
#[derive(Clone)]
pub struct SoulToolContext {
    /// Absolute path to the active `SOUL.md` file.
    pub soul_path: PathBuf,
    /// Directory for timestamped backups.
    pub backup_dir: PathBuf,
    /// Event bus for publishing `SoulUpdated` events.
    pub bus: EventBus,
    /// Maximum number of backups to keep. `None` = keep all (MVP default).
    pub max_backups: Option<usize>,
}

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

/// Return all built-in tools, including the `update_soul` tool which requires
/// additional context (file paths, event bus).
pub fn builtin_tools_with_soul_context(
    db: Option<openalpaca_storage::Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    soul_ctx: SoulToolContext,
) -> Vec<RegisteredTool> {
    let mut tools = builtin_tools(db, embedder);
    tools.push(update_soul_tool(soul_ctx));
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
        let input = arguments.get("text").and_then(|v| v.as_str()).unwrap_or("");
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

        // Safety: block writes to SOUL.md — use update_soul tool instead
        if is_soul_path(path) {
            return Err("Writing to SOUL.md via file_write is blocked. \
                 Use the update_soul tool instead, which provides validation, \
                 backup, and safe atomic writes."
                .to_string());
        }

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

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            path
        ))
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

        let limit = arguments.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let owner_id = arguments
            .get("owner_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing owner_id (should be injected by executor)".to_string())?;

        // Generate query embedding if embedder available
        let query_embedding = if let Some(ref embedder) = self.embedder {
            embedder
                .embed(&[query])
                .await
                .ok()
                .and_then(|v| v.into_iter().next())
        } else {
            None
        };

        let repo = openalpaca_storage::repository::MemoryRepository::new(&self.db);
        let memories = repo
            .search_hybrid(
                owner_id,
                query,
                query_embedding.as_deref(),
                limit,
                None,
                None,
                None,
            )
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

// --- update_soul ---

struct SoulUpdateTool {
    #[allow(dead_code)]
    ctx: SoulToolContext,
}

/// Known top-level fields in the update_soul tool arguments.
const KNOWN_SOUL_FIELDS: &[&str] = &["mode", "content_b64", "sections"];
/// Known section-patch fields inside the `sections` object.
const KNOWN_SECTION_FIELDS: &[&str] = &[
    "title",
    "summary",
    "core_truths",
    "boundaries",
    "vibe",
    "continuity",
];

#[async_trait]
impl BuiltInTool for SoulUpdateTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Reject unknown top-level fields
        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
                if !KNOWN_SOUL_FIELDS.contains(&key.as_str()) {
                    return Err(format!(
                        "Unknown field '{}'. Allowed fields: {}",
                        key,
                        KNOWN_SOUL_FIELDS.join(", ")
                    ));
                }
            }
        }

        let mode = arguments
            .get("mode")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Missing required parameter: mode (\"replace\" or \"sections\")".to_string()
            })?;

        let validated_markdown = match mode {
            "replace" => self.validate_replace(arguments)?,
            "sections" => self.validate_sections(arguments).await?,
            other => {
                return Err(format!(
                    "Invalid mode '{}'. Must be \"replace\" or \"sections\".",
                    other
                ));
            }
        };

        // Compute content hash for deduplication
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(validated_markdown.as_bytes()));

        // -- Backup current SOUL file (if it exists) --
        let backup_path = if self.ctx.soul_path.exists() {
            let backup_dir = &self.ctx.backup_dir;

            tokio::fs::create_dir_all(backup_dir)
                .await
                .map_err(|e| format!("Failed to create backup directory: {}", e))?;

            let backup_path = unique_backup_path(backup_dir);
            tokio::fs::copy(&self.ctx.soul_path, &backup_path)
                .await
                .map_err(|e| format!("Failed to create backup: {}", e))?;

            Some(backup_path.display().to_string())
        } else {
            None
        };

        // Prune old backups if retention limit is configured
        if let Some(max) = self.ctx.max_backups {
            prune_backups(&self.ctx.backup_dir, max).await;
        }

        // -- Atomic write: temp file → fsync → rename --
        let soul_dir = self
            .ctx
            .soul_path
            .parent()
            .ok_or_else(|| "SOUL path has no parent directory".to_string())?;

        let tmp_path = soul_dir.join(".SOUL.md.tmp");

        // Write to temp file
        tokio::fs::write(&tmp_path, &validated_markdown)
            .await
            .map_err(|e| format!("Failed to write temp file: {}", e))?;

        // fsync the temp file for durability
        let file = tokio::fs::File::open(&tmp_path)
            .await
            .map_err(|e| format!("Failed to open temp file for sync: {}", e))?;
        file.sync_all()
            .await
            .map_err(|e| format!("Failed to fsync temp file: {}", e))?;

        // Atomic rename
        tokio::fs::rename(&tmp_path, &self.ctx.soul_path)
            .await
            .map_err(|e| format!("Atomic rename failed: {}", e))?;

        // Publish SoulUpdated event for synchronous activation
        self.ctx
            .bus
            .publish(crate::events::SystemEvent::SoulUpdated {
                actor: "agent".to_string(),
                mode: mode.to_string(),
                content_sha256: hash.clone(),
                backup_path: backup_path.clone(),
                timestamp: chrono::Utc::now(),
            });

        let mut result = serde_json::json!({
            "status": "applied",
            "mode": mode,
            "path": self.ctx.soul_path.display().to_string(),
            "content_sha256": hash,
            "content_length": validated_markdown.len(),
            "message": "SOUL.md updated successfully."
        });

        if let Some(bp) = backup_path {
            result["backup_path"] = serde_json::Value::String(bp);
        }

        Ok(result.to_string())
    }
}

impl SoulUpdateTool {
    /// Validate the "replace" mode: decode base64, parse as SOUL markdown.
    fn validate_replace(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let b64 = arguments
            .get("content_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Missing required parameter: content_b64 (base64-encoded SOUL markdown)".to_string()
            })?;

        if b64.trim().is_empty() {
            return Err("content_b64 must not be empty.".to_string());
        }

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("Invalid base64 encoding: {}", e))?;

        let content = String::from_utf8(decoded)
            .map_err(|e| format!("Decoded content is not valid UTF-8: {}", e))?;

        if content.trim().is_empty() {
            return Err("Decoded SOUL content is empty.".to_string());
        }

        // Strict schema validation
        parse_soul_markdown(&content).map_err(|e| format!("SOUL validation failed: {}", e))?;

        Ok(content)
    }

    /// Validate the "sections" mode: read current SOUL, apply patches, render, reparse.
    async fn validate_sections(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let sections = arguments.get("sections").ok_or_else(|| {
            "Missing required parameter: sections (object with section patches)".to_string()
        })?;

        let obj = sections
            .as_object()
            .ok_or_else(|| "Parameter 'sections' must be a JSON object.".to_string())?;

        if obj.is_empty() {
            return Err("Sections patch object must not be empty.".to_string());
        }

        // Reject unknown section fields
        for key in obj.keys() {
            if !KNOWN_SECTION_FIELDS.contains(&key.as_str()) {
                return Err(format!(
                    "Unknown section field '{}'. Allowed: {}",
                    key,
                    KNOWN_SECTION_FIELDS.join(", ")
                ));
            }
        }

        // Read current SOUL file
        let current_content = tokio::fs::read_to_string(&self.ctx.soul_path)
            .await
            .map_err(|e| format!("Failed to read current SOUL file: {}", e))?;

        let mut doc = parse_soul_markdown(&current_content)
            .map_err(|e| format!("Current SOUL file is invalid (cannot patch): {}", e))?;

        // Apply patches
        if let Some(v) = obj.get("title") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.title must be a string".to_string())?;
            doc.frontmatter.title = s.to_string();
        }
        if let Some(v) = obj.get("summary") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.summary must be a string".to_string())?;
            doc.frontmatter.summary = s.to_string();
        }
        if let Some(v) = obj.get("core_truths") {
            let arr = v
                .as_array()
                .ok_or_else(|| "sections.core_truths must be an array of strings".to_string())?;
            let truths: Result<Vec<String>, String> = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "core_truths items must be strings".to_string())
                })
                .collect();
            let truths = truths?;
            if truths.is_empty() {
                return Err("core_truths must not be empty.".to_string());
            }
            doc.core_truths = truths;
        }
        if let Some(v) = obj.get("boundaries") {
            let arr = v
                .as_array()
                .ok_or_else(|| "sections.boundaries must be an array of strings".to_string())?;
            let bounds: Result<Vec<String>, String> = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "boundaries items must be strings".to_string())
                })
                .collect();
            let bounds = bounds?;
            if bounds.is_empty() {
                return Err("boundaries must not be empty.".to_string());
            }
            doc.boundaries = bounds;
        }
        if let Some(v) = obj.get("vibe") {
            let s = v
                .as_str()
                .ok_or_else(|| "sections.vibe must be a string".to_string())?;
            if s.trim().is_empty() {
                return Err("vibe must not be empty.".to_string());
            }
            doc.vibe = s.to_string();
        }
        if let Some(v) = obj.get("continuity") {
            let arr = v
                .as_array()
                .ok_or_else(|| "sections.continuity must be an array of strings".to_string())?;
            let cont: Result<Vec<String>, String> = arr
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "continuity items must be strings".to_string())
                })
                .collect();
            let cont = cont?;
            if cont.is_empty() {
                return Err("continuity must not be empty.".to_string());
            }
            doc.continuity = cont;
        }

        // Render and reparse for safety
        let rendered = render_soul_markdown(&doc);
        parse_soul_markdown(&rendered)
            .map_err(|e| format!("Patched SOUL failed re-validation: {}", e))?;

        Ok(rendered)
    }
}

fn update_soul_tool(ctx: SoulToolContext) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "update_soul".to_string(),
            description: "Safely update the system's SOUL.md personality file. \
                Use mode=\"replace\" with content_b64 (base64-encoded full markdown) \
                for complete replacement, or mode=\"sections\" with a sections object \
                to patch individual sections (core_truths, boundaries, vibe, continuity, \
                title, summary). All updates are validated before applying."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["replace", "sections"],
                        "description": "Update mode: 'replace' for full content, 'sections' for patch"
                    },
                    "content_b64": {
                        "type": "string",
                        "description": "Base64-encoded full SOUL markdown (required for 'replace' mode)"
                    },
                    "sections": {
                        "type": "object",
                        "description": "Section patches (required for 'sections' mode)",
                        "properties": {
                            "title": { "type": "string" },
                            "summary": { "type": "string" },
                            "core_truths": { "type": "array", "items": { "type": "string" } },
                            "boundaries": { "type": "array", "items": { "type": "string" } },
                            "vibe": { "type": "string" },
                            "continuity": { "type": "array", "items": { "type": "string" } }
                        }
                    }
                },
                "required": ["mode"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(SoulUpdateTool { ctx })),
    }
}

/// Validate that a path is safe for workspace-scoped file operations.
/// Rejects absolute paths and paths containing `..` components.
fn validate_workspace_path(path: &str) -> Result<(), String> {
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(
            "Absolute paths are not allowed. Use relative paths within the workspace.".to_string(),
        );
    }
    if path.contains("..") {
        return Err("Path traversal ('..') is not allowed.".to_string());
    }
    Ok(())
}

/// Check if a path refers to SOUL.md (case-insensitive filename check).
fn is_soul_path(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .map(|f| f.eq_ignore_ascii_case("soul.md"))
        .unwrap_or(false)
}

/// Generate a unique backup path with nanosecond timestamp + collision suffix.
///
/// Format: `SOUL.20260211T153042.123456789Z.md`
/// Collision: `SOUL.20260211T153042.123456789Z.1.md`, `.2.md`, ...
fn unique_backup_path(backup_dir: &std::path::Path) -> std::path::PathBuf {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S.%9fZ");
    let base_name = format!("SOUL.{}.md", ts);
    let candidate = backup_dir.join(&base_name);
    if !candidate.exists() {
        return candidate;
    }
    for suffix in 1..1000 {
        let name = format!("SOUL.{}.{}.md", ts, suffix);
        let candidate = backup_dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }
    // Fallback: UUID (uuid crate already in openalpaca_core deps)
    backup_dir.join(format!("SOUL.{}.{}.md", ts, uuid::Uuid::new_v4()))
}

/// Prune old backups in `backup_dir`, keeping at most `max` files.
///
/// Backups use timestamped names (`SOUL.<ISO-timestamp>.md`), so lexicographic
/// sorting orders them oldest-first. We remove the oldest entries that exceed
/// the retention limit. Errors are logged but never propagated — pruning
/// failure must not break the update flow.
async fn prune_backups(backup_dir: &std::path::Path, max: usize) {
    let mut entries: Vec<std::path::PathBuf> = match std::fs::read_dir(backup_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.starts_with("SOUL.") && n.ends_with(".md"))
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => return,
    };

    if entries.len() <= max {
        return;
    }

    // Sort alphabetically → oldest timestamps first
    entries.sort();

    let to_remove = entries.len() - max;
    for path in entries.into_iter().take(to_remove) {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!("Failed to prune backup {}: {}", path.display(), e);
        }
    }
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

    // --- SoulUpdateTool tests ---

    /// Helper: create a SoulUpdateTool backed by a temp directory with a valid SOUL file.
    fn make_soul_tool() -> (SoulUpdateTool, tempfile::TempDir) {
        use crate::bus::EventBus;

        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        let valid_soul = r#"---
title: "Test Soul"
summary: "A test soul"
read_when:
  - always
---

## Core Truths

Be helpful.

## Boundaries

- Stay safe.

## Vibe

Friendly and clear.

## Continuity

Remember everything.
"#;
        std::fs::write(&soul_path, valid_soul).unwrap();

        let ctx = SoulToolContext {
            soul_path,
            backup_dir: dir.path().join("backups"),
            bus: EventBus::new(16),
            max_backups: None,
        };
        (SoulUpdateTool { ctx }, dir)
    }

    #[tokio::test]
    async fn test_soul_update_replace_valid() {
        let (tool, _dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok(), "Valid replace should succeed: {:?}", result);
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "applied");
    }

    #[tokio::test]
    async fn test_soul_update_replace_invalid_base64() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": "not-valid-b64!!!"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid base64"));
    }

    #[tokio::test]
    async fn test_soul_update_replace_empty_content() {
        let (tool, _dir) = make_soul_tool();
        // Base64 of just whitespace
        let b64 = base64::engine::general_purpose::STANDARD.encode("   \n  ");
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[tokio::test]
    async fn test_soul_update_replace_invalid_schema() {
        let (tool, _dir) = make_soul_tool();
        // Missing ## Boundaries section
        let invalid = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Boundaries"));
    }

    #[tokio::test]
    async fn test_soul_update_unknown_field_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(
                &serde_json::json!({"mode": "replace", "content_b64": "x", "evil_field": true}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown field"));
    }

    #[tokio::test]
    async fn test_soul_update_missing_mode() {
        let (tool, _dir) = make_soul_tool();
        let result = tool.execute(&serde_json::json!({"content_b64": "x"})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mode"));
    }

    #[tokio::test]
    async fn test_soul_update_sections_valid() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "vibe": "Pirate style, arr!" }
            }))
            .await;
        assert!(
            result.is_ok(),
            "Valid sections patch should succeed: {:?}",
            result
        );
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "applied");
    }

    #[tokio::test]
    async fn test_soul_update_sections_empty_patch_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": {}
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[tokio::test]
    async fn test_soul_update_sections_unknown_field_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "evil": "hi" }
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown section field"));
    }

    #[tokio::test]
    async fn test_soul_update_replace_empty_b64_rejected() {
        let (tool, _dir) = make_soul_tool();
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": ""}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    // --- Step 2: Backup & Atomic Write tests ---

    #[tokio::test]
    async fn test_soul_update_creates_backup() {
        let (tool, dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"New\"\nsummary: \"New soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // Verify backup directory was created and contains a backup
        let backup_dir = dir.path().join("backups");
        assert!(backup_dir.exists(), "Backup directory should exist");
        let entries: Vec<_> = std::fs::read_dir(&backup_dir).unwrap().collect();
        assert_eq!(entries.len(), 1, "Should have exactly one backup");

        // Verify backup content matches original, not the new content
        let backup_path = entries[0].as_ref().unwrap().path();
        let backup_content = std::fs::read_to_string(&backup_path).unwrap();
        assert!(
            backup_content.contains("Test Soul"),
            "Backup should contain original title"
        );
    }

    #[tokio::test]
    async fn test_soul_update_atomic_write_applies_new_content() {
        let (tool, dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"Updated\"\nsummary: \"Updated soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe bold.\n\n## Boundaries\n\n- No harm.\n\n## Vibe\n\nPirate style.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // Verify the SOUL file now has the new content
        let current = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        assert!(
            current.contains("Updated"),
            "SOUL.md should have new content"
        );
        // Verify no temp file remains
        assert!(
            !dir.path().join(".SOUL.md.tmp").exists(),
            "Temp file should not remain"
        );
    }

    #[tokio::test]
    async fn test_soul_update_result_contains_backup_path() {
        let (tool, _dir) = make_soul_tool();
        let valid_soul = "---\ntitle: \"X\"\nsummary: \"X\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nY.\n\n## Boundaries\n\n- Z.\n\n## Vibe\n\nV.\n\n## Continuity\n\nC.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(
            json["backup_path"].is_string(),
            "Result should contain backup_path"
        );
        assert!(
            json["backup_path"].as_str().unwrap().contains("SOUL."),
            "Backup path should contain timestamped name"
        );
    }

    #[tokio::test]
    async fn test_soul_update_validation_failure_does_not_write() {
        let (tool, dir) = make_soul_tool();
        let original = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        // Invalid SOUL - missing Boundaries
        let invalid = "---\ntitle: \"Bad\"\nsummary: \"Bad\"\nread_when:\n  - a\n---\n\n## Core Truths\n\nBe good.\n\n## Vibe\n\nChill.\n\n## Continuity\n\nRemember.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(invalid);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_err());

        // Original file should be untouched
        let after = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        assert_eq!(
            original, after,
            "Failed validation should not modify SOUL.md"
        );
        // No backup should be created for failed validation
        assert!(
            !dir.path().join("backups").exists(),
            "No backup for failed validation"
        );
    }

    // --- Step 3: File write SOUL guard tests ---

    #[tokio::test]
    async fn test_file_write_blocks_soul_md() {
        let tool = FileWriteTool;
        let result = tool
            .execute(&serde_json::json!({
                "path": "SOUL.md",
                "content": "malicious content"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("update_soul"));
    }

    #[tokio::test]
    async fn test_file_write_blocks_soul_md_in_subdir() {
        let tool = FileWriteTool;
        let result = tool
            .execute(&serde_json::json!({
                "path": "config/SOUL.md",
                "content": "sneaky content"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("update_soul"));
    }

    #[test]
    fn test_is_soul_path_variants() {
        assert!(is_soul_path("SOUL.md"));
        assert!(is_soul_path("soul.md"));
        assert!(is_soul_path("Soul.MD"));
        assert!(is_soul_path("config/SOUL.md"));
        assert!(!is_soul_path("README.md"));
        assert!(!is_soul_path("SOULMATE.md"));
        assert!(!is_soul_path("my_soul.md"));
    }

    // --- Step 4: Event publishing test ---

    #[tokio::test]
    async fn test_soul_update_publishes_soul_updated_event() {
        let (tool, _dir) = make_soul_tool();

        // Subscribe BEFORE executing the tool
        let mut rx = tool.ctx.bus.subscribe();

        let valid_soul = "---\ntitle: \"Evented\"\nsummary: \"Event test\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe evented.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nEventful.\n\n## Continuity\n\nRemember events.\n";
        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // Verify the SoulUpdated event was published
        let event = rx
            .try_recv()
            .expect("Should have received SoulUpdated event");
        match event {
            crate::events::SystemEvent::SoulUpdated {
                actor,
                mode,
                content_sha256,
                ..
            } => {
                assert_eq!(actor, "agent");
                assert_eq!(mode, "replace");
                assert!(!content_sha256.is_empty(), "Hash should not be empty");
            }
            other => panic!("Expected SoulUpdated, got: {:?}", other),
        }
    }

    // --- Step 7: Backup retention tests ---

    #[tokio::test]
    async fn test_prune_backups_removes_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Create 7 backup files with sequential timestamps
        for i in 1..=7 {
            let name = format!("SOUL.20260101T00000{}Z.md", i);
            std::fs::write(backup_dir.join(&name), format!("backup {}", i)).unwrap();
        }

        prune_backups(&backup_dir, 2).await;

        let remaining: Vec<_> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        assert_eq!(
            remaining.len(),
            2,
            "Should keep only 2 backups: {:?}",
            remaining
        );
        // Most recent (6 and 7) should survive
        assert!(remaining.contains(&"SOUL.20260101T000006Z.md".to_string()));
        assert!(remaining.contains(&"SOUL.20260101T000007Z.md".to_string()));
    }

    #[tokio::test]
    async fn test_prune_backups_noop_when_under_max() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        std::fs::write(backup_dir.join("SOUL.20260101T000001Z.md"), "b1").unwrap();
        std::fs::write(backup_dir.join("SOUL.20260101T000002Z.md"), "b2").unwrap();

        prune_backups(&backup_dir, 5).await;

        let count = std::fs::read_dir(&backup_dir).unwrap().count();
        assert_eq!(count, 2, "Should not remove any backups when under max");
    }

    #[tokio::test]
    async fn test_soul_update_with_max_backups_prunes() {
        use crate::bus::EventBus;

        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let valid_soul = "---\ntitle: \"Test\"\nsummary: \"Test soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe helpful.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nFriendly.\n\n## Continuity\n\nRemember.\n";
        std::fs::write(&soul_path, valid_soul).unwrap();

        // Pre-create 3 old backups
        for i in 1..=3 {
            std::fs::write(
                backup_dir.join(format!("SOUL.20250101T00000{}Z.md", i)),
                format!("old {}", i),
            )
            .unwrap();
        }

        let ctx = SoulToolContext {
            soul_path,
            backup_dir: backup_dir.clone(),
            bus: EventBus::new(16),
            max_backups: Some(2), // Keep only 2 backups
        };
        let tool = SoulUpdateTool { ctx };

        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);
        let result = tool
            .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
            .await;
        assert!(result.is_ok());

        // After creating 1 new backup + 3 old = 4 total, pruned to 2
        let count = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|e| e.file_name().to_str().map(|n| n.starts_with("SOUL.")))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(count, 2, "Should have pruned to max_backups=2");
    }

    // --- New tests: builtin_tools_with_soul_context registration ---

    #[test]
    fn test_builtin_tools_with_soul_context_includes_update_soul() {
        use crate::bus::EventBus;

        let dir = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
        let ctx = SoulToolContext {
            soul_path: dir.path().join("SOUL.md"),
            backup_dir: dir.path().join("backups"),
            bus: EventBus::new(16),
            max_backups: None,
        };
        let tools = builtin_tools_with_soul_context(Some(db), None, ctx);
        assert_eq!(tools.len(), 9, "Should have 9 tools (8 base + update_soul)");
        assert!(
            tools.iter().any(|t| t.definition.name == "update_soul"),
            "update_soul tool must be present"
        );
    }

    // --- New tests: sections rejects wrong types ---

    #[tokio::test]
    async fn test_sections_rejects_wrong_type_string_fields() {
        let (tool, _dir) = make_soul_tool();

        // vibe: number instead of string
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "vibe": 123 }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be a string"),
            "vibe=123 should report type error"
        );

        // summary: bool instead of string
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "summary": true }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be a string"),
            "summary=true should report type error"
        );

        // title: array instead of string
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "title": ["array"] }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be a string"),
            "title=[array] should report type error"
        );
    }

    #[tokio::test]
    async fn test_sections_rejects_wrong_type_array_fields() {
        let (tool, _dir) = make_soul_tool();

        // core_truths: string instead of array
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "core_truths": "not array" }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be an array"),
            "core_truths=string should report type error"
        );

        // boundaries: object instead of array
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "boundaries": {"obj": true} }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be an array"),
            "boundaries=object should report type error"
        );

        // continuity: number instead of array
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "continuity": 42 }
            }))
            .await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("must be an array"),
            "continuity=42 should report type error"
        );
    }

    #[tokio::test]
    async fn test_wrong_type_does_not_mutate_soul_or_create_backup() {
        let (tool, dir) = make_soul_tool();
        let original = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();

        // Attempt with wrong type for vibe
        let result = tool
            .execute(&serde_json::json!({
                "mode": "sections",
                "sections": { "vibe": 999 }
            }))
            .await;
        assert!(result.is_err());

        // File should be unchanged
        let after = std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap();
        assert_eq!(original, after, "SOUL.md should not be modified on type error");

        // No backup directory should be created
        assert!(
            !dir.path().join("backups").exists(),
            "No backup dir should exist for failed type validation"
        );
    }

    // --- New tests: unique_backup_path collision avoidance ---

    #[test]
    fn test_unique_backup_path_avoids_collision() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Get a path, create the file, then get another — they should differ
        let path1 = unique_backup_path(&backup_dir);
        std::fs::write(&path1, "first").unwrap();

        let path2 = unique_backup_path(&backup_dir);
        assert_ne!(path1, path2, "Second path should differ from first");
        assert!(!path2.exists(), "Second path should not exist yet");
    }

    // --- New tests: prune_backups with mixed old/new filename formats ---

    #[tokio::test]
    async fn test_prune_backups_with_mixed_filename_formats() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        // Old format (second-precision)
        std::fs::write(backup_dir.join("SOUL.20250101T000001Z.md"), "old1").unwrap();
        // New format (nanosecond-precision)
        std::fs::write(
            backup_dir.join("SOUL.20260101T120000.123456789Z.md"),
            "new1",
        )
        .unwrap();
        // Collision-suffixed
        std::fs::write(
            backup_dir.join("SOUL.20260101T120000.123456789Z.1.md"),
            "new2",
        )
        .unwrap();
        // Another new format
        std::fs::write(
            backup_dir.join("SOUL.20260201T000000.000000000Z.md"),
            "new3",
        )
        .unwrap();

        prune_backups(&backup_dir, 2).await;

        let mut remaining: Vec<String> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        remaining.sort();

        assert_eq!(
            remaining.len(),
            2,
            "Should keep only 2 backups, got: {:?}",
            remaining
        );
        // Lexicographic sort: ".1.md" sorts before ".md" (digit < letter),
        // so the collision-suffixed file is older. The two most recent are:
        assert!(remaining.contains(&"SOUL.20260101T120000.123456789Z.md".to_string()));
        assert!(remaining.contains(&"SOUL.20260201T000000.000000000Z.md".to_string()));
    }

    // --- New test: rapid updates produce distinct backups ---

    #[tokio::test]
    async fn test_rapid_updates_produce_distinct_backups() {
        use crate::bus::EventBus;

        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        let backup_dir = dir.path().join("backups");

        let valid_soul = "---\ntitle: \"Test\"\nsummary: \"Test soul\"\nread_when:\n  - always\n---\n\n## Core Truths\n\nBe helpful.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nFriendly.\n\n## Continuity\n\nRemember.\n";
        std::fs::write(&soul_path, valid_soul).unwrap();

        let ctx = SoulToolContext {
            soul_path,
            backup_dir: backup_dir.clone(),
            bus: EventBus::new(16),
            max_backups: None,
        };
        let tool = SoulUpdateTool { ctx };

        let b64 = base64::engine::general_purpose::STANDARD.encode(valid_soul);

        // Perform 5 rapid updates
        for _ in 0..5 {
            let result = tool
                .execute(&serde_json::json!({"mode": "replace", "content_b64": b64}))
                .await;
            assert!(result.is_ok(), "Update should succeed: {:?}", result);
        }

        // Count backup files
        let backup_count = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .ok()
                    .and_then(|e| {
                        e.file_name()
                            .to_str()
                            .map(|n| n.starts_with("SOUL.") && n.ends_with(".md"))
                    })
                    .unwrap_or(false)
            })
            .count();

        assert_eq!(
            backup_count, 5,
            "Should have 5 distinct backup files, no overwrites"
        );
    }
}
