use anyhow::Result;
use clap::{Args, Subcommand};
use openalpaca_storage::{ConfigRepository, store};

use crate::output::OutputFormat;

use super::config_handlers::*;
use super::config_tui::run_interactive;

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Set a configuration value (validates against schema)
    Set { key: String, value: String },
    /// Get a configuration value
    Get { key: String },
    /// List configuration values
    List {
        /// Show all registered keys (including unset, with defaults)
        #[arg(long)]
        all: bool,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
        /// Show source column (db, llm.toml, daemon.toml)
        #[arg(long, short)]
        verbose: bool,
    },
    /// Reset configuration
    Reset {
        /// Key to reset (omit for all config)
        key: Option<String>,
        /// Full factory reset (wipes agents, memories, everything)
        #[arg(long)]
        factory: bool,
    },
}

pub async fn run(args: ConfigArgs) -> Result<()> {
    // Runs the root move first: opening the database would otherwise *create* an
    // empty one at the new path and strand the legacy install's data. Idempotent
    // — a no-op once the daemon has booted at least once.
    let db = store::migrate::open_store_database()?;
    let repo = ConfigRepository::new(&db);

    match args.action {
        Some(ConfigAction::Set { key, value }) => cmd_set(&repo, &key, &value)?,
        Some(ConfigAction::Get { key }) => cmd_get(&repo, &key)?,
        Some(ConfigAction::List {
            all,
            format,
            verbose,
        }) => cmd_list(&repo, all, format, verbose)?,
        Some(ConfigAction::Reset { key, factory }) => cmd_reset(&repo, &db, key, factory)?,
        None => run_interactive(&repo, &db)?,
    }

    Ok(())
}
