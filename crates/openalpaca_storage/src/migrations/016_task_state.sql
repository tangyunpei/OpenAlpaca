ALTER TABLE task ADD COLUMN state_json TEXT;
ALTER TABLE task ADD COLUMN state_version INTEGER NOT NULL DEFAULT 0;

UPDATE schema_version SET version = 16 WHERE version = 15;
