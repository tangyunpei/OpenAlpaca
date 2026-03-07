CREATE TABLE IF NOT EXISTS skill_execution_log (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id        TEXT    NOT NULL,
    skill_id          TEXT    NOT NULL,
    agent_id          TEXT    NOT NULL DEFAULT 'orchestrator',
    status            TEXT    NOT NULL,
    finish_reason     TEXT,
    error_message     TEXT,
    validation_failures TEXT,
    duration_ms       INTEGER NOT NULL,
    rounds_used       INTEGER,
    tool_calls_made   INTEGER,
    input_tokens      INTEGER DEFAULT 0,
    output_tokens     INTEGER DEFAULT 0,
    cost_usd          REAL    DEFAULT 0.0,
    model_used        TEXT,
    query_preview     TEXT,
    route_score       REAL,
    was_auto_selected INTEGER DEFAULT 0,
    repair_attempted  INTEGER DEFAULT 0,
    repair_succeeded  INTEGER DEFAULT 0,
    timestamp         TEXT DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_sel_request_id ON skill_execution_log(request_id);
CREATE INDEX IF NOT EXISTS idx_sel_skill_ts ON skill_execution_log(skill_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_sel_status ON skill_execution_log(skill_id, status);
CREATE INDEX IF NOT EXISTS idx_sel_agent ON skill_execution_log(agent_id, skill_id);

CREATE TABLE IF NOT EXISTS tool_execution_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id    TEXT,
    agent_id      TEXT NOT NULL,
    tool_name     TEXT NOT NULL,
    success       INTEGER NOT NULL,
    duration_ms   INTEGER NOT NULL,
    error_message TEXT,
    timestamp     TEXT DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_tel_tool_ts ON tool_execution_log(tool_name, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_tel_request ON tool_execution_log(request_id);

UPDATE schema_version SET version = 30 WHERE version = 29;
