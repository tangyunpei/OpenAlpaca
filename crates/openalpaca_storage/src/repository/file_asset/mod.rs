//! FileAssetRepository — CRUD for file assets

use crate::Database;
use crate::models::file_asset::{FileAsset, FileAssetStatus};
use anyhow::Result;

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
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, extracted_text, extract_error, metadata_json, created_at, updated_at
                 FROM file_assets WHERE id = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![id])?;
            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_asset(row)?)),
                None => Ok(None),
            }
        })
    }

    pub fn get_by_sha256(&self, sha256: &str) -> Result<Option<FileAsset>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, extracted_text, extract_error, metadata_json, created_at, updated_at
                 FROM file_assets WHERE sha256 = ?1 LIMIT 1",
            )?;
            let mut rows = stmt.query(rusqlite::params![sha256])?;
            match rows.next()? {
                Some(row) => Ok(Some(Self::row_to_asset(row)?)),
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

    /// Get total storage bytes used across all file assets.
    pub fn total_storage_bytes(&self) -> Result<i64> {
        self.db.with_connection(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM file_assets",
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

    /// List orphaned assets (not linked to any message and older than grace period).
    pub fn list_orphaned(&self, older_than_hours: i64) -> Result<Vec<FileAsset>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT f.id, f.owner_id, f.sha256, f.filename, f.mime_type, f.size_bytes, f.storage_path, f.status, f.extracted_text, f.extract_error, f.metadata_json, f.created_at, f.updated_at
                 FROM file_assets f
                 LEFT JOIN conversation_message_attachments a ON f.id = a.file_id
                 WHERE a.id IS NULL AND f.created_at < datetime('now', ?1)
                 LIMIT 100",
            )?;
            let hours_param = format!("-{older_than_hours} hours");
            let mut assets = Vec::new();
            let mut rows = stmt.query(rusqlite::params![hours_param])?;
            while let Some(row) = rows.next()? {
                assets.push(Self::row_to_asset(row)?);
            }
            Ok(assets)
        })
    }

    /// Link a file asset to a conversation message.
    pub fn link_to_message(
        &self,
        message_id: i64,
        file_id: &str,
        sort_order: i32,
        caption: Option<&str>,
    ) -> Result<()> {
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO conversation_message_attachments (message_id, file_id, sort_order, caption)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![message_id, file_id, sort_order, caption],
            )?;
            Ok(())
        })
    }

    /// List file assets by status, ordered by creation date (oldest first).
    pub fn list_by_status(&self, status: &FileAssetStatus, limit: usize) -> Result<Vec<FileAsset>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, extracted_text, extract_error, metadata_json, created_at, updated_at
                 FROM file_assets WHERE status = ?1 ORDER BY created_at ASC LIMIT ?2",
            )?;
            let mut assets = Vec::new();
            let mut rows = stmt.query(rusqlite::params![status.as_str(), limit as i64])?;
            while let Some(row) = rows.next()? {
                assets.push(Self::row_to_asset(row)?);
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

    fn row_to_asset(row: &rusqlite::Row<'_>) -> Result<FileAsset> {
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
}
