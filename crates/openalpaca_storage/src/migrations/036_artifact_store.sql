-- Migration 036: project-scoped artifact store.
-- Extends file_assets into the unified artifact record; adds version history.

-- Ownership and attribution (GAP-04)
ALTER TABLE file_assets ADD COLUMN origin TEXT NOT NULL DEFAULT 'upload';  -- 'upload' | 'produced'
ALTER TABLE file_assets ADD COLUMN kind TEXT;                -- ArtifactKind; NULL for legacy uploads
ALTER TABLE file_assets ADD COLUMN task_id TEXT REFERENCES task(id) ON DELETE SET NULL;
ALTER TABLE file_assets ADD COLUMN agent_id TEXT;            -- runtime instance id, "review_agent::a1b2c3d4"
ALTER TABLE file_assets ADD COLUMN agent_template_id TEXT;   -- "review_agent"

-- The address (the directive)
ALTER TABLE file_assets ADD COLUMN project_root TEXT;        -- NULL => home store
ALTER TABLE file_assets ADD COLUMN rel_path TEXT;            -- HEAD path relative to <store>/artifacts (or /uploads)

-- Versions (GAP-05)
ALTER TABLE file_assets ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE file_assets ADD COLUMN version_count INTEGER NOT NULL DEFAULT 1;

-- UI affordances
ALTER TABLE file_assets ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;   -- GAP-12
ALTER TABLE file_assets ADD COLUMN summary TEXT;             -- "+41 −6" / "exit 0 · 1.4s" / "3 rows"
ALTER TABLE file_assets ADD COLUMN missing_since TEXT;

CREATE INDEX IF NOT EXISTS idx_file_assets_task    ON file_assets(task_id);
CREATE INDEX IF NOT EXISTS idx_file_assets_origin  ON file_assets(origin, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_file_assets_project ON file_assets(project_root);
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_assets_addr
    ON file_assets(COALESCE(project_root, ''), rel_path) WHERE rel_path IS NOT NULL;

CREATE TABLE IF NOT EXISTS artifact_versions (
    artifact_id     TEXT    NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    rel_path        TEXT    NOT NULL,   -- '.versions/<stem>/v1.md', or = head rel_path for the head
    sha256          TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL,
    note            TEXT,               -- model-authored "why this version"
    author_agent_id TEXT,               -- NULL => a human edited the file by hand
    added_lines     INTEGER,            -- NULL on v1
    removed_lines   INTEGER,            -- NULL on v1
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (artifact_id, version)
);
CREATE INDEX IF NOT EXISTS idx_artifact_versions_artifact ON artifact_versions(artifact_id, version DESC);

-- The project a run belonged to; makes `rerun` faithful and lets the Library
-- filter by project without a join through file_assets.
ALTER TABLE task ADD COLUMN workspace_id TEXT;
CREATE INDEX IF NOT EXISTS idx_task_workspace ON task(workspace_id);

UPDATE schema_version SET version = 36 WHERE version = 35;
