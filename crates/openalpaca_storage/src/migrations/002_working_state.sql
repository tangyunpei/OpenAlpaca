-- Working state, version 2: `session`'s four summary columns become the conversation's working state — the state's JSON, its version, the last row it covers, when it was written, and the last conversation summary projected from it (design 11 §2, §6; owner decision CQ-9).
-- SQLite cannot rename or retype those columns in place, so the table is
-- rebuilt: every other column is copied, the five new ones take their
-- defaults, and the indexes the drop takes with it are created again.
-- The free-text summary is not carried over: it is not a working state, and
-- `working_state_through` starts at 0, so the next writer reads the rows the
-- old summary covered instead of leaving them in neither.
-- All statements run inside the database migration transaction.

CREATE TABLE session_new (
    id TEXT PRIMARY KEY,
    lane_key TEXT NOT NULL,
    source TEXT NOT NULL,
    title TEXT DEFAULT '',
    workspace_id TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    message_count INTEGER DEFAULT 0,
    last_message_at TEXT,
    working_state TEXT NOT NULL DEFAULT '',
    working_state_version INTEGER NOT NULL DEFAULT 0,
    working_state_through INTEGER NOT NULL DEFAULT 0,
    working_state_updated_at TEXT,
    episode_note_id TEXT,
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO session_new (id, lane_key, source, title, workspace_id, status, message_count,
                         last_message_at, ended_at, created_at, updated_at)
SELECT id, lane_key, source, title, workspace_id, status, message_count,
       last_message_at, ended_at, created_at, updated_at
FROM session;

DROP TABLE session;

ALTER TABLE session_new RENAME TO session;

CREATE UNIQUE INDEX idx_session_active_lane ON session(lane_key) WHERE status = 'active';
CREATE INDEX idx_session_source ON session(source);
CREATE INDEX idx_session_updated ON session(updated_at DESC);
CREATE INDEX idx_session_workspace ON session(workspace_id, updated_at DESC);

INSERT INTO schema_version(version) VALUES (2);
