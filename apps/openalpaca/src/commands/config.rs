use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use openalpaca_storage::{ConfigRepository, Database, store};

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
        /// Full factory reset: deletes the database file and clears llm.toml/daemon.toml
        #[arg(long, conflicts_with = "key")]
        factory: bool,
    },
}

/// Everything `config` reaches outside its own process state, behind one seam.
///
/// The factory reset's whole point is that it opens **no** database — it is the
/// one form of this verb that survives a schema the build refuses. A claim like
/// that is only worth as much as the test behind it, so the opener is injected
/// and the test uses one that panics. The terminal prompt rides along for the
/// same reason: it is not available in a test binary. (The daemon check is not
/// here: it is the daemon's own lock, which a test can hold for real.)
pub(super) trait ConfigEnv {
    /// The store database. Opened at most once, and only by the arms that need it.
    fn database(&mut self) -> Result<Database>;

    /// Shows the factory-reset warning and reads the typed confirmation word.
    /// `Ok(false)` means the user declined; an error means we could not ask.
    fn confirm_factory_reset(&self, warning: &str) -> Result<bool>;

    /// Whether a live `openalpacad` is detectable (discovery pid + process
    /// name). Used only to WORD a factory-reset refusal: the lock alone decides
    /// whether the reset runs, and a held lock with no detectable daemon is a
    /// daemon still booting or a lock file this user cannot write.
    fn daemon_is_running(&self) -> bool;
}

#[derive(Default)]
pub(super) struct RealConfigEnv {
    opened: Option<Database>,
}

impl ConfigEnv for RealConfigEnv {
    fn database(&mut self) -> Result<Database> {
        if self.opened.is_none() {
            // Checks for an older install's data directory first: opening the
            // database *creates* one, so opening before checking is what strands
            // it. Through `state_dir()`, so `state/` is 0700 even when no daemon
            // has ever run here. This is the CLI's one call site.
            self.opened = Some(store::open_home_database()?);
        }
        // `Database` is an `Arc<Mutex<Connection>>` behind a `Clone`, so handing
        // out an owned clone costs nothing and spares every caller a lifetime.
        Ok(self.opened.clone().expect("just opened"))
    }

    fn confirm_factory_reset(&self, warning: &str) -> Result<bool> {
        super::config_factory_reset::prompt_on_terminal(warning)
    }

    fn daemon_is_running(&self) -> bool {
        crate::manager::is_daemon_running()
    }
}

pub async fn run(args: ConfigArgs) -> Result<()> {
    run_with(args, &mut RealConfigEnv::default())
}

/// The dispatcher. Each arm opens the database only if it needs one — a
/// file-backed key (`ai.*`, `daemon.*`) never does, and `reset --factory` must
/// not, because the database it exists to delete may be one this build refuses.
pub(super) fn run_with(args: ConfigArgs, env: &mut dyn ConfigEnv) -> Result<()> {
    match args.action {
        Some(ConfigAction::Set { key, value }) => cmd_set(env, &key, &value),
        Some(ConfigAction::Get { key }) => cmd_get(env, &key),
        Some(ConfigAction::List {
            all,
            format,
            verbose,
        }) => cmd_list(env, all, format, verbose),
        Some(ConfigAction::Reset { key, factory }) => cmd_reset(env, key, factory),
        None => {
            // Names no remedy (ruling C6): the error underneath carries its
            // own, and it is not always the same one. A legacy-schema database
            // names `config reset --factory`; an older install's stranded data
            // must not be offered that verb, which deletes nothing there and
            // does not clear it.
            let db = env.database().context(
                "the interactive configuration editor needs the store database, and it \
                 could not be opened (`openalpaca config set|get|reset` on an `ai.*` or \
                 `daemon.*` key does not use it)",
            )?;
            run_interactive(&ConfigRepository::new(&db))
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
