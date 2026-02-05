-- Migration 007: SubAgent System
-- Extends agent table with SubAgent fields, adds metrics and task history

ALTER TABLE agent ADD COLUMN description TEXT;
ALTER TABLE agent ADD COLUMN icon TEXT;
ALTER TABLE agent ADD COLUMN status TEXT DEFAULT 'idle';
ALTER TABLE agent ADD COLUMN current_task_id TEXT;
ALTER TABLE agent ADD COLUMN skills_json TEXT DEFAULT '[]';
ALTER TABLE agent ADD COLUMN preset_json TEXT DEFAULT '{}';
ALTER TABLE agent ADD COLUMN constraints_json TEXT;
ALTER TABLE agent ADD COLUMN updated_at TEXT;

CREATE INDEX IF NOT EXISTS idx_agent_status ON agent(status);

-- Agent performance metrics
CREATE TABLE IF NOT EXISTS agent_metrics (
    agent_id TEXT PRIMARY KEY REFERENCES agent(id) ON DELETE CASCADE,
    tasks_completed INTEGER DEFAULT 0,
    tasks_failed INTEGER DEFAULT 0,
    total_runtime_seconds INTEGER DEFAULT 0,
    average_runtime_seconds REAL DEFAULT 0,
    success_rate REAL DEFAULT 1.0,
    updated_at TEXT DEFAULT (datetime('now'))
);

-- Agent task history
CREATE TABLE IF NOT EXISTS agent_task_history (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    runtime_seconds INTEGER,
    completed_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_agent_task_history ON agent_task_history(agent_id, completed_at DESC);

UPDATE schema_version SET version = 7 WHERE version = 6;
