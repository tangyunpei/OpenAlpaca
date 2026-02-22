-- Migration 021: Enforce template_id NOT NULL on agent table
-- Uses SQLite table-rebuild since ALTER TABLE cannot add NOT NULL constraints.

-- Ensure no NULLs before rebuild
UPDATE agent SET template_id = id WHERE template_id IS NULL;

-- Rebuild with NOT NULL constraint on template_id
CREATE TABLE agent_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    persona TEXT,
    config TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    description TEXT,
    icon TEXT,
    status TEXT DEFAULT 'idle',
    current_task_id TEXT,
    skills_json TEXT DEFAULT '[]',
    preset_json TEXT DEFAULT '{}',
    constraints_json TEXT,
    updated_at TEXT,
    llm_config_json TEXT,
    template_id TEXT NOT NULL DEFAULT ''
);

INSERT INTO agent_new SELECT * FROM agent;
DROP TABLE agent;
ALTER TABLE agent_new RENAME TO agent;

UPDATE schema_version SET version = 21 WHERE version = 20;
