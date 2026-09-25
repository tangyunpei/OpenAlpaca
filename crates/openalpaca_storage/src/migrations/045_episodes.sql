-- 045: Conversation summaries (tasks/memory-redesign/09 §13.3): a per-session episode cursor beside the learning cursor.
ALTER TABLE learning_cursor ADD COLUMN episode_through INTEGER NOT NULL DEFAULT 0;

-- Historic conversations are not summarised, as they are not re-extracted (design 03 §6): every existing session starts at its newest message. A session with no cursor row gets one whose learning cursor reads 0, which is what no row read.
INSERT INTO learning_cursor (session_id, extracted_through, episode_through)
    SELECT s.id, 0, COALESCE((SELECT MAX(m.id) FROM conversation_messages m WHERE m.session_id = s.id), 0)
    FROM session s WHERE true
    ON CONFLICT(session_id) DO UPDATE SET episode_through = excluded.episode_through;

INSERT INTO schema_version(version) VALUES (45);
