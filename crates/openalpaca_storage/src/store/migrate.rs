//! The one boot-time mover: the legacy app dir → `~/.openalpaca` (D1).
//!
//! `move_app_root()` runs from the daemon's boot preamble **after logging and
//! before the singleton lock** — the lock file itself moves, and the database
//! must not be open while it is renamed.
//!
//! Every step is idempotent and the whole function is re-runnable: each ledger
//! entry is a single `rename(2)` guarded by skip-if-destination-exists, so a
//! process killed mid-ledger resumes exactly where it stopped on the next boot.
//! There is no rollback and none is needed — no consumer opens any of these
//! files before the mover finishes.
//!
//! The one exception to skip-if-destination-exists is the database and its
//! sidecars: skipping there would boot the daemon on the wrong database, so
//! [`guard_database_trio`] decides the whole trio before the first rename and
//! aborts rather than choose. Anything that opens the database outside the
//! daemon's boot preamble goes through [`open_store_database`], which runs the
//! move first — `Database::open` creates what it does not find, and an empty
//! database at the destination is exactly what that guard then refuses.

use crate::database::Database;
use crate::store;
use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

/// The pre-D1 application data directory (`app_dir()` as it was):
/// `~/Library/Application Support/OpenAlpaca` on macOS,
/// `~/.local/share/openalpaca` on Linux, `%APPDATA%\OpenAlpaca\data` on Windows.
///
/// `ProjectDirs` survives only here, to compute the *old* root. The older
/// `com.openalpaca.OpenAlpaca` leg is deliberately not carried forward: that
/// rename happened long ago, and a surviving directory would simply be ignored.
pub fn legacy_app_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "OpenAlpaca").map(|p| p.data_dir().to_path_buf())
}

/// Moves the legacy app dir into the home store, then removes it if empty.
///
/// Aborts startup (`exit(1)`) on any failure, with the failing path in the log —
/// a half-moved root must never be booted on top of. The daemon's entry point;
/// [`move_app_root_result`] is the same move for callers that report rather than
/// exit, and [`move_root`] is the testable core.
pub fn move_app_root() {
    if let Err(e) = move_app_root_result() {
        error!("FATAL: {e:#}");
        std::process::exit(1);
    }
}

/// [`move_app_root`] without the `exit(1)` — for processes that print the error
/// and set an exit code themselves (the CLI).
pub fn move_app_root_result() -> Result<()> {
    let Some(old) = legacy_app_dir() else {
        warn!("Could not determine the legacy app directory; skipping the root move");
        return Ok(());
    };
    let new = store::home_root().context("cannot resolve the OpenAlpaca home root")?;
    move_root(&old, &new)
        .with_context(|| format!("cannot migrate {} to {}", old.display(), new.display()))
}

/// The store database, opened only after the root move has run to completion.
///
/// The daemon does these two things separately — the mover runs before the
/// singleton lock, the open long after. Every *other* process that opens the
/// database directly must do them in this order: `Database::open` **creates**
/// what it does not find, so opening the destination first would leave an empty
/// database in the way of the real one (which the trio guard then refuses to
/// boot past, rather than silently stranding it).
pub fn open_store_database() -> Result<Database> {
    let new = store::home_root().context("cannot resolve the OpenAlpaca home root")?;
    match legacy_app_dir() {
        Some(old) => move_then_open(&old, &new),
        None => open_moved_database(&new),
    }
}

fn move_then_open(old: &Path, new: &Path) -> Result<Database> {
    move_root(old, new)
        .with_context(|| format!("cannot migrate {} to {}", old.display(), new.display()))?;
    open_moved_database(new)
}

fn open_moved_database(new: &Path) -> Result<Database> {
    // Through `state_dir_in`, so a CLI that opens the database before any daemon
    // has run still gets the 0700 state directory the daemon would have made.
    let path = store::state_dir_in(new)?.join(DB_NAME);
    Database::open(&path).with_context(|| format!("Failed to open {}", path.display()))
}

/// Moves every ledger entry from `old` into `new`. See [`move_app_root`].
pub fn move_root(old: &Path, new: &Path) -> Result<()> {
    move_root_inner(old, new, None)
}

/// One entry of the move ledger.
#[derive(Debug)]
enum Step {
    /// Atomic rename, skipped when the destination already exists.
    Move { src: PathBuf, dst: PathBuf },
    /// Per-child merge: each child moves if absent at the destination.
    Merge { src: PathBuf, dst: PathBuf },
    /// Regenerated every boot — deleted rather than moved.
    Delete { path: PathBuf },
}

/// The ledger, in order. Sidecars precede the database: a crash mid-trio leaves
/// split halves, and the resume on the next boot reunites them *before*
/// `Database::open` runs, because the mover always completes first.
fn ledger(old: &Path, new: &Path) -> Vec<Step> {
    let state = new.join("state");
    let mv = |name: &str, dst: PathBuf| Step::Move {
        src: old.join(name),
        dst,
    };
    vec![
        mv(DB_SIDECARS[0], state.join(DB_SIDECARS[0])),
        mv(DB_SIDECARS[1], state.join(DB_SIDECARS[1])),
        mv(DB_NAME, state.join(DB_NAME)),
        mv(".master_key", state.join(".master_key")),
        Step::Merge {
            src: old.join("config"),
            dst: new.join("config"),
        },
        Step::Merge {
            src: old.join("plugins"),
            dst: new.join("plugins"),
        },
        mv("assets", state.join("assets")),
        mv("daemon.log", state.join("logs").join("daemon.log")),
        mv("gui.log", state.join("logs").join("gui.log")),
        mv("repl_history", state.join("repl_history")),
        Step::Delete {
            path: old.join("discovery.json"),
        },
        Step::Delete {
            path: old.join("openalpacad.lock"),
        },
    ]
}

/// The database file names, in move order: sidecars first, then the database.
const DB_NAME: &str = "openalpaca.db";
const DB_SIDECARS: [&str; 2] = ["openalpaca.db-wal", "openalpaca.db-shm"];

/// `stop_after: Some(n)` applies only the first `n` ledger entries and skips the
/// old-root disposal — it simulates a process killed mid-move, and exists so the
/// resume behaviour can be driven from tests.
fn move_root_inner(old: &Path, new: &Path, stop_after: Option<usize>) -> Result<()> {
    // 1. Fresh install / already moved.
    if !exists(old) {
        debug!("No legacy app dir at {}; nothing to move", old.display());
        return Ok(());
    }
    if same_dir(old, new) {
        debug!("Legacy app dir and home root are the same directory; nothing to move");
        return Ok(());
    }

    // 2. Live-daemon guard. Renaming a WAL-mode database out from under a live
    //    process is corruption; this is the one race the mover refuses to paper over.
    guard_no_live_daemon(old)?;

    // 2b. The database and its sidecars move as one, or not at all — and never
    //     onto a database that is already there. Both guards run before the
    //     first rename, so an abort leaves everything exactly as it was.
    guard_database_trio(old, new)?;

    // 3. Entry ledger.
    let steps = ledger(old, new);
    let limit = stop_after.unwrap_or(steps.len());
    for step in steps.into_iter().take(limit) {
        match step {
            Step::Move { src, dst } => move_entry(&src, &dst)?,
            Step::Merge { src, dst } => merge_children(&src, &dst)?,
            Step::Delete { path } => remove_stale(&path)?,
        }
    }
    if stop_after.is_some() {
        return Ok(());
    }

    // 5. Old root disposal.
    dispose_old_root(old);
    Ok(())
}

fn guard_no_live_daemon(old: &Path) -> Result<()> {
    let lock_path = old.join("openalpacad.lock");
    if !lock_path.exists() {
        return Ok(());
    }
    let lock_path_str = lock_path
        .to_str()
        .context("Legacy lock path is not valid UTF-8")?;
    let options = file_lock::FileOptions::new()
        .write(true)
        .create(true)
        .append(true);
    match file_lock::FileLock::lock(lock_path_str, false, options) {
        Ok(guard) => {
            drop(guard);
            Ok(())
        }
        Err(e) => bail!(
            "an old daemon is still running from {}; stop it first ({e})",
            old.display()
        ),
    }
}

/// The database trio is the one ledger group where skip-if-destination-exists is
/// unsafe, so it is decided up front rather than entry by entry.
///
/// SQLite pairs a WAL with a database by file name and validates only the WAL's
/// own checksums — there is no database-identity check. So moving a sidecar onto
/// a database that is not its own can apply foreign frames, and skipping the
/// database while its sidecars move does exactly that. It is reachable without
/// any concurrency: a CLI command that opened `state/openalpaca.db` before the
/// daemon's first boot leaves an empty database at the destination, and the
/// legacy one — conversations, memories, tasks, the local user id — would be
/// silently left behind.
///
/// Two databases are a question only their owner can answer, so both cases
/// abort with the paths named. Every other ledger entry keeps the skip rule.
fn guard_database_trio(old: &Path, new: &Path) -> Result<()> {
    let src_db = old.join(DB_NAME);
    let dst_db = new.join("state").join(DB_NAME);

    if exists(&src_db) {
        if exists(&dst_db) {
            bail!(
                "two databases: {} has not moved yet and {} already exists. \
                 Refusing to merge them — the sidecars of one would be read as the \
                 other's. Keep the one you want (the legacy file is the older \
                 install's data), remove or rename the other, and restart",
                src_db.display(),
                dst_db.display()
            );
        }
        // The database will move; the sidecars precede it as usual.
        return Ok(());
    }

    // No source database: any sidecar still in the old root belongs to a
    // database that has already moved (or is gone), so it must not travel to a
    // destination database it does not match.
    if exists(&dst_db) {
        for sidecar in DB_SIDECARS {
            let src = old.join(sidecar);
            if exists(&src) {
                bail!(
                    "{} is left over from an interrupted move, but {} is already in place \
                     and no database accompanies the sidecar. A write-ahead log from a \
                     different database can corrupt this one: delete the leftover file \
                     and restart",
                    src.display(),
                    dst_db.display()
                );
            }
        }
    }
    Ok(())
}

/// One atomic rename, guarded by skip-if-destination-exists.
fn move_entry(src: &Path, dst: &Path) -> Result<()> {
    if !exists(src) {
        return Ok(());
    }
    if exists(dst) {
        warn!(
            "Not moving {}: {} already exists — keeping the destination",
            src.display(),
            dst.display()
        );
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    match fs::rename(src, dst) {
        Ok(()) => {
            info!("Moved {} → {}", src.display(), dst.display());
            Ok(())
        }
        Err(e) if is_cross_device(&e) => bail!(
            "cannot move {} to {}: they are on different volumes, so the move \
             cannot be atomic; move the directory by hand and restart",
            src.display(),
            dst.display()
        ),
        Err(e) => {
            Err(e).with_context(|| format!("Failed to move {} to {}", src.display(), dst.display()))
        }
    }
}

/// Per-child merge, not skip-if-dir-exists: a rebuilt GUI pre-creates
/// `home_root()/config` before spawning the daemon, so the destination existing
/// is expected. Each child moves if absent at the destination.
fn merge_children(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dst).with_context(|| format!("Failed to create {}", dst.display()))?;
    let entries = fs::read_dir(src).with_context(|| format!("Failed to read {}", src.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("Failed to read {}", src.display()))?;
        move_entry(&entry.path(), &dst.join(entry.file_name()))?;
    }
    // Empty now unless a child was kept at the destination.
    let _ = fs::remove_dir(src);
    Ok(())
}

fn remove_stale(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {
            debug!("Removed stale {}", path.display());
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("Failed to remove {}", path.display())),
    }
}

fn dispose_old_root(old: &Path) {
    let removal = match fs::remove_dir(old) {
        Ok(()) => {
            info!("Removed the legacy app dir {}", old.display());
            return;
        }
        Err(e) => e,
    };
    // The directory survived: say so either way — an unreadable leftover root is
    // the case most worth reporting, not the one to stay silent about.
    match fs::read_dir(old) {
        Ok(entries) => {
            let leftovers: Vec<String> = entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            warn!(
                "Left {} in place: it still contains {}",
                old.display(),
                if leftovers.is_empty() {
                    format!("nothing this mover recognises ({removal})")
                } else {
                    leftovers.join(", ")
                }
            );
        }
        Err(e) => warn!(
            "Left {} in place and cannot list it: {e} (removal failed with: {removal})",
            old.display()
        ),
    }
}

/// `exists()` that also sees a broken symlink.
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(unix)]
fn is_cross_device(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::CrossesDevices || e.raw_os_error() == Some(libc::EXDEV)
}

#[cfg(not(unix))]
fn is_cross_device(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::CrossesDevices
}

// ============================================================================
// Post-open fixup
// ============================================================================

/// Repairs the absolute `file_assets.storage_path` values the root move broke.
///
/// Called from boot immediately after `Database::open` and before any ingress.
/// Not a numbered migration — the prefixes are runtime-computed. Runs every boot
/// and matches zero rows after the first.
pub fn rebase_asset_paths(db: &Database) {
    let Some(old) = legacy_app_dir() else {
        return;
    };
    let new = match store::home_root() {
        Ok(p) => p,
        Err(e) => {
            warn!("Skipping asset path rebase: {e:#}");
            return;
        }
    };
    match rebase_asset_paths_between(db, &old, &new) {
        Ok(0) => {}
        Ok(n) => info!("Rebased {n} file asset path(s) onto {}", new.display()),
        // Not fatal — nothing else at boot depends on it — but it is the reason
        // an uploaded file later fails to open, so name the database it could
        // not repair, like every other error path in this file.
        Err(e) => warn!(
            "Failed to rebase file asset paths in {}: {e:#}; \
             files uploaded before the move may not resolve",
            database_file(db)
        ),
    }
}

/// The file backing `db`, for error messages.
fn database_file(db: &Database) -> String {
    db.with_connection(|conn| Ok(conn.path().map(|p| p.to_string())))
        .ok()
        .flatten()
        .unwrap_or_else(|| "the database".to_string())
}

/// The testable core of [`rebase_asset_paths`]; returns the number of rows changed.
///
/// Anchored prefix replacement rather than `replace()` + `LIKE`: a home directory
/// containing `_` or `%` would make a `LIKE` pattern over-match (reporting rows it
/// did not change), and `replace()` would rewrite mid-string occurrences too.
pub fn rebase_asset_paths_between(db: &Database, old: &Path, new: &Path) -> Result<usize> {
    let old_prefix = dir_prefix(&old.join("assets"));
    let new_prefix = dir_prefix(&new.join("state").join("assets"));
    if old_prefix == new_prefix {
        return Ok(0);
    }
    db.with_connection(|conn| {
        let changed = conn
            .execute(
                "UPDATE file_assets
                    SET storage_path = ?2 || substr(storage_path, length(?1) + 1)
                  WHERE substr(storage_path, 1, length(?1)) = ?1",
                rusqlite::params![old_prefix, new_prefix],
            )
            .with_context(|| {
                format!("Failed to rewrite file_assets.storage_path {old_prefix}* to {new_prefix}*")
            })?;
        Ok(changed)
    })
}

fn dir_prefix(dir: &Path) -> String {
    format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR)
}

#[cfg(test)]
mod tests;
