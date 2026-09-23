//! `openalpaca config reset --factory`.
//!
//! The one form of `config` that opens no database: on a schema this build
//! refuses, deleting the files is the remedy, and asking the database's
//! permission first is exactly what made the verb unreachable.
//!
//! What it does, in order: take the daemon's singleton lock, refusing if a
//! daemon holds it; print the warning (which names the absolute store root)
//! and require the typed word [`CONFIRM_WORD`]; delete `state/openalpaca.db`
//! and its `-wal`/`-shm` siblings; clear `llm.toml` and reset `daemon.toml`;
//! release the lock. Paths are resolved with the non-creating store accessors
//! only, so running it against a store that does not exist creates nothing.

use anyhow::{Context, Result};
use dialoguer::{Input, theme::ColorfulTheme};
use openalpaca_storage::{discovery, store};
use std::path::Path;

use super::ai_config;
use super::ai_config_helpers::llm_config_path;
use super::config::ConfigEnv;
use super::daemon_config_cli::{self, daemon_config_path};

/// The word the user must type. Long enough that no reflex produces it,
/// short enough to type without a mistake.
pub(super) const CONFIRM_WORD: &str = "factory-reset";

/// Why the reset will not run under a live daemon. Names no pid on purpose:
/// the evidence is the daemon's lock, which says only that it is held.
const DAEMON_RUNNING_REFUSAL: &str = "\
A daemon is running against this store, and a factory reset will not delete a database \
it has open. On Unix the file would vanish from the directory while the daemon kept \
writing into the unlinked inode, and the next start would create a second, empty \
database beside it — two live databases, one of them invisible, and every row written \
in between lost without a word. Stop it first:

    openalpaca daemon stop

then run `openalpaca config reset --factory` again.";

/// Why the reset will not run when the lock is held but no daemon can be
/// seen: one still booting (it takes the lock before it writes discovery), or
/// a lock file this user cannot open — a daemon once run under `sudo` leaves
/// it owned by root. Saying "a daemon is running" there would send the user to
/// a `daemon stop` that finds nothing.
fn store_locked_refusal(lock: &Path) -> String {
    format!(
        "The store's lock ({}) is held or cannot be taken, and no running daemon was \
         found. A daemon may still be starting: wait a few seconds and run `openalpaca \
         config reset --factory` again. If none is starting, check that you can write \
         that file. Nothing was deleted.",
        lock.display()
    )
}

pub(super) fn run(env: &dyn ConfigEnv) -> Result<()> {
    // Pure path queries: neither creates `state/`, so a reset against a store
    // that is not there leaves nothing behind.
    let root = std::path::absolute(store::home_root()?)?;
    let db_path = store::database_path()?;
    let state_dir = db_path
        .parent()
        .context("the database path has no parent directory")?;
    // Resolved once, here, and cleared by these same paths: they can fall
    // back to `./config/` under the current directory — a repository
    // checkout — so the warning must say which files these are, not
    // `config/llm.toml`, which a reader takes to mean the store's.
    let targets = ResetTargets {
        root,
        llm_config: std::path::absolute(llm_config_path()?)?,
        daemon_config: std::path::absolute(daemon_config_path()?)?,
    };

    // Before the question, not after it: a refusal that arrives only once the
    // user has typed the word is a question we had no business asking. And
    // held from here until the files are gone, not merely checked: the prompt
    // waits as long as the user takes, and a daemon started meanwhile (the
    // app opened, `daemon start` in another terminal) would otherwise have
    // the database open when it is unlinked. Holding the lock, a daemon that
    // starts now exits on it instead.
    let claim = claim_store(env, state_dir)?;

    if !env.confirm_factory_reset(&factory_reset_warning(&targets))? {
        println!("Cancelled. Nothing was deleted.");
        return Ok(());
    }

    // With no `state/` there was nothing to lock — and no database — when we
    // asked. A daemon started during the prompt creates both, so take the
    // lock now or refuse; with `state/` still absent there is still nothing
    // to delete and nothing is created.
    let _claim = match claim {
        Some(held) => Some(held),
        None => claim_store(env, state_dir)?,
    };

    // Files first: if this fails (a permission, a locked file), the user's
    // configuration is still intact and re-running is safe.
    openalpaca_storage::database::delete_database_files(&db_path)?;
    ai_config::clear_ai_config_at(&targets.llm_config)?;
    daemon_config_cli::clear_daemon_config_at(&targets.daemon_config)?;

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

/// Takes the daemon's singleton lock for the length of the reset — the same
/// non-blocking acquisition the daemon makes at boot, so exactly one of the
/// two can hold the store: a daemon that is running or still booting makes
/// this refuse, and a daemon started while it is held exits on the lock
/// without opening the database. Dropping the returned guard releases it.
///
/// `Ok(None)` when `state/` does not exist: there is no database to protect,
/// and taking the lock would create the directory this verb promises not to.
///
/// The lock alone decides whether the reset runs; `env.daemon_is_running()`
/// only picks the words, so a refusal never claims a daemon nobody can find.
fn claim_store(env: &dyn ConfigEnv, state_dir: &Path) -> Result<Option<impl Sized>> {
    if !state_dir.is_dir() {
        return Ok(None);
    }
    discovery::acquire_single_instance_lock(false)
        .map(Some)
        .map_err(|e| {
            if env.daemon_is_running() {
                e.context(DAEMON_RUNNING_REFUSAL)
            } else {
                let lock =
                    store::lock_path().unwrap_or_else(|_| state_dir.join("openalpacad.lock"));
                e.context(store_locked_refusal(&lock))
            }
        })
}

/// What a factory reset deletes and clears, every path absolute.
pub(super) struct ResetTargets {
    /// The store root — `OPENALPACA_HOME_STORE` can point anywhere, so "your
    /// store" names nothing.
    pub root: std::path::PathBuf,
    /// The `llm.toml` the reset clears, resolved as `config set` resolves it.
    pub llm_config: std::path::PathBuf,
    /// The `daemon.toml` the reset resets, resolved the same way.
    pub daemon_config: std::path::PathBuf,
}

/// One line of the warning's "Gone for good" list for a configuration file:
/// its absolute path when it exists, or a plain "there is none" when it does
/// not — clearing a missing file does nothing, and saying otherwise would
/// name a file the reset never touches.
fn config_line(path: &Path, what_goes: &str, kind: &str) -> String {
    if path.exists() {
        format!("{} — {what_goes}", path.display())
    } else {
        format!("no {kind} to clear (there is none at {})", path.display())
    }
}

/// The warning printed above the prompt.
pub(super) fn factory_reset_warning(targets: &ResetTargets) -> String {
    let root = targets.root.display();
    let llm = config_line(
        &targets.llm_config,
        "provider settings and every API key in it, including the\n    keychain entries those keys point at",
        "llm.toml",
    );
    let daemon = config_line(&targets.daemon_config, "back to defaults", "daemon.toml");
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
  - {llm}
  - {daemon}

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
  - the rest of your configuration — mcp.toml, agents/, skills/, orchestrator/

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
pub(super) fn prompt_on_terminal(warning: &str) -> Result<bool> {
    eprint!("{warning}");
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
