//! OpenAlpaca CLI
//!
//! Command-line interface for interacting with the OpenAlpaca daemon.
//!
//! Commands:
//! - status: Show daemon status
//! - tail: Stream events from daemon

mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "openalpaca")]
#[command(about = "OpenAlpaca CLI - Control your AI agents", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show daemon status
    Status,
    /// Stream events from daemon (Ctrl+C to stop)
    Tail {
        /// Number of events to show (0 = unlimited)
        #[arg(short, long, default_value = "0")]
        count: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Status => commands::status::run().await,
        Commands::Tail { count } => commands::tail::run(count).await,
    }
}
