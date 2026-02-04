use crate::manager;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct GuiArgs {
    #[command(subcommand)]
    pub action: GuiAction,
}

#[derive(Subcommand)]
pub enum GuiAction {
    /// Start GUI
    Start,
    /// Stop GUI
    Stop,
}

pub async fn run(args: GuiArgs) -> Result<()> {
    match args.action {
        GuiAction::Start => manager::start_gui(),
        GuiAction::Stop => manager::stop_gui(),
    }
}
