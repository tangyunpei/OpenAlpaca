//! Search operations: FTS5, vector similarity, hybrid, and cascade.

use super::{row_to_memory_v2, MemoryRepository, ALL_COLUMNS, VEC_OVER_FETCH_FACTOR, DEFAULT_VEC_DISTANCE_THRESHOLD};
use crate::models::memory::{MemoryKind, MemoryScope, MemoryV2};
use anyhow::Result;

impl<'a> MemoryRepository<'a> {
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
}
