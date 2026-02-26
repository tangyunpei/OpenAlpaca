//! Chat command — delegates to REPL (interactive) or chat_stream (single/pipe)

use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use std::io::Write;

use crate::chat_stream::{self, StreamResult};
use crate::client::DaemonClient;

#[derive(Args)]
pub struct ChatArgs {
    /// Send a single message (non-interactive)
    #[arg(long)]
    pub message: Option<String>,

    /// Attach file(s) to the message (repeatable, requires --message)
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<std::path::PathBuf>,
}

pub async fn run(args: ChatArgs) -> Result<()> {
    if !args.files.is_empty() && args.message.is_none() {
        anyhow::bail!("--file requires --message");
    }
    if let Some(ref msg) = args.message {
        return single_message(msg, &args.files).await;
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let session = crate::repl::ReplSession::new()?;
        return session.run().await;
    }
    pipe_mode().await
}

async fn single_message(content: &str, files: &[std::path::PathBuf]) -> Result<()> {
    let client = DaemonClient::connect()?;

    // Upload files and collect attachment refs
    let attachments = upload_files(&client, files).await?;

    print!("{} ", "Alpaca:".cyan().bold());
    std::io::stdout().flush()?;
    let result = chat_stream::send_and_stream_with_attachments(
        &client,
        content,
        &attachments,
        &Default::default(),
    )
    .await?;
    if let StreamResult::Delegation { task_title, .. } = &result {
        chat_stream::poll_task_completion(&client, task_title).await?;
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

async fn pipe_mode() -> Result<()> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }
    let client = DaemonClient::connect()?;
    let result = chat_stream::send_and_stream(&client, input, &Default::default()).await?;
    if let StreamResult::Delegation { task_title, .. } = &result {
        chat_stream::poll_task_completion(&client, task_title).await?;
    }
    println!();
    Ok(())
}
