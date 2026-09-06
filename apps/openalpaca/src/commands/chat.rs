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

/// The last few turns of the resumed conversation, so the prompt does not open
/// on an empty screen with no idea what was being discussed.
async fn print_transcript_tail(client: &DaemonClient, session_id: &str) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Message {
        role: String,
        content: String,
    }
    #[derive(serde::Deserialize)]
    struct Page {
        messages: Vec<Message>,
    }

    let path = format!(
        "/v1/sessions/{}/messages?limit={TAIL_MESSAGES}",
        urlencoding::encode(session_id)
    );
    let page: Page = client.get(&path).await?;
    if page.messages.is_empty() {
        eprintln!("{}", "(no messages yet)".dimmed());
        return Ok(());
    }

    println!();
    for message in &page.messages {
        let who = match message.role.as_str() {
            "user" => "You:".bold(),
            "assistant" => "Alpaca:".cyan().bold(),
            other => other.dimmed(),
        };
        println!("{} {}", who, message.content.trim());
    }
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
}
