ALTER TABLE conversations ADD COLUMN summary TEXT NOT NULL DEFAULT '';
ALTER TABLE conversations ADD COLUMN summary_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN last_summarized_message_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN summary_updated_at TEXT;

CREATE INDEX IF NOT EXISTS idx_conv_msg_lane_id ON conversation_messages(lane_key, id);

UPDATE schema_version SET version = 14 WHERE version = 13;
