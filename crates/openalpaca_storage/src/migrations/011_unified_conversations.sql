-- Phase 5.6: Unified Conversation Pipeline

CREATE TABLE IF NOT EXISTS conversations (
    id          TEXT PRIMARY KEY,
    lane_key    TEXT NOT NULL UNIQUE,
    source      TEXT NOT NULL,
    title       TEXT DEFAULT '',
    message_count INTEGER DEFAULT 0,
    last_message_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_conversations_source ON conversations(source);
CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at DESC);

ALTER TABLE conversation_map ADD COLUMN lane_key TEXT;

ALTER TABLE conversation_messages ADD COLUMN source TEXT;

UPDATE schema_version SET version = 11 WHERE version = 10;
