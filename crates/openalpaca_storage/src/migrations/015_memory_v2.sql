-- Drop old triggers, FTS, vec, and base table
DROP TRIGGER IF EXISTS memory_ai;
DROP TRIGGER IF EXISTS memory_ad;
DROP TRIGGER IF EXISTS memory_au;
DROP TABLE IF EXISTS memory_fts;
DROP TABLE IF EXISTS memory_vec;
DROP TABLE IF EXISTS memory;

-- Memory v2 (owner-scoped, typed, deduped)
CREATE TABLE memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    scope TEXT NOT NULL,
    scope_id TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    importance REAL NOT NULL DEFAULT 0.5,
    confidence REAL NOT NULL DEFAULT 0.7,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    metadata TEXT
);

CREATE INDEX idx_memory_owner ON memory(owner_id);
CREATE INDEX idx_memory_owner_kind ON memory(owner_id, kind);
CREATE INDEX idx_memory_owner_scope ON memory(owner_id, scope, scope_id);
CREATE UNIQUE INDEX idx_memory_content_hash ON memory(owner_id, content_hash);

-- FTS5 (content-synced)
CREATE VIRTUAL TABLE memory_fts USING fts5(
    content,
    owner_id UNINDEXED,
    kind UNINDEXED,
    scope UNINDEXED,
    scope_id UNINDEXED,
    source UNINDEXED,
    content='memory',
    content_rowid='id'
);

CREATE TRIGGER memory_ai AFTER INSERT ON memory BEGIN
    INSERT INTO memory_fts(rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES (NEW.id, NEW.content, NEW.owner_id, NEW.kind, NEW.scope,
            COALESCE(NEW.scope_id, ''), NEW.source);
END;

CREATE TRIGGER memory_ad AFTER DELETE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES ('delete', OLD.id, OLD.content, OLD.owner_id, OLD.kind, OLD.scope,
            COALESCE(OLD.scope_id, ''), OLD.source);
END;

CREATE TRIGGER memory_au AFTER UPDATE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES ('delete', OLD.id, OLD.content, OLD.owner_id, OLD.kind, OLD.scope,
            COALESCE(OLD.scope_id, ''), OLD.source);
    INSERT INTO memory_fts(rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES (NEW.id, NEW.content, NEW.owner_id, NEW.kind, NEW.scope,
            COALESCE(NEW.scope_id, ''), NEW.source);
END;

-- Vec table (384-dim, Phase 6 integration)
CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(
    memory_id INTEGER PRIMARY KEY,
    embedding float[384]
);

UPDATE schema_version SET version = 15 WHERE version = 14;
