//! Whether an older OpenAlpaca install's data directory is still on this
//! machine — and saying so before it can be silently ignored.
//!
//! Nothing here moves a byte. Builds before this one relocated the pre-D1
//! application data directory into `~/.openalpaca` on first boot; that mover is
//! retired (D-D). What it protected against remains real, so this module keeps
//! the one guarantee worth keeping: an install that would come up on a **fresh,
//! empty database** while an older install's database sits untouched somewhere
//! else does not come up at all.
//!
//! Two outcomes say something. [`LegacyRoot::Stranded`] — the older directory
//! holds irreplaceable data and this install has no database yet — refuses to
//! start, because starting is what would create the empty database.
//! [`LegacyRoot::Residue`] — the older directory is still there beside a
//! database this install already uses — is one `WARN` per boot, and nothing
//! more: with no mover there is no choice between two databases to refuse over.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use directories::ProjectDirs;
use tracing::{debug, error, warn};

use super::DB_FILE;
use super::project_move::same_dir;

/// The pre-D1 application data directory (`app_dir()` as it was):
/// `~/Library/Application Support/OpenAlpaca` on macOS,
/// `~/.local/share/openalpaca` on Linux, `%APPDATA%\OpenAlpaca\data` on Windows.
///
/// `ProjectDirs` survives only here, to compute the *old* root — never to
/// resolve anything this install writes. The older `com.openalpaca.OpenAlpaca`
/// leg is deliberately not carried forward: that rename happened long ago, and
/// a surviving directory would simply be ignored.
///
/// It reads `HOME` (and `XDG_DATA_HOME` on Linux), not `OPENALPACA_HOME_STORE`:
/// a test that reaches it must sandbox those too.
pub fn legacy_app_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "OpenAlpaca").map(|p| p.data_dir().to_path_buf())
}

/// What a legacy root holds that must not be lost without a word: the database,
/// the key that decrypts the config, and the config itself.
const LEGACY_CRITICAL: [&str; 3] = ["openalpaca.db", ".master_key", "config"];

/// What is worth mentioning once but never worth refusing to start over.
///
/// Deliberately in neither list: `daemon.log`, `gui.log`, `repl_history`,
/// `discovery.json`, `openalpacad.lock`, and anything the OS or the user
/// dropped there (`.DS_Store`). Logs and a stale lock are not data, and a
/// directory holding only those is not a reason to say anything.
const LEGACY_NOTABLE: [&str; 4] = [
    "openalpaca.db-wal",
    "openalpaca.db-shm",
    "assets",
    "plugins",
];

/// What an older data directory means for this boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyRoot {
    /// No legacy root, or nothing in it this build would call data, or it *is*
    /// the root this install runs from.
    Absent,
    /// A legacy root holding something, beside an install that already has its
    /// own database — or holding nothing irreplaceable (a stray WAL, `assets/`,
    /// `plugins/`) whatever this install has. Worth one line; never worth
    /// refusing.
    Residue { holds_database: bool },
    /// A legacy root holding irreplaceable data, and no database at the new
    /// root: starting would create an empty one and strand it.
    Stranded,
}

/// Pure classification, so the rule can be driven from tests without touching
/// the process environment.
///
/// `legacy` is the old root; `home_root` is the root this install runs from;
/// `home_db` is `<home root>/state/openalpaca.db`. At most eight
/// `symlink_metadata` calls and one path comparison — no listing, no recursion,
/// nothing read from the files themselves.
pub fn classify_legacy_root(legacy: &Path, home_root: &Path, home_db: &Path) -> LegacyRoot {
    // `OPENALPACA_HOME_STORE` can be pointed at the legacy path itself. Then
    // there is no "old" directory — it is this install's root.
    if same_dir(legacy, home_root) {
        return LegacyRoot::Absent;
    }
    let holds = |name: &str| std::fs::symlink_metadata(legacy.join(name)).is_ok();
    let critical = LEGACY_CRITICAL.iter().any(|n| holds(n));
    let notable = LEGACY_NOTABLE.iter().any(|n| holds(n));

    if !critical && !notable {
        return LegacyRoot::Absent;
    }
    if critical && std::fs::symlink_metadata(home_db).is_err() {
        return LegacyRoot::Stranded;
    }
    LegacyRoot::Residue {
        holds_database: holds("openalpaca.db"),
    }
}

/// The boot check, for callers that report rather than exit (the CLI, and
/// [`super::open_home_database`]).
///
/// `Ok(())` for [`LegacyRoot::Absent`] and [`LegacyRoot::Residue`] — the latter
/// having logged one `WARN` on the way past. `Err` for [`LegacyRoot::Stranded`],
/// carrying the whole explanation.
pub fn check_legacy_root_result() -> Result<()> {
    let Some(legacy) = legacy_app_dir() else {
        debug!("Could not determine the legacy app directory; nothing to check");
        return Ok(());
    };
    let home_root = super::home_root()?;
    let home_db = super::database_path()?;
    match classify_legacy_root(&legacy, &home_root, &home_db) {
        LegacyRoot::Absent => {
            debug!("No older OpenAlpaca data directory at {}", legacy.display());
            Ok(())
        }
        LegacyRoot::Residue { holds_database } => {
            warn!("{}", residue_message(&legacy, &home_root, holds_database));
            Ok(())
        }
        LegacyRoot::Stranded => Err(anyhow!(stranded_message(&legacy, &home_root, &home_db))),
    }
}

/// The daemon's entry point: the same check, with the exit the boot preamble
/// wants. A half-configured install must never come up on an empty database.
pub fn check_legacy_root() {
    if let Err(e) = check_legacy_root_result() {
        error!("FATAL: {e:#}");
        std::process::exit(1);
    }
}

/// The refusal. It names both roots, the file that is missing, both ways out,
/// and why `assets/` must stay where it is — and nothing else. In particular it
/// does not name `openalpaca config reset --factory`: that verb deletes this
/// install's database, which does not exist here, and would not clear this.
pub(crate) fn stranded_message(legacy: &Path, home_root: &Path, home_db: &Path) -> String {
    let legacy = legacy.display();
    let root = home_root.display();
    let db = home_db.display();
    format!(
        "an older OpenAlpaca install's data is still on this machine, and this build\n\
         does not move it for you.\n\
         \n\
         \x20 Older install:  {legacy}\n\
         \x20 This install:   {root}\n\
         \x20 Missing here:   {db}\n\
         \n\
         Starting now would create an empty database at that path. The older install's\n\
         conversations, memories, tasks, encrypted config and .master_key would stay\n\
         where they are, with nothing pointing at them. Nothing has been changed, and\n\
         nothing is lost yet.\n\
         \n\
         To keep that data, with no OpenAlpaca process running, move it by hand:\n\
         \n\
         \x20 openalpaca.db, openalpaca.db-wal, openalpaca.db-shm, .master_key\n\
         \x20     -> {root}/state/\n\
         \x20 config/ and plugins/   (merge into what is already there; keep the newer\n\
         \x20                         file when a name exists on both sides)\n\
         \x20     -> {root}/\n\
         \x20 Leave assets/ where it is: uploaded files are addressed by absolute path,\n\
         \x20 so moving that directory would break the rows that point into it.\n\
         \n\
         To discard that data instead, rename or delete {legacy}.\n\
         \n\
         Either way, start again afterwards. This check does not run again once\n\
         {db} exists."
    )
}

/// The per-boot warning. It says where this install's database is, not that it
/// already exists: a residue of only `assets/` or `plugins/` warns on a first
/// boot too, before the daemon has created one.
pub(crate) fn residue_message(legacy: &Path, home_root: &Path, holds_database: bool) -> String {
    let home_db = home_root.join("state").join(DB_FILE);
    let mut message = format!(
        "An older OpenAlpaca data directory is still on this machine: {}.\n\
         This build does not move it, and reads nothing in it except uploaded files\n\
         that a database carried out of it still points at. This install runs from {}, and\n\
         its own database is {}.",
        legacy.display(),
        home_root.display(),
        home_db.display()
    );
    if holds_database {
        message.push_str(
            "\nThat directory holds an openalpaca.db of its own; this daemon is not using it.",
        );
    }
    message.push_str(
        "\nMove or delete the directory once you are sure you do not want what is in it,\n\
         and this warning stops. If you moved a database out of it by hand, check for\n\
         uploaded files still addressed by absolute path before deleting anything.",
    );
    message
}

#[cfg(test)]
mod tests;
