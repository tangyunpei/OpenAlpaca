-- Project files, version 4: a result file an agent writes into its project gets a pointer row (owner decision ST-12, design §13 R35) — the real file's address, no copy, no versions.
-- `in_project = 1` marks such a row: `project_root` is the project, `rel_path`
-- the file's path relative to it (an exact suffix of `storage_path`), `origin`
-- 'produced', `status` 'ready'. Every other row is 0. The unique address
-- index gains the column, so a pointer path equal to a by-product's or an
-- upload's address is a different row.

ALTER TABLE file_assets ADD COLUMN in_project INTEGER NOT NULL DEFAULT 0;

DROP INDEX IF EXISTS idx_file_assets_addr;

CREATE UNIQUE INDEX idx_file_assets_addr ON file_assets(COALESCE(project_root, ''), in_project, rel_path) WHERE rel_path IS NOT NULL;

INSERT INTO schema_version(version) VALUES (4);
