//! Memory v2 repository — owner-scoped, typed, content-hash deduped.

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

    /// Escape a raw query string for safe use with SQLite FTS5 MATCH.
    ///
    /// Wraps each whitespace-delimited token in double quotes so FTS5
    /// treats commas, parentheses, and reserved words (`AND`/`OR`/`NOT`) as
    /// literal search terms rather than operators.
    fn escape_fts5_query(query: &str) -> String {
        query
            .split_whitespace()
            .filter(|w| !w.is_empty())
            .map(|word| {
                // Escape internal double quotes by doubling them
                let escaped = word.replace('"', "\"\"");
                format!("\"{escaped}\"")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Full-text search with optional kind/scope/scope_id filters.
    pub fn search_fts(
        &self,
        owner_id: &str,
        query: &str,
        limit: usize,
        kind_filter: Option<MemoryKind>,
        scope_filter: Option<MemoryScope>,
        scope_id_filter: Option<&str>,
    ) -> Result<Vec<MemoryV2>> {
        let safe_query = Self::escape_fts5_query(query);
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }

        self.db.with_connection(|conn| {
            let mut sql = format!(
                "SELECT {ALL_COLUMNS} FROM memory m
                 JOIN memory_fts fts ON m.id = fts.rowid
                 WHERE memory_fts MATCH ?1 AND m.owner_id = ?2"
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                vec![Box::new(safe_query), Box::new(owner_id.to_string())];
            let mut param_idx = 3;

            if let Some(kind) = kind_filter {
                sql.push_str(&format!(" AND m.kind = ?{param_idx}"));
                params.push(Box::new(kind.as_str().to_string()));
                param_idx += 1;
            }
            if let Some(scope) = scope_filter {
                sql.push_str(&format!(" AND m.scope = ?{param_idx}"));
                params.push(Box::new(scope.as_str().to_string()));
                param_idx += 1;
            }
            if let Some(sid) = scope_id_filter {
                sql.push_str(&format!(" AND m.scope_id = ?{param_idx}"));
                params.push(Box::new(sid.to_string()));
                let _ = param_idx; // suppress unused warning
            }

            sql.push_str(" ORDER BY rank LIMIT ?");
            params.push(Box::new(limit as i64));

            // Renumber the final LIMIT param
            let limit_idx = params.len();
            // Fix: the LIMIT placeholder needs its index
            let sql = sql.replacen("LIMIT ?", &format!("LIMIT ?{limit_idx}"), 1);

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(param_refs.as_slice())?;

            let mut memories = Vec::new();
            while let Some(row) = rows.next()? {
                memories.push(row_to_memory_v2(row)?);
            }
            Ok(memories)
        })
    }

    /// Insert a vector embedding for a memory entry.
    pub fn insert_embedding(&self, memory_id: i64, embedding: &[f32]) -> Result<()> {
        // Dimension is enforced by the sqlite-vec table schema at insert time.
        self.db.with_connection(|conn| {
            let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
            conn.execute(
                "INSERT OR REPLACE INTO memory_vec(memory_id, embedding) VALUES (?1, ?2)",
                rusqlite::params![memory_id, blob],
            )?;
            Ok(())
        })
    }

    /// List memory IDs that are missing vector embeddings.
    pub fn list_missing_embeddings(
        &self,
        owner_id: &str,
        limit: usize,
    ) -> Result<Vec<(i64, String)>> {
        self.db.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT m.id, m.content FROM memory m
                 LEFT JOIN memory_vec v ON m.id = v.memory_id
                 WHERE m.owner_id = ?1 AND v.memory_id IS NULL
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![owner_id, limit as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    /// Count total memories and embedded memories for an owner.
    pub fn embedding_stats(&self, owner_id: &str) -> Result<(i64, i64)> {
        self.db.with_connection(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM memory WHERE owner_id = ?1",
                [owner_id],
                |r| r.get(0),
            )?;
            let embedded: i64 = conn.query_row(
                "SELECT COUNT(*) FROM memory m JOIN memory_vec v ON m.id = v.memory_id
                 WHERE m.owner_id = ?1",
                [owner_id],
                |r| r.get(0),
            )?;
            Ok((total, embedded))
        })
    }

    /// Vector similarity search using sqlite-vec with optional distance threshold
    /// and scope filtering. Uses a JOIN pattern to access the distance column for
    /// filtering out irrelevant matches.
    pub fn search_vec(
        &self,
        owner_id: &str,
        embedding: &[f32],
        limit: usize,
        distance_threshold: Option<f64>,
        scope_filter: Option<MemoryScope>,
        scope_id_filter: Option<&str>,
    ) -> Result<Vec<MemoryV2>> {
        // Dimension is enforced by the sqlite-vec table schema at query time.
        self.db.with_connection(|conn| {
            let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
            // sqlite-vec KNN: MATCH + k. Owner filtering happens post-KNN since
            // vec0 doesn't support compound WHERE during MATCH. Over-fetch then filter.
            let k = (limit * VEC_OVER_FETCH_FACTOR) as i64;

            let mut sql = format!(
                "SELECT {ALL_COLUMNS}, v.distance FROM memory m
                 JOIN (
                     SELECT memory_id, distance FROM memory_vec
                     WHERE embedding MATCH ?1 AND k = ?2
                 ) v ON m.id = v.memory_id
                 WHERE m.owner_id = ?3"
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                vec![Box::new(blob), Box::new(k), Box::new(owner_id.to_string())];
            let mut param_idx = 4;

            // Distance threshold filtering
            if let Some(threshold) = distance_threshold {
                sql.push_str(&format!(" AND v.distance < ?{param_idx}"));
                params.push(Box::new(threshold));
                param_idx += 1;
            }

            // Scope filtering (post-KNN, on the memory table)
            if let Some(scope) = scope_filter {
                sql.push_str(&format!(" AND m.scope = ?{param_idx}"));
                params.push(Box::new(scope.as_str().to_string()));
                param_idx += 1;
            }
            if let Some(sid) = scope_id_filter {
                sql.push_str(&format!(" AND m.scope_id = ?{param_idx}"));
                params.push(Box::new(sid.to_string()));
                param_idx += 1;
            }

            sql.push_str(&format!(" ORDER BY v.distance ASC LIMIT ?{param_idx}"));
            params.push(Box::new(limit as i64));

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(param_refs.as_slice())?;
            let mut results = Vec::new();
            while let Some(row) = rows.next()? {
                results.push(row_to_memory_v2(row)?);
            }
            Ok(results)
        })
    }

    /// Hybrid search: combine FTS + vector results, dedup by memory_id.
    #[allow(clippy::too_many_arguments)]
    pub fn search_hybrid(
        &self,
        owner_id: &str,
        query: &str,
        embedding: Option<&[f32]>,
        limit: usize,
        kind_filter: Option<MemoryKind>,
        scope_filter: Option<MemoryScope>,
        scope_id_filter: Option<&str>,
    ) -> Result<Vec<MemoryV2>> {
        // 1. FTS results (always available)
        let fts_results = self.search_fts(
            owner_id,
            query,
            limit,
            kind_filter,
            scope_filter,
            scope_id_filter,
        )?;

        // 2. Vec results (only if embedding provided)
        let vec_results = match embedding {
            Some(emb) => self
                .search_vec(
                    owner_id,
                    emb,
                    limit,
                    Some(DEFAULT_VEC_DISTANCE_THRESHOLD),
                    scope_filter,
                    scope_id_filter,
                )
                .unwrap_or_default(),
            None => vec![],
        };

        // 3. Merge + dedup by id, FTS results first (keyword match is high-signal)
        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::with_capacity(limit);
        for m in fts_results.into_iter().chain(vec_results.into_iter()) {
            if seen.insert(m.id) {
                merged.push(m);
                if merged.len() >= limit {
                    break;
                }
            }
        }
        Ok(merged)
    }

    /// Cascading hybrid search: queries multiple scope levels and merges results.
    ///
    /// Iterates `scopes` in reverse order (most-specific first: Workspace before Global),
    /// running `search_hybrid()` for each scope layer. Results are deduplicated by memory ID.
    /// Falls back to unscoped global search if `scopes` is empty.
    ///
    /// The `scopes` list is typically built by `MemoryScopeContext::cascade_scopes()`:
    /// `[(Global, None), (Workspace, Some(workspace_id))]`.
    pub fn search_hybrid_cascade(
        &self,
        owner_id: &str,
        query: &str,
        embedding: Option<&[f32]>,
        limit: usize,
        kind_filter: Option<MemoryKind>,
        scopes: &[(MemoryScope, Option<&str>)],
    ) -> Result<Vec<MemoryV2>> {
        if scopes.is_empty() {
            return self.search_hybrid(owner_id, query, embedding, limit, kind_filter, None, None);
        }

        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::with_capacity(limit);

        // Iterate in reverse: most specific scope first (Workspace before Global)
        for &(scope, scope_id) in scopes.iter().rev() {
            let (scope_filter, scope_id_filter) = match scope {
                MemoryScope::Global => (Some(scope), None), // Global: filter to scope=Global only
                _ => (Some(scope), scope_id),
            };

            let results = self.search_hybrid(
                owner_id,
                query,
                embedding,
                limit, // Over-fetch per scope to fill quota
                kind_filter,
                scope_filter,
                scope_id_filter,
            )?;

            for m in results {
                if seen.insert(m.id) {
                    merged.push(m);
                    if merged.len() >= limit {
                        return Ok(merged);
                    }
                }
            }
        }

        Ok(merged)
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

    // ── Lifecycle methods (migration 018) ───────────────────────────────

    /// Batch-update `last_accessed_at` and apply a small importance boost for a set of memory IDs.
    /// Called after retrieval (proactive injection or memory_search tool).
    /// The boost is capped at 1.0 to prevent unbounded growth.
    pub fn touch_accessed(&self, ids: &[i64], access_boost: f64) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.db.with_connection(|conn| {
            let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "UPDATE memory SET last_accessed_at = datetime('now'), \
                 importance = MIN(1.0, importance + ?1) \
                 WHERE id IN ({placeholders})"
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(access_boost)];
            for id in ids {
                params.push(Box::new(*id) as Box<dyn rusqlite::types::ToSql>);
            }
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            conn.execute(&sql, param_refs.as_slice())?;
            Ok(())
        })
    }

    /// Find semantically similar memories for supersession using vector search.
    /// Only matches Fact and Preference kinds (excludes KbChunk).
    /// Optionally scoped by `scope_filter` and `scope_id_filter` to prevent
    /// cross-scope supersession.
    /// Returns `Vec<(MemoryV2, f64)>` sorted by L2 distance ascending.
    #[allow(clippy::too_many_arguments)]
    pub fn find_similar_for_supersession(
        &self,
        owner_id: &str,
        embedding: &[f32],
        distance_threshold: f64,
        limit: usize,
        scope_filter: Option<MemoryScope>,
        scope_id_filter: Option<&str>,
    ) -> Result<Vec<(MemoryV2, f64)>> {
        self.db.with_connection(|conn| {
            let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
            let k = (limit * VEC_OVER_FETCH_FACTOR) as i64;
            let mut sql = format!(
                "SELECT {ALL_COLUMNS}, v.distance FROM memory m
                 JOIN (
                     SELECT memory_id, distance FROM memory_vec
                     WHERE embedding MATCH ?1 AND k = ?2
                 ) v ON m.id = v.memory_id
                 WHERE m.owner_id = ?3
                   AND m.kind IN ('fact', 'preference')
                   AND v.distance < ?4"
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
                Box::new(blob),
                Box::new(k),
                Box::new(owner_id.to_string()),
                Box::new(distance_threshold),
            ];
            let mut param_idx = 5;

            if let Some(scope) = scope_filter {
                sql.push_str(&format!(" AND m.scope = ?{param_idx}"));
                params.push(Box::new(scope.as_str().to_string()));
                param_idx += 1;
            }
            if let Some(sid) = scope_id_filter {
                sql.push_str(&format!(" AND m.scope_id = ?{param_idx}"));
                params.push(Box::new(sid.to_string()));
                param_idx += 1;
            }

            sql.push_str(&format!(" ORDER BY v.distance ASC LIMIT ?{param_idx}"));
            params.push(Box::new(limit as i64));

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(param_refs.as_slice())?;
            let mut results = Vec::new();
            while let Some(row) = rows.next()? {
                let memory = row_to_memory_v2(row)?;
                let distance: f64 = row.get(15)?; // column after the 15 memory columns
                results.push((memory, distance));
            }
            Ok(results)
        })
    }

    /// FTS-based fallback for supersession when vector search is unavailable.
    ///
    /// Uses AND-joined significant terms (>3 chars, up to 6) to avoid false matches on
    /// common short words. Requires at least 2 meaningful terms; returns empty if content
    /// is too short or generic. Results include a Jaccard word-overlap score for callers
    /// to apply a threshold before superseding.
    ///
    /// Optionally scoped by `scope_filter` and `scope_id_filter` to prevent
    /// cross-scope supersession.
    pub fn find_similar_fts_fallback(
        &self,
        owner_id: &str,
        content: &str,
        limit: usize,
        scope_filter: Option<MemoryScope>,
        scope_id_filter: Option<&str>,
    ) -> Result<Vec<(MemoryV2, f64)>> {
        // Extract significant terms (>3 chars to skip "I", "the", "is", "like", etc.)
        let truncated: String = content.chars().take(200).collect();
        let terms: Vec<&str> = truncated
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .take(6)
            .collect();

        // Require at least 2 meaningful terms to avoid false positives
        if terms.len() < 2 {
            return Ok(vec![]);
        }

        // AND-join: all terms must be present in the matched memory
        let query_terms = terms.join(" AND ");

        let all = self.search_fts(
            owner_id,
            &query_terms,
            limit * 2,
            None,
            scope_filter,
            scope_id_filter,
        )?;

        // Compute Jaccard word-overlap score for each result
        let new_words: std::collections::HashSet<String> = content
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect();

        let mut results: Vec<(MemoryV2, f64)> = all
            .into_iter()
            .filter(|m| m.kind == MemoryKind::Fact || m.kind == MemoryKind::Preference)
            .map(|m| {
                let old_words: std::collections::HashSet<String> = m
                    .content
                    .split_whitespace()
                    .map(|w| w.to_lowercase())
                    .collect();
                let intersection = new_words.intersection(&old_words).count();
                let union = new_words.union(&old_words).count();
                let jaccard = if union > 0 {
                    intersection as f64 / union as f64
                } else {
                    0.0
                };
                (m, jaccard)
            })
            .collect();

        // Sort by overlap score descending (best match first)
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
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

    /// Apply exponential decay to importance for non-KbChunk memories.
    ///
    /// Formula: `importance *= exp(-ln(2) * elapsed_days / half_life_days)`
    /// where `elapsed_days` is the time since the last decay run (or last access/creation).
    ///
    /// After applying decay, resets the reference timestamp so the next run only
    /// decays over the newly elapsed interval (avoiding compounding).
    /// Returns the number of memories updated.
    ///
    /// NOTE: The decay is computed in Rust (not SQL) because SQLite's `EXP()`
    /// math function requires `SQLITE_ENABLE_MATH_FUNCTIONS` at compile time,
    /// which the `bundled` rusqlite feature does not enable by default.
    pub fn apply_importance_decay(
        &self,
        owner_id: &str,
        half_life_days: f64,
        min_importance: f64,
    ) -> Result<usize> {
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;

            // Step 1: Fetch candidate memories with their elapsed days
            let mut stmt = tx.prepare(
                "SELECT id, importance,
                        julianday('now') - julianday(COALESCE(last_accessed_at, created_at)) AS elapsed_days
                 FROM memory
                 WHERE owner_id = ?1
                   AND kind != 'kb_chunk'
                   AND importance > ?2",
            )?;

            let rows: Vec<(i64, f64, f64)> = stmt
                .query_map(rusqlite::params![owner_id, min_importance], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);

            // Step 2: Compute decay in Rust and batch-update
            let ln2: f64 = std::f64::consts::LN_2;
            let mut updated: usize = 0;

            let mut update_stmt = tx.prepare(
                "UPDATE memory SET importance = ?1, last_accessed_at = datetime('now') WHERE id = ?2",
            )?;

            for (id, importance, elapsed_days) in &rows {
                let decay_factor = (-ln2 * elapsed_days / half_life_days).exp();
                let new_importance = (importance * decay_factor).max(min_importance);

                update_stmt.execute(rusqlite::params![new_importance, id])?;
                updated += 1;
            }
            drop(update_stmt);

            tx.commit()?;
            Ok(updated)
        })
    }

    /// Prune memories with low importance and enforce a soft cap per owner.
    ///
    /// Phase 1: Delete non-KbChunk memories below `min_importance`.
    /// Phase 2: If still over `soft_cap`, delete excess lowest-importance entries.
    /// Returns the total number of memories deleted.
    pub fn prune_low_importance(
        &self,
        owner_id: &str,
        min_importance: f64,
        soft_cap: usize,
    ) -> Result<usize> {
        self.db.with_connection_mut(|conn| {
            let tx = conn.transaction()?;

            // Phase 1: Delete memories at or below minimum importance (excluding KbChunk).
            // Decay clamps values with MAX(min_importance, ...), so decayed memories
            // settle exactly at the floor — use <= to catch them.
            let deleted_threshold = tx.execute(
                "DELETE FROM memory WHERE owner_id = ?1 AND kind != 'kb_chunk' AND importance <= ?2",
                rusqlite::params![owner_id, min_importance],
            )?;

            // Phase 2: If still over soft cap, delete lowest-importance non-KbChunk memories
            let current_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM memory WHERE owner_id = ?1 AND kind != 'kb_chunk'",
                [owner_id],
                |row| row.get(0),
            )?;

            let deleted_cap = if current_count as usize > soft_cap {
                let excess = (current_count as usize - soft_cap) as i64;
                tx.execute(
                    "DELETE FROM memory WHERE id IN (
                        SELECT id FROM memory
                        WHERE owner_id = ?1 AND kind != 'kb_chunk'
                        ORDER BY importance ASC
                        LIMIT ?2
                    )",
                    rusqlite::params![owner_id, excess],
                )?
            } else {
                0
            };

            // Clean up orphaned memory_vec entries
            tx.execute(
                "DELETE FROM memory_vec WHERE memory_id NOT IN (SELECT id FROM memory)",
                [],
            )?;

            tx.commit()?;
            Ok(deleted_threshold + deleted_cap)
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

#[cfg(test)]
mod tests;
