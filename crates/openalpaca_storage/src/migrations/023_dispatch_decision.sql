-- Migration 023: Dispatch decision history for orchestrator analysis
--
-- Persists DispatchDecision events emitted when dispatch_analysis_enabled is true,
-- enabling historical analysis of dispatch mode selection and routing accuracy.

CREATE TABLE IF NOT EXISTS dispatch_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    reason TEXT NOT NULL,
    agent_count INTEGER DEFAULT 0,
    dag_node_count INTEGER,
    predictability_score REAL,
    planner_requested_mode TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_dd_ts ON dispatch_decisions(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_dd_mode ON dispatch_decisions(mode, timestamp DESC);

UPDATE schema_version SET version = 23 WHERE version = 22;
