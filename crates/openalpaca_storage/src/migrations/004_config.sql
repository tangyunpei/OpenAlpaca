-- System Configuration Table
-- Used for persistent configuration management (replacing env vars)

CREATE TABLE IF NOT EXISTS system_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    kind TEXT NOT NULL, -- 'string', 'bool', 'int', 'json'
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Index for faster lookups (though primary key is already indexed)
-- No extra index needed for PK.

-- Update schema version
UPDATE schema_version SET version = 4 WHERE version = 3;
