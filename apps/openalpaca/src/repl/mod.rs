//! Interactive REPL with rustyline — readline, history, reconnect, session usage

mod commands;
mod completer;

use crate::chat_stream::{self, ChatTarget, StreamOptions, StreamResult, UsageInfo};
use crate::client::DaemonClient;
use colored::Colorize;
use completer::ReplHelper;
use std::io::{IsTerminal, Write};

const MAX_HISTORY_ENTRIES: usize = 1000;

/// Keep a Ctrl+C typed between prompts from ending the REPL.
///
/// rustyline 18 installs its SIGINT handler only while it reads a line
/// (rustyline #903; 17 kept one for the editor's life), so between prompts —
/// while the first message's POST is in flight, before any reply stream
/// awaits `tokio::signal::ctrl_c()` — SIGINT took its default action and the
/// REPL died, history unsaved. tokio's handler, registered here once, stays
/// installed for the life of the process: a Ctrl+C outside a stream is then
/// dropped, as under rustyline 17, and a reply stream's `ctrl_c()` still
/// sees every one pressed while it waits.
#[cfg(unix)]
fn keep_sigint_from_ending_the_repl() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    signal(SignalKind::interrupt()).map(drop)
}

fn history_path() -> anyhow::Result<std::path::PathBuf> {
    // Machine state, not something the user edits — it lives under state/.
    let dir = openalpaca_storage::store::state_dir()?;
    Ok(dir.join("repl_history"))
}

pub struct ReplSession {
    client: DaemonClient,
    context: ReplContext,
    /// Where every turn of this REPL goes: the CLI's project, and the
    /// conversation `--resume`/`--session` named, if any.
    target: ChatTarget,
}

pub struct ReplContext {
    pub session_usage: SessionUsage,
    pub verbose: bool,
}

pub struct SessionUsage {
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub total_duration_ms: u64,
    pub request_count: u64,
}

impl SessionUsage {
    fn new() -> Self {
        Self {
            total_tokens_in: 0,
            total_tokens_out: 0,
            total_duration_ms: 0,
            request_count: 0,
        }
    }

    fn add(&mut self, info: &UsageInfo) {
        self.total_tokens_in += info.tokens_in;
        self.total_tokens_out += info.tokens_out;
        self.total_duration_ms += info.duration_ms;
        self.request_count += 1;
    }
}

impl ReplSession {
    pub fn new(target: ChatTarget) -> anyhow::Result<Self> {
        let client = DaemonClient::connect()?;
        Ok(Self {
            client,
            context: ReplContext {
                session_usage: SessionUsage::new(),
                verbose: false,
            },
            target,
        })
    }

    fn reconnect(&mut self) -> anyhow::Result<()> {
        self.client = DaemonClient::connect()?;
        Ok(())
    }

    /// Try reconnect, print feedback, return true if reconnected.
    fn try_reconnect(&mut self) -> bool {
        match self.reconnect() {
            Ok(()) => {
                eprintln!("{}", "(reconnected to daemon)".yellow());
                true
            }
            Err(_) => false,
        }
    }

    fn should_reconnect(err: &anyhow::Error) -> bool {
        if let Some(re) = err.downcast_ref::<reqwest::Error>() {
            return re.is_connect() || re.is_timeout();
        }
        let msg = err.to_string().to_lowercase();
        msg.contains("daemon is not running")
            || msg.contains("http 401")
            || msg.contains("connection refused")
            || msg.contains("broken pipe")
            || msg.contains("connection closed")
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        #[cfg(unix)]
        keep_sigint_from_ending_the_repl()?;
        self.print_welcome().await;

        let config = rustyline::Config::builder()
            .max_history_size(MAX_HISTORY_ENTRIES)?
            .auto_add_history(true)
            .build();
        let mut editor =
            rustyline::Editor::<ReplHelper, rustyline::history::FileHistory>::with_config(config)?;
        editor.set_helper(Some(ReplHelper));

        if let Ok(path) = history_path() {
            let _ = editor.load_history(&path);
        }

        loop {
            let readline = tokio::task::block_in_place(|| editor.readline(">> "));

            match readline {
                Ok(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "exit" || line == "quit" {
                        break;
                    }

                    // Client-side commands run locally; everything else —
                    // including daemon commands (/status, /steer, /cancel, …)
                    // and unknown slashes (possible skill commands) — goes to
                    // the daemon as a chat message.
                    if line.starts_with('/') && !commands::SlashCommand::parse(line).is_forward() {
                        self.execute_slash(line).await;
                    } else {
                        self.execute_chat(line).await;
                    }
                }
                Err(rustyline::error::ReadlineError::Interrupted) => continue,
                Err(rustyline::error::ReadlineError::Eof) => break,
                Err(e) => return Err(e.into()),
            }
        }

        if let Ok(path) = history_path() {
            let _ = editor.save_history(&path);
        }
        Ok(())
    }

    async fn execute_slash(&mut self, line: &str) {
        match commands::execute_line(line, &self.client, &mut self.context).await {
            Ok(()) => {}
            Err(e) if Self::should_reconnect(&e) => {
                if self.try_reconnect() {
                    if let Err(e2) =
                        commands::execute_line(line, &self.client, &mut self.context).await
                    {
                        eprintln!("{} {}", "Error:".red(), e2);
                    }
                } else {
                    eprintln!("{} {}", "Error:".red(), e);
                }
            }
            Err(e) => eprintln!("{} {}", "Error:".red(), e),
        }
    }

    async fn execute_chat(&mut self, content: &str) {
        print!("{} ", "Alpaca:".cyan().bold());
        std::io::stdout().flush().ok();

        let opts = StreamOptions {
            verbose: self.context.verbose,
            ..StreamOptions::default()
        };

        // Phase 1: send_chat (POST) with retry-once
        let send_result = match chat_stream::send_chat(&self.client, content, &self.target).await {
            Ok(resp) => Ok(resp),
            Err(e) if Self::should_reconnect(&e) => {
                if self.try_reconnect() {
                    chat_stream::send_chat(&self.client, content, &self.target).await
                } else {
                    Err(e)
                }
            }
            Err(e) => Err(e),
        };

        let resp = match send_result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("\n{} {}", "Error:".red(), e);
                return;
            }
        };

        // Phase 2: stream_chat (GET) with retry-once using same stream_id
        let stream_result =
            match chat_stream::stream_chat(&self.client, &resp.stream_id, &opts).await {
                Ok(result) => Ok(result),
                Err(e) if Self::should_reconnect(&e) => {
                    if self.try_reconnect() {
                        chat_stream::stream_chat(&self.client, &resp.stream_id, &opts).await
                    } else {
                        Err(e)
                    }
                }
                Err(e) => Err(e),
            };

        match stream_result {
            Ok(result) => {
                if let Some(usage) = result.usage() {
                    self.context.session_usage.add(usage);
                }
                // A failed turn is said out loud and the prompt stays open —
                // the REPL's half of L12, where the one-shot paths exit
                // non-zero instead. On stderr, like every other error here.
                if let Some(message) = result.failure() {
                    eprintln!(
                        "\n{}",
                        chat_stream::failure_line(message, std::io::stderr().is_terminal())
                    );
                }
                // If delegation, poll for task completion
                if let StreamResult::Delegation { delegation, .. } = &result
                    && let Err(e) =
                        chat_stream::poll_task_completion(&self.client, &delegation.task_id).await
                {
                    eprintln!("{} {}", "Poll error:".red(), e);
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.to_lowercase().contains("not found") {
                    eprintln!(
                        "\n{}",
                        "Stream lost after reconnect; please resend your message.".yellow()
                    );
                } else {
                    eprintln!("\n{} {}", "Error:".red(), e);
                }
            }
        }
        println!();
    }

    async fn print_welcome(&self) {
        println!(
            "{} {} {}",
            "OpenAlpaca Chat".bold(),
            "|".dimmed(),
            "Type 'exit' to leave, /help for commands".dimmed()
        );
        if let Ok(config) = self
            .client
            .get::<serde_json::Value>("/v1/orchestrator/config")
            .await
            && let Some(model) = config["model"].as_str()
        {
            println!("{} {}", "Model:".dimmed(), model);
        }
        println!();
    }
}

#[cfg(all(test, unix))]
mod sigint_tests {
    /// The child half: run only when the parent below re-executes this test
    /// binary with the variable set. It registers what `run` registers, then
    /// raises SIGINT at itself; with the handler in place it returns and its
    /// harness exits 0, without it the default action ends it by the signal.
    #[test]
    #[ignore = "run by a_ctrl_c_between_prompts_does_not_end_the_repl in a child process"]
    fn sigint_child() {
        use nix::sys::signal::{SigHandler, Signal, raise, signal};
        if std::env::var_os("OPENALPACA_SIGINT_CHILD").is_none() {
            return;
        }
        // An ignored SIGINT survives exec (a `cargo test &` from a script
        // starts the run with one) and would keep this child alive without
        // the registration, so start from the default action.
        // SAFETY: SIG_DFL installs no handler, and nothing else in this
        // process has set one for SIGINT.
        unsafe { signal(Signal::SIGINT, SigHandler::SigDfl) }.unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            super::keep_sigint_from_ending_the_repl().unwrap();
            raise(Signal::SIGINT).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });
    }

    /// Ctrl+C while the first message's POST is in flight (P27's manual gate,
    /// "the skeptic's window"): rustyline 18 holds no handler between prompts,
    /// so this process survives a SIGINT only by the one `run` registers. The
    /// SIGINT goes to a child, a re-execution of this binary, so a failure
    /// ends the child and not the test run.
    #[test]
    fn a_ctrl_c_between_prompts_does_not_end_the_repl() {
        use std::os::unix::process::ExitStatusExt;
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repl::sigint_tests::sigint_child",
                "--ignored",
                "--quiet",
            ])
            .env("OPENALPACA_SIGINT_CHILD", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "the child ended with {:?} (signal {:?})",
            out.status,
            out.status.signal()
        );
        // A filter that matched no test would exit 0 too.
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(" 1 passed;"),
            "the child ran no test: {stdout}"
        );
    }
}
