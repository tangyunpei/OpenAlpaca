-- Migration 022: Orchestrator latency metrics for request-level observability
--
-- Tracks planner/dispatch/ack latencies per orchestrator request,
-- enabling P50/P95 analysis by mode and time range.

CREATE TABLE IF NOT EXISTS orchestrator_latency (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    planner_ms INTEGER DEFAULT 0,
    dispatch_ms INTEGER DEFAULT 0,
    ack_ms INTEGER DEFAULT 0,
    fallback_reason TEXT,
    auto_promotion_reason TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_orch_latency_ts ON orchestrator_latency(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_orch_latency_mode ON orchestrator_latency(mode, timestamp DESC);

UPDATE schema_version SET version = 22 WHERE version = 21;
