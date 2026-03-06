use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
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
        // The primary timeout is enforced by the SandboxManager (default 60s from
        // daemon_config.execution.agent_defaults.max_tool_runtime_secs). This 300s
        // timeout is a defense-in-depth safety net in case the sandbox layer is
        // bypassed (e.g. RegistryToolExecutor used directly).
        let timeout = std::time::Duration::from_secs(300);

        let output = tokio::time::timeout(timeout, {
            crate::tools::platform::shell_command(command).output()
        })
        .await
        .map_err(|_| "Command timed out after 300s".to_string())?
        .map_err(|e| format!("Failed to execute command: {}", e))?;

        // Cap output size to prevent memory pressure from commands producing
        // unbounded stdout/stderr (e.g. `find /` or `yes`).
        const MAX_OUTPUT_BYTES: usize = 512 * 1024; // 512 KB
        let stdout_raw = &output.stdout;
        let stderr_raw = &output.stderr;
        let stdout = String::from_utf8_lossy(&stdout_raw[..stdout_raw.len().min(MAX_OUTPUT_BYTES)]);
        let stderr = String::from_utf8_lossy(&stderr_raw[..stderr_raw.len().min(MAX_OUTPUT_BYTES)]);

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
            description: "Execute a single shell command and return its output. Command \
                chaining (;, &&, ||) and subshells ($()) are blocked for security. \
                Timeout: 300 seconds. Output is truncated to 512KB. Use for system \
                operations, package management, git commands, build tools, and any \
                operations not covered by other tools."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute (single command only, no chaining)"
                    }
                },
                "required": ["command"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"command": "ls -la /tmp"}),
                serde_json::json!({"command": "grep -rn 'TODO' src/"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(ShellExecuteTool)),
    }
}
