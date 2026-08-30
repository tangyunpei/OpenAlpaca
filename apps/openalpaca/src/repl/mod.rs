//! Interactive REPL with rustyline — readline, history, reconnect, session usage

mod commands;
mod completer;

use crate::chat_stream::{self, StreamOptions, StreamResult, UsageInfo};
use crate::client::DaemonClient;
use colored::Colorize;
use completer::ReplHelper;
use std::io::Write;

const MAX_HISTORY_ENTRIES: usize = 1000;

fn history_path() -> anyhow::Result<std::path::PathBuf> {
    let dir = openalpaca_storage::discovery::ensure_app_dir()?;
    Ok(dir.join("repl_history"))
}

pub struct ReplSession {
    client: DaemonClient,
    context: ReplContext,
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
    pub fn new() -> anyhow::Result<Self> {
        let client = DaemonClient::connect()?;
        Ok(Self {
            client,
            context: ReplContext {
                session_usage: SessionUsage::new(),
                verbose: false,
            },
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

                    if line.starts_with('/') {
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
        };

        // Phase 1: send_chat (POST) with retry-once
        let send_result = match chat_stream::send_chat(&self.client, content).await {
            Ok(resp) => Ok(resp),
            Err(e) if Self::should_reconnect(&e) => {
                if self.try_reconnect() {
                    chat_stream::send_chat(&self.client, content).await
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
