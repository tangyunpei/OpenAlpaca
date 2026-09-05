//! `UploadStore` — the one writer for upload bytes and upload rows (plan D2).
//!
//! `file_assets` is one table with two byte-writers, split by `origin`:
//!
//! - [`crate::ArtifactStore`] writes everything an agent *produced*
//!   (`origin = 'produced'`), plus `artifact_versions`.
//! - `UploadStore` writes everything a human *uploaded* (`origin = 'upload'`).
//!   `POST /v1/files/upload` and the connector attachment path
//!   (`openalpaca_connectors::common::store_attachment`) both go through it, so
//!   there is exactly one place that hashes the bytes, applies the owner-scoped
//!   sha256 dedup, places the file and inserts the row.
//!   [`crate::FileAssetRepository`] stays the read/CRUD surface for those rows.
//!
//! ## Placement (D2)
//!
//! ```text
//! <store>/uploads/<YYYY-MM-DD>/NN-<slug(original_name,60)>.<ext>
//! ```
//!
//! built from `crate::store::{upload_dir, upload_file_name, confine_to_root}` —
//! this module never joins a literal directory name onto a store root. The
//! store is the *request's*: `POST /v1/files/upload` resolves its
//! `x-workspace-path` through `MemoryScopeContext::for_request` and passes the
//! project; a connector attachment carries no project signal at all and passes
//! [`StoreScope::Home`].
//!
//! The address of record is `project_root` + `rel_path` (relative to
//! `<store>/uploads`); `storage_path` stays the resolved absolute path, so the
//! content and open routes need no change and rows written before D2 — the
//! content-addressed blobs under `state/assets/` — keep resolving exactly as
//! they did. Re-homing those is Phase 8's job, not this writer's.
//!
//! **Dedup is the owner-scoped sha256 query, never the path.** Two uploads of
//! the same bytes by the same owner resolve to the first row and write nothing —
//! including when that first row is a pre-D2 content-addressed one. The same
//! bytes from a different owner get their own row and their own file.
//!
//! ## Write protocol (§4.2)
//!
//! ```text
//! 1. write  <dir>/.<stem>.tmp
//! 2. fsync  the tmp file
//! 3. rename <dir>/.<stem>.tmp -> <dir>/<NN-stem.ext>
//! ```
//!
//! No rotation step: an upload is bytes that arrived once, so it has no
//! versions to supersede. Every failure path removes the tmp file, and the
//! rename is followed by an `fsync` of the directory it changed.
//!
//! Everything from the dedup query to the committed row runs inside one
//! `with_connection` transaction, which holds the database mutex for its whole
//! duration — that is what serialises concurrent puts, so two uploads landing in
//! the same day directory can never be handed the same `NN` and rename over each
//! other. The bytes are in place before the transaction commits, so a failure
//! anywhere leaves at worst an unreferenced file, never a row pointing at
//! nothing.

use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::Database;
use crate::content_io::{fsync_dir, remove_best_effort, sha256_hex};
use crate::models::ArtifactOrigin;
use crate::models::file_asset::{FileAsset, FileAssetStatus};
use crate::repository::file_asset::{FILE_ASSET_COLUMNS, row_to_file_asset};
use crate::store::{
    ContentKind, StoreScope, confine_to_root, content_dir, leading_sequence, project_root_of,
    relative_to, upload_dir, upload_file_name,
};

// ============================================================================
// Errors
// ============================================================================

/// The typed failures a caller branches on. Returned inside `anyhow::Error`, so
/// a route recovers them with `err.downcast_ref::<UploadError>()` and maps
/// [`UploadError::code`] onto the error code it already returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadError {
    /// The storage path could not be computed.
    Path(String),
    /// Creating the directory or writing the bytes failed.
    Io(String),
    /// The row could not be inserted or read back.
    Db(String),
}

impl UploadError {
    /// The stable error code a route puts in `{error:{code:…}}`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Path(_) => "PATH_ERROR",
            Self::Io(_) => "IO_ERROR",
            Self::Db(_) => "DB_ERROR",
        }
    }
}

impl fmt::Display for UploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(m) | Self::Io(m) | Self::Db(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for UploadError {}

fn path_error(e: impl fmt::Display) -> anyhow::Error {
    anyhow::Error::new(UploadError::Path(format!(
        "Failed to compute storage path: {e}"
    )))
}

fn io_error(message: String) -> anyhow::Error {
    anyhow::Error::new(UploadError::Io(message))
}

fn db_error(message: String) -> anyhow::Error {
    anyhow::Error::new(UploadError::Db(message))
}

// ============================================================================
// Inputs and outputs
// ============================================================================

/// One upload to store.
pub struct NewUpload<'a> {
    pub owner_id: &'a str,
    /// The name the client sent, kept verbatim in `file_assets.filename` — it
    /// is what the user sees and what `Content-Disposition` echoes back. The
    /// slugified form of it names the file on disk.
    pub filename: &'a str,
    pub mime_type: &'a str,
    pub data: &'a [u8],
    /// Which store the bytes land in. The request's project when the client
    /// named one, [`StoreScope::Home`] otherwise — connector attachments always
    /// take the home store, having no project signal to offer.
    pub scope: &'a StoreScope,
    /// Names the `uploads/<YYYY-MM-DD>` directory. Normally `Utc::now()`.
    pub created: DateTime<Utc>,
}

/// What [`UploadStore::put`] resolved to.
#[derive(Debug, Clone)]
pub struct StoredUpload {
    /// The row — the *existing* one when `deduped`.
    pub asset: FileAsset,
    /// `true` when an owner-scoped sha256 match returned an existing row and
    /// nothing was written to disk or to the database.
    pub deduped: bool,
}

// ============================================================================
// Store
// ============================================================================

/// The one writer for upload rows.
pub struct UploadStore<'a> {
    db: &'a Database,
}

impl<'a> UploadStore<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Hash, dedup, place, insert — in that order.
    ///
    /// Callers do their own admission control first (MIME and magic-byte
    /// validation, the size cap, the storage quota): those are policy the route
    /// and the connectors already own and disagree about deliberately, and none
    /// of it decides where the bytes go.
    pub fn put(&self, new: NewUpload<'_>) -> Result<StoredUpload> {
        let sha256 = sha256_hex(new.data);
        let size_bytes = new.data.len() as i64;

        // Placement, by the §4.2 grammar. Creates directories only — `upload_dir`
        // goes through `content_dir`, which seeds a project store (and its
        // `.gitignore`) before `uploads/` can exist inside it.
        let uploads_root = content_dir(new.scope, ContentKind::Uploads).map_err(path_error)?;
        let uploads_root = uploads_root.canonicalize().map_err(path_error)?;
        let dir = upload_dir(new.scope, new.created).map_err(path_error)?;
        let rel_dir = relative_to(&uploads_root, &dir).map_err(path_error)?;
        let project_root = project_root_of(new.scope).map_err(path_error)?;
        let project_key = project_root.clone().unwrap_or_default();

        self.db.with_connection(|conn| {
            let tx = conn.unchecked_transaction()?;

            // Dedup: the owner-scoped sha256 query, never the path. A pre-D2
            // content-addressed row answers it just as well as a new one.
            if let Some(existing) = load_by_sha256(&tx, &sha256)?
                && existing.owner_id == new.owner_id
            {
                return Ok(StoredUpload {
                    asset: existing,
                    deduped: true,
                });
            }
            // Same content, different owner — fall through and create a new row.

            let seq = next_sequence(&tx, &project_key, &rel_dir)?;
            let head_name = upload_file_name(seq, new.filename);
            let head_path =
                confine_to_root(&uploads_root, &dir.join(&head_name)).map_err(path_error)?;
            let rel_path = format!("{rel_dir}/{head_name}");

            fs::create_dir_all(&dir)
                .map_err(|e| io_error(format!("Failed to create storage directory: {e}")))?;
            write_bytes(&uploads_root, &dir, &head_path, &head_name, new.data)?;

            let id = uuid::Uuid::new_v4().to_string();
            let insert = tx.execute(
                "INSERT INTO file_assets
                    (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path,
                     status, origin, project_root, rel_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    id,
                    new.owner_id,
                    sha256,
                    new.filename,
                    new.mime_type,
                    size_bytes,
                    head_path.to_string_lossy(),
                    // Uploaded bytes still owe the background extractor a pass.
                    FileAssetStatus::Uploaded.as_str(),
                    ArtifactOrigin::Upload.as_str(),
                    project_root,
                    rel_path,
                ],
            );
            if let Err(e) = insert {
                // The row is what makes the bytes reachable; without it the file
                // is unreferenced garbage.
                remove_best_effort(&head_path);
                return Err(db_error(format!("Failed to insert file record: {e}")));
            }

            let asset = load_by_id(&tx, &id)?.ok_or_else(|| {
                db_error(format!("Upload {id} vanished inside its own transaction"))
            })?;
            tx.commit()?;
            Ok(StoredUpload {
                asset,
                deduped: false,
            })
        })
    }
}

// ============================================================================
// Internals
// ============================================================================

fn load_by_sha256(conn: &Connection, sha256: &str) -> Result<Option<FileAsset>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_ASSET_COLUMNS} FROM file_assets WHERE sha256 = ?1 LIMIT 1"
    ))?;
    let mut rows = stmt.query(rusqlite::params![sha256])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_file_asset(row)?)),
        None => Ok(None),
    }
}

fn load_by_id(conn: &Connection, id: &str) -> Result<Option<FileAsset>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_ASSET_COLUMNS} FROM file_assets WHERE id = ?1"
    ))?;
    let mut rows = stmt.query(rusqlite::params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_file_asset(row)?)),
        None => Ok(None),
    }
}

/// The next free `NN` in a day directory: one past the highest sequence any
/// upload row already addresses there.
///
/// Rows, not directory entries, are the source of truth — the same rule
/// `ArtifactStore` follows — so a file the sweep deleted alongside its row
/// frees its number, and a file left behind without one does not reserve it.
/// Two digits, widening past 99, because [`upload_file_name`] formats it.
///
/// `rel_dir` is a `<YYYY-MM-DD>` day directory: digits and hyphens only, so it
/// carries no `LIKE` metacharacter and the pattern needs no `ESCAPE` clause.
fn next_sequence(conn: &Connection, project_key: &str, rel_dir: &str) -> Result<u32> {
    let prefix = format!("{rel_dir}/");
    let mut stmt = conn.prepare(
        "SELECT rel_path FROM file_assets
         WHERE origin = 'upload' AND COALESCE(project_root, '') = ?1
           AND rel_path IS NOT NULL AND rel_path LIKE ?2",
    )?;
    let mut rows = stmt.query(rusqlite::params![project_key, format!("{prefix}%")])?;
    let mut highest = 0;
    while let Some(row) = rows.next()? {
        let rel_path: String = row.get(0)?;
        // LIKE is a coarse filter; the exact test is a direct child of the dir.
        let Some(file_name) = rel_path.strip_prefix(&prefix) else {
            continue;
        };
        if file_name.contains('/') {
            continue;
        }
        if let Some(seq) = leading_sequence(file_name) {
            highest = highest.max(seq);
        }
    }
    Ok(highest + 1)
}

/// The §4.2 write protocol, minus the rotation an upload has no use for: write
/// `.<stem>.tmp` → fsync → rename onto the head path. Every failure path removes
/// the tmp file, so a write that never completes strands nothing in the day
/// directory.
fn write_bytes(
    uploads_root: &Path,
    dir: &Path,
    head_path: &Path,
    head_name: &str,
    data: &[u8],
) -> Result<()> {
    let stem = Path::new(head_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("upload file name has no stem: {head_name}"))
        .map_err(path_error)?;
    let tmp =
        confine_to_root(uploads_root, &dir.join(format!(".{stem}.tmp"))).map_err(path_error)?;

    let outcome = write_bytes_steps(&tmp, dir, head_path, data);
    if outcome.is_err() {
        remove_best_effort(&tmp);
    }
    outcome
}

/// The three steps themselves. Separated from [`write_bytes`] only so the tmp
/// file has exactly one cleanup site.
fn write_bytes_steps(tmp: &Path, dir: &Path, head_path: &Path, data: &[u8]) -> Result<()> {
    let mut file = fs::File::create(tmp)
        .map_err(|e| io_error(format!("Failed to create {}: {e}", tmp.display())))?;
    file.write_all(data)
        .and_then(|()| file.sync_all())
        .map_err(|e| io_error(format!("Failed to write file: {e}")))?;
    drop(file);

    fs::rename(tmp, head_path).map_err(|e| {
        io_error(format!(
            "Failed to move {} into place at {}: {e}",
            tmp.display(),
            head_path.display()
        ))
    })?;
    fsync_dir(dir);
    Ok(())
}

#[cfg(test)]
mod tests;
