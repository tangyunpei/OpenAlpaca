use crate::commands::{status, tail};
use crate::manager;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub action: DaemonAction,
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Show daemon status
    Status,
    /// Stream events from daemon (Ctrl+C to stop)
    Tail {
        /// Number of events to show (0 = unlimited)
        #[arg(short, long, default_value = "0")]
        count: usize,
    },
    /// Start Daemon (and GUI by default)
    Start {
        /// Start only the daemon, skip GUI
        #[arg(long)]
        daemon_only: bool,
    },
    /// Stop Daemon (and GUI)
    Stop,
    /// Restart Daemon
    Restart,
}

pub async fn run(args: DaemonArgs) -> Result<()> {
    match args.action {
        DaemonAction::Status => status::run().await,
        DaemonAction::Tail { count } => tail::run(count).await,
        DaemonAction::Start { daemon_only } => {
            manager::start_daemon()?;
            if !daemon_only {
                // Brief wait for daemon to initialize resources if needed
                std::thread::sleep(std::time::Duration::from_secs(2));
                manager::start_gui()?;
            }
            Ok(())
        }
        DaemonAction::Stop => {
            manager::stop_daemon()?;
            manager::stop_gui()?;
            Ok(())
        }
        DaemonAction::Restart => {
            manager::stop_daemon()?;
            std::thread::sleep(std::time::Duration::from_secs(1));
            manager::start_daemon()?;
            Ok(())
        }
    }
}
