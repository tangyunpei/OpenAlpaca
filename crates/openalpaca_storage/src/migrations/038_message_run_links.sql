-- Migration 038: message → run links (GAP-23).
--
-- One nullable column, no link table. A chat message points at the run it
-- started (the delegating turn) or the run it reports on (the completion
-- report); the artifacts that run produced are already reachable, because
-- `conversation_message_attachments` has carried a `role` since 028 — the
-- report's files are linked there with `role='artifact'`, beside the
-- `role='attachment'` uploads a user turn carries.
--
-- Deliberately not a foreign key. `task` rows are prunable and a transcript is
-- not: a purged run must leave the turn that started it readable, with a
-- dangling id, rather than take the message with it or block the delete.

ALTER TABLE conversation_messages ADD COLUMN task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_conv_msg_task ON conversation_messages(task_id);

UPDATE schema_version SET version = 38 WHERE version = 37;
