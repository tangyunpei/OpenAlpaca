//! Chat command — delegates to REPL (interactive) or chat_stream (single/pipe)
//!
//! Every turn now carries the project it belongs to (the CLI's working
//! directory, as `x-workspace-path`) and, when the caller resumed one, the
//! conversation it belongs to (`session_id`). See `chat_stream::ChatTarget`.

use anyhow::{Context, Result, bail};
use clap::Args;
use colored::Colorize;
use std::io::Write;

use crate::chat_stream::{self, ChatTarget, StreamResult};
use crate::client::DaemonClient;
use crate::commands::sessions::{self, SessionItem};

/// How much of a resumed conversation is printed before the prompt opens.
const TAIL_MESSAGES: usize = 8;

#[derive(Args)]
pub struct ChatArgs {
    /// Send a single message (non-interactive)
    #[arg(long)]
    pub message: Option<String>,

    /// Attach file(s) to the message (repeatable, requires --message)
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<std::path::PathBuf>,

    /// Continue a stored conversation: pick one, or take the most recent when
    /// there is no terminal to pick with
    #[arg(long, conflicts_with = "session")]
    pub resume: bool,

    /// Continue the conversation with this id (`openalpaca sessions` lists them)
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
}

pub async fn run(args: ChatArgs) -> Result<()> {
    if !args.files.is_empty() && args.message.is_none() {
        anyhow::bail!("--file requires --message");
    }

    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let mut target = ChatTarget::for_cwd();

    if args.resume || args.session.is_some() {
        let client = DaemonClient::connect()?;
        let session_id = match args.session.as_deref() {
            Some(id) => id.to_string(),
            None => pick_session(&client, interactive).await?,
        };
        let resumed = resume(&client, &session_id).await?;
        print_transcript_tail(&client, &resumed.id).await?;
        target = target.resuming(resumed.id);
    }

    if let Some(ref msg) = args.message {
        return single_message(msg, &args.files, &target).await;
    }
    if interactive {
        let session = crate::repl::ReplSession::new(target)?;
        return session.run().await;
    }
    pipe_mode(&target).await
}

/// Which conversation `--resume` continues.
///
/// Interactively that is the user's choice from this lane's conversations,
/// newest first. With no terminal to ask at, it is the most recent — the same
/// row the picker would open on — because a prompt written to a pipe is a
/// hang, not a question.
async fn pick_session(client: &DaemonClient, interactive: bool) -> Result<String> {
    let rows = sessions::lane_sessions(client, None, 25).await?;
    let newest = sessions::require_one(&rows)?;
    if !interactive {
        return Ok(newest.id.clone());
    }

    let labels: Vec<String> = rows.iter().map(sessions::picker_label).collect();
    let choice = tokio::task::block_in_place(|| {
        dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Continue which conversation?")
            .items(&labels)
            .default(0)
            .interact()
    })?;
    rows.get(choice)
        .map(|row| row.id.clone())
        .context("The picker returned a row that is no longer in the list")
}

/// Re-open a conversation, whatever state it is in.
///
/// `POST /v1/sessions/{id}/activate` is explicit on purpose: a lane holds one
/// active conversation, so resuming an archived one **archives whatever is
/// live**. Doing it here, before a turn is sent, means the user is told what
/// happened rather than discovering it in the transcript — and it is what
/// keeps `POST /v1/chat` from answering `409 SESSION_ARCHIVED` mid-send.
async fn resume(client: &DaemonClient, session_id: &str) -> Result<SessionItem> {
    let path = format!("/v1/sessions/{}/activate", urlencoding::encode(session_id));
    let session: SessionItem = client
        .post(&path, &serde_json::json!({}))
        .await
        .with_context(|| format!("Could not resume conversation {session_id}"))?;

    let title = if session.title.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        session.title.clone()
    };
    let project = session.workspace_id.clone().unwrap_or_else(|| {
        // Not a failure: a conversation opened outside any project has none,
        // and this turn's own directory will bind it.
        "no project".to_string()
    });
    eprintln!(
        "{}",
        format!("Resuming {title} ({}) · {project}", session.id).dimmed()
    );
    Ok(session)
}

#[derive(serde::Deserialize)]
struct TranscriptMessage {
    role: String,
    content: String,
}

#[derive(serde::Deserialize)]
struct TranscriptPage {
    messages: Vec<TranscriptMessage>,
    /// Every message the conversation holds, not just this page's — the count
    /// the tail's offset is derived from. Defaulted so a daemon that predates
    /// the field still renders (the tail is then the first page, as before).
    #[serde(default)]
    total: i64,
}

/// Where a conversation's last `limit` messages begin.
///
/// `GET /v1/sessions/{id}/messages` is oldest-first
/// (`ORDER BY created_at ASC, id ASC LIMIT ?2 OFFSET ?3`), so a page asked for
/// without an offset is the conversation's **opening** — the exact opposite of
/// a tail. `total` is on the envelope for this.
fn tail_offset(total: i64, limit: usize) -> usize {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    usize::try_from((total - limit).max(0)).unwrap_or(0)
}

/// One page of a conversation's messages.
fn transcript_page_path(session_id: &str, limit: usize, offset: usize) -> String {
    format!(
        "/v1/sessions/{}/messages?limit={limit}&offset={offset}",
        urlencoding::encode(session_id)
    )
}

/// The last `limit` of what a page returned, in the order they were said.
///
/// The offset already narrows the request; this is what makes the *printing*
/// a tail rather than trusting the page to be one.
fn tail_of(messages: &[TranscriptMessage], limit: usize) -> &[TranscriptMessage] {
    &messages[messages.len().saturating_sub(limit)..]
}

/// The block printed above the prompt, one line per turn.
fn render_tail(messages: &[TranscriptMessage]) -> String {
    let mut out = String::new();
    for message in messages {
        let who = match message.role.as_str() {
            "user" => "You:".bold(),
            "assistant" => "Alpaca:".cyan().bold(),
            other => other.dimmed(),
        };
        out.push_str(&format!("{} {}\n", who, message.content.trim()));
    }
    out
}

/// The last few turns of the resumed conversation, so the prompt does not open
/// on an empty screen with no idea what was being discussed.
///
/// Two requests at most: the first answers *how long* the conversation is, and
/// only a conversation longer than the tail needs a second one aimed at its
/// end.
async fn print_transcript_tail(client: &DaemonClient, session_id: &str) -> Result<()> {
    let mut page: TranscriptPage = client
        .get(&transcript_page_path(session_id, TAIL_MESSAGES, 0))
        .await?;
    let offset = tail_offset(page.total, TAIL_MESSAGES);
    if offset > 0 {
        page = client
            .get(&transcript_page_path(session_id, TAIL_MESSAGES, offset))
            .await?;
    }

    if page.messages.is_empty() {
        eprintln!("{}", "(no messages yet)".dimmed());
        return Ok(());
    }

    println!();
    print!("{}", render_tail(tail_of(&page.messages, TAIL_MESSAGES)));
    println!("{}", "─".repeat(40).dimmed());
    Ok(())
}

async fn single_message(
    content: &str,
    files: &[std::path::PathBuf],
    target: &ChatTarget,
) -> Result<()> {
    let client = DaemonClient::connect()?;

    // Upload files and collect attachment refs
    let attachments = upload_files(&client, files).await?;

    print!("{} ", "Alpaca:".cyan().bold());
    std::io::stdout().flush()?;
    let result = chat_stream::send_and_stream_with_attachments(
        &client,
        content,
        &attachments,
        target,
        &Default::default(),
    )
    .await?;
    if let StreamResult::Delegation { delegation, .. } = &result {
        chat_stream::poll_task_completion(&client, &delegation.task_id).await?;
    }
    println!();
    Ok(())
}

async fn upload_files(
    client: &DaemonClient,
    files: &[std::path::PathBuf],
) -> Result<Vec<serde_json::Value>> {
    let mut attachments = Vec::new();
    for path in files {
        let resp = client.upload_file(path).await?;
        let file_id = resp["id"]
            .as_str()
            .context("Upload response missing 'id'")?
            .to_string();
        eprintln!(
            "{}",
            format!(
                "Uploaded: {} ({})",
                path.file_name().unwrap_or_default().to_string_lossy(),
                file_id
            )
            .dimmed()
        );
        attachments.push(serde_json::json!({ "file_id": file_id }));
    }
    Ok(attachments)
}

async fn pipe_mode(target: &ChatTarget) -> Result<()> {
    use std::io::Read;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let content = input.trim();
    if content.is_empty() {
        bail!("No input on stdin");
    }

    let client = DaemonClient::connect()?;
    print!("{} ", "Alpaca:".cyan().bold());
    std::io::stdout().flush()?;
    let result = chat_stream::send_and_stream_with_attachments(
        &client,
        content,
        &[],
        target,
        &Default::default(),
    )
    .await?;
    if let StreamResult::Delegation { delegation, .. } = &result {
        chat_stream::poll_task_completion(&client, &delegation.task_id).await?;
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        args: ChatArgs,
    }

    #[test]
    fn resume_and_session_both_parse() {
        let args = Harness::try_parse_from(["chat", "--resume"]).unwrap().args;
        assert!(args.resume);
        assert_eq!(args.session, None);

        let args = Harness::try_parse_from(["chat", "--session", "sess-1"])
            .unwrap()
            .args;
        assert!(!args.resume);
        assert_eq!(args.session.as_deref(), Some("sess-1"));
    }

    /// Picking *and* naming a conversation are two different answers to the
    /// same question; taking both would silently make one of them a no-op.
    #[test]
    fn resume_and_session_are_mutually_exclusive() {
        assert!(Harness::try_parse_from(["chat", "--resume", "--session", "sess-1"]).is_err());
    }

    #[test]
    fn a_resumed_conversation_can_still_be_a_one_shot_message() {
        let args = Harness::try_parse_from(["chat", "--session", "sess-1", "--message", "hi"])
            .unwrap()
            .args;
        assert_eq!(args.session.as_deref(), Some("sess-1"));
        assert_eq!(args.message.as_deref(), Some("hi"));
    }

    /// `turn 1` … `turn n`, alternating who said it, oldest first — the order
    /// `GET /v1/sessions/{id}/messages` returns.
    fn transcript(count: usize) -> Vec<TranscriptMessage> {
        (1..=count)
            .map(|n| TranscriptMessage {
                role: if n % 2 == 1 { "user" } else { "assistant" }.to_string(),
                content: format!("turn {n}"),
            })
            .collect()
    }

    /// The route orders oldest-first, so the tail of a fifty-message
    /// conversation starts at 42 — not at 0, which is where it starts.
    #[test]
    fn the_tail_of_a_long_conversation_starts_where_its_last_eight_begin() {
        assert_eq!(tail_offset(50, TAIL_MESSAGES), 42);
        assert_eq!(tail_offset(9, TAIL_MESSAGES), 1);
        // Nothing to skip: the whole conversation *is* the tail.
        assert_eq!(tail_offset(8, TAIL_MESSAGES), 0);
        assert_eq!(tail_offset(3, TAIL_MESSAGES), 0);
        assert_eq!(tail_offset(0, TAIL_MESSAGES), 0);
        // A daemon that sends no `total` deserializes as 0 and must not
        // produce a negative offset.
        assert_eq!(tail_offset(-1, TAIL_MESSAGES), 0);
    }

    #[test]
    fn a_long_conversation_is_re_requested_from_where_its_tail_begins() {
        assert_eq!(
            transcript_page_path("sess-1", TAIL_MESSAGES, 0),
            "/v1/sessions/sess-1/messages?limit=8&offset=0"
        );
        assert_eq!(
            transcript_page_path("sess-1", TAIL_MESSAGES, tail_offset(50, TAIL_MESSAGES)),
            "/v1/sessions/sess-1/messages?limit=8&offset=42"
        );
        // An id is a path segment, never a second path.
        assert!(transcript_page_path("a/b", TAIL_MESSAGES, 0).contains("a%2Fb"));
    }

    /// The bug this replaced: a fifty-message conversation printed `turn 1` …
    /// `turn 8` and called it the tail.
    #[test]
    fn a_resumed_conversation_prints_its_last_eight_turns_in_order() {
        colored::control::set_override(false);

        let all = transcript(50);
        let printed = render_tail(tail_of(&all, TAIL_MESSAGES));
        let lines: Vec<&str> = printed.lines().collect();

        assert_eq!(lines.len(), TAIL_MESSAGES, "{printed}");
        assert_eq!(lines[0], "You: turn 43", "{printed}");
        assert_eq!(lines[7], "Alpaca: turn 50", "{printed}");
        assert!(
            !printed.contains("turn 1\n") && !lines.iter().any(|line| line.ends_with("turn 8")),
            "the opening of the conversation is not its tail: {printed}"
        );
    }

    /// A conversation shorter than the tail prints whole, still in order.
    #[test]
    fn a_short_conversation_prints_whole() {
        colored::control::set_override(false);

        let printed = render_tail(tail_of(&transcript(3), TAIL_MESSAGES));
        let lines: Vec<&str> = printed.lines().collect();
        assert_eq!(lines, vec!["You: turn 1", "Alpaca: turn 2", "You: turn 3"]);
    }
}
