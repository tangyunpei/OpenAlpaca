-- File assets storage for multimodal chat
CREATE TABLE IF NOT EXISTS file_assets (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    storage_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'uploaded',
    extracted_text TEXT,
    extract_error TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_file_assets_owner ON file_assets(owner_id);
CREATE INDEX IF NOT EXISTS idx_file_assets_sha256 ON file_assets(sha256);

UPDATE schema_version SET version = 27 WHERE version = 26;
