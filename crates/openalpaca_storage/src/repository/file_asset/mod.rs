//! FileAssetRepository — CRUD for file assets

use crate::Database;
use crate::models::file_asset::{FileAsset, FileAssetStatus, MessageArtifact};
use anyhow::Result;
use std::collections::HashMap;

/// `conversation_message_attachments.role` for a file a user turn carried in —
/// the column's own default since `028:7`.
pub const ATTACHMENT_ROLE: &str = "attachment";

/// `conversation_message_attachments.role` for a file the message's *run*
/// produced (GAP-23).
pub const ARTIFACT_ROLE: &str = "artifact";

/// The `file_assets` columns every [`FileAsset`] read selects, in the order
/// [`row_to_file_asset`] expects.
///
/// Named explicitly, never `SELECT *`, so the artifact-store columns migration
/// 036 added (`origin`, `kind`, `project_root`, …) can never shift these
/// indexes. Shared with [`crate::uploads::UploadStore`], which reads a row back
/// inside its own transaction and so cannot go through this repository.
pub(crate) const FILE_ASSET_COLUMNS: &str = "id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, extracted_text, extract_error, metadata_json, created_at, updated_at";

/// One row of [`FILE_ASSET_COLUMNS`] as a [`FileAsset`].
pub(crate) fn row_to_file_asset(row: &rusqlite::Row<'_>) -> Result<FileAsset> {
    let status_str: String = row.get(7)?;
    Ok(FileAsset {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        sha256: row.get(2)?,
        filename: row.get(3)?,
        mime_type: row.get(4)?,
        size_bytes: row.get(5)?,
        storage_path: row.get(6)?,
        status: FileAssetStatus::parse(&status_str),
        extracted_text: row.get(8)?,
        extract_error: row.get(9)?,
        metadata_json: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

pub struct FileAssetRepository<'a> {
    db: &'a Database,
}

impl<'a> FileAssetRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    pub fn insert(&self, asset: &FileAsset) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, extracted_text, extract_error, metadata_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    asset.id, asset.owner_id, asset.sha256, asset.filename, asset.mime_type,
                    asset.size_bytes, asset.storage_path, asset.status.as_str(),
                    asset.extracted_text, asset.extract_error, asset.metadata_json,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_by_id(&self, id: &str) -> Result<Option<FileAsset>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {FILE_ASSET_COLUMNS} FROM file_assets WHERE id = ?1"
            ))?;
            let mut rows = stmt.query(rusqlite::params![id])?;
            match rows.next()? {
                Some(row) => Ok(Some(row_to_file_asset(row)?)),
                None => Ok(None),
            }
        })
    }

    pub fn get_by_sha256(&self, sha256: &str) -> Result<Option<FileAsset>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {FILE_ASSET_COLUMNS} FROM file_assets WHERE sha256 = ?1 LIMIT 1"
            ))?;
            let mut rows = stmt.query(rusqlite::params![sha256])?;
            match rows.next()? {
                Some(row) => Ok(Some(row_to_file_asset(row)?)),
                None => Ok(None),
            }
        })
    }

    pub fn update_status(
        &self,
        id: &str,
        status: &FileAssetStatus,
        extracted_text: Option<&str>,
        extract_error: Option<&str>,
    ) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "UPDATE file_assets SET status = ?1, extracted_text = ?2, extract_error = ?3, updated_at = datetime('now') WHERE id = ?4",
                rusqlite::params![status.as_str(), extracted_text, extract_error, id],
            )?;
            Ok(())
        })
    }

    /// Get total storage bytes used by uploads.
    ///
    /// Read at `routes/files.rs` against the 500 MB upload cap, so it counts
    /// only what the user uploaded: agent output written into the user's own
    /// project (`origin = 'produced'`) is not upload traffic and must never
    /// start rejecting uploads.
    pub fn total_storage_bytes(&self) -> Result<i64> {
        self.db.with_connection(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM file_assets WHERE origin = 'upload'",
                [],
                |row| row.get(0),
            )?;
            Ok(total)
        })
    }

    pub fn delete_by_id(&self, id: &str) -> Result<bool> {
        self.db.with_connection(|conn| {
            let rows = conn.execute("DELETE FROM file_assets WHERE id = ?1", [id])?;
            Ok(rows > 0)
        })
    }

    /// List orphaned *uploads* (not linked to any message and older than the
    /// grace period).
    ///
    /// The caller deletes the row **and** the file on disk, so the predicate is
    /// deliberately narrow: a produced artifact is the user's own file and is
    /// never linked to a conversation message, and a pinned upload is one the
    /// user asked to keep. Neither is ever garbage-collected.
    pub fn list_orphaned(&self, older_than_hours: i64) -> Result<Vec<FileAsset>> {
        self.db.with_connection(|conn| {
            // [`FILE_ASSET_COLUMNS`] in the same order, table-qualified for the
            // join — the one read that cannot use the bare constant.
            let mut stmt = conn.prepare(
                "SELECT f.id, f.owner_id, f.sha256, f.filename, f.mime_type, f.size_bytes, f.storage_path, f.status, f.extracted_text, f.extract_error, f.metadata_json, f.created_at, f.updated_at
                 FROM file_assets f
                 LEFT JOIN conversation_message_attachments a ON f.id = a.file_id
                 WHERE a.id IS NULL
                   AND f.origin = 'upload'
                   AND f.pinned = 0
                   AND f.created_at < datetime('now', ?1)
                 LIMIT 100",
            )?;
            let hours_param = format!("-{older_than_hours} hours");
            let mut assets = Vec::new();
            let mut rows = stmt.query(rusqlite::params![hours_param])?;
            while let Some(row) = rows.next()? {
                assets.push(row_to_file_asset(row)?);
            }
            Ok(assets)
        })
    }

    /// Link a file asset to a conversation message as an *attachment* — a file
    /// the turn carried in.
    pub fn link_to_message(
        &self,
        message_id: i64,
        file_id: &str,
        sort_order: i32,
        caption: Option<&str>,
    ) -> Result<()> {
        self.link_to_message_with_role(message_id, file_id, sort_order, caption, ATTACHMENT_ROLE)
    }

    /// Link a file asset to a conversation message under an explicit `role`
    /// (`028:7`).
    ///
    /// [`ARTIFACT_ROLE`] is what a completion report uses for the files its run
    /// produced (GAP-23): same table, same message, but a chip the client draws
    /// from the Library rather than an upload the user attached.
    pub fn link_to_message_with_role(
        &self,
        message_id: i64,
        file_id: &str,
        sort_order: i32,
        caption: Option<&str>,
        role: &str,
    ) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO conversation_message_attachments (message_id, file_id, sort_order, role, caption)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![message_id, file_id, sort_order, role, caption],
            )?;
            Ok(())
        })
    }

    /// The [`ARTIFACT_ROLE`] links of a whole page of messages, in one query.
    ///
    /// Keyed by `message_id`; a message with no artifacts has no entry at all,
    /// so the caller's default is an empty list rather than a second query.
    /// `role='attachment'` rows are not returned — they are the upload half and
    /// belong to [`Self::get_attachments_for_message`].
    pub fn artifact_links_for_messages(
        &self,
        message_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<MessageArtifact>>> {
        let mut links: HashMap<i64, Vec<MessageArtifact>> = HashMap::new();
        if message_ids.is_empty() {
            return Ok(links);
        }
        // rusqlite has no array binding: one `?` per id, all bound.
        let placeholders = std::iter::repeat_n("?", message_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT a.message_id, f.id, f.filename, f.kind
                 FROM conversation_message_attachments a
                 JOIN file_assets f ON f.id = a.file_id
                 WHERE a.role = '{ARTIFACT_ROLE}' AND a.message_id IN ({placeholders})
                 ORDER BY a.message_id ASC, a.sort_order ASC, a.id ASC"
            ))?;
            let mut rows = stmt.query(rusqlite::params_from_iter(message_ids.iter()))?;
            while let Some(row) = rows.next()? {
                links
                    .entry(row.get(0)?)
                    .or_default()
                    .push(MessageArtifact {
                        id: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                    });
            }
            Ok(())
        })?;
        Ok(links)
    }

    /// The ids of the artifacts a run *produced*, oldest first.
    ///
    /// `origin != 'upload'` on purpose: a file the user attached during the run
    /// also carries its `task_id`, and it is not the run's output.
    pub fn produced_ids_for_task(&self, task_id: &str) -> Result<Vec<String>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id FROM file_assets
                 WHERE task_id = ?1 AND origin != 'upload'
                 ORDER BY created_at ASC, rowid ASC",
            )?;
            let mut ids = Vec::new();
            let mut rows = stmt.query(rusqlite::params![task_id])?;
            while let Some(row) = rows.next()? {
                ids.push(row.get(0)?);
            }
            Ok(ids)
        })
    }

    /// List file assets by status, ordered by creation date (oldest first).
    pub fn list_by_status(&self, status: &FileAssetStatus, limit: usize) -> Result<Vec<FileAsset>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {FILE_ASSET_COLUMNS} FROM file_assets
                 WHERE status = ?1 ORDER BY created_at ASC LIMIT ?2"
            ))?;
            let mut assets = Vec::new();
            let mut rows = stmt.query(rusqlite::params![status.as_str(), limit as i64])?;
            while let Some(row) = rows.next()? {
                assets.push(row_to_file_asset(row)?);
            }
            Ok(assets)
        })
    }

    /// Get all attachment file_ids for a message.
    pub fn get_attachments_for_message(
        &self,
        message_id: i64,
    ) -> Result<Vec<(String, i32, Option<String>)>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT file_id, sort_order, caption FROM conversation_message_attachments WHERE message_id = ?1 ORDER BY sort_order ASC",
            )?;
            let mut results = Vec::new();
            let mut rows = stmt.query(rusqlite::params![message_id])?;
            while let Some(row) = rows.next()? {
                results.push((row.get(0)?, row.get(1)?, row.get(2)?));
            }
            Ok(results)
        })
    }
}

#[cfg(test)]
mod tests;
