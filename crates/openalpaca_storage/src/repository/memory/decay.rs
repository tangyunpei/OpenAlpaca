//! Importance decay, access tracking, and pruning for memories.

use super::MemoryRepository;
use anyhow::Result;

impl<'a> MemoryRepository<'a> {
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
}
