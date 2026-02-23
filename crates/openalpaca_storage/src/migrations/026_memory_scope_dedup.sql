-- Replace owner-only dedup with scope-aware dedup.
-- Allows the same content to exist in Global scope AND Workspace scope simultaneously.
DROP INDEX IF EXISTS idx_memory_content_hash;
CREATE UNIQUE INDEX idx_memory_content_hash ON memory(owner_id, scope, scope_id, content_hash);
UPDATE schema_version SET version = 26 WHERE version = 25;
