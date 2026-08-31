-- Drop context_compaction_log: the table was added in migration 032 but no
-- code ever wrote to or read from it (2026-08-30 wiring audit §3).
DROP TABLE IF EXISTS context_compaction_log;

UPDATE schema_version SET version = 34 WHERE version = 33;
