use crate::tools::platform::{RunFailure, run_bounded, shell_command};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use crate::tools::working_folder::{WorkingFolderRefusal, working_folder};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::path::Path;
use std::sync::Arc;

use super::annotations_for_builtin;
use super::approval_preview::command_card;
use super::helpers::acting_folder;

/// The defence-in-depth bound on one command. The primary timeout is the
/// SandboxManager's (default 60s, `daemon_config.execution.agent_defaults.
/// max_tool_runtime_secs`); this one holds when the sandbox layer is bypassed
/// (e.g. ToolRegistry used directly). Either way the command and what it
/// started are killed (R45, `run_bounded`).
const SHELL_TIMEOUT_SECS: u64 = 300;

struct ShellExecuteTool;

impl ShellExecuteTool {
    /// Run `command` with `folder` as its working directory (storage design
    /// §10, R26) and shape its output for the model.
    async fn run_in(&self, command: &str, folder: &Path) -> Result<String, String> {
        // Note: InputSanitizer blocks backticks, $( command substitution, and
        // newlines/carriage returns in arguments. Shell chaining (;, &&, ||) and
        // pipes are intentionally allowed — the agent constructs the full command
        // string (see security::sanitizer::check_string_safety). The folder is a
        // working directory, not a confinement (ST-14): an absolute path, `~`
        // or `cd ..` reaches anywhere the user can.
        let mut cmd = shell_command(command);
        cmd.current_dir(folder);
        let output = run_bounded(cmd, std::time::Duration::from_secs(SHELL_TIMEOUT_SECS))
            .await
            .map_err(|failure| match failure {
                RunFailure::TimedOut => format!("Command timed out after {SHELL_TIMEOUT_SECS}s"),
                RunFailure::Spawn(e) | RunFailure::Io(e) => {
                    format!("Failed to execute command: {e}")
                }
            })?;

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

#[async_trait]
impl BuiltInTool for ShellExecuteTool {
    /// R26: a command runs in its conversation's working folder, and a call
    /// reached with no context names none — refused, never run in the
    /// daemon's own directory.
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err(WorkingFolderRefusal::NoContext.to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: command".to_string())?;
        // R26, R27, R44: the project's folder, or the conversation's scratch
        // folder made on first use; a missing project, or a call with
        // neither, is refused here with its sentence and nothing runs.
        let (folder, _) = acting_folder(ctx)?;
        self.run_in(command, &folder).await
    }

    /// R34 (ST-14): the card names the folder the command starts in. A call
    /// the body would refuse — no conversation, a missing project — raises
    /// no card: its sentence is the answer (R27).
    async fn confirmation_preview(
        &self,
        _arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        match working_folder(ctx) {
            Ok(folder) => Ok(Some(command_card(&folder))),
            Err(refusal) => Err(refusal.to_string()),
        }
    }
}

pub(super) fn shell_execute_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "shell_execute".to_string(),
            description: "Execute a single shell command and return its output. The command \
                starts in the working folder — the project, or this conversation's scratch \
                folder — and every command asks first; an approved command runs as you and can \
                reach files outside that folder. Subshells ($(), backticks) and embedded \
                newlines are blocked for security; shell chaining (;, &&, ||) and pipes are \
                allowed. Timeout: 300 seconds. Output is truncated to 512KB. Use for system \
                operations, package management, git commands, build tools, and any operations \
                not covered by other tools."
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
        // Stage 2 (R26): a command runs in its call's working folder, so the
        // call carries one — a guarded project made for the test.
        let gp = crate::test_util::GuardedProject::new();
        let ctx = gp.ctx();
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let arguments = serde_json::json!({"command": starts_a_sleep(dir.path(), &pidfile)});
        let call = tokio::spawn(async move {
            let tool = ShellExecuteTool;
            tool.execute_with_context(&arguments, &ctx).await
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

        // Stage 2 (R26): the call carries a working folder (auto_approve skips
        // the card, so only the body resolves it).
        let gp = crate::test_util::GuardedProject::new();
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
            .execute_tool(&call, &policy, &gp.ctx())
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

#[cfg(test)]
mod preview_tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::events::SystemEvent;
    use crate::security::capabilities::Allowlist;
    use crate::security::confirmation::{ConfirmationBroker, ConfirmationResponse};
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    use crate::test_util::GuardedProject;
    use crate::tools::ToolRegistry;
    use openalpaca_llm::ToolCall;

    fn registry() -> ToolRegistry {
        let registry = ToolRegistry::default();
        registry.register(shell_execute_tool()).unwrap();
        registry
    }

    fn ls() -> serde_json::Value {
        serde_json::json!({"command": "ls"})
    }

    fn project_card(root: &std::path::Path) -> String {
        format!(
            "Runs in the project folder {}. The command itself can reach files outside it.",
            root.display()
        )
    }

    /// R34, ST-14: the card names the folder the command starts in.
    #[tokio::test]
    async fn a_shell_card_names_the_project_folder_the_command_starts_in() {
        let gp = GuardedProject::new();

        let preview = registry()
            .confirmation_preview("shell_execute", &ls(), &gp.ctx())
            .await;

        assert_eq!(preview, Ok(Some(project_card(&gp.root))));
    }

    #[tokio::test]
    async fn a_shell_card_with_no_project_names_the_conversations_scratch_folder() {
        let gp = GuardedProject::new();

        let preview = registry()
            .confirmation_preview("shell_execute", &ls(), &gp.scratch_ctx())
            .await;

        let dir = gp.home_root().join("scratch").join("conv-test");
        assert_eq!(
            preview,
            Ok(Some(format!(
                "Runs in this conversation's scratch folder {}. The command itself can reach \
                 files outside it.",
                dir.display()
            )))
        );
        assert!(!dir.exists(), "the card makes no folder");
    }

    /// The card's text reaches the event every client draws the card from, and
    /// a denied command never runs. The command would only touch a file in the
    /// temp project; denied, nothing is spawned at all.
    #[tokio::test]
    async fn the_shell_card_reaches_the_confirmation_event_and_a_denied_command_never_runs() {
        let gp = GuardedProject::new();
        let bus = EventBus::default();
        let mut events = bus.subscribe();
        let mut sandbox = SandboxManager::with_defaults(Arc::new(registry()), bus);
        let broker = Arc::new(ConfirmationBroker::new());
        sandbox.set_confirmation_broker(broker.clone());
        let policy = SandboxPolicy {
            agent_id: "general_agent".to_string(),
            allowed_capabilities: Allowlist::Unrestricted,
            denied_capabilities: vec![],
            require_confirmation_for: vec![],
            max_tool_calls: None,
            max_tool_runtime_secs: 60,
            stream_id: None,
            lane_key: None,
            confirmation_timeout_secs: Some(5),
            auto_approve: false,
            unattended: false,
        };
        let marker = gp.root.join("ran.txt");
        let call = ToolCall {
            id: "tc_1".to_string(),
            name: "shell_execute".to_string(),
            arguments: serde_json::json!({"command": format!("touch {}", marker.display())}),
        };
        let responder = tokio::spawn({
            let broker = broker.clone();
            async move {
                loop {
                    match events.recv().await {
                        Ok(SystemEvent::ToolConfirmationRequested {
                            request_id,
                            preview,
                            ..
                        }) => {
                            broker
                                .respond(
                                    &request_id,
                                    ConfirmationResponse {
                                        approved: false,
                                        approval_scope: None,
                                    },
                                )
                                .unwrap();
                            return preview;
                        }
                        Ok(_) => continue,
                        Err(e) => panic!("the bus closed before a card was raised: {e}"),
                    }
                }
            }
        });

        let result = sandbox.execute_tool(&call, &policy, &gp.ctx()).await;

        // The denial first: a call refused before any card (capability,
        // sanitizer) publishes no event, and awaiting the responder then would
        // wait for ever — this panics cleanly instead, aborting the task.
        let refusal = result.unwrap_err();
        assert!(refusal.contains("denied by user"), "the person said no: {refusal}");
        assert_eq!(responder.await.unwrap(), Some(project_card(&gp.root)));
        assert!(!marker.exists(), "a denied command never runs");
    }

    // ── R26, R27, R44 (storage design §10): the command's working folder ──
    // `ToolContext`, `BuiltInTool` and `ShellExecuteTool` come in through
    // `use super::*`. `WorkingFolderRefusal` is named in full: `shell_execute.rs`
    // imports it only from Step 3 on, and Step 2 must compile.
    fn command_args(command: &str) -> serde_json::Value {
        serde_json::json!({ "command": command })
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_starts_in_the_project_folder() {
        let gp = crate::test_util::GuardedProject::new();
        let out = ShellExecuteTool
            .execute_with_context(&command_args("pwd -P"), &gp.ctx())
            .await
            .unwrap();
        assert_eq!(out.trim_end(), gp.root.display().to_string());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_with_no_project_starts_in_the_conversations_scratch_folder() {
        let gp = crate::test_util::GuardedProject::new();
        let scratch = gp.home_root().join("scratch").join("conv-test");
        assert!(!scratch.exists());

        let out = ShellExecuteTool
            .execute_with_context(&command_args("pwd -P"), &gp.scratch_ctx())
            .await
            .unwrap();
        assert_eq!(out.trim_end(), scratch.display().to_string());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&scratch).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    /// R26: reached with no context, the shell runs nothing — never in the
    /// daemon's own directory.
    #[tokio::test]
    async fn a_command_with_no_context_is_refused() {
        let err = ShellExecuteTool
            .execute(&command_args("echo hi"))
            .await
            .expect_err("no context");
        assert_eq!(
            err,
            "This tool acts in the conversation's working folder, and this call carries no \
             conversation to name one."
        );
    }

    /// R44 and the lead's ruling R-T2: a run parked by `POST /v1/tasks` on a
    /// lane with no session is given the lane's session at dispatch (S3.13),
    /// so its calls carry an owner, a task and a conversation — the card
    /// names the conversation's scratch folder and the command runs there.
    #[tokio::test]
    async fn a_parked_run_works_in_its_conversations_scratch_folder() {
        let gp = crate::test_util::GuardedProject::new();
        let ctx = ToolContext {
            owner_id: Some("owner-test".to_string()),
            task_id: Some("task-parked".to_string()),
            ..gp.scratch_ctx()
        };
        let scratch = gp.home_root().join("scratch").join("conv-test");

        assert_eq!(
            ShellExecuteTool
                .confirmation_preview(&command_args("echo hi"), &ctx)
                .await,
            Ok(Some(format!(
                "Runs in this conversation's scratch folder {}. The command itself can reach \
                 files outside it.",
                scratch.display()
            )))
        );
        assert!(!scratch.exists(), "the card makes no folder");

        let out = ShellExecuteTool
            .execute_with_context(&command_args("echo hi"), &ctx)
            .await
            .unwrap();
        assert_eq!(out.trim_end(), "hi");
        assert!(scratch.is_dir(), "the body made the run's scratch folder");
    }

    /// R44: a call with an owner and a task but neither a project nor a
    /// conversation — since S3.13 no dispatched run is one, and the sentence
    /// stays for a call that has neither — is refused at the card and in the
    /// body, with the one sentence.
    #[tokio::test]
    async fn a_call_with_neither_a_project_nor_a_conversation_is_refused() {
        let ctx = ToolContext {
            owner_id: Some("owner-test".to_string()),
            task_id: Some("task-parked".to_string()),
            ..Default::default()
        };
        let sentence = "This call has neither a project nor a conversation, so it has no working \
                        folder: file tools and commands run only inside a conversation or a project.";

        assert_eq!(
            ShellExecuteTool
                .confirmation_preview(&command_args("echo hi"), &ctx)
                .await,
            Err(sentence.to_string())
        );
        assert_eq!(
            ShellExecuteTool
                .execute_with_context(&command_args("echo hi"), &ctx)
                .await,
            Err(sentence.to_string())
        );
    }

    /// R27: a missing project is refused before anything runs, and no card
    /// asks a person about a command that would be refused after the click.
    /// (`echo`, not a command that writes: before Step 3 the body still runs
    /// it in the test process's own directory, the repository.)
    #[tokio::test]
    async fn a_missing_project_refuses_the_command_and_raises_no_card() {
        let gp = crate::test_util::GuardedProject::new();
        std::fs::remove_dir_all(&gp.root).unwrap();
        let missing = crate::tools::working_folder::WorkingFolderRefusal::ProjectMissing {
            root: gp.root.display().to_string(),
        }
        .to_string();

        assert_eq!(
            ShellExecuteTool
                .confirmation_preview(&command_args("echo hi"), &gp.ctx())
                .await,
            Err(missing.clone())
        );
        assert_eq!(
            ShellExecuteTool
                .execute_with_context(&command_args("echo hi"), &gp.ctx())
                .await,
            Err(missing)
        );
        assert!(!gp.root.exists(), "nothing re-created the folder");
    }
}
