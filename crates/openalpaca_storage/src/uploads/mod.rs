//! `UploadStore` — the one writer for upload bytes and upload rows.
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
//! Collapsing those two callers is a prerequisite for D2, not a tidy-up: the
//! route and the connector each carried their own copy of the same five steps,
//! so every change to upload placement had to land twice, correctly, or the two
//! halves of the same table would disagree about where a file lives.
//!
//! **Dedup is the owner-scoped sha256 query, never the path.** Two uploads of
//! the same bytes by the same owner resolve to the first row and write nothing;
//! the same bytes from a different owner get their own row and their own file.

use std::fmt;
use std::fs;

use anyhow::{Context, Result};

use crate::Database;
use crate::content_io::{remove_best_effort, sha256_hex};
use crate::models::file_asset::{FileAsset, FileAssetStatus};
use crate::repository::FileAssetRepository;
use crate::store;

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

fn db_error(e: impl fmt::Display) -> anyhow::Error {
    anyhow::Error::new(UploadError::Db(format!(
        "Failed to insert file record: {e}"
    )))
}

// ============================================================================
// Inputs and outputs
// ============================================================================

/// One upload to store.
pub struct NewUpload<'a> {
    pub owner_id: &'a str,
    /// The name the client sent, kept verbatim in `file_assets.filename` — it
    /// is what the user sees and what `Content-Disposition` echoes back.
    pub filename: &'a str,
    pub mime_type: &'a str,
    pub data: &'a [u8],
}

/// What [`UploadStore::put`] resolved to.
#[derive(Debug, Clone)]
pub struct StoredUpload {
    /// The head row — the *existing* one when `deduped`.
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
        let repo = FileAssetRepository::new(self.db);

        // Dedup: the owner-scoped sha256 query, never the path.
        if let Some(existing) = repo.get_by_sha256(&sha256).map_err(|e| {
            anyhow::Error::new(UploadError::Db(format!("Failed to look up file: {e}")))
        })? && existing.owner_id == new.owner_id
        {
            return Ok(StoredUpload {
                asset: existing,
                deduped: true,
            });
        }
        // Same content, different owner — fall through and create a new record.

        let storage_path = store::interim_asset_storage_path(&sha256).map_err(path_error)?;
        if let Some(parent) = storage_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| io_error(format!("Failed to create storage directory: {e}")))?;
        }
        fs::write(&storage_path, new.data)
            .map_err(|e| io_error(format!("Failed to write file: {e}")))?;

        let id = uuid::Uuid::new_v4().to_string();
        let asset = FileAsset {
            id: id.clone(),
            owner_id: new.owner_id.to_string(),
            sha256,
            filename: new.filename.to_string(),
            mime_type: new.mime_type.to_string(),
            size_bytes: new.data.len() as i64,
            storage_path: storage_path.to_string_lossy().to_string(),
            status: FileAssetStatus::Uploaded,
            extracted_text: None,
            extract_error: None,
            metadata_json: None,
            created_at: String::new(), // set by the column default
            updated_at: String::new(), // set by the column default
        };

        if let Err(e) = repo.insert(&asset) {
            // The row is what makes the bytes reachable; without it the file is
            // unreferenced garbage.
            remove_best_effort(&storage_path);
            return Err(db_error(e));
        }

        // Read back so the caller gets the row the database actually holds —
        // the timestamps are column defaults, not values `insert` supplied.
        let asset = repo
            .get_by_id(&id)
            .with_context(|| format!("Failed to read back upload {id}"))
            .map_err(|e| anyhow::Error::new(UploadError::Db(e.to_string())))?
            .ok_or_else(|| {
                anyhow::Error::new(UploadError::Db(format!(
                    "Upload {id} vanished right after it was inserted"
                )))
            })?;

        Ok(StoredUpload {
            asset,
            deduped: false,
        })
    }
}

#[cfg(test)]
mod tests;
