CREATE TABLE IF NOT EXISTS conversation_messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    lane_key    TEXT    NOT NULL,
    role        TEXT    NOT NULL,
    content     TEXT    NOT NULL,
    model       TEXT,
    tokens_in   INTEGER,
    tokens_out  INTEGER,
    duration_ms INTEGER,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_conv_msg_lane ON conversation_messages(lane_key);
CREATE INDEX IF NOT EXISTS idx_conv_msg_created ON conversation_messages(created_at);

UPDATE schema_version SET version = 9 WHERE version = 8;
