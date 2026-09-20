-- Migration 035: drop the planner telemetry columns.
--
-- `orchestrator_latency.planner_ms` / `.dispatch_ms` and
-- `dispatch_decisions.planner_requested_mode` outlived the planner ladder,
-- which was deleted in Routing V2 Phase 5. Every writer since then has stored
-- a constant 0 / NULL "for schema stability", and no reader rendered them.
--
-- Plain DROP COLUMN is safe here: the bundled SQLite is 3.51.1 (rusqlite
-- "bundled"), far past the 3.35 that introduced it, and none of the three
-- columns is referenced by an index, view or generated column.
--
-- Retired mode *strings* in historical `mode` values are data, not schema —
-- they stay.
ALTER TABLE orchestrator_latency DROP COLUMN planner_ms;
ALTER TABLE orchestrator_latency DROP COLUMN dispatch_ms;
ALTER TABLE dispatch_decisions DROP COLUMN planner_requested_mode;

-- Normalise the pre-RFC3339 `event_log.timestamp` rows. The column DEFAULT
-- `datetime('now')` writes "YYYY-MM-DD HH:MM:SS" (exactly 19 characters);
-- every row the repository has written since carries RFC3339 (20+ characters,
-- 'T' separator), so the two forms sort inconsistently against each other.
-- The LIKE pattern matches only the 19-character legacy shape, which makes
-- this a no-op on a database that never held one.
UPDATE event_log
SET timestamp = replace(timestamp, ' ', 'T') || 'Z'
WHERE timestamp LIKE '____-__-__ __:__:__';

UPDATE schema_version SET version = 35 WHERE version = 34;
