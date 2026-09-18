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
//! Everything here rides routes that already exist: `POST
//! /v1/chat/confirmations/{request_id}` to answer, `GET /v1/events/history` to
//! list what was raised, and the `/v1/events` socket to watch. No new daemon
//! surface, and no pre-approval of anything — the owner answers every prompt.
//!
//! **What `list` can and cannot say.** The daemon keeps its pending prompts in
//! memory (`ConfirmationBroker`) and publishes no list route and no resolution
//! event, so the event log is the only record a client can read: it holds what
//! was *raised*, not what is still waiting. The listing says exactly that, and
//! the answer is where the truth is — a prompt already answered, or timed out,
//! is refused by id rather than silently re-approved.

use anyhow::{Result, bail};
use clap::Subcommand;
use colored::Colorize;
use futures_util::StreamExt;
use openalpaca_storage::discovery;
use serde::{Deserialize, Serialize};
use std::io::Write;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list};

/// How many raised prompts `list` asks the event log for by default.
const DEFAULT_LIMIT: usize = 20;

#[derive(Subcommand)]
pub enum ConfirmationCommands {
    /// List the approval prompts this daemon has raised, newest first
    List {
        /// Maximum number of prompts to read from the event log
        #[arg(long, default_value = "20")]
        limit: usize,
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
        ConfirmationCommands::List { limit, format } => list(limit, format).await,
        ConfirmationCommands::Watch => watch().await,
        ConfirmationCommands::Approve {
            request_id,
            entire_tool,
        } => answer(&request_id, true, entire_tool).await,
        ConfirmationCommands::Deny { request_id } => answer(&request_id, false, false).await,
    }
}

// ── The wire ─────────────────────────────────────────────────────

/// One row of `GET /v1/events/history`, as this command reads it.
#[derive(Debug, Deserialize)]
struct EventRow {
    timestamp: String,
    event_type: String,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    detail: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct EventHistoryPage {
    events: Vec<EventRow>,
}

/// A prompt the daemon raised, as the listing shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RaisedPrompt {
    pub request_id: String,
    pub tool_name: String,
    /// The run that is waiting, or `null` for a main-loop prompt.
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub raised_at: String,
}

impl TableRow for RaisedPrompt {
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
            self.tool_name,
            run,
            self.raised_at.chars().take(19).collect::<String>(),
        )
    }
}

/// The `tool_confirmation_requested` rows of an event page, newest first.
///
/// A row whose payload carries no `request_id` is dropped rather than shown
/// with a blank id: an id that cannot be answered with is not a listing, it is
/// a decoy.
fn raised_prompts(page: &EventHistoryPage) -> Vec<RaisedPrompt> {
    let mut out = Vec::new();
    for event in &page.events {
        if event.event_type != "tool_confirmation_requested" {
            continue;
        }
        let detail = event.detail.clone().unwrap_or(serde_json::Value::Null);
        let Some(request_id) = detail["request_id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        out.push(RaisedPrompt {
            request_id: request_id.to_string(),
            tool_name: detail["tool_name"]
                .as_str()
                .unwrap_or("unknown_tool")
                .to_string(),
            // The column is the indexed one; the payload is the fallback for a
            // row written before migration 037 filled it.
            task_id: event
                .task_id
                .clone()
                .or_else(|| detail["task_id"].as_str().map(|id| id.to_string())),
            agent_id: detail["agent_id"].as_str().map(|id| id.to_string()),
            raised_at: event.timestamp.clone(),
        });
    }
    out
}

/// What the listing says above the rows: what these are, and what they are not.
fn listing_note(count: usize) -> String {
    if count == 0 {
        return "No approval prompt has been raised. A run that needs one shows it here; \
                `openalpaca tasks confirmations watch` waits for the next."
            .to_string();
    }
    "Prompts this daemon raised, newest first. It keeps no list of which are still waiting, \
     so answering is what says: one already answered, or timed out, is refused by id."
        .to_string()
}

async fn list(limit: usize, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    let page: EventHistoryPage = client
        .get(&format!(
            "/v1/events/history?event_type=tool_confirmation_requested&limit={limit}"
        ))
        .await?;
    let prompts = raised_prompts(&page);

    if matches!(format, OutputFormat::Table) {
        println!("{}", listing_note(prompts.len()).dimmed());
        if prompts.is_empty() {
            return Ok(());
        }
    }
    print_list(&prompts, format);
    Ok(())
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
fn answered_line(approved: bool, entire_tool: bool, tool_name: Option<&str>) -> String {
    let what = tool_name.unwrap_or("the tool call");
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
fn prompt_block(frame: &ConfirmationFrame) -> String {
    let mut out = format!("Tool '{}' needs your approval", frame.tool_name);
    if let Some(task_id) = &frame.task_id {
        out.push_str(&format!("\n  run:   {task_id}"));
    }
    if let Some(agent_id) = &frame.agent_id {
        out.push_str(&format!("\n  agent: {agent_id}"));
    }
    out.push_str(&format!("\n  id:    {}", frame.request_id));
    if !frame.tool_arguments.is_null() {
        out.push_str(&format!(
            "\n  args:  {}",
            serde_json::to_string(&frame.tool_arguments).unwrap_or_default()
        ));
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
        "Waiting for approval prompts (Ctrl+C to stop). Prompts raised before now are not \
         replayed — `openalpaca tasks confirmations list` shows those."
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
                    let (approved, entire_tool) = ask();
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

/// The three answers, read from the terminal. Anything else is a denial —
/// fail-closed is the rule, and a typo must never widen anything.
fn ask() -> (bool, bool) {
    tokio::task::block_in_place(|| {
        print!("Allow? [y]es / [a]lways this tool / [N]o ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        read_answer(&input)
    })
}

/// `(approved, entire_tool)` for what was typed.
fn read_answer(input: &str) -> (bool, bool) {
    match input.trim().to_lowercase().as_str() {
        "y" | "yes" => (true, false),
        "a" | "always" => (true, true),
        _ => (false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(events: serde_json::Value) -> EventHistoryPage {
        serde_json::from_value(serde_json::json!({ "events": events })).expect("page")
    }

    /// The listing reads the payload the daemon's own persistence writes
    /// (`events/persistence.rs`: request_id, agent_id, tool_name, task_id).
    #[test]
    fn a_raised_prompt_is_read_off_the_event_the_daemon_logged() {
        let prompts = raised_prompts(&page(serde_json::json!([
            {
                "timestamp": "2026-09-18T17:10:05.123Z",
                "event_type": "tool_confirmation_requested",
                "task_id": "76751dc6-1111-2222-3333-444444444444",
                "detail": {
                    "request_id": "req-1",
                    "agent_id": "lead_agent",
                    "tool_name": "artifact_write",
                    "task_id": "76751dc6-1111-2222-3333-444444444444"
                }
            },
            {
                "timestamp": "2026-09-18T17:09:00.000Z",
                "event_type": "tool_executed",
                "detail": { "tool_name": "memory_search" }
            }
        ])));

        assert_eq!(prompts.len(), 1, "only the confirmation rows are prompts");
        let prompt = &prompts[0];
        assert_eq!(prompt.request_id, "req-1");
        assert_eq!(prompt.tool_name, "artifact_write");
        assert_eq!(
            prompt.task_id.as_deref(),
            Some("76751dc6-1111-2222-3333-444444444444")
        );
        assert_eq!(prompt.agent_id.as_deref(), Some("lead_agent"));
    }

    /// A main-loop prompt belongs to no run; the row says `-` rather than
    /// attaching it to one.
    #[test]
    fn a_prompt_outside_a_run_names_no_run() {
        colored::control::set_override(false);
        let prompts = raised_prompts(&page(serde_json::json!([{
            "timestamp": "2026-09-18T17:10:05Z",
            "event_type": "tool_confirmation_requested",
            "detail": { "request_id": "req-2", "tool_name": "file_write" }
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
            raised_prompts(&page(serde_json::json!([{
                "timestamp": "2026-09-18T17:10:05Z",
                "event_type": "tool_confirmation_requested",
                "detail": { "tool_name": "file_write" }
            }])))
            .is_empty()
        );
    }

    /// The listing never claims the rows are still waiting — the daemon
    /// publishes no such list, and saying otherwise would be the silent
    /// degradation the rules reject.
    #[test]
    fn the_listing_says_what_it_does_not_know() {
        let note = listing_note(3);
        assert!(note.contains("still waiting"), "{note}");
        assert!(note.contains("refused by id"), "{note}");
        assert!(listing_note(0).contains("watch"), "the empty case points on");
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
        assert_eq!(read_answer("y\n"), (true, false));
        assert_eq!(read_answer("YES\n"), (true, false));
        assert_eq!(read_answer("a\n"), (true, true));
        assert_eq!(read_answer("always\n"), (true, true));
        assert_eq!(read_answer("n\n"), (false, false));
        assert_eq!(read_answer("\n"), (false, false), "Enter alone denies");
        assert_eq!(read_answer("yolo\n"), (false, false), "a typo denies");
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
}
