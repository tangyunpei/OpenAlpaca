-- Migration 012: Add result_output to task_agent_assignment
ALTER TABLE task_agent_assignment ADD COLUMN result_output TEXT;

UPDATE schema_version SET version = 12 WHERE version = 11;
