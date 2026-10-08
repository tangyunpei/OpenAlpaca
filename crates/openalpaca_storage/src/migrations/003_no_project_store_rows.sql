-- No project-store rows, version 3: every `file_assets` row whose bytes lie inside a project's own folder — the in-project `.openalpaca/` store this build never reads again (owner decisions ST-1, ST-9) — is deleted, and its versions and attachment links go with it by cascade (design R25).
-- The test is the stored address, not a directory name: `storage_path`
-- begins with `project_root` followed by a separator, compared with `substr`
-- (never `LIKE`, whose `_` and `%` a path may hold). A store reached through
-- a link was recorded under the link's target, so it matches too. No row the
-- new layout writes matches: its bytes are under the home store's
-- `projects/<name>/`, which never lies inside a project (R13). A message that
-- attached a deleted row keeps its text and shows the file as unavailable.
-- All statements run inside the database migration transaction, after
-- `PRAGMA foreign_keys = ON` (`database/mod.rs`), so the cascades fire.

DELETE FROM file_assets
 WHERE project_root IS NOT NULL
   AND substr(storage_path, 1, length(project_root) + 1)
       IN (project_root || '/', project_root || '\');

INSERT INTO schema_version(version) VALUES (3);
