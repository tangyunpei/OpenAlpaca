# Database Migrations

> Generated from migration registry in `crates/openalpaca_storage/src/migrations/mod.rs`.

## Overview

- Total registered migrations: 4
- Migration SQL directory: `crates/openalpaca_storage/src/migrations`

## Files

| Version | Name | SQL File | Summary |
|---|---|---|---|
| 1 | `baseline` | `001_baseline.sql` | Baseline schema, version 1: the whole schema in one pass (the unreleased 001–045 development history squashed on 2026-09-28). A database from any earlier schema line is refused, never upgraded. |
| 2 | `working_state` | `002_working_state.sql` | Working state, version 2: `session`'s four summary columns become the conversation's working state — the state's JSON, its version, the last row it covers, when it was written, and the last conversation summary projected from it (design 11 §2, §6; owner decision CQ-9) — beside whether a new conversation carries the previous one's state (CQ-5). |
| 3 | `no_project_store_rows` | `003_no_project_store_rows.sql` | No project-store rows, version 3: every `file_assets` row whose bytes lie inside a project's own folder — the in-project `.openalpaca/` store this build never reads again (owner decisions ST-1, ST-9) — is deleted, and its versions and attachment links go with it by cascade (design R25). |
| 4 | `project_files` | `004_project_files.sql` | Project files, version 4: a result file an agent writes into its project gets a pointer row (owner decision ST-12, design §13 R35) — the real file's address, no copy, no versions. |
