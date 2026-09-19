//! Shared chat streaming — SSE send + stream + event parsing
//!
//! Used by REPL (interactive mode), single-message mode, and pipe mode.

use anyhow::Result;
use colored::Colorize;
use futures_util::StreamExt;
use openalpaca_core::gateway::DelegationInfo;
use serde::Deserialize;
use std::io::{IsTerminal, Write};

use crate::client::DaemonClient;

#[derive(Debug, Deserialize)]
pub struct ChatSendResponse {
    pub stream_id: String,
    #[allow(dead_code)]
    pub lane_key: String,
}

pub struct StreamOptions {
    pub verbose: bool,
    /// Whether **stdout is a terminal**, which is what decides how a turn is
    /// shown (S2, S13).
    ///
    /// A terminal is a live view: the model's tokens are printed as they
    /// arrive and its reasoning runs dim beside them. A pipe is somebody
    /// else's input: it gets the authoritative answer once, at `done`, and
    /// nothing else — deltas cannot be taken back, and since real streaming
    /// landed they no longer concatenate to `done.content` (a multi-round turn
    /// streams text before a tool call; a stream that broke mid-way is
    /// answered by the non-streaming fallback).
    pub tty: bool,
}

impl Default for StreamOptions {
    /// Not `derive(Default)`: `false` would mean "no terminal", and every
    /// caller that writes `&Default::default()` is a real invocation whose
    /// stdout can be asked.
    fn default() -> Self {
        Self {
            verbose: false,
            tty: std::io::stdout().is_terminal(),
        }
    }
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
    /// The turn did not produce an answer: the daemon sent an `error` event, or
    /// the stream broke before one arrived (L12).
    ///
    /// Carried rather than printed here, because the two callers want opposite
    /// things from it. A one-shot (`--message`, or a pipe) must **exit
    /// non-zero** — a script that cannot tell a failed turn from an answered
    /// one is the bug this closes — while the REPL prints it and keeps the
    /// prompt open. Whoever handles it says so; nothing is swallowed.
    Failed { message: String },
}

impl StreamResult {
    pub fn usage(&self) -> Option<&UsageInfo> {
        match self {
            StreamResult::Response(u) => u.as_ref(),
            StreamResult::Delegation { usage, .. } => usage.as_ref(),
            StreamResult::Failed { .. } => None,
        }
    }

    /// The failure this turn ended in, if it ended in one.
    pub fn failure(&self) -> Option<&str> {
        match self {
            StreamResult::Failed { message } => Some(message),
            _ => None,
        }
    }
}

/// Percent-encode a path for a header value (R81) — non-ASCII, the control
/// range, and `%` itself, nothing else.
///
/// Not `urlencoding::encode`: that escapes `/` too, which would turn every
/// ordinary path in a request log into `%2FUsers%2F…` for no gain. What has to
/// be escaped is what a header value cannot carry; `%` goes with it so that a
/// directory literally named `50%20off` does not decode into `50 off` at the
/// other end.
fn encode_header_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte == b'%' || !(0x20..=0x7e).contains(&byte) {
            encoded.push_str(&format!("%{byte:02X}"));
        } else {
            encoded.push(byte as char);
        }
    }
    encoded
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
    unattended: bool,
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
            unattended: false,
        }
    }

    /// Address a stored conversation from here on.
    pub fn resuming(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Declare that nobody will be here to answer a tool-approval prompt (M6,
    /// scoped by S10).
    ///
    /// Set when this process has no terminal on stdin or on stdout
    /// (`crate::unattended`): a redirected run raises its confirmations into a
    /// pipe nobody reads, and the run sits on each one until the 300 s timeout
    /// — five and a half minutes per tool call, which is what the acceptance
    /// run measured. A `--message` typed at a prompt is *not* this: the
    /// confirmation arrives on the stream it is still reading and it asks
    /// inline, which is the behaviour S10 gave back.
    ///
    /// This is a **declaration, not an approval**: the daemon refuses a
    /// confirm-listed tool at once and tells the model where it *can* be
    /// approved. Nothing is auto-allowed, here or there.
    pub fn unattended(mut self) -> Self {
        self.unattended = true;
        self
    }

    /// The extra request headers this turn carries. Empty is a real answer.
    ///
    /// The path is **percent-encoded UTF-8** (ruling R81): a header value is
    /// bytes, and the daemon's read (`HeaderValue::to_str`) refuses anything
    /// above `\x7f`, so a CJK project directory — an ordinary path — used to be
    /// dropped in transit and the turn ran with no project at all. Only what
    /// must be escaped is: a plain ASCII path is its own encoding.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        match &self.workspace_path {
            Some(path) => vec![("x-workspace-path", encode_header_path(path))],
            None => Vec::new(),
        }
    }

    /// The `POST /v1/chat` body. Absent fields are absent, not null: the
    /// daemon's `session_id` is `Option`, `attachments` defaults to empty, and
    /// `unattended` defaults to false — a client that says nothing is taken to
    /// be one that can answer, which is exactly today's behaviour.
    pub fn body(&self, content: &str, attachments: &[serde_json::Value]) -> serde_json::Value {
        let mut body = serde_json::json!({ "content": content });
        if !attachments.is_empty() {
            body["attachments"] = serde_json::json!(attachments);
        }
        if let Some(session_id) = &self.session_id {
            body["session_id"] = serde_json::json!(session_id);
        }
        if self.unattended {
            body["unattended"] = serde_json::json!(true);
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
#[derive(Default)]
struct SseState {
    usage: Option<UsageInfo>,
    /// The answer text this process has already **printed** from `delta`
    /// frames — empty on a pipe, which prints none (S13).
    shown: String,
    /// A dim reasoning run is open on the current line, so the answer owes it
    /// a newline before it starts (S2).
    reasoning_open: bool,
    /// Structured delegation metadata from the done event, if the server
    /// delegated the message to a background task.
    delegation: Option<DelegationInfo>,
    /// The `error` event's message, when one arrived — the turn failed and the
    /// caller decides what that costs (L12).
    failure: Option<String>,
    /// A terminal frame has been handled: the turn is over and nothing else is
    /// coming (V7).
    ///
    /// The daemon keeps the SSE open for five seconds after the last frame so
    /// a late subscriber can still read it (`ChatService::send_message` →
    /// `sleep(Duration::from_secs(5))` → `stream_manager.remove`), and the
    /// stream ends only when that sender is dropped. A reader that waits for
    /// the socket to close therefore pays those five seconds on **every**
    /// turn — the tail a one-shot `chat --message` printed its answer and then
    /// sat through. The turn is over when its terminal frame is over, so the
    /// reader stops there.
    finished: bool,
}

/// What `done` still owes the reader, given what the stream already printed
/// (S13).
///
/// Before real streaming the deltas always concatenated to `done.content`, so
/// "print `done.content` when no delta arrived" was a complete rule. It is not
/// any more: a multi-round turn streams the text the model wrote *before* a
/// tool call, and a stream that failed mid-way is answered by the
/// non-streaming fallback — in both cases the terminal held a prefix, or
/// something else entirely, and never the answer.
#[derive(Debug, PartialEq, Eq)]
enum Reconciliation {
    /// The reader already has the whole answer, exactly once.
    Nothing,
    /// Print this tail: what was shown is a strict prefix of the answer.
    Append(String),
    /// Print the whole answer on a fresh line: what was shown diverged from
    /// it. The narration above stays — it is an honest record of the turn —
    /// and the answer itself still appears exactly once.
    Redraw(String),
}

/// Reconcile the authoritative `done.content` against what was printed.
fn reconcile(shown: &str, content: &str) -> Reconciliation {
    if content.is_empty() || shown == content {
        // A delegation's `done` carries no content, and a turn whose stream
        // was complete owes nothing.
        return Reconciliation::Nothing;
    }
    match content.strip_prefix(shown) {
        // `shown` empty (a pipe, or a turn that streamed nothing) lands here
        // too, and prints the answer once with no leading blank line.
        Some(tail) => Reconciliation::Append(tail.to_string()),
        None => Reconciliation::Redraw(content.to_string()),
    }
}

/// What a `delta` frame prints.
///
/// A terminal watches the model type. A pipe waits: its bytes are somebody
/// else's input, and it must contain the final answer once and nothing else —
/// which, now that deltas no longer concatenate to `done.content`, is only
/// true if nothing is written before `done` (S13).
fn delta_to_print(tty: bool, content: &str) -> Option<&str> {
    (tty && !content.is_empty()).then_some(content)
}

/// What a `reasoning` frame prints (S2).
///
/// Dim, on a terminal, as it arrives — it is the 13 s a thinking model spends
/// before its first token, and printing nothing made that look like a hang.
/// Never on a pipe: reasoning is not the answer, and a script must not have to
/// tell them apart.
fn reasoning_to_print(tty: bool, text: &str) -> Option<&str> {
    (tty && !text.is_empty()).then_some(text)
}

async fn stream_sse_events(
    response: reqwest::Response,
    opts: &StreamOptions,
    client: &DaemonClient,
) -> Result<StreamResult> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut state = SseState::default();

    'stream: loop {
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
                                process_sse_event(&event_text, opts, &mut state)?;
                            }
                            // V7: the turn ended on this frame. Reading on
                            // would only wait out the daemon's five-second
                            // late-subscriber window — and nothing is
                            // auto-approved by leaving: a confirmation is
                            // raised *before* `done`, and an unanswered one
                            // still times out on the daemon as a refusal.
                            if state.finished {
                                break 'stream;
                            }
                        }
                    }
                    // The connection broke mid-turn: no answer arrived and none
                    // is coming. Recorded as a failure rather than printed here
                    // so it reaches the exit code too (L12).
                    Some(Err(e)) => {
                        state.failure.get_or_insert_with(|| format!("stream error: {e}"));
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

    // A failure outranks both: a turn that ended in an `error` event delegated
    // nothing and answered nothing, whatever else was on the wire.
    if let Some(message) = state.failure {
        return Ok(StreamResult::Failed { message });
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

/// Close an open reasoning run so the answer starts on its own line.
fn end_reasoning(state: &mut SseState) -> Result<()> {
    if !state.reasoning_open {
        return Ok(());
    }
    state.reasoning_open = false;
    println!();
    std::io::stdout().flush()?;
    Ok(())
}

fn process_sse_event(event_text: &str, opts: &StreamOptions, state: &mut SseState) -> Result<()> {
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
            if opts.verbose {
                print!("{}", "Thinking...".dimmed());
                std::io::stdout().flush()?;
                print!("\r            \r");
                std::io::stdout().flush()?;
            }
        }
        // S2: the model's own reasoning, live. Never recorded in `shown` — it
        // is not part of the answer and `done.content` never carries it.
        "reasoning" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(text) = parsed["text"].as_str()
                && let Some(visible) = reasoning_to_print(opts.tty, text)
            {
                print!("{}", visible.dimmed());
                std::io::stdout().flush()?;
                state.reasoning_open = true;
            }
        }
        "delta" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(content) = parsed["content"].as_str()
                && let Some(visible) = delta_to_print(opts.tty, content)
            {
                end_reasoning(state)?;
                print!("{}", visible);
                std::io::stdout().flush()?;
                state.shown.push_str(visible);
            }
        }
        // Terminal (V7): `done` is the last frame of an answered turn — the
        // service sends it or `error`, never both, and then nothing.
        "done" => {
            state.finished = true;
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data) {
                // S13: the terminal ends on the authoritative answer, exactly
                // once, whatever the deltas showed.
                if let Some(content) = parsed["content"].as_str() {
                    match reconcile(&state.shown, content) {
                        Reconciliation::Nothing => end_reasoning(state)?,
                        Reconciliation::Append(tail) => {
                            end_reasoning(state)?;
                            print!("{}", tail);
                            state.shown.push_str(&tail);
                        }
                        Reconciliation::Redraw(answer) => {
                            end_reasoning(state)?;
                            println!();
                            print!("{}", answer);
                            state.shown = answer;
                        }
                    }
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
        // Recorded, not printed: this went to **stdout** before, which put the
        // daemon's error text into whatever a pipe was feeding, and the process
        // still exited 0. The caller prints it — on stderr — and a one-shot
        // exits non-zero (L12). An event whose data will not parse is still a
        // failed turn, so it is never dropped on the floor.
        // Terminal too (V7): a turn that errored answered nothing and sends
        // no `done` afterwards.
        "error" => {
            state.finished = true;
            let msg = serde_json::from_str::<serde_json::Value>(&data)
                .ok()
                .and_then(|parsed| {
                    parsed["message"]
                        .as_str()
                        .map(|m| m.to_string())
                        .filter(|m| !m.is_empty())
                })
                .unwrap_or_else(|| "Unknown error".to_string());
            state.failure.get_or_insert(msg);
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
            // Terminal, but not a failure: the daemon went away mid-run
            // (§5.6b). Polling must stop — the run will never move again — and
            // the restart verb is `rerun`, not `start` (R43).
            "interrupted" => {
                let summary = task["result_summary"].as_str().unwrap_or("");
                println!("{}", "[Task interrupted — the daemon restarted]".yellow());
                if !summary.is_empty() {
                    println!("{} {}", "Detail:".yellow(), summary);
                }
                println!(
                    "{}",
                    "Nothing was lost — re-run it to start the same goal again.".dimmed()
                );
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
