//! Memory v2 repository — owner-scoped, typed, content-hash deduped.

mod decay;
mod embedding;
mod search;

#[cfg(test)]
mod tests;

use crate::Database;
use crate::models::memory::{MemoryKind, MemoryScope, MemorySource, MemoryV2};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Default L2 distance threshold for vector search (768-dim embeddings).
/// Distances above this are considered irrelevant. L2 distance of 1.5 with
/// normalized 768-dim embeddings filters out clearly unrelated matches.
const DEFAULT_VEC_DISTANCE_THRESHOLD: f64 = 1.5;

/// KNN over-fetch multiplier. Since sqlite-vec can't filter by owner_id during
/// MATCH, we over-fetch then filter. 3× is sufficient for single-owner deployments.
const VEC_OVER_FETCH_FACTOR: usize = 3;

/// Repository for Memory v2 operations.
pub struct MemoryRepository<'a> {
    db: &'a Database,
}

/// Compute SHA-256 hex digest of content for dedup.
fn content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// Map a rusqlite Row to MemoryV2.
fn row_to_memory_v2(row: &rusqlite::Row<'_>) -> Result<MemoryV2> {
    let id: i64 = row.get(0)?;
    let owner_id: String = row.get(1)?;
    let kind_str: String = row.get(2)?;
    let scope_str: String = row.get(3)?;
    let scope_id: String = row.get(4)?;
    let source_str: String = row.get(5)?;
    let content: String = row.get(6)?;
    let content_hash: String = row.get(7)?;
    let importance: f64 = row.get(8)?;
    let confidence: f64 = row.get(9)?;
    let created_at: String = row.get(10)?;
    let metadata_str: Option<String> = row.get(11)?;

    let kind: MemoryKind = kind_str.parse().context("Invalid memory kind")?;
    let scope: MemoryScope = scope_str.parse().context("Invalid memory scope")?;
    let source: MemorySource = source_str.parse().context("Invalid memory source")?;
    let metadata = metadata_str
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .context("Invalid memory metadata JSON")?;

    let updated_at: Option<String> = row.get(12)?;
    let supersedes_id: Option<i64> = row.get(13)?;
    let last_accessed_at: Option<String> = row.get(14)?;

    Ok(MemoryV2 {
        id,
        owner_id,
        kind,
        scope,
        scope_id,
        source,
        content,
        content_hash,
        importance,
        confidence,
        created_at,
        metadata,
        updated_at,
        supersedes_id,
        last_accessed_at,
    })
}

const ALL_COLUMNS: &str = "m.id, m.owner_id, m.kind, m.scope, m.scope_id, m.source, m.content, m.content_hash, m.importance, m.confidence, m.created_at, m.metadata, m.updated_at, m.supersedes_id, m.last_accessed_at";

/// Unqualified columns for non-JOIN queries (uses implicit table alias).
const ALL_COLUMNS_PLAIN: &str = "id, owner_id, kind, scope, scope_id, source, content, content_hash, importance, confidence, created_at, metadata, updated_at, supersedes_id, last_accessed_at";

impl<'a> MemoryRepository<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Add a memory entry. Uses INSERT OR IGNORE on (owner_id, content_hash)
    /// unique index. Returns the row id if inserted, or 0 if deduped.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        owner_id: &str,
        kind: MemoryKind,
        scope: MemoryScope,
        scope_id: &str,
        source: MemorySource,
        content: &str,
        metadata: Option<&serde_json::Value>,
        importance: f64,
        confidence: f64,
    ) -> Result<i64> {
        let hash = content_hash(content);
        self.db.with_connection(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO memory (owner_id, kind, scope, scope_id, source, content, content_hash, importance, confidence, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    owner_id,
                    kind.as_str(),
                    scope.as_str(),
                    scope_id,
                    source.as_str(),
                    content,
                    hash,
                    importance,
                    confidence,
                    metadata.map(|v| v.to_string()),
                ],
            )?;
            // changes() returns 0 when INSERT OR IGNORE ignores the row (deduped)
            if conn.changes() == 0 {
                Ok(0)
            } else {
                Ok(conn.last_insert_rowid())
            }
        })
    }

    /// Get recent memories for an owner, ordered by created_at DESC.
    pub fn recent(&self, owner_id: &str, limit: usize) -> Result<Vec<MemoryV2>> {
        self.db.with_connection(|conn| {
            let sql = format!(
                "SELECT {ALL_COLUMNS_PLAIN} FROM memory WHERE owner_id = ?1 ORDER BY created_at DESC LIMIT ?2"
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params![owner_id, limit as i64])?;

            let mut memories = Vec::new();
            while let Some(row) = rows.next()? {
                memories.push(row_to_memory_v2(row)?);
            }
            Ok(memories)
        })
    }

    /// List memories for an owner with pagination and optional kind filter.
    /// Returns `(items, total_count)`.
    pub fn list_paginated(
        &self,
        owner_id: &str,
        limit: usize,
        offset: usize,
        kind_filter: Option<MemoryKind>,
    ) -> Result<(Vec<MemoryV2>, i64)> {
        self.db.with_connection(|conn| {
            // Count query
            let (count_sql, total): (String, i64) = if let Some(ref kind) = kind_filter {
                let sql = "SELECT COUNT(*) FROM memory WHERE owner_id = ?1 AND kind = ?2";
                let total =
                    conn.query_row(sql, rusqlite::params![owner_id, kind.as_str()], |r| {
                        r.get(0)
                    })?;
                (sql.to_string(), total)
            } else {
                let sql = "SELECT COUNT(*) FROM memory WHERE owner_id = ?1";
                let total = conn.query_row(sql, [owner_id], |r| r.get(0))?;
                (sql.to_string(), total)
            };
            let _ = count_sql;

            // Data query
            let mut sql = format!("SELECT {ALL_COLUMNS_PLAIN} FROM memory WHERE owner_id = ?1");
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                vec![Box::new(owner_id.to_string())];
            let mut param_idx = 2;

            if let Some(ref kind) = kind_filter {
                sql.push_str(&format!(" AND kind = ?{param_idx}"));
                params.push(Box::new(kind.as_str().to_string()));
                param_idx += 1;
            }

            sql.push_str(&format!(
                " ORDER BY created_at DESC LIMIT ?{param_idx} OFFSET ?{}",
                param_idx + 1
            ));
            params.push(Box::new(limit as i64));
            params.push(Box::new(offset as i64));

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(param_refs.as_slice())?;
            let mut memories = Vec::new();
            while let Some(row) = rows.next()? {
                memories.push(row_to_memory_v2(row)?);
            }
            Ok((memories, total))
        })
    }

    /// Clear all memories for an owner (including vector embeddings).
    pub fn clear(&self, owner_id: &str) -> Result<u64> {
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;
            // Delete vector embeddings first to avoid orphans
            tx.execute(
                "DELETE FROM memory_vec WHERE memory_id IN (SELECT id FROM memory WHERE owner_id = ?1)",
                [owner_id],
            )?;
            let count = tx.execute("DELETE FROM memory WHERE owner_id = ?1", [owner_id])?;
            tx.commit()?;
            Ok(count as u64)
        })
    }

    /// Delete a single memory by its primary key (including vector embedding).
    pub fn delete(&self, id: i64) -> Result<bool> {
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;
            // Delete vector embedding first to avoid orphan
            tx.execute("DELETE FROM memory_vec WHERE memory_id = ?1", [id])?;
            let count = tx.execute("DELETE FROM memory WHERE id = ?1", [id])?;
            tx.commit()?;
            Ok(count > 0)
        })
    }

    /// Get a memory by its primary key.
    pub fn get(&self, id: i64) -> Result<Option<MemoryV2>> {
        self.db.with_connection(|conn| {
            let sql = format!("SELECT {ALL_COLUMNS_PLAIN} FROM memory WHERE id = ?1");
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query([id])?;
            match rows.next()? {
                Some(row) => Ok(Some(row_to_memory_v2(row)?)),
                None => Ok(None),
            }
        })
    }

    /// Get a memory by its primary key, scoped to a specific owner.
    /// Returns `None` if the memory doesn't exist or belongs to a different owner.
    pub fn get_for_owner(&self, id: i64, owner_id: &str) -> Result<Option<MemoryV2>> {
        self.db.with_connection(|conn| {
            let sql =
                format!("SELECT {ALL_COLUMNS_PLAIN} FROM memory WHERE id = ?1 AND owner_id = ?2");
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params![id, owner_id])?;
            match rows.next()? {
                Some(row) => Ok(Some(row_to_memory_v2(row)?)),
                None => Ok(None),
            }
        })
    }

    /// Delete a single memory by its primary key, scoped to a specific owner.
    /// Returns `false` if the memory doesn't exist or belongs to a different owner.
    pub fn delete_for_owner(&self, id: i64, owner_id: &str) -> Result<bool> {
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;
            // Delete vector embedding first to avoid orphan
            tx.execute(
                "DELETE FROM memory_vec WHERE memory_id IN (SELECT id FROM memory WHERE id = ?1 AND owner_id = ?2)",
                rusqlite::params![id, owner_id],
            )?;
            let count = tx.execute(
                "DELETE FROM memory WHERE id = ?1 AND owner_id = ?2",
                rusqlite::params![id, owner_id],
            )?;
            tx.commit()?;
            Ok(count > 0)
        })
    }

    /// Supersede an existing memory: insert a new memory and mark the old one.
    ///
    /// The new memory gets `supersedes_id = existing_id` and `importance = max(old, new)`.
    /// The old memory gets `importance = 0.1` and `updated_at = now` so decay will prune it.
    ///
    /// Returns the new memory's id, or 0 if the new content already exists (hash collision).
    #[allow(clippy::too_many_arguments)]
    pub fn supersede(
        &self,
        existing_id: i64,
        new_content: &str,
        new_kind: MemoryKind,
        new_scope: MemoryScope,
        new_scope_id: &str,
        new_source: MemorySource,
        new_importance: f64,
        new_confidence: f64,
        new_metadata: Option<&serde_json::Value>,
    ) -> Result<i64> {
        let new_hash = content_hash(new_content);
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;

            // Get the existing memory's owner_id and importance
            let (owner_id, existing_importance): (String, f64) = tx.query_row(
                "SELECT owner_id, importance FROM memory WHERE id = ?1",
                [existing_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;

            let final_importance = existing_importance.max(new_importance);

            // Insert new memory (INSERT OR IGNORE handles hash collision)
            tx.execute(
                "INSERT OR IGNORE INTO memory
                 (owner_id, kind, scope, scope_id, source, content, content_hash,
                  importance, confidence, metadata, supersedes_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    owner_id,
                    new_kind.as_str(),
                    new_scope.as_str(),
                    new_scope_id,
                    new_source.as_str(),
                    new_content,
                    new_hash,
                    final_importance,
                    new_confidence,
                    new_metadata.map(|v| v.to_string()),
                    existing_id,
                ],
            )?;

            if tx.changes() == 0 {
                // Hash collision with existing memory — skip
                tx.commit()?;
                return Ok(0);
            }

            let new_id = tx.last_insert_rowid();

            // Mark old memory: reduce importance so decay will eventually prune it
            tx.execute(
                "UPDATE memory SET importance = 0.1, updated_at = datetime('now') WHERE id = ?1",
                [existing_id],
            )?;

            // Remove old memory's embedding so it no longer matches vector searches
            tx.execute("DELETE FROM memory_vec WHERE memory_id = ?1", [existing_id])?;

            tx.commit()?;
            Ok(new_id)
        })
    }

    /// List all distinct owner_ids in the memory table.
    pub fn list_owner_ids(&self) -> Result<Vec<String>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare("SELECT DISTINCT owner_id FROM memory")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
    }
}
