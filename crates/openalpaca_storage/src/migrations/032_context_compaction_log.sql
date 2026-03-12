-- Context compaction telemetry (Phase B)
CREATE TABLE IF NOT EXISTS context_compaction_log (
    id INTEGER PRIMARY KEY,
    request_id TEXT NOT NULL,
    lane_key TEXT NOT NULL,
    trigger_utilization_pct REAL,
    messages_before INTEGER,
    messages_after INTEGER,
    memories_extracted INTEGER,
    messages_discarded INTEGER,
    summary_tokens INTEGER,
    extract_ms INTEGER,
    discard_ms INTEGER,
    summarize_ms INTEGER,
    total_ms INTEGER,
    compaction_model TEXT,
    fallback_used INTEGER DEFAULT 0,
    timestamp TEXT DEFAULT (datetime('now'))
);

UPDATE schema_version SET version = 32 WHERE version = 31;
