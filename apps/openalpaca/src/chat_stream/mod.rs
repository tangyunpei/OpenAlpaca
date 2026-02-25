//! Shared chat streaming — SSE send + stream + event parsing
//!
//! Used by REPL (interactive mode), single-message mode, and pipe mode.

use anyhow::Result;
use colored::Colorize;
use futures_util::StreamExt;
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
        task_title: String,
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
    stream_sse_events(http_resp, opts).await
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
    /// Accumulated content from done event (used for delegation detection)
    done_content: Option<String>,
}

async fn stream_sse_events(
    response: reqwest::Response,
    opts: &StreamOptions,
) -> Result<StreamResult> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut state = SseState {
        usage: None,
        had_delta: false,
        done_content: None,
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
                            process_sse_event(&event_text, opts.verbose, &mut state)?;
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

    // Check for delegation: zero tokens AND content contains delegation marker
    if let Some(ref content) = state.done_content
        && is_delegation(&state.usage, content)
        && let Some(title) = parse_task_title(content)
    {
        return Ok(StreamResult::Delegation {
            usage: state.usage,
            task_title: title,
        });
    }

    Ok(StreamResult::Response(state.usage))
}

/// Detect delegation: tokens_in == 0 && tokens_out == 0 AND content contains marker
fn is_delegation(usage: &Option<UsageInfo>, content: &str) -> bool {
    let zero_tokens = match usage {
        Some(u) => u.tokens_in == 0 && u.tokens_out == 0,
        None => true,
    };
    zero_tokens && content.contains("I've created a task")
}

/// Parse task title from delegation message.
/// Expected format: "...Task: {title}\nYou'll see..."
fn parse_task_title(content: &str) -> Option<String> {
    // Look for "Task: <title>" pattern
    let marker = "Task: ";
    let start = content.find(marker)?;
    let rest = &content[start + marker.len()..];
    // Title ends at newline or end of string
    let end = rest.find('\n').unwrap_or(rest.len());
    let title = rest[..end].trim().to_string();
    if title.is_empty() { None } else { Some(title) }
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

                // Capture content for delegation detection
                if let Some(content) = parsed["content"].as_str() {
                    state.done_content = Some(content.to_string());
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
/// Fetches tasks by title, then polls the matched task until it reaches
/// a terminal status. Prints the result summary when done.
pub async fn poll_task_completion(client: &DaemonClient, task_title: &str) -> Result<()> {
    // Find the task by title
    let tasks: serde_json::Value = client.get("/v1/tasks?limit=5").await?;
    let task_id = tasks.as_array().and_then(|arr| {
        arr.iter().find_map(|t| {
            let title = t["title"].as_str().unwrap_or("");
            if title == task_title {
                t["id"].as_str().map(|s| s.to_string())
            } else {
                None
            }
        })
    });

    let task_id = match task_id {
        Some(id) => id,
        None => return Ok(()), // Task not found — don't crash
    };

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
                // Print per-assignment results if available
                if let Some(assignments) = resp["assignments"].as_array() {
                    for a in assignments {
                        if let Some(output) = a["result_output"].as_str()
                            && !output.is_empty()
                        {
                            println!("{}", output);
                        }
                    }
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
