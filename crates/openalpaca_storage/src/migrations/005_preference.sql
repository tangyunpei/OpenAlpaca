-- Migration 005: User Preference Table
-- Stores per-user preferences with optimistic locking

CREATE TABLE IF NOT EXISTS preference (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    UNIQUE(user_id, key)
);

CREATE INDEX IF NOT EXISTS idx_preference_user ON preference(user_id);

-- Update schema version
UPDATE schema_version SET version = 5 WHERE version = 4;
