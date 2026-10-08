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
pub(crate) fn encode_header_path(path: &str) -> String {
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
    /// `workspace_path` is a root the daemon answered or recorded — `--project`'s
    /// answer, a resumed conversation's project — not a directory to resolve.
    resolved: bool,
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

    /// The root `--project` chose: what `POST /v1/workspaces` answered.
    pub fn for_project(root: String) -> Self {
        Self {
            resolved: true,
            ..Self::for_workspace(Some(root))
        }
    }

    pub fn for_workspace(workspace_path: Option<String>) -> Self {
        Self {
            workspace_path,
            resolved: false,
            session_id: None,
            unattended: false,
        }
    }

    /// The project path this turn carries as `x-workspace-path`, if any.
    pub fn workspace_path(&self) -> Option<&str> {
        self.workspace_path.as_deref()
    }

    /// The path this turn carries when it is already a project root the
    /// daemon answered or recorded ([`for_project`](Self::for_project),
    /// [`in_project`](Self::in_project)) — `None` for a directory that still
    /// needs resolving, such as [`for_cwd`](Self::for_cwd)'s.
    pub fn project_root(&self) -> Option<&str> {
        match self.resolved {
            true => self.workspace_path.as_deref(),
            false => None,
        }
    }

    /// Address a stored conversation from here on.
    pub fn resuming(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Carry `root` — a project root the daemon recorded — from here on, in
    /// place of the path this target was made with: a resumed conversation's
    /// project, where R49 runs its turns (R6).
    pub fn in_project(mut self, root: String) -> Self {
        self.workspace_path = Some(root);
        self.resolved = true;
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

impl Reconciliation {
    /// What `done` prints for the answer: the tail, or the whole answer on a
    /// fresh line — at a terminal with no control character in it
    /// ([`model_text`]), into a pipe exactly as the daemon sent it.
    fn text(&self, tty: bool) -> Option<String> {
        match self {
            Reconciliation::Nothing => None,
            Reconciliation::Append(tail) => Some(model_text(tty, tail).into_owned()),
            Reconciliation::Redraw(answer) => Some(format!("\n{}", model_text(tty, answer))),
        }
    }
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
/// true if nothing is written before `done` (S13). What a terminal is shown
/// is the model's text with no control character in it ([`model_text`]).
fn delta_to_print(tty: bool, content: &str) -> Option<String> {
    (tty && !content.is_empty()).then(|| model_text(tty, content).into_owned())
}

/// The model's prose as this process writes it: at a terminal, every control
/// character but `\n` and `\t` shown as `<U+XXXX>`
/// (`openalpaca_core::utils::text::escape_controls`); into a pipe, as the
/// daemon sent it.
///
/// A terminal acts on a control character instead of showing it, and the
/// state an ESC or a C1 CSI sets outlives the text: the model that wrote this
/// answer also writes the arguments of the tool call whose `[y/N]` prompt is
/// printed next, into the same terminal, and turning auto-wrap off first
/// would cut the argument that runs at the margin, with nothing hidden in it
/// for the prompt's notice to count. Display only: `SseState::shown` keeps
/// what the daemon sent, so `done` still reconciles against it.
///
/// Every text of the model's or a tool's that `openalpaca chat` prints goes
/// through here: the streamed answer and its reasoning, what `done` adds, a
/// delegated run's result ([`task_outcome_text`]), a resumed conversation's
/// turns (`commands::chat::render_tail`) and a failed turn's message
/// ([`failure_line`], `commands::chat::refuse_failed_turn`, by stderr's own
/// terminal) — and so does the provider's model id, a catalogue entry a
/// remote Ollama or a gateway names, on the usage line after every turn and
/// in the REPL's `/models`. So none of it can set a mode, a scroll region,
/// the cursor or a palette colour; the prompt's reset
/// (`crate::output::TERMINAL_RESET`) is for what other programs left in the
/// terminal.
pub(crate) fn model_text(tty: bool, text: &str) -> std::borrow::Cow<'_, str> {
    if tty {
        std::borrow::Cow::Owned(openalpaca_core::utils::text::escape_controls(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// The line the REPL prints on stderr for a turn that failed:
/// `Error: <message>`, the message through [`model_text`] for a stderr
/// that is a terminal.
///
/// The message is the daemon's `error` frame, and for a loop that ended in
/// an error it quotes the last tool's own error text — an MCP server's or a
/// plugin's words. The REPL prints it into the terminal the next turn's
/// inline prompt is printed into, where a scroll region or a palette change
/// in it would outlive it.
pub(crate) fn failure_line(message: &str, tty: bool) -> String {
    format!("{} {}", "Error:".red(), model_text(tty, message))
}

/// U3(d) — one line per attachment the daemon says never reached the model.
///
/// The turn answered; it just answered without that file, and the round-7
/// acceptance run showed what silence costs — the model guessed, and the only
/// clue was that the answer was wrong. Printed on **stderr**, under the
/// answer, for the same reason `Uploaded: …` is: a pipe's stdout is somebody
/// else's input and carries the answer alone (S13).
fn skipped_attachment_lines(done: &serde_json::Value) -> Vec<String> {
    let Some(skipped) = done.get("attachments_skipped").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    skipped
        .iter()
        .map(|entry| {
            let id = entry["id"].as_str().unwrap_or("(unknown)");
            match entry["reason"].as_str().filter(|r| !r.is_empty()) {
                Some(reason) => format!("Attachment {id} did not reach the model: {reason}"),
                None => format!("Attachment {id} did not reach the model"),
            }
        })
        .collect()
}

/// Design 11 §5 (owner decision CQ-7): the one line a terminal is shown when
/// the turn was given the previous conversation — `continuing from: <topic>`,
/// from `done.continued_from` — or `None`: into a pipe, which carries the
/// answer alone (S13), and on a turn that continued from nothing. The topic is
/// the working-state writer's text, so every hidden character in it is shown
/// as `<U+XXXX>` (`escape_hidden_line`); a previous conversation that left no
/// state is named as such.
fn continued_from_line(done: &serde_json::Value, tty: bool) -> Option<String> {
    if !tty {
        return None;
    }
    let from = done.get("continued_from").filter(|v| v.is_object())?;
    let topic = from["topic"].as_str().unwrap_or("");
    Some(if topic.is_empty() {
        "continuing from: the previous conversation".to_string()
    } else {
        format!(
            "continuing from: {}",
            openalpaca_core::utils::text::escape_hidden_line(topic).text
        )
    })
}

/// What a `reasoning` frame prints (S2).
///
/// Dim, on a terminal, as it arrives — it is the 13 s a thinking model spends
/// before its first token, and printing nothing made that look like a hang.
/// Never on a pipe: reasoning is not the answer, and a script must not have to
/// tell them apart. Like a delta, it reaches a terminal with no control
/// character in it ([`model_text`]).
fn reasoning_to_print(tty: bool, text: &str) -> Option<String> {
    (tty && !text.is_empty()).then(|| model_text(tty, text).into_owned())
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
                                handle_confirmation_prompt(client, &event_text, opts.tty).await?;
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

/// What the inline `[y/N]` prompt prints, in order.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ConfirmationDetails {
    /// `Tool '<name>' requires confirmation`.
    pub headline: String,
    /// The daemon-authored preview, under `From OpenAlpaca:`.
    pub preview: Option<String>,
    /// `Arguments: ` and the pretty-printed JSON.
    pub arguments: Option<String>,
    /// The hidden-character notice, directly under the arguments.
    pub notice: Option<String>,
}

impl ConfirmationDetails {
    /// Everything the inline prompt writes before it asks, painted, one line
    /// each: on a terminal first [`crate::output::TERMINAL_RESET`], then a
    /// blank line, the headline, the preview, the arguments and, directly
    /// under them, the notice.
    pub(crate) fn text(&self, tty: bool) -> String {
        use std::fmt::Write as _;

        let mut out = String::from(crate::output::terminal_reset(tty));
        // Writing into a `String` cannot fail.
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", self.headline.yellow().bold());
        if let Some(preview) = &self.preview {
            let _ = writeln!(out, "{preview}");
        }
        if let Some(arguments) = &self.arguments {
            let _ = writeln!(out, "{}", arguments.dimmed());
        }
        if let Some(notice) = &self.notice {
            let _ = writeln!(out, "{}", notice.yellow());
        }
        out
    }
}

/// What the inline prompt prints (design 09 §4.4): the headline naming the
/// tool, the daemon-authored preview, labelled as OpenAlpaca's own text, then
/// the literal arguments, which stay. Either of the last two may be absent.
///
/// The name and the arguments are what the person approves, and a terminal
/// is the renderer where a raw ESC or C1 CSI rewrites the line and a bidi
/// override reorders it, so every hidden character in them is shown as
/// `<U+XXXX>` (`openalpaca_core::utils::text::escape_hidden`, after the
/// JSON is serialized) and one notice under the arguments counts them. The
/// name is one line — an MCP `<server>__<tool>` or plugin `<plugin>::<tool>`
/// name is whatever its third party lists — so it takes `escape_hidden_line`,
/// which also shows a line break and a tab: a name cannot print a forged
/// `Arguments:` block above the real one. The preview is left as it comes:
/// the daemon composed it for reading and took every character the escape
/// would show out of what it quotes (`utils::text::one_line`).
/// Display only — the answer is built from the frame's `request_id`, untouched.
pub(crate) fn confirmation_details(frame: &serde_json::Value) -> ConfirmationDetails {
    use openalpaca_core::utils::text::{escape_hidden, escape_hidden_line, hidden_notice};

    let name = escape_hidden_line(frame["tool_name"].as_str().unwrap_or(""));
    let preview = frame["preview"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| format!("From OpenAlpaca:\n{}", indent(p)));
    let args = &frame["tool_arguments"];
    let arguments = (!args.is_null())
        .then(|| escape_hidden(&serde_json::to_string_pretty(args).unwrap_or_default()));
    let hidden = name.hidden + arguments.as_ref().map_or(0, |args| args.hidden);
    ConfirmationDetails {
        headline: format!("Tool '{}' requires confirmation", name.text),
        preview,
        arguments: arguments.map(|args| format!("Arguments: {}", args.text)),
        notice: hidden_notice(hidden),
    }
}

/// Where the inline prompt's answer goes, and what it says: `watch`'s and
/// `approve`'s own path and body (`commands::confirmations`), so the id is
/// one path segment and the body is `{"approved"}` with no scope — the
/// inline prompt offers no "always". Both come from the frame's
/// `request_id` as the daemon sent it, never from [`ConfirmationDetails`]:
/// nothing the prompt showed, escaped or not, reaches the broker.
pub(crate) fn inline_answer(
    frame: &serde_json::Value,
    approved: bool,
) -> (String, serde_json::Value) {
    use crate::commands::confirmations::{answer_body, answer_path};

    let request_id = frame["request_id"].as_str().unwrap_or("");
    (answer_path(request_id), answer_body(approved, false))
}

/// Every line of `text`, two spaces in.
pub(crate) fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
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
async fn handle_confirmation_prompt(
    client: &DaemonClient,
    event_text: &str,
    tty: bool,
) -> Result<()> {
    let mut data = String::new();
    for line in event_text.lines() {
        if let Some(val) = line.strip_prefix("data:") {
            data = val.trim().to_string();
        }
    }
    let parsed: serde_json::Value = serde_json::from_str(&data)?;

    print!("{}", confirmation_details(&parsed).text(tty));

    let approved = tokio::task::block_in_place(|| {
        print!("Allow execution? [y/N] ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
    });

    let (path, body) = inline_answer(&parsed, approved);
    client.post_raw(&path, &body).await?;

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
fn end_reasoning(state: &mut SseState, out: &mut impl Write) -> Result<()> {
    if !state.reasoning_open {
        return Ok(());
    }
    state.reasoning_open = false;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

/// [`process_sse_event_to`] on this process's stdout.
fn process_sse_event(event_text: &str, opts: &StreamOptions, state: &mut SseState) -> Result<()> {
    process_sse_event_to(event_text, opts, state, &mut std::io::stdout())
}

/// Handle one SSE frame: record what it says in `state` and write what it
/// shows to `out` — stdout in production, a buffer in a test, so what the
/// model's text looks like on a terminal is observed where it is written,
/// not only in the helpers that compute it. The skipped-attachment lines go
/// to stderr whatever `out` is.
fn process_sse_event_to(
    event_text: &str,
    opts: &StreamOptions,
    state: &mut SseState,
    out: &mut impl Write,
) -> Result<()> {
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
                write!(out, "{}", "Thinking...".dimmed())?;
                out.flush()?;
                write!(out, "\r            \r")?;
                out.flush()?;
            }
        }
        // S2: the model's own reasoning, live. Never recorded in `shown` — it
        // is not part of the answer and `done.content` never carries it.
        "reasoning" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(text) = parsed["text"].as_str()
                && let Some(visible) = reasoning_to_print(opts.tty, text)
            {
                write!(out, "{}", visible.dimmed())?;
                out.flush()?;
                state.reasoning_open = true;
            }
        }
        "delta" => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data)
                && let Some(content) = parsed["content"].as_str()
                && let Some(visible) = delta_to_print(opts.tty, content)
            {
                end_reasoning(state, out)?;
                write!(out, "{}", visible)?;
                out.flush()?;
                // What the daemon sent, not its escaped form: `done` is
                // reconciled against this.
                state.shown.push_str(content);
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
                    let reconciliation = reconcile(&state.shown, content);
                    end_reasoning(state, out)?;
                    if let Some(text) = reconciliation.text(opts.tty) {
                        write!(out, "{text}")?;
                    }
                    match reconciliation {
                        Reconciliation::Nothing => {}
                        Reconciliation::Append(tail) => state.shown.push_str(&tail),
                        Reconciliation::Redraw(answer) => state.shown = answer,
                    }
                }

                // Capture structured delegation metadata if present
                if let Some(value) = parsed.get("delegation")
                    && let Ok(delegation) = serde_json::from_value::<DelegationInfo>(value.clone())
                {
                    state.delegation = Some(delegation);
                }

                writeln!(out)?;

                // U3(d): what the turn answered *without*, before the usage
                // line — one line per withheld attachment, naming the reason
                // the daemon gave.
                for line in skipped_attachment_lines(&parsed) {
                    eprintln!("{}", line.yellow());
                }

                // Design 11 §5: the previous conversation this turn was
                // given, dim, at a terminal only.
                if let Some(line) = continued_from_line(&parsed, opts.tty) {
                    writeln!(out, "{}", line.dimmed())?;
                }

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
                    // The id is the provider catalogue's, a third party's
                    // string printed where the next prompt will be: A10's
                    // rule, like the model's own text.
                    let line = format_usage_line(&info);
                    writeln!(out, "{}", model_text(opts.tty, &line).dimmed())?;
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
        if let Some(text) = task_outcome_text(&resp["task"], std::io::stdout().is_terminal()) {
            print!("{text}");
            return Ok(());
        }
    }

    eprintln!(
        "{}",
        "(stopped waiting — task still running, check /tasks)".yellow()
    );
    Ok(())
}

/// What a delegated run's outcome prints once the run has stopped, one line
/// each, painted — or `None` while it is still running.
///
/// The summary is the run's `result_summary`: the lead model's own final
/// answer, followed, when a tool was refused because nobody could approve
/// it, by a line naming that tool — a name an MCP server or a plugin chose.
/// In the REPL it is printed into the
/// terminal the next turn's inline `[y/N]` prompt is printed into, so a
/// terminal gets it as it gets the streamed answer ([`model_text`]): a
/// scroll region or a palette change in it would outlive it, and the
/// prompt's reset undoes neither. A pipe gets it as the daemon sent it.
fn task_outcome_text(task: &serde_json::Value, tty: bool) -> Option<String> {
    use std::fmt::Write as _;

    let summary = model_text(tty, task["result_summary"].as_str().unwrap_or(""));
    let mut out = String::new();
    // Writing into a `String` cannot fail.
    match task["status"].as_str().unwrap_or("") {
        "completed" => {
            // Calculate duration from created_at → completed_at
            let duration_str = match (task["created_at"].as_str(), task["completed_at"].as_str()) {
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
            let _ = writeln!(
                out,
                "{}",
                format!("[Task completed{}]", duration_str).green()
            );
            if !summary.is_empty() {
                let _ = writeln!(out, "{} {}", "Result:".bold(), summary);
            }
        }
        status @ ("failed" | "cancelled") => {
            let _ = writeln!(out, "{}", format!("[Task {}]", status).red());
            if !summary.is_empty() {
                let _ = writeln!(out, "{} {}", "Error:".red(), summary);
            }
        }
        // Terminal, but not a failure: the daemon went away mid-run
        // (§5.6b). Polling must stop — the run will never move again — and
        // the restart verb is `rerun`, not `start` (R43).
        "interrupted" => {
            let _ = writeln!(
                out,
                "{}",
                "[Task interrupted — the daemon restarted]".yellow()
            );
            if !summary.is_empty() {
                let _ = writeln!(out, "{} {}", "Detail:".yellow(), summary);
            }
            let _ = writeln!(
                out,
                "{}",
                "Nothing was lost — re-run it to start the same goal again.".dimmed()
            );
        }
        // Still running — continue polling
        _ => return None,
    }
    Some(out)
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
