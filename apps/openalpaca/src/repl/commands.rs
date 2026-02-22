//! Slash command parsing, dispatch, and handlers

use crate::chat_stream;
use crate::client::DaemonClient;
use colored::Colorize;

use super::ReplContext;

pub enum SlashCommand {
    Help,
    Status,
    Model,
    Models,
    Agents,
    Tasks(usize),
    Keys,
    Usage,
    Clear,
    Verbose,
    Unknown(String),
}

impl SlashCommand {
    pub fn parse(input: &str) -> Self {
        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd = parts[0];
        let arg = parts.get(1).map(|s| s.trim());
        match cmd {
            "/help" => Self::Help,
            "/status" => Self::Status,
            "/model" => Self::Model,
            "/models" => Self::Models,
            "/agents" => Self::Agents,
            "/tasks" => Self::Tasks(arg.and_then(|a| a.parse().ok()).unwrap_or(5)),
            "/keys" => Self::Keys,
            "/usage" => Self::Usage,
            "/clear" => Self::Clear,
            "/verbose" => Self::Verbose,
            other => Self::Unknown(other.to_string()),
        }
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
        SlashCommand::Status => cmd_status(client).await,
        SlashCommand::Model => cmd_model(client).await,
        SlashCommand::Models => cmd_models(client).await,
        SlashCommand::Agents => cmd_agents(client).await,
        SlashCommand::Tasks(n) => cmd_tasks(client, n).await,
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
        SlashCommand::Unknown(c) => {
            println!(
                "{} Unknown command: {}. Type {}",
                "?".yellow(),
                c,
                "/help".bold()
            );
            Ok(())
        }
    }
}

fn print_help() {
    println!("{}", "Commands:".bold());
    println!("  {} — Active tasks summary", "/status".bold());
    println!("  {} — Current model info", "/model".bold());
    println!("  {} — List available models", "/models".bold());
    println!("  {} — List agents", "/agents".bold());
    println!("  {} — Recent tasks (default 5)", "/tasks [n]".bold());
    println!("  {} — API key health per provider", "/keys".bold());
    println!("  {} — Session token usage", "/usage".bold());
    println!("  {} — Clear terminal", "/clear".bold());
    println!("  {} — Toggle thinking indicator", "/verbose".bold());
    println!("  {} — Show this help", "/help".bold());
    println!();
    println!("  {} / {} — Exit chat", "exit".bold(), "quit".bold());
}

async fn cmd_status(client: &DaemonClient) -> anyhow::Result<()> {
    let tasks: serde_json::Value = client.get("/v1/tasks?limit=5&status=active").await?;
    if let Some(arr) = tasks.as_array() {
        if arr.is_empty() {
            println!("{}", "No active tasks.".dimmed());
        } else {
            for t in arr {
                let id = t["id"].as_str().unwrap_or("-");
                let title = t["title"].as_str().unwrap_or("-");
                let status = t["status"].as_str().unwrap_or("-");
                let short_id = &id[..8.min(id.len())];
                println!(
                    "  {} {} - {}",
                    short_id.dimmed(),
                    title,
                    crate::output::status_color(status)
                );
            }
        }
    }
    Ok(())
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

async fn cmd_tasks(client: &DaemonClient, n: usize) -> anyhow::Result<()> {
    let path = format!("/v1/tasks?limit={}", n);
    let tasks: serde_json::Value = client.get(&path).await?;
    if let Some(arr) = tasks.as_array() {
        if arr.is_empty() {
            println!("{}", "No tasks.".dimmed());
        } else {
            for t in arr {
                let id = t["id"].as_str().unwrap_or("-");
                let title = t["title"].as_str().unwrap_or("-");
                let status = t["status"].as_str().unwrap_or("-");
                let short_id = &id[..8.min(id.len())];
                println!(
                    "  {} {} - {}",
                    short_id.dimmed(),
                    title,
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
mod tests {
    use super::*;

    #[test]
    fn test_parse_help() {
        assert!(matches!(SlashCommand::parse("/help"), SlashCommand::Help));
    }

    #[test]
    fn test_parse_status() {
        assert!(matches!(
            SlashCommand::parse("/status"),
            SlashCommand::Status
        ));
    }

    #[test]
    fn test_parse_model() {
        assert!(matches!(SlashCommand::parse("/model"), SlashCommand::Model));
    }

    #[test]
    fn test_parse_models() {
        assert!(matches!(
            SlashCommand::parse("/models"),
            SlashCommand::Models
        ));
    }

    #[test]
    fn test_parse_agents() {
        assert!(matches!(
            SlashCommand::parse("/agents"),
            SlashCommand::Agents
        ));
    }

    #[test]
    fn test_parse_tasks_default() {
        match SlashCommand::parse("/tasks") {
            SlashCommand::Tasks(n) => assert_eq!(n, 5),
            _ => panic!("expected Tasks"),
        }
    }

    #[test]
    fn test_parse_tasks_with_arg() {
        match SlashCommand::parse("/tasks 10") {
            SlashCommand::Tasks(n) => assert_eq!(n, 10),
            _ => panic!("expected Tasks"),
        }
    }

    #[test]
    fn test_parse_tasks_invalid_arg() {
        match SlashCommand::parse("/tasks abc") {
            SlashCommand::Tasks(n) => assert_eq!(n, 5),
            _ => panic!("expected Tasks"),
        }
    }

    #[test]
    fn test_parse_keys() {
        assert!(matches!(SlashCommand::parse("/keys"), SlashCommand::Keys));
    }

    #[test]
    fn test_parse_usage() {
        assert!(matches!(SlashCommand::parse("/usage"), SlashCommand::Usage));
    }

    #[test]
    fn test_parse_clear() {
        assert!(matches!(SlashCommand::parse("/clear"), SlashCommand::Clear));
    }

    #[test]
    fn test_parse_verbose() {
        assert!(matches!(
            SlashCommand::parse("/verbose"),
            SlashCommand::Verbose
        ));
    }

    #[test]
    fn test_parse_unknown() {
        match SlashCommand::parse("/foo") {
            SlashCommand::Unknown(c) => assert_eq!(c, "/foo"),
            _ => panic!("expected Unknown"),
        }
    }
}
