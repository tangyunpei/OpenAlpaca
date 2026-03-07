//! Embedding management: insert, list missing, and stats.

use super::MemoryRepository;
use anyhow::Result;

impl<'a> MemoryRepository<'a> {
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
}
