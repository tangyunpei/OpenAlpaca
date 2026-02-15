-- Migration 018: Memory lifecycle — supersession + importance decay
--
-- Adds:
--   updated_at       — tracks when a memory was last modified (supersession)
--   supersedes_id    — FK to the memory this entry replaced (audit trail)
--   last_accessed_at — tracks last retrieval time (for importance decay)

ALTER TABLE memory ADD COLUMN updated_at TEXT;
ALTER TABLE memory ADD COLUMN supersedes_id INTEGER REFERENCES memory(id) ON DELETE SET NULL;
ALTER TABLE memory ADD COLUMN last_accessed_at TEXT;

-- Index for decay query: find memories by owner not accessed recently, excluding KbChunk
CREATE INDEX idx_memory_decay ON memory(owner_id, kind, last_accessed_at);

-- Index for supersession audit trail
CREATE INDEX idx_memory_supersedes ON memory(supersedes_id);

-- Index for soft-cap pruning: quickly find lowest-importance memories per owner
CREATE INDEX idx_memory_importance ON memory(owner_id, importance);

-- Optimize: only re-index FTS when content changes, not on touch_accessed/decay UPDATEs
DROP TRIGGER IF EXISTS memory_au;
CREATE TRIGGER memory_au AFTER UPDATE OF content ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES ('delete', OLD.id, OLD.content, OLD.owner_id, OLD.kind, OLD.scope,
            COALESCE(OLD.scope_id, ''), OLD.source);
    INSERT INTO memory_fts(rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES (NEW.id, NEW.content, NEW.owner_id, NEW.kind, NEW.scope,
            COALESCE(NEW.scope_id, ''), NEW.source);
END;

UPDATE schema_version SET version = 18 WHERE version = 17;
