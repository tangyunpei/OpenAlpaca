-- Message attachments linking messages to file assets
CREATE TABLE IF NOT EXISTS conversation_message_attachments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    role TEXT NOT NULL DEFAULT 'attachment',
    caption TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_msg_attach_message ON conversation_message_attachments(message_id);

ALTER TABLE conversation_messages ADD COLUMN content_json TEXT;
ALTER TABLE conversation_messages ADD COLUMN display_text TEXT;

UPDATE schema_version SET version = 28 WHERE version = 27;
