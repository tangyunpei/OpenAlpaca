use async_trait::async_trait;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

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
            crate::tools::platform::shell_command(command).output()
        })
        .await
        .map_err(|_| "Command timed out after 30s".to_string())?
        .map_err(|e| format!("Failed to execute command: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push_str("\n\n");
                }
                result.push_str("STDERR:\n");
                result.push_str(&stderr);
            }
            Ok(result)
        } else {
            Err(format!(
                "Command failed (exit {}):\n{}{}",
                output.status.code().unwrap_or(-1),
                stdout,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!("\nSTDERR:\n{}", stderr)
                }
            ))
        }
    }
}

pub(super) fn shell_execute_tool() -> RegisteredTool {
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
