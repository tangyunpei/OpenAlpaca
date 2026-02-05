-- Migration 006: Task System
-- Adds task tracking and agent assignment tables

CREATE TABLE IF NOT EXISTS task (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'queued',
    priority INTEGER NOT NULL DEFAULT 0,
    progress_current INTEGER,
    progress_total INTEGER,
    result_summary TEXT,
    created_by TEXT NOT NULL,
    source_lane TEXT NOT NULL,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    completed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_task_status ON task(status);
CREATE INDEX IF NOT EXISTS idx_task_created_by ON task(created_by);

CREATE TABLE IF NOT EXISTS task_agent_assignment (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    step_order INTEGER,
    started_at TEXT,
    completed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_task_agent_task ON task_agent_assignment(task_id);

UPDATE schema_version SET version = 6 WHERE version = 5;
