-- Migration 039: sessions. Rebuilds `conversations` as `session`
-- (drops the column-level UNIQUE(lane_key); adds workspace + lifecycle),
-- keys messages/tasks/followups by session, and turns tool_execution_log
-- into the tool-call index for the session event log.

CREATE TABLE session (
    id             TEXT PRIMARY KEY,               -- UUIDv4 (same ids carried over)
    lane_key       TEXT NOT NULL,                  -- routing address, no longer UNIQUE
    source         TEXT NOT NULL,
    title          TEXT DEFAULT '',
    workspace_id   TEXT,                           -- canonical project root; NULL = none
    status         TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    message_count  INTEGER DEFAULT 0,
    last_message_at TEXT,
    summary        TEXT NOT NULL DEFAULT '',       -- carried from 014
    summary_version INTEGER NOT NULL DEFAULT 0,
    last_summarized_message_id INTEGER NOT NULL DEFAULT 0,
    summary_updated_at TEXT,
    ended_at       TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO session (id, lane_key, source, title, workspace_id, status, message_count,
                     last_message_at, summary, summary_version, last_summarized_message_id,
                     summary_updated_at, created_at, updated_at)
SELECT id, lane_key, source, title, NULL, 'active', message_count,
       last_message_at, summary, summary_version, last_summarized_message_id,
       summary_updated_at, created_at, updated_at
FROM conversations;
DROP TABLE conversations;

-- The invariant: at most one active session per lane.
CREATE UNIQUE INDEX idx_session_active_lane ON session(lane_key) WHERE status = 'active';
CREATE INDEX idx_session_workspace ON session(workspace_id, updated_at DESC);
CREATE INDEX idx_session_updated   ON session(updated_at DESC);
CREATE INDEX idx_session_source    ON session(source);

ALTER TABLE conversation_messages ADD COLUMN session_id TEXT;
UPDATE conversation_messages
   SET session_id = (SELECT s.id FROM session s WHERE s.lane_key = conversation_messages.lane_key);
CREATE INDEX idx_conv_msg_session ON conversation_messages(session_id, id);

ALTER TABLE task ADD COLUMN session_id TEXT;
CREATE INDEX idx_task_session ON task(session_id);

-- Follow-ups remember the conversation they came from (§5.3).
ALTER TABLE lane_followups ADD COLUMN session_id TEXT;

-- tool_execution_log → the tool-call index. Payloads live in the session
-- JSONL (or its results/ spill tier); rows hold previews and the pointer.
ALTER TABLE tool_execution_log ADD COLUMN session_id TEXT;
ALTER TABLE tool_execution_log ADD COLUMN task_id TEXT;
ALTER TABLE tool_execution_log ADD COLUMN log_seq INTEGER;      -- seq of the tool_call record
ALTER TABLE tool_execution_log ADD COLUMN args_preview TEXT;    -- ≤ 2048 chars
ALTER TABLE tool_execution_log ADD COLUMN result_preview TEXT;  -- ≤ 2048 chars
ALTER TABLE tool_execution_log ADD COLUMN result_ref TEXT;      -- 'log:<seq>' | 'file:results/<...>'
CREATE INDEX idx_tel_session ON tool_execution_log(session_id, id);
CREATE INDEX idx_tel_task    ON tool_execution_log(task_id, id);

UPDATE schema_version SET version = 39 WHERE version = 38;
