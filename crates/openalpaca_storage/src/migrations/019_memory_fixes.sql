-- Migration 019: Performance index for recent() query
--
-- The recent() method queries ORDER BY created_at DESC with owner_id filter.
-- Without this index, SQLite must scan all owner rows and sort in memory.

CREATE INDEX IF NOT EXISTS idx_memory_owner_created ON memory(owner_id, created_at DESC);

UPDATE schema_version SET version = 19 WHERE version = 18;
