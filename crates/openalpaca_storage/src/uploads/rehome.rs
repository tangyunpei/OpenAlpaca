//! The one boot-time re-home: pre-D2 upload blobs → `uploads/` (Phase 8.3).
//!
//! Uploads written before D2 are content-addressed: `state/assets/ab/cd/<sha>`,
//! a row with `origin = 'upload'` and no `rel_path`. This pass gives each of them
//! the D2 address every new upload already has —
//! `uploads/<created-date>/NN-<name>.<ext>`, `project_root` + `rel_path` — and
//! then removes the interim directory. It is the last thing in the system that
//! knows `state/assets` exists.
//!
//! Same discipline as the root mover ([`crate::store::migrate`]): every step is
//! idempotent, the whole pass is re-runnable, and a process killed anywhere in it
//! resumes on the next boot. Nothing here is fatal — a row it cannot move keeps
//! resolving from where it is, exactly as it did before.
//!
//! ## Per row
//!
//! ```text
//! 0. reserve  uploads/<date>/NN-<slug>.<ext>   with O_EXCL — never overwrite
//! 1. copy     the blob through .<stem>.tmp → fsync → rename
//! 2. UPDATE   storage_path / rel_path / project_root, in one transaction
//! 3. unlink   the old blob, once no row addresses it any more
//! ```
//!
//! The row is the commit, so the boundaries fall where they do: a crash before
//! step 2 leaves a file no row references (reclaimed by [`reclaim_row_less_head`]
//! on the next pass, R33 — this pass runs before any ingress, so bytes at an
//! unreferenced upload address can only be its own residue), and a crash between
//! step 2 and step 3 leaves the old blob behind, which keeps the interim
//! directory alive with a warning rather than deleting something unaccounted for.
//!
//! ## Two rows, one blob
//!
//! Pre-D2 placement was content-addressed, so two rows — a second owner's upload
//! of the same bytes — can share one file. Each gets **its own copy** at its own
//! D2 address: the address is per row (§4.3), and the alternative, two rows
//! pointing at one file, would make either row's deletion silently break the
//! other. Step 3 is what makes that safe: the blob is unlinked only once no row
//! addresses it any more, so the second row still finds its source.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::Connection;
use tracing::{debug, info, warn};

use super::{next_sequence, reserve_head, write_bytes};
use crate::Database;
use crate::content_io::remove_best_effort;
use crate::store::{
    ContentKind, StoreScope, content_dir, interim_assets_dir, project_root_at, relative_to,
    upload_dir, upload_file_name,
};

/// Re-homes every pre-D2 upload, then disposes of the interim directory.
///
/// Called from the daemon's boot preamble, after the root move and
/// [`crate::store::migrate::rebase_asset_paths`] (whose rewritten `storage_path`
/// values are this pass's input) and before any sweep or ingress. Idempotent:
/// after the first successful pass there are no rows to move and no directory to
/// remove, and it does nothing but one query.
pub fn rehome_pre_d2_uploads(db: &Database) {
    rehome_inner(db, None);
}

/// What one pass did, for the summary line and the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RehomeSummary {
    /// Rows given a D2 address.
    moved: usize,
    /// Rows whose blob is not on disk — left exactly as they are.
    missing: usize,
    /// Rows that failed for any other reason.
    failed: usize,
    /// Rows still carrying no `rel_path` when the pass ended.
    remaining: usize,
}

/// Where a pass may be cut off, to drive the resume behaviour from tests — the
/// [`crate::store::migrate`] mover's `stop_after`, per row. The pass stops after
/// applying `Stop` to the first row it processes, which is what a process killed
/// at that instant leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// After the O_EXCL reservation, before the bytes.
    Reserve,
    /// After the bytes are in place, before the transaction commits.
    Copy,
    /// After the commit, before the old blob is unlinked.
    Commit,
}

fn rehome_inner(db: &Database, stop_after: Option<Stop>) -> RehomeSummary {
    let mut summary = RehomeSummary::default();
    let rows = match pre_d2_rows(db) {
        Ok(rows) => rows,
        Err(e) => {
            warn!("Skipping the pre-D2 upload re-home: {e:#}");
            return summary;
        }
    };
    summary.remaining = rows.len();

    if !rows.is_empty() {
        let placement = match Placement::resolve() {
            Ok(p) => p,
            Err(e) => {
                warn!("Skipping the pre-D2 upload re-home: {e:#}");
                return summary;
            }
        };
        for row in &rows {
            match rehome_row(db, &placement, row, stop_after) {
                Ok(RowOutcome::Moved) => summary.moved += 1,
                Ok(RowOutcome::MissingSource) => summary.missing += 1,
                Ok(RowOutcome::Stopped) => {}
                Err(e) => {
                    summary.failed += 1;
                    warn!("Failed to re-home upload {}: {e:#}", row.id);
                }
            }
            if stop_after.is_some() {
                // The kill this injection stands in for: nothing after it runs.
                return summary;
            }
        }
        // A missing-blob row leaves the `remaining` set once marked (Important
        // 1, fix round 1): its bytes are gone either way, and holding
        // `state/assets` open for it forever would make the pass's own
        // deliverable unreachable on a plausible install.
        summary.remaining -= summary.moved + summary.missing;
        info!(
            "Re-homed {} pre-D2 upload(s) into uploads/ ({} blob(s) missing, {} failed, {} left)",
            summary.moved, summary.missing, summary.failed, summary.remaining
        );
    }

    if summary.remaining == 0 {
        dispose_interim_assets();
    }
    summary
}

/// One row that predates D2: `origin = 'upload'` with no address.
#[derive(Debug)]
struct PreD2Row {
    id: String,
    filename: String,
    storage_path: String,
    created: DateTime<Utc>,
    /// Set once [`mark_missing`] has stamped this row on an earlier pass —
    /// read so a later pass does not warn about it again (Important 1, fix
    /// round 1).
    missing_since: Option<String>,
}

fn pre_d2_rows(db: &Database) -> Result<Vec<PreD2Row>> {
    db.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, filename, storage_path, created_at, missing_since FROM file_assets
              WHERE origin = 'upload' AND rel_path IS NULL
              ORDER BY created_at, id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let created_at: String = row.get(3)?;
            let created = created_date(&created_at, &id);
            out.push(PreD2Row {
                id,
                filename: row.get(1)?,
                storage_path: row.get(2)?,
                created,
                missing_since: row.get(4)?,
            });
        }
        Ok(out)
    })
}

/// The day directory a row's bytes belong in.
///
/// `created_at` is SQLite's `datetime('now')` spelling (`YYYY-MM-DD HH:MM:SS`),
/// and only its date names the directory. A value that is not one — a row
/// written by something else, or a corrupted column — takes today rather than
/// costing the row its move: the address has to be *some* day, and the row's
/// own `created_at` stays the record of when it arrived.
fn created_date(created_at: &str, id: &str) -> DateTime<Utc> {
    let parsed = created_at
        .get(..10)
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc());
    match parsed {
        Some(dt) => dt,
        None => {
            warn!(
                "Upload {id} has an unreadable created_at ({created_at:?}); filing it under today"
            );
            Utc::now()
        }
    }
}

/// The home store's upload placement context, resolved once per pass.
///
/// Pre-D2 uploads carried no project signal — `state/assets` is under the home
/// root and nothing else — so they re-home into the home store, and
/// [`project_root_at`] answers `None` for it exactly as [`super::UploadStore::put`]
/// computes it.
struct Placement {
    uploads_root: PathBuf,
    project_root: Option<String>,
    project_key: String,
}

impl Placement {
    fn resolve() -> Result<Self> {
        let uploads_root = content_dir(&StoreScope::Home, ContentKind::Uploads)?;
        let uploads_root = uploads_root
            .canonicalize()
            .with_context(|| format!("Failed to resolve {}", uploads_root.display()))?;
        let store_root = uploads_root
            .parent()
            .with_context(|| format!("{} has no parent store root", uploads_root.display()))?;
        let project_root = project_root_at(store_root)?;
        let project_key = project_root.clone().unwrap_or_default();
        Ok(Self {
            uploads_root,
            project_root,
            project_key,
        })
    }
}

enum RowOutcome {
    Moved,
    /// The blob is not on disk. The row keeps its `storage_path` — a row with no
    /// bytes is the file routes' 404 either way, and rewriting its address would
    /// only move the hole. Marked `missing_since` (once — see [`mark_missing`])
    /// so it stops blocking disposal and this warning does not repeat every
    /// boot (Important 1, fix round 1).
    MissingSource,
    /// A `stop_after` injection ended the pass mid-row.
    Stopped,
}

fn rehome_row(
    db: &Database,
    placement: &Placement,
    row: &PreD2Row,
    stop_after: Option<Stop>,
) -> Result<RowOutcome> {
    let source = PathBuf::from(&row.storage_path);
    let data = match fs::read(&source) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if row.missing_since.is_none() {
                mark_missing(db, &row.id)
                    .with_context(|| format!("Failed to mark upload {} missing", row.id))?;
                warn!(
                    "Upload {} has no bytes at {}; marking it missing so this warning \
                     does not repeat",
                    row.id,
                    source.display()
                );
            }
            return Ok(RowOutcome::MissingSource);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read {}", source.display()));
        }
    };

    let dir = upload_dir(&StoreScope::Home, row.created)?;
    let rel_dir = relative_to(&placement.uploads_root, &dir)?;
    fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;

    // Everything from the sequence lookup to the committed row runs in one
    // transaction, exactly as `UploadStore::put` does: the bytes are in place
    // before the commit, so a failure leaves at worst an unreferenced file.
    let (stopped, old_unreferenced) = db.with_connection(|conn| {
        let tx = conn.unchecked_transaction()?;
        let seq = next_sequence(&tx, &placement.project_key, &rel_dir)?;
        reclaim_row_less_head(&tx, placement, &dir, &rel_dir, seq, &row.filename)?;
        let (head_name, head_path) =
            reserve_head(&placement.uploads_root, &dir, seq, &row.filename)?;
        if stop_after == Some(Stop::Reserve) {
            return Ok((true, false));
        }

        if let Err(e) = write_bytes(&placement.uploads_root, &dir, &head_path, &head_name, &data) {
            remove_best_effort(&head_path);
            return Err(e);
        }
        if stop_after == Some(Stop::Copy) {
            return Ok((true, false));
        }

        let rel_path = format!("{rel_dir}/{head_name}");
        if let Err(e) = tx.execute(
            "UPDATE file_assets SET storage_path = ?2, rel_path = ?3, project_root = ?4
              WHERE id = ?1",
            rusqlite::params![
                row.id,
                head_path.to_string_lossy(),
                rel_path,
                placement.project_root,
            ],
        ) {
            remove_best_effort(&head_path);
            return Err(e).with_context(|| format!("Failed to re-address upload {}", row.id));
        }

        // Asked *after* the UPDATE, so this row no longer counts: what is left is
        // the other rows that were sharing the blob.
        let old_unreferenced = !storage_path_is_referenced(&tx, &row.storage_path)?;
        tx.commit()?;
        Ok((false, old_unreferenced))
    })?;

    if stopped {
        return Ok(RowOutcome::Stopped);
    }
    if stop_after == Some(Stop::Commit) {
        return Ok(RowOutcome::Stopped);
    }
    if old_unreferenced {
        remove_best_effort(&source);
    }
    Ok(RowOutcome::Moved)
}

/// Stamps `missing_since` on a row whose blob is gone — T23's own convention
/// (the artifact store's `verify`, `store/artifacts/mod.rs:1011`), reused here
/// rather than invented. Idempotent by construction: [`rehome_row`] calls this
/// only the first time a row's blob is found missing, so the boot log warns
/// once and a marked row stops blocking [`dispose_interim_assets`] forever
/// (Important 1, fix round 1).
fn mark_missing(db: &Database, id: &str) -> Result<()> {
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE file_assets SET missing_since = datetime('now'), updated_at = datetime('now')
              WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    })
}

/// R33 at boot: a head that no upload row addresses is the residue of a re-home
/// that died before its commit, so it is removed and its name reused.
///
/// The rule the artifact store applies to its own heads, and it is safe here for
/// a reason that pass cannot claim: this one runs in the boot preamble, before
/// any ingress, so nothing else can be writing an upload. Bytes standing at an
/// address no row holds can only be this pass's own leftovers.
///
/// Only the first candidate name is examined. [`reserve_head`] probes forward
/// from there, and a name held by a file some row *does* reference stays taken —
/// this store does not destroy bytes it can account for.
fn reclaim_row_less_head(
    conn: &Connection,
    placement: &Placement,
    dir: &Path,
    rel_dir: &str,
    seq: u32,
    filename: &str,
) -> Result<()> {
    let head_name = upload_file_name(seq, filename);
    let head_path = dir.join(&head_name);
    let Ok(meta) = fs::symlink_metadata(&head_path) else {
        return Ok(());
    };
    if !meta.is_file() {
        return Ok(());
    }
    let rel_path = format!("{rel_dir}/{head_name}");
    if rel_path_is_referenced(conn, &placement.project_key, &rel_path)? {
        return Ok(());
    }
    warn!(
        "Reclaiming {} ({} bytes): no upload row addresses it, so it is the \
         residue of an interrupted re-home",
        head_path.display(),
        meta.len()
    );
    remove_best_effort(&head_path);
    Ok(())
}

/// Does any upload row of this store address `rel`?
fn rel_path_is_referenced(conn: &Connection, project_key: &str, rel: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM file_assets
              WHERE origin = 'upload' AND COALESCE(project_root, '') = ?1 AND rel_path = ?2
         )",
        rusqlite::params![project_key, rel],
        |row| row.get(0),
    )?)
}

/// Does any row still point at these bytes? Asked of every row, not just the
/// pre-D2 ones: what makes the blob deletable is that nothing at all reads it.
fn storage_path_is_referenced(conn: &Connection, storage_path: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM file_assets WHERE storage_path = ?1)",
        rusqlite::params![storage_path],
        |row| row.get(0),
    )?)
}

/// Removes the interim directory once nothing addresses anything inside it.
///
/// Called only when no pre-D2 row is left. A file still standing there is
/// something this pass did not account for — the old blob of a row whose unlink
/// never ran, or something a human put there — so the directory is kept and its
/// contents named. Deleting what we cannot explain is not this pass's call.
fn dispose_interim_assets() {
    let dir = match interim_assets_dir() {
        Ok(dir) => dir,
        Err(e) => {
            warn!("Cannot resolve the interim asset directory: {e:#}");
            return;
        }
    };
    if !dir.exists() {
        debug!("No interim asset directory at {}", dir.display());
        return;
    }
    match files_under(&dir) {
        Ok(leftovers) if leftovers.is_empty() => match fs::remove_dir_all(&dir) {
            Ok(()) => info!("Removed the interim asset directory {}", dir.display()),
            Err(e) => warn!("Failed to remove {}: {e}", dir.display()),
        },
        Ok(leftovers) => warn!(
            "Left {} in place: it still holds {} file(s), the first of them {}",
            dir.display(),
            leftovers.len(),
            leftovers[0].display()
        ),
        Err(e) => warn!("Left {} in place and cannot list it: {e:#}", dir.display()),
    }
}

/// Every non-directory entry under `dir`, recursively. A symlink counts as a
/// file: it is something a human put there, and `remove_dir_all` would take it.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .with_context(|| format!("Failed to read {}", current.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("Failed to read {}", current.display()))?;
            let path = entry.path();
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => stack.push(path),
                _ => found.push(path),
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests;
