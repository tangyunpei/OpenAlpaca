-- Migration 024: Rename task_id -> request_id, add task_id as nullable backfill
--
-- dispatch_decisions records are created at routing time (before task creation).
-- The primary identifier is request_id (the orchestrator's request UUID).
-- task_id is backfilled after the task is actually created and may be NULL
-- if the dispatch path fails before task creation.

DROP INDEX IF EXISTS idx_dd_ts;
DROP INDEX IF EXISTS idx_dd_mode;

CREATE TABLE dispatch_decisions_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    task_id TEXT,
    mode TEXT NOT NULL,
    reason TEXT NOT NULL,
    agent_count INTEGER DEFAULT 0,
    dag_node_count INTEGER,
    predictability_score REAL,
    planner_requested_mode TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

INSERT INTO dispatch_decisions_new
    (id, request_id, task_id, mode, reason, agent_count, dag_node_count,
     predictability_score, planner_requested_mode, timestamp)
SELECT id, task_id, NULL, mode, reason, agent_count, dag_node_count,
       predictability_score, planner_requested_mode, timestamp
FROM dispatch_decisions;

DROP TABLE dispatch_decisions;
ALTER TABLE dispatch_decisions_new RENAME TO dispatch_decisions;

CREATE INDEX IF NOT EXISTS idx_dd_ts ON dispatch_decisions(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_dd_mode ON dispatch_decisions(mode, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_dd_request ON dispatch_decisions(request_id);

UPDATE schema_version SET version = 24 WHERE version = 23;
