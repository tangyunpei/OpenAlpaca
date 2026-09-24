-- 044: Learning v2 (tasks/memory-redesign/09): the turn id and project root on each message and a per-session learning cursor.
ALTER TABLE conversation_messages ADD COLUMN request_id TEXT;
ALTER TABLE conversation_messages ADD COLUMN workspace_root TEXT;

CREATE TABLE learning_cursor (
    session_id TEXT PRIMARY KEY,
    extracted_through INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Historic sessions are not re-extracted (design §6): every existing session starts at its newest message.
INSERT INTO learning_cursor (session_id, extracted_through)
    SELECT s.id, COALESCE((SELECT MAX(m.id) FROM conversation_messages m WHERE m.session_id = s.id), 0)
    FROM session s;

INSERT INTO schema_version(version) VALUES (44);
