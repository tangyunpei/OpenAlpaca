//! OpenAlpaca CLI
//!
//! Command-line interface for interacting with the OpenAlpaca daemon.
//!
//! Commands:
//! - daemon: Manage daemon process (start/stop/status/tail)
//! - config: Manage system configuration
//! - gui: Manage GUI process

mod commands;
mod manager;

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
    /// Manage daemon process (status, tail, start, stop)
    Daemon(commands::daemon::DaemonArgs),

    /// Manage system configuration (interactive if no subcommands)
    Config(commands::config::ConfigArgs),

    /// Manage GUI process
    Gui(commands::gui::GuiArgs),

    /// Manage platform connectors (Telegram, etc.)
    Connector(commands::connector::ConnectorArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon(args) => commands::daemon::run(args).await,
        Commands::Config(args) => commands::config::run(args).await,
        Commands::Gui(args) => commands::gui::run(args).await,
        Commands::Connector(args) => commands::connector::run(args).await,
    }
}
