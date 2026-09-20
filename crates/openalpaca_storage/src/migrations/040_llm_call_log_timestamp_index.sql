-- Migration 040: index llm_call_log(timestamp) for the usage-summary query.
-- GET /v1/usage/summary's provider_usage_since (WHERE timestamp >= ?) had no
-- leading index on timestamp — idx_llm_call_log_agent and idx_llm_call_log_task
-- both lead on agent_id/task_id — so every refetch (once per llm_call_completed
-- event while Settings is open) full-scanned the append-only log under the
-- daemon's single connection lock (review R63).

CREATE INDEX IF NOT EXISTS idx_llm_call_log_timestamp ON llm_call_log(timestamp);

UPDATE schema_version SET version = 40 WHERE version = 39;
