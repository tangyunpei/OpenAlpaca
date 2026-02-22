use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

use super::helpers::{
    MAX_FILE_READ_SIZE, is_identity_path, is_soul_path, is_user_path, resolve_workspace_path,
    resolve_workspace_path_for_write, validate_workspace_path,
};

// --- file_read ---

struct FileReadTool;

#[async_trait]
impl BuiltInTool for FileReadTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        // Security: resolve path, reject traversal and symlink escapes
        let full_path = resolve_workspace_path(path)?;

        // Guard against OOM: reject files larger than 10 MB
        let metadata = tokio::fs::metadata(&full_path)
            .await
            .map_err(|e| format!("Cannot access file '{}': {}", path, e))?;
        if metadata.len() > MAX_FILE_READ_SIZE {
            return Err(format!(
                "File '{}' is {} bytes, exceeding the {} byte limit",
                path,
                metadata.len(),
                MAX_FILE_READ_SIZE
            ));
        }

        tokio::fs::read_to_string(&full_path)
            .await
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))
    }
}

pub(super) fn file_read_tool() -> RegisteredTool {
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

        // Guard against disk exhaustion: limit write size to 10 MB
        const MAX_FILE_WRITE_SIZE: u64 = 10 * 1024 * 1024;
        if content.len() as u64 > MAX_FILE_WRITE_SIZE {
            return Err(format!(
                "Content size {} bytes exceeds the {} byte write limit",
                content.len(),
                MAX_FILE_WRITE_SIZE
            ));
        }

        // Security: reject absolute paths and .. components
        validate_workspace_path(path)?;

        // Safety: block writes to SOUL.md — use update_soul tool instead
        if is_soul_path(path) {
            return Err("Writing to SOUL.md via file_write is blocked. \
                 Use the update_soul tool instead, which provides validation, \
                 backup, and safe atomic writes."
                .to_string());
        }

        // Safety: block writes to USER.md — use update_user tool instead
        if is_user_path(path) {
            return Err("Writing to USER.md via file_write is blocked. \
                 Use the update_user tool instead, which provides validation, \
                 backup, and safe atomic writes."
                .to_string());
        }

        // Safety: block writes to IDENTITY.md — use update_identity tool instead
        if is_identity_path(path) {
            return Err("Writing to IDENTITY.md via file_write is blocked. \
                 Use the update_identity tool instead, which provides validation, \
                 backup, and safe atomic writes."
                .to_string());
        }

        // Create parent directories before resolving (so canonicalize can work)
        let workspace_root = std::env::current_dir()
            .map_err(|e| format!("Cannot determine working directory: {}", e))?;
        let preliminary_path = workspace_root.join(path);
        if let Some(parent) = preliminary_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }

        // Security: resolve path, reject symlink escapes
        let full_path = resolve_workspace_path_for_write(path)?;

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

pub(super) fn file_write_tool() -> RegisteredTool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
