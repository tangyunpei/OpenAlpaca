use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::path::PathBuf;
use std::sync::Arc;

use super::helpers::{
    MAX_FILE_READ_SIZE, is_identity_path, is_soul_path, is_user_path, resolve_workspace_path,
    resolve_workspace_path_for_write, validate_workspace_path,
};

// --- file_read ---

struct FileReadTool {
    workspace_root: PathBuf,
}

#[async_trait]
impl BuiltInTool for FileReadTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: path".to_string())?;

        // Security: resolve path, reject traversal and symlink escapes
        let full_path = resolve_workspace_path(path, &self.workspace_root)?;

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

pub(super) fn file_read_tool(workspace_root: PathBuf) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file's full contents from the workspace directory. \
                Returns the file content as a UTF-8 string. Files larger than 10MB are \
                rejected. Use this to inspect configuration files, source code, or data \
                before processing. For binary files, use shell_execute with appropriate \
                commands instead."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace (e.g., 'src/main.rs', 'config/settings.toml'). Absolute paths and '..' traversal are rejected."
                    }
                },
                "required": ["path"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"path": "src/main.rs"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileReadTool { workspace_root })),
        provides_capabilities: vec![],
    }
}

// --- file_write ---

struct FileWriteTool {
    workspace_root: PathBuf,
}

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

        // Safety: block writes to SOUL.md — use update_persona tool instead
        if is_soul_path(path) {
            return Err("Writing to SOUL.md via file_write is blocked. \
                 Use the update_persona tool with target=\"soul\" instead, \
                 which provides validation, backup, and safe atomic writes."
                .to_string());
        }

        // Safety: block writes to USER.md — use update_persona tool instead
        if is_user_path(path) {
            return Err("Writing to USER.md via file_write is blocked. \
                 Use the update_persona tool with target=\"user\" instead, \
                 which provides validation, backup, and safe atomic writes."
                .to_string());
        }

        // Safety: block writes to IDENTITY.md — use update_persona tool instead
        if is_identity_path(path) {
            return Err("Writing to IDENTITY.md via file_write is blocked. \
                 Use the update_persona tool with target=\"identity\" instead, \
                 which provides validation, backup, and safe atomic writes."
                .to_string());
        }

        // Security: resolve path, reject symlink escapes BEFORE creating directories.
        // This prevents out-of-workspace directory creation via symlinked intermediates.
        let full_path = resolve_workspace_path_for_write(path, &self.workspace_root)?;

        // Create parent directories AFTER boundary validation
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

pub(super) fn file_write_tool(workspace_root: PathBuf) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "file_write".to_string(),
            description: "Write or overwrite a file in the workspace directory. Creates \
                parent directories automatically if they don't exist. Content is limited \
                to 10MB. Writing to persona files (SOUL.md, USER.md, IDENTITY.md) is \
                blocked — use update_persona instead. Returns a confirmation with the \
                byte count written."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace (e.g., 'src/main.rs', 'output/report.txt'). Absolute paths and '..' traversal are rejected."
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file as a UTF-8 string"
                    }
                },
                "required": ["path", "content"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"path": "output/result.txt", "content": "Hello, world!"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(FileWriteTool { workspace_root })),
        provides_capabilities: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_write_blocks_soul_md() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "SOUL.md",
                "content": "malicious content"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("update_persona"));
    }

    #[tokio::test]
    async fn test_file_write_creates_subdirs_within_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "subdir/nested/test.txt",
                "content": "hello"
            }))
            .await;
        assert!(result.is_ok());
        assert!(dir.path().join("subdir/nested/test.txt").exists());
    }

    #[tokio::test]
    async fn test_file_write_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        // Create a symlink inside workspace pointing outside
        let link_path = dir.path().join("escape_link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link_path).unwrap();
        #[cfg(not(unix))]
        {
            // On non-unix, skip this test
            return;
        }

        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "escape_link/evil.txt",
                "content": "should not be written"
            }))
            .await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("outside the workspace") || err_msg.contains("boundary"),
            "Should reject symlink escape, got: {}",
            err_msg
        );
        // Verify nothing was written outside
        assert!(!outside.path().join("evil.txt").exists());
    }

    #[tokio::test]
    async fn test_file_write_blocks_soul_md_in_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let tool = FileWriteTool {
            workspace_root: dir.path().to_path_buf(),
        };
        let result = tool
            .execute(&serde_json::json!({
                "path": "config/SOUL.md",
                "content": "sneaky content"
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("update_persona"));
    }
}
