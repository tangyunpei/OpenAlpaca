use crate::tools::platform::{RunFailure, run_bounded, shell_command, shown_output};
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

/// What the input sanitizer admits in a `shell_execute` command
/// (`security::sanitizer`, `check_string_safety`: backticks, `$(`, `\n` and
/// `\r` are refused, everything else passes) — said once, so the description
/// and the `command` parameter cannot disagree (N-TOOL-1, Z1-05).
const WHAT_A_COMMAND_MAY_HOLD: &str = "One command line: `;`, `&&`, `||`, pipes and \
    redirections are allowed; command substitution (`$(…)` or backticks) and line breaks \
    are refused.";

/// `shell_execute`'s description, stating `timeout_secs` — the limit a call
/// really runs under — or, with `None`, no figure at all.
///
/// The figure belongs to the loop that hands the definition to a model: its
/// sandbox cuts a call at `max_tool_runtime_secs` ([`state_enforced_timeout`]).
/// The registered definition states none, so no surface that shows it
/// without a loop can state a wrong one.
fn shell_execute_description(timeout_secs: Option<u64>) -> String {
    let limit = match timeout_secs {
        Some(1) => "A command still running after 1 second is stopped, with every process \
            it started."
            .to_string(),
        Some(secs) => format!(
            "A command still running after {secs} seconds is stopped, with every process it \
             started."
        ),
        None => "A command still running at the calling agent's time limit is stopped, with \
            every process it started."
            .to_string(),
    };
    format!(
        "Run a shell command and return its output. The command starts in the working folder \
         — the project, or this conversation's scratch folder — and every command asks first; \
         an approved command runs as you and can reach files outside that folder. \
         {WHAT_A_COMMAND_MAY_HOLD} {limit} Output is truncated to 512KB. Use for system \
         operations, package management, git commands, build tools, and any operations not \
         covered by other tools."
    )
}

/// N-TOOL-1 (Z1-05): state on `shell_execute`'s definition the limit a loop
/// whose sandbox cuts a tool at `sandbox_secs` really enforces — the
/// sandbox's cut, or the tool's own [`SHELL_TIMEOUT_SECS`] where that comes
/// first. Every loop calls it with its policy (`run_agentic_loop_core`); the
/// other definitions are left as they are.
pub(crate) fn state_enforced_timeout(tools: &mut [ToolDefinition], sandbox_secs: u64) {
    let secs = sandbox_secs.min(SHELL_TIMEOUT_SECS);
    for definition in tools.iter_mut().filter(|d| d.name == "shell_execute") {
        definition.description = shell_execute_description(Some(secs));
    }
}

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
                RunFailure::TimedOut => timed_out(SHELL_TIMEOUT_SECS),
                RunFailure::Spawn(e) | RunFailure::Io(e) => {
                    format!("Failed to execute command: {e}")
                }
            })?;

        // Each stream at most 512 KB: `run_bounded` kept no more than that
        // and one byte while the command ran (`find /` or `yes` included,
        // R-T28), and the cut to the cap, with its line, is here.
        let stdout = shown_output(&output.stdout, "stdout");
        let stderr = shown_output(&output.stderr, "stderr");

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
            let code = output.status.code();
            let mut failure = format!(
                "Command failed (exit {}):\n{}{}",
                code.unwrap_or(-1),
                stdout,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!("\nSTDERR:\n{}", stderr)
                }
            );
            // N-TOOL-4 (Z1-13): the shell's own word for "no such command"
            // is exit 127 — sharper than the old "not found" anywhere in the
            // output, which a failing grep could trip.
            if code == Some(127) {
                failure.push('\n');
                failure.push_str(NO_SUCH_COMMAND_HINT);
            }
            Err(failure)
        }
    }
}

/// What a command the shell could not find is told (exit 127; N-TOOL-4).
const NO_SUCH_COMMAND_HINT: &str =
    "Hint: check that the command is installed, or give its full path.";

/// The shell's own bound passed (N-TOOL-4): the limit, and what to do.
fn timed_out(secs: u64) -> String {
    format!(
        "Command timed out after {secs}s\n{}",
        crate::tools::platform::COMMAND_TIMED_OUT_HINT
    )
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
            description: shell_execute_description(None),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": format!("The shell command to run. {WHAT_A_COMMAND_MAY_HOLD}")
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
        // N-TOOL-4: the sandbox, where the limit is enforced, tells a command
        // it cut what to do — never to raise a `timeout` (N-TOOL-1).
        assert_eq!(
            err,
            "Tool 'shell_execute' timed out after 1s\nHint: that is the most a command may run \
             here; split the work into smaller commands that each finish sooner."
        );
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

    // ── N-TOOL-1 (Z1-05): what the definition says about chaining ──

    /// The description and the `command` parameter say the same thing about
    /// chaining, and it is what the sanitizer admits: `;`, `&&`, `||`, pipes
    /// and redirections pass, command substitution and line breaks are
    /// refused. Neither text may forbid what the gate lets through.
    #[test]
    fn the_description_and_the_parameter_say_what_the_sanitizer_admits() {
        use crate::security::sanitizer::InputSanitizer;

        let definition = shell_execute_tool().definition;
        let parameter = definition.parameters["properties"]["command"]["description"]
            .as_str()
            .expect("the command parameter is described")
            .to_string();
        for text in [&definition.description, &parameter] {
            assert!(!text.contains("no chaining"), "forbids what passes: {text}");
            assert!(
                !text.contains("single command"),
                "forbids what passes: {text}"
            );
            for named in [
                "`;`",
                "`&&`",
                "`||`",
                "pipes",
                "redirections",
                "`$(…)`",
                "backticks",
                "line breaks",
            ] {
                assert!(text.contains(named), "{named:?} missing from: {text}");
            }
        }

        let admits = |command: &str| {
            InputSanitizer::sanitize_tool_args(
                "shell_execute",
                &serde_json::json!({ "command": command }),
                &[],
                &[],
            )
            .is_ok()
        };
        for command in [
            "make && make test",
            "make || true",
            "cd src; ls",
            "ls | wc -l",
            "ls > out.txt 2>&1",
        ] {
            assert!(admits(command), "the definitions say {command:?} passes");
        }
        for command in [
            "echo $(id)",
            "echo `id`",
            "ls\nrm out.txt",
            "ls\rrm out.txt",
        ] {
            assert!(
                !admits(command),
                "the definitions say {command:?} is refused"
            );
        }
    }

    // ── R26, R27, R44 (storage design §10): the command's working folder ──
    // `ToolContext`, `BuiltInTool` and `ShellExecuteTool` come in through
    // `use super::*`. `WorkingFolderRefusal` is named in full: `shell_execute.rs`
    // imports it only from Step 3 on, and Step 2 must compile.
    fn command_args(command: &str) -> serde_json::Value {
        serde_json::json!({ "command": command })
    }

    /// Residual X7: a command that wrote more than the 512 KB a stream keeps
    /// is told so — the kept text, then one line naming the cut — where a
    /// command that wrote exactly the cap shows no such line; the model can
    /// tell the two apart.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_cut_output_says_it_was_cut() {
        let gp = crate::test_util::GuardedProject::new();
        let cap = crate::tools::platform::OUTPUT_CAP;
        let run = |bytes: usize| {
            let ctx = gp.ctx();
            async move {
                ShellExecuteTool
                    .execute_with_context(
                        &command_args(&format!("head -c {bytes} /dev/zero | tr '\\0' a")),
                        &ctx,
                    )
                    .await
                    .unwrap()
            }
        };

        let cut = run(cap + 100_000).await;
        let marker = "\n[stdout cut at 512 KB; the rest was not kept]";
        assert!(
            cut.ends_with(marker),
            "{} bytes, the last 60 reversed: {:?}",
            cut.len(),
            cut.chars().rev().take(60).collect::<String>()
        );
        assert_eq!(cut.len(), cap + marker.len());
        assert!(cut.as_bytes()[..cap].iter().all(|b| *b == b'a'));

        let whole = run(cap).await;
        assert_eq!(whole.len(), cap, "exactly the cap is shown whole");
        assert!(!whole.contains("cut at"));
    }

    /// N-TOOL-4 (Z1-13): a command the shell cannot find (exit 127) is
    /// answered with what to do about it, in the tool's own error.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_the_shell_cannot_find_is_answered_with_the_hint_to_check_it() {
        let gp = crate::test_util::GuardedProject::new();
        let err = ShellExecuteTool
            .execute_with_context(
                &command_args("openalpaca-no-such-command-n-tool-4"),
                &gp.ctx(),
            )
            .await
            .expect_err("no such command");
        assert!(err.starts_with("Command failed (exit 127):"), "{err}");
        assert!(
            err.ends_with("\nHint: check that the command is installed, or give its full path."),
            "{err}"
        );
    }

    /// N-TOOL-4: the shell's own bound — what holds when the sandbox is
    /// bypassed — tells a command it stopped the same thing the sandbox
    /// does, and names no `timeout` parameter (N-TOOL-1).
    #[test]
    fn the_shells_own_bound_tells_a_command_what_to_do() {
        let text = super::timed_out(SHELL_TIMEOUT_SECS);
        assert_eq!(
            text,
            "Command timed out after 300s\nHint: that is the most a command may run here; \
             split the work into smaller commands that each finish sooner."
        );
        assert!(!text.to_lowercase().contains("timeout"), "{text}");
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

    /// R44, the context half of the lead's ruling R-T2: a call that carries
    /// an owner, a task and a conversation — the context a run's tools get
    /// once the dispatch has given the run a session — has its card name the
    /// conversation's scratch folder and its command run there. The context
    /// is built by hand: that a parked run is really given that session is
    /// the dispatcher's tests' to prove
    /// (`starting_a_parked_run_on_a_lane_with_no_session_stores_the_session_it_opened`
    /// and its `rerun` twin in `orchestrator/dispatcher/tests.rs`).
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
