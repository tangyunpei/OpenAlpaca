-- Lane follow-up queue (Routing V2): explicit `queue_followup` items and
-- unprocessed steering leftovers, executed as fresh turns after a workflow ends.
CREATE TABLE IF NOT EXISTS lane_followups (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    lane_key       TEXT    NOT NULL,
    kind           TEXT    NOT NULL CHECK(kind IN ('followup','unprocessed_steering')),
    content        TEXT    NOT NULL,
    principal_json TEXT    NOT NULL,
    workspace_path TEXT,
    source_task_id TEXT,
    status         TEXT    NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','running','done','cancelled')),
    created_at     TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_lane_followups_lane_status ON lane_followups(lane_key, status, id);

UPDATE schema_version SET version = 33 WHERE version = 32;
