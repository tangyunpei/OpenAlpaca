-- Migration 042: the "this client cannot answer a confirmation" declaration,
-- on the two rows that outlive the turn that made it (S5).
--
-- M6 gave `POST /v1/chat` an `unattended` flag: the client says it cannot
-- answer a tool-approval prompt, and the run refuses a confirm-listed tool at
-- once instead of holding for the 300-second timeout with nobody listening.
-- The flag lived only in memory, for the length of one turn — so the two
-- places where work is *parked now and run later* lost it:
--
--   * `lane_followups` — `queue_followup` promises a follow-up turn that the
--     follow-up runner re-enters after the workflow finalises, possibly after
--     a daemon restart. The ruling: it inherits the flag of the turn that
--     queued it.
--   * `task` — `POST /v1/tasks` parks a row that `start` / `rerun` / `resume`
--     dispatch later. The ruling: that route accepts the same field, so the
--     declaration has to survive until something launches the row.
--
-- Both default to 0, which is exactly today's behaviour: a client that says
-- nothing is a client that can be asked. Never an approval — the flag only
-- ever makes a refusal faster and more honest.

ALTER TABLE lane_followups ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task ADD COLUMN unattended INTEGER NOT NULL DEFAULT 0;

UPDATE schema_version SET version = 42 WHERE version = 41;
