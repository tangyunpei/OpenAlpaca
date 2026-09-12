-- Migration 041: timestamp-leading indexes on the two execution logs.
--
-- GET /v1/tools' invocations_today (tool_invocations_since) and GET /v1/skills'
-- (skill_invocations_since) are both
--   SELECT <name>, COUNT(*) FROM <log> WHERE timestamp >= ?1 GROUP BY <name>
-- and 030's indexes lead on the grouping column -- idx_tel_tool_ts
-- (tool_name, timestamp DESC), idx_sel_skill_ts (skill_id, timestamp DESC) --
-- so the only plan available was a full covering scan of an append-only log,
-- growing for ever, under the daemon's single connection lock. Migration 040
-- fixed the same shape for llm_call_log (review R63); this is review R80.
--
-- The two queries name these indexes with INDEXED BY (see
-- SkillExecutionRepository): with no statistics the planner keeps the 030
-- index, which satisfies the GROUP BY in index order and so costs less in its
-- estimate than a range search plus a temp B-tree -- whatever the log has grown
-- to. Today's rows are a vanishing fraction of it.
--
-- Two columns, not one: these queries read exactly the timestamp and the name,
-- so a (timestamp, name) index turns the scan into a SEARCH that still answers
-- from the index alone.
--
-- GET /v1/agent-templates' run_counts_by_template is deliberately not indexed
-- here: it reads subagent_span, whose rows are updated rather than appended,
-- and its leading predicate is state != 'running' with started_at optional --
-- a different query needing a different index, if one at all.

CREATE INDEX IF NOT EXISTS idx_tel_timestamp ON tool_execution_log(timestamp, tool_name);
CREATE INDEX IF NOT EXISTS idx_sel_timestamp ON skill_execution_log(timestamp, skill_id);

UPDATE schema_version SET version = 41 WHERE version = 40;
