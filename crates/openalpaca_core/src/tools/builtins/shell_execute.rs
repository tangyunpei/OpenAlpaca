use crate::tools::platform::{RunFailure, run_bounded, shell_command};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

use super::annotations_for_builtin;

struct ShellExecuteTool;

#[async_trait]
impl BuiltInTool for ShellExecuteTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: command".to_string())?;

        // Note: InputSanitizer blocks backticks, $( command substitution, and
        // newlines/carriage returns in arguments. Shell chaining (;, &&, ||) and
        // pipes are intentionally allowed — the agent constructs the full command
        // string (see security::sanitizer::check_string_safety).
        // The primary timeout is enforced by the SandboxManager (default 60s from
        // daemon_config.execution.agent_defaults.max_tool_runtime_secs). This 300s
        // timeout is a defense-in-depth safety net in case the sandbox layer is
        // bypassed (e.g. ToolRegistry used directly).
        let timeout = std::time::Duration::from_secs(300);

        // R45 (storage design §11): a command that runs out its time, or
        // whose call is dropped (the sandbox's own per-tool timeout, a run
        // cancelled mid-call), is killed with the processes it started.
        let output = match run_bounded(shell_command(command), timeout).await {
            Ok(output) => output,
            Err(RunFailure::TimedOut) => return Err("Command timed out after 300s".to_string()),
            Err(RunFailure::Spawn(e) | RunFailure::Io(e)) => {
                return Err(format!("Failed to execute command: {}", e));
            }
        };

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
            description: "Execute a single shell command and return its output. Subshells \
                ($(), backticks) and embedded newlines are blocked for security; shell \
                chaining (;, &&, ||) and pipes are allowed. \
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
        provides_capabilities: vec!["shell_execute".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("shell_execute"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_util::{GONE_WITHIN, KillOnFail, pid_from_file, wait_until_gone};
    use std::time::Duration;

    /// A command, run in `dir`, that starts a background `sleep`, writes its
    /// pid to `pidfile` and waits on it (R45).
    fn starts_a_sleep(dir: &std::path::Path, pidfile: &std::path::Path) -> String {
        format!(
            "cd '{}'; sleep 30 & echo $! > '{}'; wait",
            dir.display(),
            pidfile.display()
        )
    }

    #[tokio::test]
    async fn a_cancelled_shell_command_takes_the_processes_it_started_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let arguments = serde_json::json!({"command": starts_a_sleep(dir.path(), &pidfile)});
        let call = tokio::spawn(async move {
            let tool = ShellExecuteTool;
            tool.execute(&arguments).await
        });
        let pid = pid_from_file(&pidfile, Duration::from_secs(5)).await;
        let _kill = KillOnFail(pid);
        call.abort();
        let _ = call.await;
        assert!(
            wait_until_gone(pid, GONE_WITHIN).await,
            "the background sleep {pid} outlived the cancelled command"
        );
    }

    /// The path that matters in production: the sandbox's per-tool timeout
    /// (60 s by default) is far shorter than the tool's own 300 s.
    #[tokio::test]
    async fn a_shell_command_the_sandbox_times_out_takes_the_processes_it_started_with_it() {
        use crate::bus::EventBus;
        use crate::security::capabilities::Allowlist;
        use crate::security::sandbox::{SandboxManager, SandboxPolicy};
        use crate::tools::ToolRegistry;
        use crate::tools::registry::ToolContext;

        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let registry = ToolRegistry::default();
        registry.register(shell_execute_tool()).unwrap();
        let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
        let policy = SandboxPolicy {
            agent_id: "agent1".to_string(),
            allowed_capabilities: Allowlist::Unrestricted,
            denied_capabilities: vec![],
            require_confirmation_for: vec![],
            max_tool_calls: None,
            max_tool_runtime_secs: 1,
            stream_id: None,
            lane_key: None,
            confirmation_timeout_secs: None,
            // shell_execute is destructive: the policy answers its card here.
            auto_approve: true,
            unattended: false,
        };
        let call = openalpaca_llm::ToolCall {
            id: "tc_r45".to_string(),
            name: "shell_execute".to_string(),
            arguments: serde_json::json!({"command": starts_a_sleep(dir.path(), &pidfile)}),
        };
        let err = sandbox
            .execute_tool(&call, &policy, &ToolContext::default())
            .await
            .unwrap_err();
        let pid = pid_from_file(&pidfile, Duration::from_secs(1)).await;
        let _kill = KillOnFail(pid);
        assert_eq!(err, "Tool 'shell_execute' timed out after 1s");
        assert!(
            wait_until_gone(pid, GONE_WITHIN).await,
            "the background sleep {pid} outlived the sandbox's timeout"
        );
    }
}
