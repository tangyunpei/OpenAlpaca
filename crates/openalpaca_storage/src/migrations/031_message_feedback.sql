-- Phase 4b: User Feedback Signal
-- Adds message-level feedback (thumbs up/down) and links skill executions to response messages.

CREATE TABLE IF NOT EXISTS message_feedback (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL UNIQUE,
    feedback    TEXT    NOT NULL CHECK(feedback IN ('positive', 'negative')),
    comment     TEXT,
    created_at  TEXT    DEFAULT (datetime('now')),
    updated_at  TEXT    DEFAULT (datetime('now')),
    FOREIGN KEY (message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_mf_feedback ON message_feedback(feedback);

ALTER TABLE skill_execution_log ADD COLUMN response_message_id INTEGER;
CREATE INDEX IF NOT EXISTS idx_sel_response_msg ON skill_execution_log(response_message_id);

UPDATE schema_version SET version = 31 WHERE version = 30;
