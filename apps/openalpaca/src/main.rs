//! OpenAlpaca CLI
//!
//! Command-line interface for interacting with the OpenAlpaca daemon.
//!
//! Commands:
//! - daemon: Manage daemon process (start/stop/status/tail)
//! - config: Manage system configuration
//! - gui: Manage GUI process
//! - connector: Manage platform connectors
//! - tasks: Manage tasks (list, status, create, cancel, pause, resume)
//! - agents: Manage agents (list, status, config, create, remove)
//! - llm: Manage LLM settings, keys, and usage
//! - chat: Chat with the Orchestrator

mod chat_stream;
mod client;
mod commands;
mod manager;
mod output;
mod repl;

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

    /// Manage tasks (list, status, create, cancel, pause, resume)
    Tasks(commands::tasks::TasksArgs),

    /// Manage agents (list, status, config, create, remove)
    Agents(commands::agents::AgentsArgs),

    /// Manage LLM settings, keys, and usage
    Llm(commands::llm::LlmArgs),

    /// Chat with the Orchestrator
    Chat(commands::chat::ChatArgs),

    /// Manage plugins (list, approve, deny, enable, disable, config)
    Plugin(commands::plugin::PluginArgs),

    /// Manage extensions — MCP servers and plugins (list, info, enable, disable, reload, approve, deny, remove)
    Ext(commands::ext::ExtArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon(args) => commands::daemon::run(args).await,
        Commands::Config(args) => commands::config::run(args).await,
        Commands::Gui(args) => commands::gui::run(args).await,
        Commands::Connector(args) => commands::connector::run(args).await,
        Commands::Tasks(args) => commands::tasks::run(args).await,
        Commands::Agents(args) => commands::agents::run(args).await,
        Commands::Llm(args) => commands::llm::run(args).await,
        Commands::Chat(args) => commands::chat::run(args).await,
        Commands::Plugin(args) => commands::plugin::run(args).await,
        Commands::Ext(args) => commands::ext::run(args).await,
    }
}
