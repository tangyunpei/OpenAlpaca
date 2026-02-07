//! Chat command — SSE streaming + interactive REPL + pipe mode

use anyhow::Result;
use clap::Args;
use colored::Colorize;
use futures_util::StreamExt;
use serde::Deserialize;
use std::io::Write;

use crate::client::DaemonClient;

#[derive(Args)]
pub struct ChatArgs {
    /// Send a single message (non-interactive)
    #[arg(long)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatSendResponse {
    stream_id: String,
    #[allow(dead_code)]
    lane_key: String,
}

pub async fn run(args: ChatArgs) -> Result<()> {
    if let Some(ref msg) = args.message {
        // Single message mode
        single_message(msg).await
    } else if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        // Interactive REPL
        interactive_repl().await
    } else {
        // Pipe mode: read all stdin, send as one message
        pipe_mode().await
    }
}

async fn single_message(content: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    send_and_stream(&client, content).await
}

async fn pipe_mode() -> Result<()> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }
    let client = DaemonClient::connect()?;
    send_and_stream(&client, input).await
}

async fn interactive_repl() -> Result<()> {
    let client = DaemonClient::connect()?;

    println!(
        "{} {} {}",
        "OpenAlpaca Chat".bold(),
        "|".dimmed(),
        "Type 'exit' or 'quit' to leave, /help for commands".dimmed()
    );
    println!();

    loop {
        // Print prompt
        print!("{} ", ">".green().bold());
        std::io::stdout().flush()?;

        // Read line
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => break, // EOF (Ctrl+D)
            Ok(_) => {}
            Err(_) => break,
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Handle special commands
        match line {
            "exit" | "quit" => break,
            "/help" => {
                println!("{}", "Commands:".dimmed());
                println!("  {} - Quick task summary", "/status".bold());
                println!("  {} - Show this help", "/help".bold());
                println!("  {} - Exit chat", "exit".bold());
                continue;
            }
            "/status" => {
                match client
                    .get::<serde_json::Value>("/v1/tasks?limit=5&status=active")
                    .await
                {
                    Ok(tasks) => {
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
                    }
                    Err(e) => {
                        println!("{} {}", "Error:".red(), e);
                    }
                }
                continue;
            }
            _ => {}
        }

        // Send message and stream response
        print!("{} ", "Alpaca:".cyan().bold());
        std::io::stdout().flush()?;

        if let Err(e) = send_and_stream(&client, line).await {
            println!();
            println!("{} {}", "Error:".red(), e);
        }
        println!();
    }

    Ok(())
}

async fn send_and_stream(client: &DaemonClient, content: &str) -> Result<()> {
    // 1. POST /v1/chat → get stream_id
    let body = serde_json::json!({ "content": content });
    let resp: ChatSendResponse = client.post("/v1/chat", &body).await?;

    // 2. GET /v1/chat/stream/{stream_id}?token=... → SSE
    let path = format!(
        "/v1/chat/stream/{}?token={}",
        resp.stream_id,
        urlencoding::encode(client.token())
    );
    let http_resp = client.get_sse_stream(&path).await?;

    // 3. Parse SSE events from byte stream
    let mut stream = http_resp.bytes_stream();
    let mut buffer = String::new();

    loop {
        tokio::select! {
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        // Process complete SSE events (delimited by \n\n)
                        while let Some(pos) = buffer.find("\n\n") {
                            let event_text = buffer[..pos].to_string();
                            buffer = buffer[pos + 2..].to_string();
                            process_sse_event(&event_text)?;
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("\n{} Stream error: {}", "✗".red(), e);
                        break;
                    }
                    None => break, // Stream ended
                }
            }
            _ = tokio::signal::ctrl_c() => {
                // Ctrl+C during streaming: abort gracefully
                println!();
                println!("{}", "(interrupted)".dimmed());
                break;
            }
        }
    }

    Ok(())
}

fn process_sse_event(event_text: &str) -> Result<()> {
    let mut event_type = String::new();
    let mut data = String::new();

    for line in event_text.lines() {
        if let Some(val) = line.strip_prefix("event:") {
            event_type = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("data:") {
            data = val.trim().to_string();
        }
    }

    match event_type.as_str() {
        "thinking" => {
            print!("{}", "Thinking...".dimmed());
            std::io::stdout().flush()?;
            // Clear "Thinking..." when delta arrives by using \r
            print!("\r            \r");
            std::io::stdout().flush()?;
        }
        "delta" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(content) = parsed["content"].as_str() {
                    print!("{}", content);
                    std::io::stdout().flush()?;
                }
            }
        }
        "done" => {
            println!();
            // Optionally show metadata
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                let model = parsed["model"].as_str().unwrap_or("");
                let tokens_in = parsed["tokens_in"].as_i64().unwrap_or(0);
                let tokens_out = parsed["tokens_out"].as_i64().unwrap_or(0);
                let duration_ms = parsed["duration_ms"].as_i64().unwrap_or(0);
                if !model.is_empty() {
                    println!(
                        "{}",
                        format!(
                            "[{} | {}/{} tokens | {}ms]",
                            model, tokens_in, tokens_out, duration_ms
                        )
                        .dimmed()
                    );
                }
            }
        }
        "error" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                let msg = parsed["message"].as_str().unwrap_or("Unknown error");
                println!();
                println!("{} {}", "Error:".red(), msg);
            }
        }
        _ => {
            // Ignore unknown event types (keep-alive comments, etc.)
        }
    }

    Ok(())
}
