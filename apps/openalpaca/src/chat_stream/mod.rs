//! Shared chat streaming — SSE send + stream + event parsing
//!
//! Used by REPL (interactive mode), single-message mode, and pipe mode.

use anyhow::Result;
use colored::Colorize;
use futures_util::StreamExt;
use openalpaca_core::gateway::DelegationInfo;
use serde::Deserialize;
use std::io::Write;

use crate::client::DaemonClient;

#[derive(Debug, Deserialize)]
pub struct ChatSendResponse {
    pub stream_id: String,
    #[allow(dead_code)]
    pub lane_key: String,
}

#[derive(Default)]
pub struct StreamOptions {
    pub verbose: bool,
}

pub struct UsageInfo {
    pub model: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub duration_ms: u64,
}

/// Result of streaming a chat response.
pub enum StreamResult {
    /// Normal response with optional usage info.
    Response(Option<UsageInfo>),
    /// Server delegated to agents — content was printed, poll for results.
    Delegation {
        usage: Option<UsageInfo>,
        delegation: DelegationInfo,
    },
}

impl StreamResult {
    pub fn usage(&self) -> Option<&UsageInfo> {
        match self {
            StreamResult::Response(u) => u.as_ref(),
            StreamResult::Delegation { usage, .. } => usage.as_ref(),
        }
    }
}

/// POST /v1/chat → ChatSendResponse { stream_id, lane_key }
pub async fn send_chat(client: &DaemonClient, content: &str) -> Result<ChatSendResponse> {
    let body = serde_json::json!({ "content": content });
    client.post("/v1/chat", &body).await
}

/// GET /v1/chat/stream/{stream_id}?token=... → parse SSE → render → StreamResult
pub async fn stream_chat(
    client: &DaemonClient,
    stream_id: &str,
    opts: &StreamOptions,
) -> Result<StreamResult> {
    let path = format!(
        "/v1/chat/stream/{}?token={}",
        stream_id,
        urlencoding::encode(client.token())
    );
    let http_resp = client.get_sse_stream(&path).await?;
    stream_sse_events(http_resp, opts, client).await
}

/// POST /v1/chat with attachments → ChatSendResponse
pub async fn send_chat_with_attachments(
    client: &DaemonClient,
    content: &str,
    attachments: &[serde_json::Value],
) -> Result<ChatSendResponse> {
    let body = serde_json::json!({
        "content": content,
        "attachments": attachments,
    });
    client.post("/v1/chat", &body).await
}

/// Convenience: send_chat + stream_chat (for non-REPL modes)
pub async fn send_and_stream(
    client: &DaemonClient,
    content: &str,
    opts: &StreamOptions,
) -> Result<StreamResult> {
    let resp = send_chat(client, content).await?;
    stream_chat(client, &resp.stream_id, opts).await
}

/// Convenience: send_chat_with_attachments + stream_chat
pub async fn send_and_stream_with_attachments(
    client: &DaemonClient,
    content: &str,
    attachments: &[serde_json::Value],
    opts: &StreamOptions,
) -> Result<StreamResult> {
    let resp = if attachments.is_empty() {
        send_chat(client, content).await?
    } else {
        send_chat_with_attachments(client, content, attachments).await?
    };
    stream_chat(client, &resp.stream_id, opts).await
}

/// Internal state accumulated during SSE event processing.
struct SseState {
    usage: Option<UsageInfo>,
    had_delta: bool,
    /// Structured delegation metadata from the done event, if the server
    /// delegated the message to a background task.
    delegation: Option<DelegationInfo>,
}

async fn stream_sse_events(
    response: reqwest::Response,
    opts: &StreamOptions,
    client: &DaemonClient,
) -> Result<StreamResult> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut state = SseState {
        usage: None,
        had_delta: false,
        delegation: None,
    };

    loop {
        tokio::select! {
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(pos) = find_event_boundary(&buffer) {
                            let event_text = buffer[..pos].to_string();
                            let skip = if buffer[pos..].starts_with("\r\n\r\n") { 4 } else { 2 };
                            buffer = buffer[pos + skip..].to_string();
                            if is_confirmation_event(&event_text) {
                                handle_confirmation_prompt(client, &event_text).await?;
                            } else {
                                process_sse_event(&event_text, opts.verbose, &mut state)?;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("\n{}", format!("Stream error: {}", e).red());
                        break;
                    }
                    None => break,
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n{}", "(interrupted)".dimmed());
                break;
            }
        }
    }

    if let Some(delegation) = state.delegation {
        return Ok(StreamResult::Delegation {
            usage: state.usage,
            delegation,
        });
    }

    Ok(StreamResult::Response(state.usage))
}

/// Check if an SSE event is a tool confirmation request.
fn is_confirmation_event(event_text: &str) -> bool {
    event_text
        .lines()
        .any(|line| line.strip_prefix("event:").map(|v| v.trim()) == Some("confirmation_requested"))
}

/// Handle a tool confirmation prompt: show details, prompt Y/N, POST response.
///
/// Uses `block_in_place()` to read stdin synchronously. While blocked, the SSE
/// stream buffers incoming events (including additional confirmations). This is
/// fine for CLI — the user can only answer one prompt at a time, and the 300s
/// server timeout (configurable via `confirmation_timeout_secs`) is sufficient
/// for sequential multi-prompt scenarios.
async fn handle_confirmation_prompt(client: &DaemonClient, event_text: &str) -> Result<()> {
    let mut data = String::new();
    for line in event_text.lines() {
        if let Some(val) = line.strip_prefix("data:") {
            data = val.trim().to_string();
        }
    }
    let parsed: serde_json::Value = serde_json::from_str(&data)?;
    let request_id = parsed["request_id"].as_str().unwrap_or("");
    let tool_name = parsed["tool_name"].as_str().unwrap_or("");
    let args = &parsed["tool_arguments"];

    println!();
    println!(
        "{}",
        format!("Tool '{}' requires confirmation", tool_name)
            .yellow()
            .bold()
    );
    if !args.is_null() {
        println!(
            "{}",
            format!("Arguments: {}", serde_json::to_string_pretty(args).unwrap_or_default())
                .dimmed()
        );
    }

    let approved = tokio::task::block_in_place(|| {
        print!("Allow execution? [y/N] ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
    });

    let body = serde_json::json!({ "approved": approved });
    client
        .post_raw(
            &format!("/v1/chat/confirmations/{}", request_id),
            &body,
        )
        .await?;

    let status = if approved {
        "Approved".green()
    } else {
        "Denied".red()
    };
    println!("{}", status);
    Ok(())
}

fn find_event_boundary(buf: &str) -> Option<usize> {
    if let Some(pos) = buf.find("\r\n\r\n") {
        return Some(pos);
    }
    buf.find("\n\n")
}

fn process_sse_event(event_text: &str, verbose: bool, state: &mut SseState) -> Result<()> {
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
            if verbose {
                print!("{}", "Thinking...".dimmed());
                std::io::stdout().flush()?;
                print!("\r            \r");
                std::io::stdout().flush()?;
            }
        }
        "delta" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(content) = parsed["content"].as_str()
            {
                print!("{}", content);
                std::io::stdout().flush()?;
                state.had_delta = true;
            }
        }
        "done" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                // BUG FIX: Print done.content if no delta events printed it
                if !state.had_delta
                    && let Some(content) = parsed["content"].as_str()
                    && !content.is_empty()
                {
                    print!("{}", content);
                }

                // Capture structured delegation metadata if present
                if let Some(value) = parsed.get("delegation")
                    && let Ok(delegation) = serde_json::from_value::<DelegationInfo>(value.clone())
                {
                    state.delegation = Some(delegation);
                }

                println!();
                let model = parsed["model"].as_str().unwrap_or("").to_string();
                let tokens_in = parsed["tokens_in"].as_u64().unwrap_or(0);
                let tokens_out = parsed["tokens_out"].as_u64().unwrap_or(0);
                let duration_ms = parsed["duration_ms"].as_u64().unwrap_or(0);

                if !model.is_empty() {
                    let info = UsageInfo {
                        model: model.clone(),
                        tokens_in,
                        tokens_out,
                        duration_ms,
                    };
                    println!("{}", format_usage_line(&info).dimmed());
                    state.usage = Some(info);
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
        _ => {}
    }
    Ok(())
}

/// Poll for task completion after delegation.
///
/// Polls the task by id until it reaches a terminal status.
/// Prints the result summary when done.
pub async fn poll_task_completion(client: &DaemonClient, task_id: &str) -> Result<()> {
    eprintln!("{}", "Waiting for task to complete...".dimmed());

    let poll_interval = std::time::Duration::from_secs(2);
    let max_polls = 150; // 5 minutes at 2s intervals

    for _ in 0..max_polls {
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            _ = tokio::signal::ctrl_c() => {
                println!("\n{}", "(stopped waiting)".dimmed());
                return Ok(());
            }
        }

        let resp: serde_json::Value = match client.get(&format!("/v1/tasks/{}", task_id)).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{}", format!("(poll error: {}, retrying...)", e).dimmed());
                continue;
            }
        };

        // GET /v1/tasks/{id} returns { "task": {...}, "assignments": [...] }
        let task = &resp["task"];
        let status = task["status"].as_str().unwrap_or("");

        match status {
            "completed" => {
                let summary = task["result_summary"].as_str().unwrap_or("");
                // Calculate duration from created_at → completed_at
                let duration_str =
                    match (task["created_at"].as_str(), task["completed_at"].as_str()) {
                        (Some(start), Some(end)) => {
                            match (
                                chrono::DateTime::parse_from_rfc3339(start),
                                chrono::DateTime::parse_from_rfc3339(end),
                            ) {
                                (Ok(s), Ok(e)) => {
                                    let secs = (e - s).num_seconds().max(0);
                                    format!(" in {}s", secs)
                                }
                                _ => String::new(),
                            }
                        }
                        _ => String::new(),
                    };
                println!("{}", format!("[Task completed{}]", duration_str).green());
                if !summary.is_empty() {
                    println!("{} {}", "Result:".bold(), summary);
                }
                return Ok(());
            }
            "failed" | "cancelled" => {
                let summary = task["result_summary"].as_str().unwrap_or("");
                println!("{}", format!("[Task {}]", status).red());
                if !summary.is_empty() {
                    println!("{} {}", "Error:".red(), summary);
                }
                return Ok(());
            }
            _ => {
                // Still running — continue polling
            }
        }
    }

    eprintln!(
        "{}",
        "(stopped waiting — task still running, check /tasks)".yellow()
    );
    Ok(())
}

pub fn format_usage_line(info: &UsageInfo) -> String {
    format!(
        "[{} | {} in | {} out | {}ms]",
        info.model,
        format_token_count(info.tokens_in),
        format_token_count(info.tokens_out),
        info.duration_ms,
    )
}

pub fn format_token_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests;
