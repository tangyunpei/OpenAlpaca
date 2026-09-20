-- Migration 037: run observability.
--
-- `subagent_span` is the per-subagent span the swimlanes are drawn from
-- (GAP-09). It deliberately does *not* extend `agent_task_history`: that table
-- is FK-bound to `agent(id)`, has a timezone-less `completed_at`, backs the
-- legacy `assignments` payload, and — decisively — gets no row at all until a
-- run finishes, so an in-flight span cannot exist there.
--
-- `id` is the spawn's `node_id`, so the span and the `DagNodeStarted` /
-- `DagNodeCompleted` pair name the same thing. `label` is unique per task
-- because the GUI keys its lanes by label (`ParallelWork.tsx`).
--
-- `blocked` is in the CHECK but is never written: it is derived at read time
-- from the confirmation broker's pending requests, so it cannot go stale when
-- a confirmation is answered while the daemon is down.

CREATE TABLE IF NOT EXISTS subagent_span (
    id                TEXT PRIMARY KEY,          -- the spawn's node_id
    task_id           TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    template_id       TEXT NOT NULL,             -- "research_agent"
    agent_instance_id TEXT NOT NULL,             -- "research_agent::a1b2c3d4"
    label             TEXT NOT NULL,             -- "review·3"; unique per task
    objective         TEXT,
    state             TEXT NOT NULL DEFAULT 'running'
        CHECK (state IN ('running', 'done', 'failed', 'blocked', 'cancelled')),
    detail            TEXT,
    started_at        TEXT NOT NULL,             -- RFC 3339, UTC
    ended_at          TEXT,
    duration_ms       INTEGER,
    output_preview    TEXT
);

CREATE INDEX IF NOT EXISTS idx_subagent_span_task ON subagent_span(task_id, started_at);
CREATE INDEX IF NOT EXISTS idx_subagent_span_state ON subagent_span(state);
CREATE UNIQUE INDEX IF NOT EXISTS idx_subagent_span_label ON subagent_span(task_id, label);

-- GAP-10: a run-scoped event log. The column is added here; the persistence
-- layer starts filling it in the next commit of this phase.
ALTER TABLE event_log ADD COLUMN task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_event_log_task ON event_log(task_id);

-- GAP-06: the re-run provenance link — which run a re-run was cloned from.
ALTER TABLE task ADD COLUMN source_task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_task_source ON task(source_task_id);

UPDATE schema_version SET version = 37 WHERE version = 36;
