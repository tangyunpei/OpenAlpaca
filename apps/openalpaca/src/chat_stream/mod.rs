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

/// Where a CLI turn goes: which project it belongs to, and which conversation.
///
/// **The project** is the CLI's own working directory, sent as
/// `x-workspace-path` exactly as the GUI's window sends its picker's path —
/// §4.7's client story, whose CLI half was missing. The daemon resolves it to a
/// project root itself (R22, `MemoryScopeContext::for_request`), so this must
/// be an absolute path or nothing at all: a relative one would be resolved
/// against the *daemon's* directory, which is the bug that ruling closed.
///
/// **The conversation** is named only by `--resume` / `--session`. An unnamed
/// turn lands in the lane's active session, which is what lets the daemon open
/// a new one when the project changed (R48). A named one addresses that
/// conversation and, by R49, **takes its project** — the header is still sent
/// because a session with no binding of its own takes it as a first binding.
#[derive(Debug, Clone, Default)]
pub struct ChatTarget {
    workspace_path: Option<String>,
    session_id: Option<String>,
}

impl ChatTarget {
    /// The CLI's working directory, canonicalized — or nothing when it cannot
    /// be resolved, which is honestly "this turn belongs to no project".
    pub fn for_cwd() -> Self {
        let workspace_path = std::env::current_dir()
            .and_then(|dir| dir.canonicalize())
            .ok()
            .map(|dir| dir.to_string_lossy().to_string());
        Self::for_workspace(workspace_path)
    }

    pub fn for_workspace(workspace_path: Option<String>) -> Self {
        Self {
            workspace_path,
            session_id: None,
        }
    }

    /// Address a stored conversation from here on.
    pub fn resuming(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// The extra request headers this turn carries. Empty is a real answer.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        match &self.workspace_path {
            Some(path) => vec![("x-workspace-path", path.clone())],
            None => Vec::new(),
        }
    }

    /// The `POST /v1/chat` body. Absent fields are absent, not null: the
    /// daemon's `session_id` is `Option`, and `attachments` defaults to empty.
    pub fn body(&self, content: &str, attachments: &[serde_json::Value]) -> serde_json::Value {
        let mut body = serde_json::json!({ "content": content });
        if !attachments.is_empty() {
            body["attachments"] = serde_json::json!(attachments);
        }
        if let Some(session_id) = &self.session_id {
            body["session_id"] = serde_json::json!(session_id);
        }
        body
    }
}

/// POST /v1/chat → ChatSendResponse { stream_id, lane_key }
pub async fn send_chat(
    client: &DaemonClient,
    content: &str,
    target: &ChatTarget,
) -> Result<ChatSendResponse> {
    send_chat_with_attachments(client, content, &[], target).await
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
    target: &ChatTarget,
) -> Result<ChatSendResponse> {
    let body = target.body(content, attachments);
    let headers = target.headers();
    client.post_with_headers("/v1/chat", &body, &headers).await
}

/// Convenience: send_chat_with_attachments + stream_chat
pub async fn send_and_stream_with_attachments(
    client: &DaemonClient,
    content: &str,
    attachments: &[serde_json::Value],
    target: &ChatTarget,
    opts: &StreamOptions,
) -> Result<StreamResult> {
    let resp = send_chat_with_attachments(client, content, attachments, target).await?;
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

        // GET /v1/tasks/{id} returns { "task": {...}, "outcome": {...}? }
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
