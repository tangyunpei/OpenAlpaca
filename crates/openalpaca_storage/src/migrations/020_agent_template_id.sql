-- Migration 020: Add template_id to agent table
-- Supports the template + instance model: template_id references the template
-- this agent was spawned from. For existing agents, template_id = id.

ALTER TABLE agent ADD COLUMN template_id TEXT;

-- Backfill: set template_id = id for all existing rows
UPDATE agent SET template_id = id WHERE template_id IS NULL;

UPDATE schema_version SET version = 20 WHERE version = 19;
