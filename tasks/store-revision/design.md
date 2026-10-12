# Store revision — the migration squash to version 1

**Status: rev 1.1 (2026-09-28) — design of record for Track A; SQ-1 and SQ-2 taken by the owner, SQ-1a/SQ-3/SQ-4 lead rulings. S1 (`62cbf02d`) and S2 (the documentation commit that records rev 1.1) are landed; S3–S6 are not built.** Map of record: session scratchpad `program/maps/migrations-store.md` (read-only, `wf_d8917bd5-a0f`, at HEAD `9bbe19ba`); every `path:line` below is at that HEAD.

## 0. The directive and the decisions

Owner, 2026-09-28: *"since the app is never distributed, I think we can revise the store and the migration schema, combine all the migration scripts into 001 baseline and configure the new db migrations starting from 002"*, then: *"I want the store now to start at 1 or 2, which is matching the migration table naming."*

| Id | Decision | Taken by |
|---|---|---|
| **SQ-1** | **The schema version is the migration file's number.** `001_baseline.sql` registers **1**; the next migration is `002_<name>.sql` registering **2**; `BASELINE_VERSION` stays `MIGRATIONS[0].version` (= 1). No lineage marker, no version-range arm, no compatibility path for a database from before this squash: it is refused (SQ-1a) and the owner factory-resets, which the switch-over note already prescribes. | owner |
| **SQ-1a** | The one refusal a pre-squash database meets is the *above this build* arm, and its text states both possibilities plainly (§1.4). It names the reset verb, which the old wording deliberately did not (04 §4 *Rollback*); with one schema line and no distributed build that property has no beneficiary, and a message that says "use the build that created it" to the owner's 42 store would be false. | lead ruling (say if you want the old no-verb wording kept) |
| **SQ-2** | Which *other* never-distributed compatibility paths go (each reverses or moots an earlier decision): (a) `store/legacy_root.rs` (D-D/T28), (b) the home-README refresh with its six superseded texts and digest tripwire, (c) the pre-D2 `state/assets/` tolerance (T31; `file_assets.rel_path`/`project_root` NOT NULL if every writer sets them), (d) the `.layout` backfills. **All four go** (slices S3–S6, §3); T28 and T31 become moot, D-D's diagnostic half is reversed. | owner, 2026-09-28 |
| **SQ-3** | The ~37 provenance comments citing the old chain ("migration 037", "predate 036"): rewrite only in files the squash touches; the rest stay as history (the 2026-09-20 standing decision, `tasks/todo.md`). | lead ruling |
| **SQ-4** | The on-disk layout (`state/`, `config/`, `plugins/`, the content kinds, project stores, `.layout` at layout version 1) is unchanged: this is a schema-line change, not a layout change. | lead ruling |

The 42 arm, the 1..41 arm, `PRE_MEMORY_REDESIGN_VERSION`, the 3.5 MB `schema42.db` fixture and its JSON fixture go with SQ-1 regardless of SQ-2 — dead once the baseline is 1.

## 1. The registry and the open path (slice S1, one commit)

### 1.1 `crates/openalpaca_storage/src/migrations/`

- `001_baseline.sql` (today 489 lines, `VALUES (43)` at `:489`): fold 044 and 045 in — `conversation_messages` (`:221-236`) gains `request_id TEXT, workspace_root TEXT` after `session_id TEXT`; a new `CREATE TABLE learning_cursor (session_id TEXT PRIMARY KEY, extracted_through INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, updated_at TEXT NOT NULL DEFAULT (datetime('now')), episode_through INTEGER NOT NULL DEFAULT 0)` beside `session`/`conversation_messages`; **no** `INSERT … SELECT` seeding (a session with no row already reads as the zero cursor, `repository/learning_cursor.rs:9-12`, `:20-30`); the first `--` comment (it is the generated summary) becomes *"Baseline schema, version 1: the whole schema in one pass (the unreleased 001–045 development history squashed on 2026-09-28). A database from any earlier schema line is refused, never upgraded."*; last line `INSERT INTO schema_version(version) VALUES (1);`.
- Delete `044_learning.sql` and `045_episodes.sql` (the generator's parity check, `scripts/gen_api_docs.py:1680-1685`, needs exactly one `NNN_*.sql` file).
- `mod.rs`: one entry `Migration { version: 1, name: "baseline", sql: include_str!("001_baseline.sql") }`; the header doc (`:1-6`) says the file number is the version; `:15` → *"Future schema changes start at version 2: add `002_<name>.sql` ending in `INSERT INTO schema_version(version) VALUES (2);` and append its entry here."*; `BASELINE_VERSION` unchanged in form.

### 1.2 `crates/openalpaca_storage/src/database/mod.rs`

Delete `PRE_MEMORY_REDESIGN_VERSION` (`:12-16`) and the 42 arm (`:128-151`); delete the 1..41 arm (`:152-165`: no version satisfies `0 < v < 1`); keep the *above this build* arm (`:167-186`) with the new text (§1.4); rewrite the docs at `:25-31`, `:97-110`, `:116-127`. `delete_database_files` (`:274-279`) is unchanged.

### 1.3 The cases at open, after

| Case | Predicate | Result |
|---|---|---|
| Fresh | `schema_version() == 0` (no table, or no row) | runs every registered migration in one transaction; opens at the newest (1 today) |
| In place | `0 < v < newest` | runs the pending migrations (none until `002` exists) |
| Up to date | `v == newest` | returns |
| **Above this build** | `v > newest` | **refused, nothing changed** — this is what a 42/43/44/45 database reads as now (`schema_version()` is `MAX(version)`, `:88-90`) |

### 1.4 The refusal text (pinned verbatim by the test and by fixture 06)

> Database `{file}` is at schema version {n}, which this build does not know (it creates schema version 1 and knows versions up to {newest}). Nothing was changed. If a newer OpenAlpaca build created this database, use that build. If it was created by a build from before the schema was re-baselined at version 1, it cannot be read or upgraded: run `openalpaca config reset --factory` to start again (it deletes the database and `llm.toml`'s keys; see the Installation Manual, "Switching to the Markdown Memory Build").

The rule "start-up refusals never name each other's remedies" (`mod.rs:171-173`, CLAUDE.md) still holds — this one names its own.

### 1.5 Tests (`database/tests.rs`) and fixtures

| Test | Change |
|---|---|
| `an_unsupported_old_database_is_refused_without_changing_its_schema_or_data` (`:67-218`) | delete (both wordings and the `[1, 41, 42]` loop are gone); the *above this build* test below is its successor |
| `a_database_newer_than_this_build_is_refused_without_changing_its_schema_or_data` (`:221-316`) | keep; pin the new sentence; assert it contains `openalpaca config reset --factory` exactly once and not "Unsupported legacy" |
| `a_fresh_database_opens_at_the_newest_version_with_no_memory_tables` (`:385-413`) | drop the `BASELINE_VERSION == 43` literal (`:386`); keep the `memory`/`memory_%` absence check (a redesign invariant) |
| `a_refused_legacy_database_opens_at_the_baseline_after_its_files_are_deleted` (`:1022-1064`) | stamp `newest_version() + 1` (what a pre-squash store now reads as; `BASELINE_VERSION - 1` would stamp 0 = fresh); the proof stays "refused → deleted → opens at the baseline" |
| `the_schema42_fixture_is_refused_without_vec0_and_its_bytes_are_unchanged` (`:1067-1212`), `SCHEMA42_FIXTURE`, the row constants, `schema42_fixture_path`, `sha256_hex` | delete (`sha2` stays a dependency for `store/tests.rs`) |
| `migration_044_…` (`:1221-1291`), `migration_045_…` (`:1302-1373`) | delete; add **one** test that a fresh database has `conversation_messages.request_id`/`.workspace_root` and `learning_cursor`'s six columns (the column assertions of both, minus the seeding) |
| `apps/openalpaca/src/commands/config/tests.rs:151-175` | stamp `newest + 1`; expect the new wording and exactly one `config reset --factory` |
| `crates/openalpaca_storage/tests/fixtures/schema42.db`, `tests/fixtures/memory/05-schema42-database-refused.json`, the two README rows (`:29`, `:283`'s `"fixture"` form), `.gitignore`'s `!crates/openalpaca_storage/tests/fixtures/*.db` (`:94-95`; no `.db` fixture remains — confirm with `git ls-files`) | delete |
| `tests/fixtures/memory/06-newer-database-refused.json` | keep the stamped 99; update its expectations to the new wording (contains the verb once) |
| `repository/learning_cursor.rs:9-12`, `:109` | reword: a session with no row reads as the zero cursor — the whole rule |

### 1.6 Generated docs

`python3 scripts/gen_api_docs.py` then `--check` clean: `docs/api/database/migrations.md` one row; `schema.md` "Registered migrations: 1", `conversation_messages` and `learning_cursor` sourced from `001_baseline.sql`.

## 2. Manuals, CLAUDE.md and records (slice S2, one commit)

- `docs/Installation_Manual.md:209-277` *Switching to the Markdown Memory Build*: quote the new refusal (§1.4); the seven steps minus step 7 (`state/cache/fastembed` — a pre-redesign leftover the owner deletes by hand once; keep one sentence, drop it after the owner's reset); **Keep the builds apart** re-derived: *a build from before 2026-09-28 (schema line 43–45, or 42) started against the store after the switch-over refuses it as a "legacy" schema and tells you to factory-reset — do not start one; the commit `ae8faca8` sentence goes.* If SQ-2 (b) is taken: add *"delete `README.md` at the store root; the next start re-seeds it"*; if (d): *"`.layout` keeps whatever it has"*. `:646-649` troubleshooting row → the new first sentence.
- `docs/QuickStart_Manual.md:131-135`, `docs/Daemon_Manual.md:1415-1421` (one baseline at 1, next is 2, no seeding, the one refusal), `docs/CLI_Manual.md:176` (the way out of "…which this build does not know"), `README.md:212`, `docs/tools/TECHNICAL.md:7`, `:1584`, `apps/openalpaca-gui/API_MAP.md:1086`.
- `CLAUDE.md`: `:56` (MD-6 sentence, the fixture), `:60` (044/045 → "the schema line restarted at 1 on 2026-09-28; a pre-squash build refuses the new store"), `:72`, `:103`, the *Storage* paragraph (`:274`: one entry, the four cases, the "To add migration 002" recipe, no `vec![43,44,45]`), `:278`, `:328` (the fixture row) — same commit.
- Records: `tasks/memory-redesign/04-implementation-plan.md` §4 rev-log line (MD-6's 42 arm superseded by SQ-1; *Rollback* inverted as above); `tasks/api-fix-plan.md` §0 *Owner decisions taken* row for SQ-1 (2026-09-28) with the commits; `tasks/memory-redesign/README.md` rev; this file's revision log; vault `10-decision-log.md` **ADR-043** and the `05-storage-and-data-model.md` callouts (same session).

## 3. The other never-distributed paths (slices S3…, one commit each, **after SQ-2**)

| Slice | Item | What goes | Tests |
|---|---|---|---|
| S3 | SQ-2 (a) `store/legacy_root.rs` | module, callers `apps/openalpacad/src/main.rs:118` and `store/mod.rs:123-127`, `store/legacy_root/tests.rs`, the `HOME`/`XDG_DATA_HOME` halves of `apps/openalpaca/src/test_util.rs:18-79` and `store/tests.rs:24-66` (keep the `OPENALPACA_HOME_STORE` sandboxing), `config/tests.rs:122-146`, `docs/Installation_Manual.md:149-207`, `:641-645`, `docs/QuickStart_Manual.md:136-140`, the `install.sh:133-141` comment, the `directories::ProjectDirs` use; T28/T31 rows become moot (record in api-fix-plan §0) | the deleted tests; `cargo test` still green |
| S4 | SQ-2 (b) README refresh | `SUPERSEDED_HOME_READMES`, `is_superseded_home_readme`, the refresh branch of `ensure_store` (`store/mod.rs:476-492`, leaving "write when absent"), `store/superseded_readmes/home-1…6.txt`, tests `store/tests.rs:342-393`, `:437-446`, `:469-484` (keep `:451-464` "an edited README is left alone") | as listed |
| S5 | SQ-2 (c) pre-D2 `state/assets/` | `uploads/tests.rs:605-640`, `store/tests.rs:255-267` (`interim_blob_path`), the `uploads/mod.rs:28-40` paragraph; then an audit of every `file_assets` writer — if all set `rel_path` and `project_root`, make both `NOT NULL` in the baseline and drop the NULL tolerance at `artifacts/mod.rs:402`, `:2169`, `routes/files_types.rs:71`, `routes/artifacts.rs:175`; if not, keep nullable and say so | the audit's result in the commit message |
| S6 | SQ-2 (d) `.layout` backfills | `store/mod.rs:822-836` | `store/tests.rs` cases that plant an old marker |

## 4. Gates (every slice)

`CARGO_TERM_COLOR=never`, full output to a file, the command's own exit code, then: `cargo test --workspace --exclude openalpaca_gui` ×2 (`pkill -f "sleep 3600"` after each); `cargo clippy --workspace --exclude openalpaca_gui --all-targets` — 80 warnings is the baseline (count lines starting `warning` less the "generated N warnings" summaries); `python3 scripts/gen_api_docs.py --check`; `cargo check -p openalpaca_gui`; the four `bun run` gates. Then the live check, **never against `~/.openalpaca`**: a release `openalpacad` + `openalpaca` started with `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in a scratch directory, CWD outside the repo, under `sandbox-exec -f scripts/eval/memory/no-network.sb`, showing `GET /v1/status.schema_version == 1`; then the same daemon pointed at a copy of that database stamped `newest + 1` (a `sqlite3` UPDATE on the copy), exiting with §1.4's text in `daemon.log`. `cargo fmt --check` is not a gate (fails on `main`); the edited files pass `rustfmt --check` on their own.

## 5. What the owner meets on the real store

Unchanged in substance from the switch-over note: the store (schema 42 per the record; 45 if this branch ever started on it) is refused at the first start of this build, `openalpaca config reset --factory` is the way through, `llm.toml`'s keys go with it, `USER.md` is reviewed by hand (G38). What changes is only the message (§1.4) and that a **pre-squash build must not be started against the store afterwards** (it would refuse the new store as "legacy" and prescribe a reset). Take the `state/openalpaca.db` copy before the first start, as before.

## Revision log

- rev 1.1 (2026-09-28): S1 landed `62cbf02d`; S2 landed in the documentation commit that records this entry. What S2 did beyond, or short of, §2: (1) `CLAUDE.md:278` is left to S3 — it cites `store/legacy_root.rs`, not the schema line. (2) The history records — `tasks/memory-redesign/01`, `06b`, `06d`, `07`, `08`, `09` and `tasks/todo.md` — keep their text: they state what was built or planned then (SQ-3's reasoning); 03 (rev 7.16), 04 (rev 4.8, a callout opening §4) and `tasks/api-fix-plan.md` (rev 3.13, the SQ-1 row, and marks on MD-6, MD-10 and MD-16) are marked in place. (3) Beyond §2: the behaviour comment at `apps/openalpaca/src/commands/config.rs:113-118` ("a database this build does not know"), the GUI test's mock `schema_version` (43 → 1), 03 §8's migrations row, and in `CLAUDE.md` a *Current Work* paragraph, a documents-of-record row for this file, and SQ-1/SQ-2 among the taken decisions. (4) The Installation Manual's SQ-2 (b)/(d) sentences are not written: S4 and S6 are not built. (5) *Keep the builds apart* is derived from each old build's code: every build from the first squash (`d7848bc4`, 2026-09-21) on reads version 1 as below its baseline and refuses it as `Unsupported legacy schema version 1`, naming `openalpaca config reset --factory` from `73a0698d` (2026-09-22) and telling the user to delete the file before it; a build from before 2026-09-21 has no such check and would run its migrations 2–42 on a version-1 store. (6) Not changed: §1.4's "it creates schema version 1" stays exact only while 1 is the newest version — once `002` exists a fresh database is created at 1 and migrated in the same transaction (S1's report).
- rev 1 (2026-09-28): first design; SQ-1 taken by the owner ("start at 1 or 2, matching the migration table naming"); SQ-2 taken the same day (all four paths go); SQ-1a/SQ-3/SQ-4 lead rulings.
