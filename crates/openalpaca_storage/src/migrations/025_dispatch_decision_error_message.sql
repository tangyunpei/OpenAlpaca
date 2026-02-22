-- Migration 025: Add error_message column to dispatch_decisions
-- Captures error details when heuristic skill matching fails,
-- enabling analytics on failed dispatch attempts.

ALTER TABLE dispatch_decisions ADD COLUMN error_message TEXT;

UPDATE schema_version SET version = 25 WHERE version = 24;
