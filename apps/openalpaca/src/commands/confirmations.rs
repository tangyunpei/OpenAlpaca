//! `openalpaca tasks confirmations` — the CLI's half of M6.
//!
//! A tool on the confirm list stops the run that called it and asks. In the
//! GUI the card is right there; from the CLI it used to be answerable only
//! *inside* the turn that raised it, on the SSE stream `openalpaca chat`
//! happens to be reading. A **workflow's** prompt arrives long after that
//! stream is `done`, so it reached nobody and the run sat on it for the whole
//! 300 s timeout.
//!
//! Two halves close that. The one-shot and the pipe now declare themselves
//! unable to answer (`chat_stream::ChatTarget::unattended`), so their runs are
//! refused at once instead of hanging. And an interactive session gets this:
//!
//! ```text
//! openalpaca tasks confirmations list            # what has been raised
//! openalpaca tasks confirmations watch           # answer them as they arrive
//! openalpaca tasks confirmations approve <id>    # answer one by id
//! openalpaca tasks confirmations deny <id>
//! ```
//!
//! Everything here rides daemon routes: `POST
//! /v1/chat/confirmations/{request_id}` to answer, `GET /v1/chat/confirmations`
//! to list what is still waiting, and the `/v1/events` socket to watch. No
//! pre-approval of anything — the owner answers every prompt.
//!
//! **What `list` says (S9).** It reads the broker's own pending set, so the
//! rows are the prompts a run is waiting on *right now* — not the event log's
//! record of what was once raised, which is what this command used to show and
//! which listed answered and timed-out prompts as if they were live. It is a
//! **snapshot**: a prompt answered a microsecond later is still in the list,
//! and answering is still where the truth is — one already answered, or timed
//! out, is refused by id rather than silently re-approved.

use anyhow::{Result, bail};
use clap::Subcommand;
use colored::Colorize;
use futures_util::StreamExt;
use openalpaca_core::utils::text::{Escaped, escape_hidden, escape_hidden_line, hidden_notice};
use openalpaca_storage::discovery;
use serde::{Deserialize, Serialize};
use std::io::Write;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list};

#[derive(Subcommand)]
pub enum ConfirmationCommands {
    /// List the approval prompts a run is waiting on, oldest first
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Wait for approval prompts and answer them here, until Ctrl+C
    Watch,
    /// Allow the tool call a run is waiting on
    Approve {
        /// The prompt's request id
        request_id: String,
        /// Allow every later call of this tool for the rest of the session
        #[arg(long)]
        entire_tool: bool,
    },
    /// Refuse the tool call a run is waiting on
    Deny {
        /// The prompt's request id
        request_id: String,
    },
}

pub async fn run(command: ConfirmationCommands) -> Result<()> {
    match command {
        ConfirmationCommands::List { format } => list(format).await,
        ConfirmationCommands::Watch => watch().await,
        ConfirmationCommands::Approve {
            request_id,
            entire_tool,
        } => answer(&request_id, true, entire_tool).await,
        ConfirmationCommands::Deny { request_id } => answer(&request_id, false, false).await,
    }
}

// ── The wire ─────────────────────────────────────────────────────

/// One prompt of `GET /v1/chat/confirmations` (S9) — a request some run is
/// still parked on, not an event-log row about one that once existed.
///
/// Serialized as it is deserialized, so `--format json` hands on the daemon's
/// own fields rather than a re-spelling of them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PendingPrompt {
    pub request_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub tool_arguments: serde_json::Value,
    /// The run that is waiting, or `null` for a main-loop prompt.
    #[serde(default)]
    pub task_id: Option<String>,
    pub agent_id: String,
    #[serde(default)]
    pub lane_key: Option<String>,
    /// The daemon-authored text of the card (design 09 §4.4), for
    /// `memory_confirm`; absent for the ordinary card.
    #[serde(default)]
    pub preview: Option<String>,
    /// The approval is good for this one call only: `--entire-tool` is
    /// refused for it.
    #[serde(default)]
    pub per_call: bool,
    pub raised_at: String,
}

#[derive(Debug, Deserialize)]
struct PendingPrompts {
    #[serde(default)]
    confirmations: Vec<PendingPrompt>,
}

impl TableRow for PendingPrompt {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("REQUEST_ID", 38),
            ("TOOL", 18),
            ("RUN", 10),
            ("RAISED", 20),
        ]
    }

    fn table_row(&self) -> String {
        let run = match &self.task_id {
            Some(id) if id.len() > 8 => id[..8].to_string(),
            Some(id) => id.clone(),
            // A main-loop prompt belongs to no run, and says so rather than
            // borrowing one.
            None => "-".to_string(),
        };
        format!(
            "{:<38} {:<18} {:<10} {:<20}",
            self.request_id,
            escape_hidden_line(&self.tool_name).text,
            run,
            self.raised_at.chars().take(19).collect::<String>(),
        )
    }
}

/// The prompts a listing shows: what the daemon handed back, minus anything
/// without an answerable id.
///
/// A row with a blank `request_id` is dropped rather than shown: an id that
/// cannot be answered with is not a listing, it is a decoy.
fn pending_prompts(page: PendingPrompts) -> Vec<PendingPrompt> {
    page.confirmations
        .into_iter()
        .filter(|prompt| !prompt.request_id.is_empty())
        .collect()
}

/// What the listing says above the rows: what these are, and what they are not.
fn listing_note(count: usize) -> String {
    if count == 0 {
        return "Nothing is waiting for an approval. A run that needs one shows it here; \
                `openalpaca tasks confirmations watch` waits for the next."
            .to_string();
    }
    "Prompts a run is waiting on, oldest first. It is a snapshot — one answered or timed out \
     since this was read is refused by id, and nothing is changed."
        .to_string()
}

async fn list(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let page: PendingPrompts = client.get("/v1/chat/confirmations").await?;
    let prompts = pending_prompts(page);

    if matches!(format, OutputFormat::Json) {
        println!("{}", listing_json(&prompts));
        return Ok(());
    }
    println!("{}", listing_note(prompts.len()).dimmed());
    if prompts.is_empty() {
        return Ok(());
    }
    print_list(&prompts, OutputFormat::Table);
    for line in notice_lines(&prompts) {
        println!("{}", line.yellow());
    }
    for block in preview_blocks(&prompts) {
        println!();
        println!("{block}");
    }
    Ok(())
}

/// A prompt's tool-controlled text as a person is shown it: the tool's name
/// and the agent's id on one line each (`escape_hidden_line`, which also
/// shows a line break and every space but the plain one), the arguments
/// serialized on one line and then escaped (`escape_hidden`). The name comes
/// from a third party (an MCP `<server>__<tool>` or plugin `<plugin>::<tool>`
/// name), the id from a template, the arguments from the model.
struct EscapedPrompt {
    name: Escaped,
    agent: Option<Escaped>,
    arguments: Option<Escaped>,
}

impl EscapedPrompt {
    fn new(tool_name: &str, agent_id: Option<&str>, arguments: &serde_json::Value) -> Self {
        Self {
            name: escape_hidden_line(tool_name),
            agent: agent_id.map(escape_hidden_line),
            arguments: (!arguments.is_null())
                .then(|| escape_hidden(&serde_json::to_string(arguments).unwrap_or_default())),
        }
    }

    /// The one line that counts every character shown escaped, or `None`.
    fn notice(&self) -> Option<String> {
        let hidden = |escaped: &Option<Escaped>| escaped.as_ref().map_or(0, |e| e.hidden);
        hidden_notice(self.name.hidden + hidden(&self.agent) + hidden(&self.arguments))
    }
}

/// Under the table, one notice for each prompt that carries a hidden
/// character, named by its id: `<request_id> — Note: this request contains
/// 2 hidden characters, shown as <U+…>.`
///
/// It counts the whole request — the name, the agent and the arguments —
/// not only the name the table shows escaped: the arguments are what
/// `approve <id>` lets run and the table has no room for them, so this line
/// is the one warning a person answering by id gets. `--format json` carries
/// the arguments, and `watch` shows a prompt's arguments escaped.
fn notice_lines(prompts: &[PendingPrompt]) -> Vec<String> {
    prompts
        .iter()
        .filter_map(|prompt| {
            EscapedPrompt::new(
                &prompt.tool_name,
                Some(&prompt.agent_id),
                &prompt.tool_arguments,
            )
            .notice()
            .map(|notice| format!("{} — {notice}", prompt.request_id))
        })
        .collect()
}

/// `--format json`: the daemon's own fields, as they came, with every
/// character a one-line field would show as `<U+XXXX>` written instead with
/// JSON's own escape (a backslash, `u` and four hex digits; a surrogate pair
/// past U+FFFF). A JSON reader gets exactly the values the daemon sent; a
/// terminal never receives a raw C1 control, bidi override or zero-width
/// character, which `serde_json` writes as they are (it escapes only the C0
/// controls). No notice line: it would not be JSON.
fn listing_json(prompts: &[PendingPrompt]) -> String {
    json_with_hidden_escaped(&serde_json::to_string_pretty(prompts).unwrap_or_default())
}

/// `json` with each hidden character written as a JSON `\uXXXX` escape.
///
/// Sound on `serde_json`'s output and only there: it writes JSON's structure
/// in ASCII laid out with `\n` and spaces, and escapes every C0 control
/// inside a string itself, so every other character of the one-line set
/// (`needs_escape_in_line`) it leaves raw is inside a string, where the
/// escape is the same character to any reader.
fn json_with_hidden_escaped(json: &str) -> String {
    use openalpaca_core::utils::text::needs_escape_in_line;
    use std::fmt::Write as _;

    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c != '\n' && needs_escape_in_line(c) {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                // Writing into a `String` cannot fail.
                let _ = write!(out, "\\u{unit:04x}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Under the table, what each card with a daemon-authored preview says
/// (design 09 §4.4) — the table has no room for it, and without it a
/// `memory_confirm` row is three opaque ids.
fn preview_blocks(prompts: &[PendingPrompt]) -> Vec<String> {
    prompts
        .iter()
        .filter_map(|prompt| {
            let preview = prompt.preview.as_deref().filter(|p| !p.is_empty())?;
            Some(format!(
                "{} — from OpenAlpaca:\n{}",
                prompt.request_id,
                crate::chat_stream::indent(preview)
            ))
        })
        .collect()
}

/// `--entire-tool` on a prompt whose approval is good for one call only
/// (design 09 §4.2 rule 2): refused here, before anything is sent — the
/// daemon would ignore the scope, and "every later call" would be untrue.
fn refuse_entire_tool(request_id: &str, prompt: Option<&PendingPrompt>) -> Result<()> {
    match prompt {
        Some(prompt) if prompt.per_call => bail!(
            "`{}` asks about one call only, so --entire-tool does not apply to it: run \
             `openalpaca tasks confirmations approve {request_id}` without it. Nothing was answered.",
            escape_hidden_line(&prompt.tool_name).text
        ),
        _ => Ok(()),
    }
}

/// The body `POST /v1/chat/confirmations/{request_id}` takes.
///
/// `approval_scope` is only meaningful on an approval, and `entire_tool` is the
/// owner widening it deliberately — it is never inferred, and never sent with a
/// denial.
fn answer_body(approved: bool, entire_tool: bool) -> serde_json::Value {
    let mut body = serde_json::json!({ "approved": approved });
    if approved && entire_tool {
        body["approval_scope"] = serde_json::json!("entire_tool");
    }
    body
}

fn answer_path(request_id: &str) -> String {
    format!(
        "/v1/chat/confirmations/{}",
        urlencoding::encode(request_id)
    )
}

/// What an answered prompt prints.
///
/// It reports the **answer**, not the outcome: the tool runs on the daemon
/// after this returns, and whether it then succeeded is the run's to say.
/// The name is escaped on one line like the prompt's, with no notice: the
/// prompt above it already counted it.
fn answered_line(approved: bool, entire_tool: bool, tool_name: Option<&str>) -> String {
    let what = tool_name.map_or_else(
        || "the tool call".to_string(),
        |name| escape_hidden_line(name).text,
    );
    match (approved, entire_tool) {
        (true, true) => {
            format!("Approved — {what}, and every later call of it this session. The run continues.")
        }
        (true, false) => format!("Approved — {what} is allowed to run. The run continues."),
        (false, _) => format!("Denied — {what} will be skipped and the agent told so."),
    }
}

/// What a refusal from the route means, said in the caller's terms.
///
/// The daemon answers `404 NOT_FOUND: No pending confirmation: <id>` for an id
/// it is not holding, which is one fact with three readings — answered
/// already, timed out, or never raised — and none of them is a failure of this
/// command.
fn refusal_line(request_id: &str, error: &str) -> String {
    if error.contains("No pending confirmation") {
        return format!(
            "Nothing is waiting on {request_id}: it was answered already, it timed out, or the \
             daemon restarted. Nothing was changed."
        );
    }
    format!("Could not answer {request_id}: {error}")
}

async fn answer(request_id: &str, approved: bool, entire_tool: bool) -> Result<()> {
    let client = DaemonClient::connect()?;
    if approved && entire_tool {
        let page: PendingPrompts = client.get("/v1/chat/confirmations").await?;
        let prompts = pending_prompts(page);
        refuse_entire_tool(
            request_id,
            prompts.iter().find(|p| p.request_id == request_id),
        )?;
    }
    match client
        .post_raw(&answer_path(request_id), &answer_body(approved, entire_tool))
        .await
    {
        Ok(_) => {
            println!(
                "{} {}",
                if approved { "✓".green() } else { "✗".yellow() },
                answered_line(approved, entire_tool, None)
            );
            Ok(())
        }
        Err(error) => bail!("{}", refusal_line(request_id, &error.to_string())),
    }
}

// ── Watch ────────────────────────────────────────────────────────

/// One `tool_confirmation_requested` frame off `/v1/events`.
#[derive(Debug, Deserialize)]
struct ConfirmationFrame {
    request_id: String,
    tool_name: String,
    #[serde(default)]
    tool_arguments: serde_json::Value,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    task_id: Option<String>,
    /// The daemon-authored text of the card (design 09 §4.4).
    #[serde(default)]
    preview: Option<String>,
    /// The approval is good for this call only: no `[a]lways`.
    #[serde(default)]
    per_call: bool,
}

/// The frame this socket message is, or `None` for every other event.
///
/// The socket carries every event type there is, so the filter is on `type`
/// and nothing else; a frame missing the fields this command needs is not a
/// prompt it can answer and is left alone.
fn confirmation_frame(text: &str) -> Option<ConfirmationFrame> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value["type"].as_str() != Some("tool_confirmation_requested") {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// The block printed when a prompt arrives — what is being asked, and by whom.
///
/// The tool's name, the agent's id and the arguments come from outside
/// OpenAlpaca, and this prints them at a terminal, where a raw ESC or C1 CSI
/// rewrites the line and a bidi override reorders it: every hidden character
/// in them is shown as `<U+XXXX>` ([`EscapedPrompt`]) and one notice directly
/// under the arguments counts them. The run and request ids are the daemon's
/// own; the preview is the daemon's text, composed with every control and
/// format character taken out of what it quotes (`utils::text::one_line`),
/// and printed as it comes.
fn prompt_block(frame: &ConfirmationFrame) -> String {
    let shown = EscapedPrompt::new(
        &frame.tool_name,
        frame.agent_id.as_deref(),
        &frame.tool_arguments,
    );
    let mut out = format!("Tool '{}' needs your approval", shown.name.text);
    if let Some(task_id) = &frame.task_id {
        out.push_str(&format!("\n  run:   {task_id}"));
    }
    if let Some(agent) = &shown.agent {
        out.push_str(&format!("\n  agent: {}", agent.text));
    }
    out.push_str(&format!("\n  id:    {}", frame.request_id));
    if let Some(preview) = frame.preview.as_deref().filter(|p| !p.is_empty()) {
        out.push_str(&format!(
            "\n  from OpenAlpaca:\n{}",
            crate::chat_stream::indent(&crate::chat_stream::indent(preview))
        ));
    }
    if let Some(arguments) = &shown.arguments {
        out.push_str(&format!("\n  args:  {}", arguments.text));
    }
    if let Some(notice) = shown.notice() {
        out.push_str(&format!("\n  {notice}"));
    }
    out
}

/// Sit on the daemon's event socket and answer prompts as they are raised.
///
/// This is the answer to "a workflow's confirmation reaches nobody": the run
/// that raised it is somebody else's — a background workflow, a follow-up, a
/// connector's turn — and this window is a responder for all of them. Ctrl+C
/// leaves every unanswered prompt exactly as it was.
async fn watch() -> Result<()> {
    let disc = discovery::read_discovery()?
        .ok_or_else(|| anyhow::anyhow!("Daemon is not running (no discovery file)"))?;
    discovery::ensure_not_expired(&disc)?;
    let info = discovery::ConnectionInfo::from(&disc);
    let ws_url = format!(
        "{}/v1/events?token={}",
        info.base_url.replace("http", "ws"),
        urlencoding::encode(&info.token)
    );

    let client = DaemonClient::connect()?;
    let (ws_stream, _) = connect_async(&ws_url).await?;
    println!(
        "{}",
        "Waiting for approval prompts (Ctrl+C to stop). Prompts already waiting are not \
         replayed here — `openalpaca tasks confirmations list` shows those."
            .dimmed()
    );

    let (_, mut read) = ws_stream.split();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            message = read.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    let Some(frame) = confirmation_frame(&text) else { continue };
                    println!();
                    println!("{}", prompt_block(&frame).yellow().bold());
                    let (approved, entire_tool) = ask(frame.per_call);
                    match client
                        .post_raw(
                            &answer_path(&frame.request_id),
                            &answer_body(approved, entire_tool),
                        )
                        .await
                    {
                        Ok(_) => println!(
                            "{}",
                            answered_line(approved, entire_tool, Some(&frame.tool_name))
                        ),
                        // A prompt that timed out while it was being read is
                        // the common case, and it is not this command's
                        // failure: say so and keep waiting.
                        Err(error) => println!(
                            "{}",
                            refusal_line(&frame.request_id, &error.to_string()).yellow()
                        ),
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    println!("{}", "The daemon closed the event stream.".yellow());
                    break;
                }
                Some(Err(error)) => bail!("Event stream failed: {error}"),
                _ => {}
            },
            _ = &mut ctrl_c => {
                println!();
                println!("{}", "Stopped waiting. Nothing was answered.".dimmed());
                break;
            }
        }
    }
    Ok(())
}

/// The answers, read from the terminal. Anything else is a denial —
/// fail-closed is the rule, and a typo must never widen anything. A per-call
/// prompt (design 09 §4.2) offers no `[a]lways`.
fn ask(per_call: bool) -> (bool, bool) {
    tokio::task::block_in_place(|| {
        print!("{}", ask_line(per_call));
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        read_answer(&input, per_call)
    })
}

fn ask_line(per_call: bool) -> &'static str {
    if per_call {
        "Allow this one call? [y]es / [N]o "
    } else {
        "Allow? [y]es / [a]lways this tool / [N]o "
    }
}

/// `(approved, entire_tool)` for what was typed. On a per-call prompt
/// "always" was never offered, so it is not a yes — it denies, like any
/// other word.
fn read_answer(input: &str, per_call: bool) -> (bool, bool) {
    match input.trim().to_lowercase().as_str() {
        "y" | "yes" => (true, false),
        "a" | "always" if !per_call => (true, true),
        _ => (false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(confirmations: serde_json::Value) -> PendingPrompts {
        serde_json::from_value(serde_json::json!({ "confirmations": confirmations }))
            .expect("page")
    }

    /// S9: the rows are the broker's pending set, read off
    /// `GET /v1/chat/confirmations` exactly as the daemon serializes it
    /// (`routes/chat_types.rs::PendingConfirmation`).
    #[test]
    fn a_pending_prompt_is_read_off_the_route_that_lists_them() {
        let prompts = pending_prompts(page(serde_json::json!([
            {
                "request_id": "req-1",
                "tool_name": "artifact_write",
                "tool_arguments": { "path": "notes.md" },
                "task_id": "76751dc6-1111-2222-3333-444444444444",
                "agent_id": "lead_agent",
                "lane_key": null,
                "raised_at": "2026-09-18T17:10:05.123+00:00"
            },
            {
                "request_id": "req-2",
                "tool_name": "file_write",
                "tool_arguments": null,
                "task_id": null,
                "agent_id": "orchestrator",
                "lane_key": "alice:gui",
                "raised_at": "2026-09-18T17:11:00+00:00"
            }
        ])));

        assert_eq!(prompts.len(), 2, "the daemon's order is kept: oldest first");
        let prompt = &prompts[0];
        assert_eq!(prompt.request_id, "req-1");
        assert_eq!(prompt.tool_name, "artifact_write");
        assert_eq!(
            prompt.task_id.as_deref(),
            Some("76751dc6-1111-2222-3333-444444444444")
        );
        assert_eq!(prompt.agent_id, "lead_agent");
        assert_eq!(prompt.tool_arguments["path"], "notes.md");
    }

    /// The daemon with no broker answers `{"confirmations": []}`, and a client
    /// that polls this must read that as "nothing is waiting", not as a
    /// failure.
    #[test]
    fn an_empty_list_is_an_answer() {
        assert!(pending_prompts(page(serde_json::json!([]))).is_empty());
        let empty: PendingPrompts = serde_json::from_value(serde_json::json!({})).expect("page");
        assert!(pending_prompts(empty).is_empty());
    }

    /// A main-loop prompt belongs to no run; the row says `-` rather than
    /// attaching it to one.
    #[test]
    fn a_prompt_outside_a_run_names_no_run() {
        colored::control::set_override(false);
        let prompts = pending_prompts(page(serde_json::json!([{
            "request_id": "req-2",
            "tool_name": "file_write",
            "agent_id": "orchestrator",
            "raised_at": "2026-09-18T17:10:05+00:00"
        }])));
        assert_eq!(prompts[0].task_id, None);
        let row = prompts[0].table_row();
        let cells: Vec<&str> = row.split_whitespace().collect();
        assert_eq!(cells, vec!["req-2", "file_write", "-", "2026-09-18T17:10:05"]);
    }

    /// A row with no answerable id is not shown: a blank id in a column of ids
    /// is an invitation to type a command that cannot work.
    #[test]
    fn a_row_with_no_request_id_is_not_listed() {
        assert!(
            pending_prompts(page(serde_json::json!([{
                "request_id": "",
                "tool_name": "file_write",
                "agent_id": "orchestrator",
                "raised_at": "2026-09-18T17:10:05+00:00"
            }])))
            .is_empty()
        );
    }

    /// The listing says these are waiting — which is now true, and was not
    /// when the rows came off the event log — and still says it is a snapshot
    /// rather than claiming more than a poll can know.
    #[test]
    fn the_listing_says_what_it_knows_and_what_it_cannot() {
        let note = listing_note(3);
        assert!(note.contains("waiting on"), "{note}");
        assert!(note.contains("snapshot"), "{note}");
        assert!(note.contains("refused by id"), "{note}");
        assert!(listing_note(0).contains("watch"), "the empty case points on");
        assert!(
            listing_note(0).contains("Nothing is waiting"),
            "an empty broker is not 'nothing was ever raised'"
        );
    }

    /// The scope is only ever what the owner asked for, and a denial carries
    /// none at all.
    #[test]
    fn the_answer_body_widens_nothing_on_its_own() {
        assert_eq!(
            answer_body(true, false),
            serde_json::json!({ "approved": true })
        );
        assert_eq!(
            answer_body(true, true),
            serde_json::json!({ "approved": true, "approval_scope": "entire_tool" })
        );
        assert_eq!(
            answer_body(false, true),
            serde_json::json!({ "approved": false }),
            "a denial is a denial; there is no scope to widen"
        );
    }

    #[test]
    fn a_request_id_is_one_path_segment() {
        assert_eq!(answer_path("req-1"), "/v1/chat/confirmations/req-1");
        assert!(answer_path("a/b").contains("a%2Fb"));
    }

    /// Fail-closed at the keyboard too: only an explicit yes approves, and
    /// only an explicit "always" widens.
    #[test]
    fn only_a_deliberate_yes_approves() {
        assert_eq!(read_answer("y\n", false), (true, false));
        assert_eq!(read_answer("YES\n", false), (true, false));
        assert_eq!(read_answer("a\n", false), (true, true));
        assert_eq!(read_answer("always\n", false), (true, true));
        assert_eq!(read_answer("n\n", false), (false, false));
        assert_eq!(
            read_answer("\n", false),
            (false, false),
            "Enter alone denies"
        );
        assert_eq!(
            read_answer("yolo\n", false),
            (false, false),
            "a typo denies"
        );
    }

    /// An id the daemon is not holding is three ordinary situations, not an
    /// error in what the user typed.
    #[test]
    fn a_prompt_that_is_no_longer_waiting_says_so_plainly() {
        let line = refusal_line(
            "req-1",
            "NOT_FOUND: No pending confirmation: req-1 (HTTP 404)",
        );
        assert!(line.contains("timed out"), "{line}");
        assert!(line.contains("Nothing was changed"), "{line}");

        // Any other refusal is reported verbatim rather than explained away.
        let other = refusal_line("req-1", "CONFIRMATION_NOT_CONFIGURED: … (HTTP 503)");
        assert!(other.contains("CONFIRMATION_NOT_CONFIGURED"), "{other}");
    }

    /// The line reports the answer, never the outcome: the tool has not run
    /// yet when this prints, and claiming it did would be a result this
    /// command cannot know.
    #[test]
    fn the_answer_line_names_the_answer_not_the_outcome() {
        let approved = answered_line(true, false, Some("file_write"));
        assert!(approved.starts_with("Approved — file_write"), "{approved}");
        assert!(approved.contains("allowed to run"), "{approved}");
        assert!(!approved.contains("ran."), "{approved}");
        assert!(answered_line(true, true, None).contains("every later call"));
        assert!(answered_line(false, false, Some("file_write")).contains("skipped"));
    }

    /// The socket carries every event type; only one of them is a prompt.
    #[test]
    fn only_a_confirmation_frame_is_taken_off_the_socket() {
        let frame = confirmation_frame(
            r#"{"type":"tool_confirmation_requested","request_id":"req-1","agent_id":"lead_agent",
                "tool_name":"artifact_write","tool_arguments":{"path":"notes.md"},
                "task_id":"task-1","stream_id":null,"lane_key":null,
                "ts":"2026-09-18T17:10:05Z","instance_id":"i"}"#,
        )
        .expect("a confirmation frame is read");
        assert_eq!(frame.request_id, "req-1");
        assert_eq!(frame.tool_name, "artifact_write");

        assert!(confirmation_frame(r#"{"type":"heartbeat","ts":"now"}"#).is_none());
        assert!(confirmation_frame("not json").is_none());
    }

    /// The block names the run, so an owner with two workflows in flight can
    /// tell which one is asking.
    #[test]
    fn the_prompt_block_names_the_run_the_tool_and_the_id() {
        let frame = confirmation_frame(
            r#"{"type":"tool_confirmation_requested","request_id":"req-1","agent_id":"lead_agent",
                "tool_name":"artifact_write","tool_arguments":{"path":"notes.md"},
                "task_id":"task-1"}"#,
        )
        .expect("frame");
        let block = prompt_block(&frame);
        assert!(block.contains("artifact_write"), "{block}");
        assert!(block.contains("task-1"), "{block}");
        assert!(block.contains("req-1"), "{block}");
        assert!(block.contains("notes.md"), "{block}");
    }

    /// Design 09 §4.2 rule 2: a per-call prompt refuses `--entire-tool`
    /// before anything is sent; an ordinary prompt, or one the listing no
    /// longer holds, is left to the daemon as before.
    #[test]
    fn entire_tool_is_refused_for_a_per_call_prompt() {
        let prompts = pending_prompts(page(serde_json::json!([
            {
                "request_id": "req-memory",
                "tool_name": "memory_confirm",
                "tool_arguments": {"id": "s"},
                "agent_id": "orchestrator",
                "lane_key": "alice:gui",
                "preview": "Replace your saved note “Editor” with “Editor”?",
                "per_call": true,
                "raised_at": "2026-09-24T10:00:00+00:00"
            },
            {
                "request_id": "req-shell",
                "tool_name": "shell_execute",
                "agent_id": "orchestrator",
                "raised_at": "2026-09-24T10:00:01+00:00"
            }
        ])));
        assert!(prompts[0].per_call);
        assert_eq!(
            prompts[0].preview.as_deref(),
            Some("Replace your saved note “Editor” with “Editor”?")
        );
        assert!(!prompts[1].per_call, "absent means false");

        let err = refuse_entire_tool("req-memory", Some(&prompts[0])).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("--entire-tool"), "{err}");
        assert!(err.contains("approve req-memory"), "{err}");
        assert!(err.contains("Nothing was answered"), "{err}");
        assert!(refuse_entire_tool("req-shell", Some(&prompts[1])).is_ok());
        assert!(refuse_entire_tool("req-gone", None).is_ok());
    }

    /// Design 09 §4.4: `list` prints each card's preview under the table,
    /// and only the cards that have one.
    #[test]
    fn the_listing_prints_what_a_per_call_card_says() {
        let prompts = pending_prompts(page(serde_json::json!([
            {
                "request_id": "req-memory",
                "tool_name": "memory_confirm",
                "agent_id": "orchestrator",
                "preview": "Replace your saved note “Editor” with “Editor”?\nYours: vim.",
                "per_call": true,
                "raised_at": "2026-09-24T10:00:00+00:00"
            },
            {
                "request_id": "req-shell",
                "tool_name": "shell_execute",
                "agent_id": "orchestrator",
                "raised_at": "2026-09-24T10:00:01+00:00"
            }
        ])));
        assert_eq!(
            preview_blocks(&prompts),
            vec![
                "req-memory — from OpenAlpaca:\n  Replace your saved note “Editor” with “Editor”?\n  Yours: vim."
                    .to_string()
            ]
        );
    }

    /// Design 09 §4.2, §4.4 on `watch`: a per-call prompt shows the preview
    /// before the arguments, offers no `[a]lways`, and reads "always" as the
    /// denial it is there — fail-closed.
    #[test]
    fn watch_shows_the_preview_and_offers_no_always_for_a_per_call_prompt() {
        colored::control::set_override(false);
        let frame = confirmation_frame(
            r#"{"type":"tool_confirmation_requested","request_id":"req-9","agent_id":"orchestrator",
                "tool_name":"memory_confirm","tool_arguments":{"id":"s"},
                "preview":"Replace your saved note “Editor” with “Editor”?","per_call":true}"#,
        )
        .expect("a prompt");
        assert!(frame.per_call);
        let block = prompt_block(&frame);
        let preview_at = block.find("Replace your saved note").expect("the preview");
        let args_at = block.find("args:").expect("the arguments stay");
        assert!(preview_at < args_at, "{block}");
        assert!(block.contains("from OpenAlpaca"), "{block}");
        assert!(!ask_line(true).contains("always"));
        assert!(ask_line(false).contains("[a]lways"));
        assert_eq!(read_answer("a\n", true), (false, false));
        assert_eq!(read_answer("always\n", true), (false, false));
        assert_eq!(read_answer("y\n", true), (true, false));
        assert_eq!(read_answer("a\n", false), (true, true));
    }

    // ── Hidden characters (approval surfaces show them as <U+XXXX>) ──

    /// Every character an approval surface must never print raw: a bidi
    /// override, a zero-width space, a C1 CSI, a carriage return, a tag
    /// character, a line break and an isolate.
    const RAW: [char; 7] = [
        '\u{202e}',
        '\u{200b}',
        '\u{9b}',
        '\r',
        '\u{e0041}',
        '\n',
        '\u{2066}',
    ];

    fn assert_no_raw(text: &str, allow_newline: bool) {
        for c in RAW {
            if allow_newline && c == '\n' {
                continue;
            }
            assert!(
                !text.contains(c),
                "raw U+{:04X} printed: {text:?}",
                c as u32
            );
        }
    }

    /// A pending prompt whose name, agent and arguments all hide characters.
    fn spoofed_prompt() -> PendingPrompt {
        PendingPrompt {
            request_id: "req-spoof".to_string(),
            tool_name: "srv__rm\u{202e}fdp\u{200b}".to_string(),
            tool_arguments: serde_json::json!({
                "path": "/srv/allow\u{200b}ed/\u{202e}txt.exe",
                "c1": "\u{9b}31m",
                "tag": "x\u{e0041}",
            }),
            task_id: None,
            agent_id: "lead\u{2066}".to_string(),
            lane_key: Some("alice:gui".to_string()),
            preview: None,
            per_call: false,
            raised_at: "2026-09-25T10:00:00+00:00".to_string(),
        }
    }

    /// The table's TOOL cell is a third party's name (MCP `<server>__<tool>`,
    /// plugin `<plugin>::<tool>`): a bidi override or a zero-width space in
    /// it is shown as an escape, and a line break cannot start a forged row.
    #[test]
    fn the_listing_shows_a_hidden_character_in_the_tool_name_as_an_escape() {
        let mut prompt = spoofed_prompt();
        let row = prompt.table_row();
        assert!(row.contains("srv__rm<U+202E>fdp<U+200B>"), "{row}");
        assert_no_raw(&row, false);

        prompt.tool_name = "file_write\r\nreq-fake   shell_execute".to_string();
        let row = prompt.table_row();
        assert!(row.contains("file_write<U+000D><U+000A>req-fake"), "{row}");
        assert_eq!(row.lines().count(), 1, "one prompt, one row: {row:?}");
    }

    /// Under the table, one notice per prompt that hides anything, named by
    /// its id. It counts the whole request — name, agent and arguments —
    /// because the table prints only the name, and the arguments it does
    /// not print are what `approve <id>` lets run.
    #[test]
    fn the_listing_names_each_request_that_hides_characters() {
        let spoofed = spoofed_prompt();
        let clean = PendingPrompt {
            request_id: "req-clean".to_string(),
            tool_name: "file_write".to_string(),
            tool_arguments: serde_json::json!({ "path": "notes/日本語.md", "text": "a\r\nb\tc" }),
            agent_id: "orchestrator".to_string(),
            ..spoofed_prompt()
        };
        let only_arguments = PendingPrompt {
            request_id: "req-args".to_string(),
            tool_name: "shell_execute".to_string(),
            tool_arguments: serde_json::json!({ "command": "ls\u{200b}" }),
            agent_id: "orchestrator".to_string(),
            ..spoofed_prompt()
        };
        assert_eq!(
            notice_lines(&[spoofed, clean, only_arguments]),
            vec![
                // Name 2, agent 1, arguments 4 (U+200B, U+202E, U+009B, U+E0041).
                "req-spoof — Note: this request contains 7 hidden characters, shown as <U+…>."
                    .to_string(),
                "req-args — Note: this request contains 1 hidden character, shown as <U+…>."
                    .to_string(),
            ]
        );
    }

    /// `--format json` hands on the daemon's own values, so it writes a
    /// hidden character with JSON's own escape: a JSON reader gets exactly
    /// what the daemon sent, and a terminal never receives the character.
    #[test]
    fn the_json_listing_writes_hidden_characters_as_json_escapes() {
        let prompts = vec![PendingPrompt {
            // A wide blank: every JSON string is one line, so the one-line set.
            lane_key: Some("alice\u{3000}:gui".to_string()),
            ..spoofed_prompt()
        }];
        let json = listing_json(&prompts);
        assert_no_raw(&json, true);
        assert!(!json.contains('\u{3000}'), "{json}");
        for escape in [
            "u202e", "u200b", "u009b", "udb40", "udc41", "u2066", "u3000",
        ] {
            assert!(json.contains(&format!("\\{escape}")), "{escape}: {json}");
        }
        assert!(json.lines().count() > 10, "still pretty-printed: {json}");

        let read: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            read,
            serde_json::to_value(&prompts).unwrap(),
            "the same values the daemon sent"
        );
    }

    /// `watch` shows the name and the agent on one line each and the
    /// arguments escaped, with one notice counting all of them directly
    /// under the arguments.
    #[test]
    fn the_watch_block_shows_hidden_characters_as_escapes_and_counts_them() {
        let frame: ConfirmationFrame = serde_json::from_value(serde_json::json!({
            "type": "tool_confirmation_requested",
            "request_id": "req-1",
            "tool_name": "srv__rm\u{202e}fdp\u{200b}\r",
            "agent_id": "lead\u{2066}\nagent: orchestrator",
            "tool_arguments": {
                "path": "/srv/allow\u{200b}ed/\u{202e}txt.exe",
                "c1": "\u{9b}31m",
                "tag": "x\u{e0041}",
            },
            "task_id": "task-1",
        }))
        .expect("frame");
        let block = prompt_block(&frame);
        assert_no_raw(&block, true);

        let lines: Vec<&str> = block.lines().collect();
        assert_eq!(
            lines[0],
            "Tool 'srv__rm<U+202E>fdp<U+200B><U+000D>' needs your approval"
        );
        assert!(
            lines.contains(&"  agent: lead<U+2066><U+000A>agent: orchestrator"),
            "{block}"
        );
        let args = lines
            .iter()
            .position(|line| line.starts_with("  args:  "))
            .expect("the arguments");
        assert!(
            lines[args].contains("allow<U+200B>ed/<U+202E>txt.exe"),
            "{block}"
        );
        assert!(lines[args].contains("<U+009B>31m"), "{block}");
        assert!(lines[args].contains("x<U+E0041>"), "{block}");
        // Name 3, agent 2, arguments 4 — directly under the arguments.
        assert_eq!(
            lines.get(args + 1).copied(),
            Some("  Note: this request contains 9 hidden characters, shown as <U+…>.")
        );
        assert_eq!(lines.len(), args + 2, "{block}");
    }

    /// Nothing escaped, no notice: a CJK path, a right-to-left letter and a
    /// newline inside a string (which `serde_json` writes as `\n`) are not
    /// hidden characters.
    #[test]
    fn the_watch_block_prints_no_notice_when_nothing_is_hidden() {
        let frame: ConfirmationFrame = serde_json::from_value(serde_json::json!({
            "request_id": "req-1",
            "tool_name": "file_write",
            "agent_id": "lead_agent",
            "tool_arguments": { "path": "notes/日本語.md", "text": "שלום\r\nb\tc" },
        }))
        .expect("frame");
        let block = prompt_block(&frame);
        assert!(!block.contains("Note:"), "{block}");
        assert!(!block.contains("<U+"), "{block}");
        assert!(block.contains("notes/日本語.md"), "{block}");
        assert!(block.contains(r"שלום\r\nb\tc"), "{block}");
    }

    /// The name repeated after an answer, and in the refusal of
    /// `--entire-tool`, is escaped on one line like the prompt's.
    #[test]
    fn the_answer_and_the_refusal_escape_the_tool_name() {
        let name = "srv__rm\u{202e}fdp\u{200b}\n";
        let line = answered_line(true, false, Some(name));
        assert!(
            line.contains("srv__rm<U+202E>fdp<U+200B><U+000A>"),
            "{line}"
        );
        assert_no_raw(&line, false);

        let per_call = PendingPrompt {
            tool_name: name.to_string(),
            per_call: true,
            ..spoofed_prompt()
        };
        let err = refuse_entire_tool("req-spoof", Some(&per_call))
            .unwrap_err()
            .to_string();
        assert!(err.contains("srv__rm<U+202E>fdp<U+200B><U+000A>"), "{err}");
        assert_no_raw(&err, false);
    }

    /// Display only: rendering takes the frame by reference, and what is
    /// answered — the path built from the request id and the body — is
    /// what it was.
    #[test]
    fn showing_a_prompt_changes_nothing_that_is_answered() {
        let frame: ConfirmationFrame = serde_json::from_value(serde_json::json!({
            "request_id": "req-1",
            "tool_name": "srv__rm\u{202e}",
            "tool_arguments": { "path": "/srv/\u{202e}x" },
        }))
        .expect("frame");
        let _ = prompt_block(&frame);
        assert_eq!(frame.tool_name, "srv__rm\u{202e}");
        assert_eq!(frame.tool_arguments["path"], "/srv/\u{202e}x");
        assert_eq!(
            answer_path(&frame.request_id),
            "/v1/chat/confirmations/req-1"
        );
        assert_eq!(
            answer_body(true, false),
            serde_json::json!({ "approved": true })
        );
    }
}
