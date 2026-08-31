//! Slash command parsing, dispatch, and handlers

use crate::chat_stream;
use crate::client::DaemonClient;
use colored::Colorize;

use super::ReplContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Model,
    Models,
    Agents,
    Keys,
    Usage,
    Clear,
    Verbose,
    /// Not a client-side command: forward the whole line to the daemon as a
    /// chat message. Covers the daemon's chat-level commands (`/status`,
    /// `/tasks`, `/steer`, `/cancel`, `/pause`, `/resume`) and any unknown
    /// slash command, which may be a skill invocation only the daemon knows.
    Forward,
}

impl SlashCommand {
    pub fn parse(input: &str) -> Self {
        let cmd = input.split_whitespace().next().unwrap_or(input);
        match cmd {
            "/help" => Self::Help,
            "/model" => Self::Model,
            "/models" => Self::Models,
            "/agents" => Self::Agents,
            "/keys" => Self::Keys,
            "/usage" => Self::Usage,
            "/clear" => Self::Clear,
            "/verbose" => Self::Verbose,
            _ => Self::Forward,
        }
    }

    /// True when the line should be sent to the daemon instead of being
    /// handled locally.
    pub fn is_forward(&self) -> bool {
        matches!(self, Self::Forward)
    }
}

pub async fn execute_line(
    input: &str,
    client: &DaemonClient,
    context: &mut ReplContext,
) -> anyhow::Result<()> {
    match SlashCommand::parse(input) {
        SlashCommand::Help => {
            print_help();
            Ok(())
        }
        SlashCommand::Model => cmd_model(client).await,
        SlashCommand::Models => cmd_models(client).await,
        SlashCommand::Agents => cmd_agents(client).await,
        SlashCommand::Keys => cmd_keys(client).await,
        SlashCommand::Usage => {
            cmd_usage(context);
            Ok(())
        }
        SlashCommand::Clear => {
            cmd_clear();
            Ok(())
        }
        SlashCommand::Verbose => {
            cmd_verbose(context);
            Ok(())
        }
        // Forwarded lines are routed to the chat path by the REPL loop
        // before this function is reached.
        SlashCommand::Forward => Ok(()),
    }
}

fn print_help() {
    println!("{}", "Client commands:".bold());
    println!("  {} — Current model info", "/model".bold());
    println!("  {} — List available models", "/models".bold());
    println!("  {} — List agents", "/agents".bold());
    println!("  {} — API key health per provider", "/keys".bold());
    println!("  {} — Session token usage", "/usage".bold());
    println!("  {} — Clear terminal", "/clear".bold());
    println!("  {} — Toggle thinking indicator", "/verbose".bold());
    println!("  {} — Show this help", "/help".bold());
    println!();
    println!("{}", "Daemon commands (sent as chat):".bold());
    println!(
        "  {} — Active tasks (or one task's status)",
        "/status [id]".bold()
    );
    println!("  {} — Recent tasks", "/tasks".bold());
    println!(
        "  {} — Correct the running workflow",
        "/steer <text>".bold()
    );
    println!(
        "  {} — Control a workflow (bare form targets active ones)",
        "/cancel|/pause|/resume [id]".bold()
    );
    println!(
        "  {}",
        "Any other /command is forwarded too (e.g. skill commands like /review).".dimmed()
    );
    println!();
    println!("  {} / {} — Exit chat", "exit".bold(), "quit".bold());
}

async fn cmd_model(client: &DaemonClient) -> anyhow::Result<()> {
    let config: serde_json::Value = client.get("/v1/orchestrator/config").await?;
    if let Some(model) = config["model"].as_str() {
        println!("{} {}", "Model:".bold(), model);
    }
    if let Some(fallbacks) = config["fallback_models"].as_array()
        && !fallbacks.is_empty()
    {
        let names: Vec<&str> = fallbacks.iter().filter_map(|v| v.as_str()).collect();
        if !names.is_empty() {
            println!("{} {}", "Fallbacks:".dimmed(), names.join(", "));
        }
    }
    Ok(())
}

async fn cmd_models(client: &DaemonClient) -> anyhow::Result<()> {
    let models: serde_json::Value = client.get("/v1/models").await?;
    if let Some(arr) = models.as_array() {
        if arr.is_empty() {
            println!("{}", "No models available.".dimmed());
        } else {
            for m in arr {
                let id = m["id"].as_str().unwrap_or("-");
                let provider = m["provider"].as_str().unwrap_or("-");
                println!("  {} {}", id, format!("({})", provider).dimmed());
            }
        }
    }
    Ok(())
}

async fn cmd_agents(client: &DaemonClient) -> anyhow::Result<()> {
    let agents: serde_json::Value = client.get("/v1/agents?limit=50").await?;
    if let Some(arr) = agents.as_array() {
        if arr.is_empty() {
            println!("{}", "No agents.".dimmed());
        } else {
            for a in arr {
                let id = a["id"].as_str().unwrap_or("-");
                let name = a["name"].as_str().unwrap_or("-");
                let status = a["status"].as_str().unwrap_or("-");
                let short_id = &id[..8.min(id.len())];
                println!(
                    "  {} {} - {}",
                    short_id.dimmed(),
                    name,
                    crate::output::status_color(status)
                );
            }
        }
    }
    Ok(())
}

async fn cmd_keys(client: &DaemonClient) -> anyhow::Result<()> {
    let status: serde_json::Value = client.get("/v1/settings/llm/status").await?;
    if let Some(obj) = status.as_object() {
        if obj.is_empty() {
            println!("{}", "No provider keys configured.".dimmed());
        } else {
            for (provider, keys) in obj {
                if let Some(arr) = keys.as_array() {
                    if arr.is_empty() {
                        println!("  {} — {}", provider.bold(), "no keys".dimmed());
                    } else {
                        let available = arr
                            .iter()
                            .filter(|k| k["is_available"].as_bool().unwrap_or(false))
                            .count();
                        let total = arr.len();
                        let worst = arr
                            .iter()
                            .map(|k| k["health"].as_str().unwrap_or("unknown"))
                            .fold("healthy", |acc, h| match (acc, h) {
                                (_, "error") | ("error", _) => "error",
                                (_, "rate_limited") | ("rate_limited", _) => "rate_limited",
                                (_, "unknown") | ("unknown", _) => "unknown",
                                _ => "healthy",
                            });
                        println!(
                            "  {} — {} ({}/{} keys available)",
                            provider.bold(),
                            crate::output::status_color(worst),
                            available,
                            total
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

fn cmd_usage(context: &ReplContext) {
    let u = &context.session_usage;
    println!("{}", "Session usage:".bold());
    println!("  Requests: {}", u.request_count);
    println!(
        "  Tokens in:  {}",
        chat_stream::format_token_count(u.total_tokens_in)
    );
    println!(
        "  Tokens out: {}",
        chat_stream::format_token_count(u.total_tokens_out)
    );
    println!("  Total time: {}ms", u.total_duration_ms);
}

fn cmd_clear() {
    print!("\x1B[2J\x1B[1;1H");
    std::io::Write::flush(&mut std::io::stdout()).ok();
}

fn cmd_verbose(context: &mut ReplContext) {
    context.verbose = !context.verbose;
    if context.verbose {
        println!("Verbose mode: {}", "on".green());
    } else {
        println!("Verbose mode: {}", "off".dimmed());
    }
}

#[cfg(test)]
mod tests;
