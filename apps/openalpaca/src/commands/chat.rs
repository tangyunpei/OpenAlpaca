//! Chat command — delegates to REPL (interactive) or chat_stream (single/pipe)

use anyhow::Result;
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
}

pub async fn run(args: ChatArgs) -> Result<()> {
    if let Some(ref msg) = args.message {
        return single_message(msg).await;
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let session = crate::repl::ReplSession::new()?;
        return session.run().await;
    }
    pipe_mode().await
}

async fn single_message(content: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    print!("{} ", "Alpaca:".cyan().bold());
    std::io::stdout().flush()?;
    let result = chat_stream::send_and_stream(&client, content, &Default::default()).await?;
    if let StreamResult::Delegation { task_title, .. } = &result {
        chat_stream::poll_task_completion(&client, task_title).await?;
    }
    println!();
    Ok(())
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
