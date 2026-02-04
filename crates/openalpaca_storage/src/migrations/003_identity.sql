-- Migration 003: Identity System
-- Adds tables for unified identity management across connectors

-- Global User: The canonical identity in the system
-- Think of this as the "Passport" - each user has exactly one.
CREATE TABLE IF NOT EXISTS global_user (
    id TEXT PRIMARY KEY,                      -- UUID
    display_name TEXT,                        -- User's preferred name
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

-- External Identity: A user's identity from an external provider (Telegram, iMessage, etc.)
-- Multiple external identities can map to one global_user via /link
CREATE TABLE IF NOT EXISTS external_identity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,                   -- 'telegram', 'imessage', 'wechat', etc.
    provider_user_id TEXT NOT NULL,           -- The ID from the provider (e.g. Telegram user ID)
    global_user_id TEXT,                      -- NULL if not linked yet
    display_name TEXT,                        -- Name from provider
    metadata TEXT,                            -- JSON, provider-specific data
    created_at TEXT DEFAULT (datetime('now')),
    linked_at TEXT,                           -- When /link was executed
    FOREIGN KEY (global_user_id) REFERENCES global_user(id),
    UNIQUE(provider, provider_user_id)
);

CREATE INDEX IF NOT EXISTS idx_external_identity_provider ON external_identity(provider, provider_user_id);
CREATE INDEX IF NOT EXISTS idx_external_identity_global_user ON external_identity(global_user_id);

-- Conversation Map: Maps a provider's conversation/chat to a global_user
-- This allows the daemon to know which user is associated with a chat thread.
CREATE TABLE IF NOT EXISTS conversation_map (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,                   -- 'telegram', 'imessage', etc.
    provider_conversation_id TEXT NOT NULL,   -- Chat ID / Thread ID from provider
    global_user_id TEXT,                      -- The user who owns this conversation
    created_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (global_user_id) REFERENCES global_user(id),
    UNIQUE(provider, provider_conversation_id)
);

CREATE INDEX IF NOT EXISTS idx_conversation_map_provider ON conversation_map(provider, provider_conversation_id);

-- Link Token: Temporary tokens for /link command
CREATE TABLE IF NOT EXISTS link_token (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token TEXT NOT NULL UNIQUE,               -- The 4-6 char OTP or random token
    global_user_id TEXT NOT NULL,             -- Which global_user this token will link to
    expires_at TEXT NOT NULL,                 -- Token expiry (short TTL, e.g. 5 min)
    used_at TEXT,                             -- When was it consumed
    created_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (global_user_id) REFERENCES global_user(id)
);

CREATE INDEX IF NOT EXISTS idx_link_token_token ON link_token(token);

-- Update schema version
UPDATE schema_version SET version = 3 WHERE version = 2;
