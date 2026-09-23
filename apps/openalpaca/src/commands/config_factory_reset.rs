//! `openalpaca config reset --factory`.
//!
//! The one form of `config` that opens no database: on a schema this build
//! refuses, deleting the files is the remedy, and asking the database's
//! permission first is exactly what made the verb unreachable.
//!
//! What it does, in order: refuse if a daemon is running; print the warning
//! (which names the absolute store root) and require the typed word
//! [`CONFIRM_WORD`]; delete `state/openalpaca.db` and its `-wal`/`-shm`
//! siblings; clear `llm.toml` and reset `daemon.toml`. Paths are resolved with
//! the non-creating store accessors only, so running it against a store that
//! does not exist creates nothing.

use anyhow::{Result, bail};
use dialoguer::{Input, theme::ColorfulTheme};
use openalpaca_storage::store;
use std::path::Path;

use super::ai_config;
use super::config::ConfigEnv;
use super::daemon_config_cli;

/// The word the user must type. Long enough that no reflex produces it,
/// short enough to type without a mistake.
pub(super) const CONFIRM_WORD: &str = "factory-reset";

/// Why the reset will not run under a live daemon. Names no pid on purpose:
/// the predicate is a `bool`, and widening it would make this a second reader
/// of `discovery.json` for a cosmetic gain.
const DAEMON_RUNNING_REFUSAL: &str = "\
A daemon is running against this store, and a factory reset will not delete a database \
it has open. On Unix the file would vanish from the directory while the daemon kept \
writing into the unlinked inode, and the next start would create a second, empty \
database beside it — two live databases, one of them invisible, and every row written \
in between lost without a word. Stop it first:

    openalpaca daemon stop

then run `openalpaca config reset --factory` again.";

pub(super) fn run(env: &dyn ConfigEnv) -> Result<()> {
    // Pure path queries: neither creates `state/`, so a reset against a store
    // that is not there leaves nothing behind.
    let root = std::path::absolute(store::home_root()?)?;
    let db_path = store::database_path()?;

    // Before the question, not after it: a refusal that arrives only once the
    // user has typed the word is a question we had no business asking.
    if env.daemon_is_running() {
        bail!("{DAEMON_RUNNING_REFUSAL}");
    }

    if !env.confirm_factory_reset(&root)? {
        println!("Cancelled. Nothing was deleted.");
        return Ok(());
    }

    // Files first: if this fails (a permission, a locked file), the user's
    // configuration is still intact and re-running is safe.
    openalpaca_storage::database::delete_database_files(&db_path)?;
    ai_config::clear_ai_config()?;
    daemon_config_cli::clear_daemon_config()?;

    println!(
        "Factory reset complete. {} is gone; the next daemon start builds an empty one.",
        db_path.display()
    );
    println!(
        "Your files under artifacts/, uploads/ and sessions/ are still on disk, \
         with nothing pointing at them."
    );
    Ok(())
}

/// The warning printed above the prompt. `root` is absolute —
/// `OPENALPACA_HOME_STORE` can point anywhere, so "your store" names nothing.
pub(super) fn factory_reset_warning(root: &Path) -> String {
    let root = root.display();
    format!(
        "\
FACTORY RESET

This deletes the OpenAlpaca database

    {root}/state/openalpaca.db
    {root}/state/openalpaca.db-wal
    {root}/state/openalpaca.db-shm

and clears your LLM and daemon configuration. The next daemon start builds an
empty database at the current schema version.

Gone for good:
  - every conversation, message and session row
  - every task, every run and its history
  - every memory
  - the rows that index your artifacts and uploads
  - config/llm.toml — provider settings and every API key in it, including the
    keychain entries those keys point at
  - config/daemon.toml — back to defaults

Left on disk, and from now on unreferenced:
  - {root}/artifacts/
  - {root}/uploads/
  - {root}/sessions/
The files stay exactly where they are; nothing in the new database points at
them any more. Delete them yourself if you want the space back.

Untouched:
  - {root}/state/cache/ — the local embedding model (~1 GB)
  - {root}/state/.master_key
  - {root}/state/logs/
  - {root}/state/backups/
  - {root}/plugins/
  - config/mcp.toml, config/agents/, config/skills/, config/orchestrator/

No backup is taken. There is no undo.
"
    )
}

/// Whether what the user typed is the confirmation word. Case and surrounding
/// whitespace are forgiven; `y`, `yes` and an empty line are not.
pub(super) fn prompt_accepts(typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case(CONFIRM_WORD)
}

/// Prints the warning and reads the confirmation word. The only place in this
/// module that touches a terminal.
///
/// `dialoguer` refuses with "not a terminal" when stderr is not a TTY, so a
/// piped or scripted invocation cannot answer this and deletes nothing. There
/// is deliberately no `--yes` to skip it.
pub(super) fn prompt_on_terminal(root: &Path) -> Result<bool> {
    eprint!("{}", factory_reset_warning(root));
    let typed: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "Type {CONFIRM_WORD} to continue (anything else cancels)"
        ))
        // Without this an empty line re-prompts for ever instead of meaning "no".
        .allow_empty(true)
        .interact_text()?;
    Ok(prompt_accepts(&typed))
}

#[cfg(test)]
mod tests;
