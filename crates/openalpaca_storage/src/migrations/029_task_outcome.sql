-- Migration 029: Task outcome fields
-- Adds structured outcome persistence for completed/failed tasks.

ALTER TABLE task ADD COLUMN outcome_json TEXT;
ALTER TABLE task ADD COLUMN outcome_kind TEXT;
ALTER TABLE task ADD COLUMN artifact_count INTEGER NOT NULL DEFAULT 0;

UPDATE schema_version SET version = 29 WHERE version = 28;
