-- Migration 008: LLM Usage Tracking
-- Adds LLM call logging, usage aggregation, and per-agent LLM config

-- LLM call log: records every API call for audit and cost tracking
CREATE TABLE IF NOT EXISTS llm_call_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT DEFAULT (datetime('now')),
    agent_id TEXT,
    task_id TEXT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    key_id TEXT,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cost_usd REAL DEFAULT 0,
    status TEXT DEFAULT 'success',
    latency_ms INTEGER,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_llm_call_log_agent ON llm_call_log(agent_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_llm_call_log_task ON llm_call_log(task_id, timestamp DESC);

-- Daily usage aggregates for dashboard / budget enforcement
CREATE TABLE IF NOT EXISTS llm_usage_daily (
    date TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    model TEXT NOT NULL,
    total_requests INTEGER DEFAULT 0,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cost_usd REAL DEFAULT 0,
    PRIMARY KEY (date, agent_id, model)
);

-- Per-agent LLM config (model override, fallback models)
ALTER TABLE agent ADD COLUMN llm_config_json TEXT;

UPDATE schema_version SET version = 8 WHERE version = 7;
