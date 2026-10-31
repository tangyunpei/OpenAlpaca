# Storage group: rusqlite 0.38.0 → 0.40.2, sysinfo 0.38.4 → 0.39.6, lru 0.12.5 → 0.18.5

Researcher: storage group, 2026-09-25. Worktree `SP/wt-storage` (detached at `5e5ac317`), target dir `SP/target-198`.
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.

Evidence tags: **[measured]** (I ran it; command and result given), **[our-code]** (file:line in the worktree at HEAD), **[dep-source]** (a path under `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`, abbreviated `REG/`), **[docs]** (a changelog or release-notes URL).

## 1. Summary

| Package | From → to | Compile errors before any fix | Code lines changed | Needs Rust ≥ | Tests | Recommendation |
|---|---|---|---|---|---|---|
| rusqlite (+ libsqlite3-sys 0.36.0 → 0.38.2, hashlink 0.11.1 → 0.12.2; bundled SQLite **3.51.1 → 3.53.2**) | 0.38.0 → 0.40.2 | **0** | 0 (1 manifest line) | 1.93 is enough (measured) | storage 678/678, core `memory` 335/335, connectors+imessage 70/70 | upgrade now (own commit; it can land before the toolchain bump) |
| lru | 0.12.5 → 0.18.5 | **0** | 0 (1 manifest line) | 1.85 (declared); built on 1.93 (measured) | core `compose` 82/82 (incl. the three `global_cache` tests) | upgrade now (own commit) |
| sysinfo (+ new macOS-only transitive `objc2-open-directory` 0.3.2) | 0.38.4 → 0.39.6 | **0** | 0 (1 manifest line) | **1.95** (declared, and cargo refuses 1.93: measured) | `daemon_lifecycle` 9/9 incl. the real-zombie test (macOS) | upgrade with the toolchain commit or after it |
| directories 6.0.0, similar 3.2.0, unicode-normalization 0.1.25 | — | — | — | — | — | nothing to do: the locked version is the latest (`SP/direct-deps.tsv`) |

Whole diff (`SP/patches/storage.diff`): `Cargo.toml` 3 lines, `Cargo.lock` +29 −37. The lock also drops `hashbrown 0.15.5`, `foldhash 0.1.5` and `allocator-api2` (lru was their last user) and adds `objc2-open-directory`. No `.rs` file changes. **It compiles** (`cargo +1.98.1 check --workspace --all-targets --exclude openalpaca_gui`: 0 errors).

The one behaviour change that matters is in SQLite, not in a Rust API: **SQLite 3.53 renders REAL → TEXT with 17 significant digits instead of 15** (`CAST(0.1+0.2 AS TEXT)` is now `0.30000000000000004`, was `0.3`). I found no SQL in the tree that converts a REAL to text, and rusqlite never converts one on read, so our stored data and wire formats do not change (§3.4). Files are compatible in both directions: a database and memory index written by 3.53.2 open, verify and search in 3.51.1, and the reverse also works. WAL left un-checkpointed by the other version replays correctly (§3.5). The upgrade also fixes three SQLite bugs present in our bundled 3.51.1: the WAL-reset corruption race, a deadlock in the broken-POSIX-lock detector, and wrong results from the EXISTS-to-JOIN optimisation (§3.3).

## 2. What I ran

All cargo commands ran as `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=SP/target-198 cargo +<toolchain> …` in `SP/wt-storage`, one at a time, with output in `SP/logs/`. Warnings and errors were counted with `grep -c '^warning'` / `grep -c '^error'` on those colour-free logs.

| # | State | Command | Result | Log |
|---|---|---|---|---|
| A | HEAD (baseline) | `+1.98.1 test -p openalpaca_storage` | 678 passed, 3 ignored (the benches); 0 warnings | `storage-A-baseline-test-storage.log` |
| B1 | rusqlite only | `+1.98.1 check -p openalpaca_storage --all-targets` | 0 errors, 0 warnings. Lock: `hashlink 0.11.1→0.12.2`, `libsqlite3-sys 0.36.0→0.38.2`, `rusqlite 0.38.0→0.40.2` | `storage-B-rusqlite-check-storage.log` |
| B2 | rusqlite only | `+1.98.1 check -p openalpaca_connectors --features imessage --all-targets` | 0 errors, 7 warning lines = 5 `unused import` in `startup.rs` + 2 summary lines, not from the bump (§3.8) | `storage-B-rusqlite-check-connectors.log` |
| B3 | rusqlite only | `+1.93.0 check -p openalpaca_storage --all-targets` | **0 errors, 0 warnings on Rust 1.93.0** | `storage-B-rusqlite-check-storage-rust193.log` |
| B4 | rusqlite only | `+1.98.1 test -p openalpaca_storage` | 678 passed, 3 ignored; 0 warnings | `storage-B-rusqlite-test-storage.log` |
| C1 | + sysinfo | `+1.98.1 check -p openalpaca_storage -p openalpaca --all-targets` | 0 errors, 0 warnings. Lock: `sysinfo 0.38.4→0.39.6 (requires Rust 1.95)`, adds `objc2-open-directory 0.3.2` | `storage-C-sysinfo-check.log` |
| C2 | + sysinfo | `+1.93.0 check -p openalpaca_storage` | exit 101: `rustc 1.93.0 is not supported by the following package: sysinfo@0.39.6 requires rustc 1.95` | `storage-C-sysinfo-check-rust193.log` |
| C3 | + sysinfo | `+1.98.1 test -p openalpaca_storage daemon_lifecycle` | 9 passed, including `a_real_unreaped_child_is_not_a_live_process` | `storage-C-sysinfo-test-lifecycle.log` |
| D1 | + lru | `+1.98.1 check -p openalpaca_core --all-targets` | 0 errors, 0 warnings. Lock: `lru 0.12.5→0.18.5` | `storage-D-lru-check-core.log` |
| D2 | scratch crate `SP/lru193` (`lru = "=0.18.5"`, the exact four calls `compose/mod.rs` makes) | `+1.93.0 run` | builds on 1.93.0; `iter()` is MRU-first (`[1, 2]` after `get(&1)`); `put` at capacity evicts the LRU entry (`[3, 1]`) | `storage-D-lru-rust193.log` |
| E1 | all three | `+1.98.1 check --workspace --all-targets --exclude openalpaca_gui` | 0 errors; 1 warning: the future-incompat note for `proc-macro-error2 v2.0.1`, pulled in by `teloxide → aquamarine`, not by this group (`cargo tree -i proc-macro-error2`) | `storage-E-all-check-workspace.log` |
| E2 | all three | `+1.98.1 test -p openalpaca_storage` | **678 passed**, 0 failed, 3 ignored; 0 warnings | `storage-E-all-test-storage.log` |
| E3 | all three | `+1.98.1 test -p openalpaca_core memory` | **335 passed**, 0 failed, 4 ignored; 0 warnings | `storage-E-all-test-core-memory.log` |
| E4 | all three | `+1.98.1 test -p openalpaca_core --lib compose` | 82 passed (incl. `test_global_cache_persona_hit_on_second_call`, `…_miss_on_version_bump`, `test_global_cache_static_prompt_hit`) | `storage-E-all-test-core-compose.log` |
| E5 | all three | `+1.98.1 test -p openalpaca_connectors --features imessage` | **70 passed**, 0 failed (4 `imessage::reader` tests on in-memory chat.db schemas) | `storage-E-all-test-connectors-imessage.log` |
| P1 | probe, old vs new SQLite | `SP/probe/{old,new}`: `Cargo.toml`, `Cargo.lock` and `crates/openalpaca_storage` from HEAD and from the bumped worktree, plus a probe binary (`SP/probe/probe_main.rs`) that calls the real `Database::open` and `MemoryIndex::open` | see §3.4–§3.6 | `storage-probe-*.log` |
| P2 | 10K memory-index bench, old vs new, release | `cargo test --release -p openalpaca_storage --lib -- --ignored --exact memory_index::bench::bench_search_10k_notes_with_verification --nocapture` in each probe tree, then the two test binaries alternated 3× | no regression outside machine noise (§3.7) | `storage-bench10k-*.log` |

Not run: `cargo check -p openalpaca_gui` (the Tauri crate links `openalpaca_storage`, so it links rusqlite and sysinfo too, and CI compiles nothing under `src-tauri`); `cargo clippy` (with 1.98.1 its lints come from the toolchain bump, not from this group); the CLI's own tests (`apps/openalpaca`). The CLI's sysinfo call site compiled (C1); its tests are pure name matching (`is_gui_process_name`).

## 3. rusqlite 0.38.0 → 0.40.2 (bundled SQLite 3.51.1 → 3.53.2)

### 3.1 Which SQLite is bundled

- 0.38.0 → `libsqlite3-sys 0.36.0` → **SQLite 3.51.1** (`SQLITE_SOURCE_ID "2025-11-28 17:28:25 …"`), [dep-source] `REG/libsqlite3-sys-0.36.0/sqlite3/sqlite3.h`.
- 0.40.2 → `libsqlite3-sys 0.38.2` → **SQLite 3.53.2** (`"2026-06-03 19:12:13 d6e03d8c…"`), [dep-source] `REG/libsqlite3-sys-0.38.2/sqlite3/sqlite3.h`. The probe's `SELECT sqlite_version()` printed `3.51.1` and `3.53.2` [measured, `storage-probe-fp.log`].
- The bundled compile flags are **identical** between the two libsqlite3-sys versions: the sorted set of `-DSQLITE_*`/`-USQLITE_*` flags in the two `build.rs` files diffs empty [dep-source]. That includes `SQLITE_ENABLE_FTS5`, `SQLITE_ENABLE_FTS3`, `SQLITE_ENABLE_DBSTAT_VTAB` (the bench's `dbstat` read), `SQLITE_ENABLE_STAT4`, `SQLITE_DEFAULT_FOREIGN_KEYS=1`, `SQLITE_THREADSAFE=1` and `SQLITE_USE_URI`. The rest of the `build.rs` diff is a local `cfg_select!` shim ("Just to keep MSRV low").
- `bundled` still implies `modern_sqlite`: 0.38 has `bundled = ["libsqlite3-sys/bundled", "modern_sqlite"]`, and 0.40.2 has `["libsqlite3-sys?/bundled", "modern_sqlite"]`, where the `?` is there because libsqlite3-sys became a target-specific dependency. It is non-optional on every non-wasm target [dep-source `REG/rusqlite-0.40.2/Cargo.toml`]. No feature was renamed that we use: we enable `bundled` plus the default `cache` feature.
- The path through rusqlite releases: 0.39.0 → SQLite 3.51.3; 0.40.0 → 3.53.1; 0.40.1 → 3.53.2 [docs: https://github.com/rusqlite/rusqlite/releases]. SQLite **3.52.0 was withdrawn**; its features shipped in 3.53.0 [docs: https://sqlite.org/changes.html].

### 3.2 rusqlite API changes and whether they hit us

**Compile errors before fixing: 0** in `openalpaca_storage` (all targets), in `openalpaca_connectors --features imessage`, and across the workspace [measured, B1/B2/E1]. Our API surface is `params!`, `params_from_iter`, `query_row`/`query_map`/`query`, `prepare`/`prepare_cached`, `execute`/`execute_batch`, `transaction`/`unchecked_transaction`, `OptionalExtension`, `types::{ToSql, Value}`, `Connection::open`/`open_with_flags`/`open_in_memory`, `OpenFlags`, `busy_timeout` and `last_insert_rowid` (grep over `crates/openalpaca_storage`, `crates/openalpaca_connectors`). Changes between the versions [dep-source diffs of `REG/rusqlite-0.38.0/src` vs `REG/rusqlite-0.40.2/src`; docs: release notes above]:

| Change | Hits us? |
|---|---|
| `ToSql`/`FromSql` for `u64`/`usize` are still gated behind the `fallible_uint` feature exactly as in 0.38. `types/to_sql.rs` changes only `ToSqlOutput::Array` → `ToSqlOutput::Pointer` (feature `pointer`). `i128` needs `i128_blob`, as before. | No: we enable neither feature, and it compiles. |
| `rusqlite::Error::Utf8Error(Utf8Error)` → `Utf8Error(usize, Utf8Error)`. Invalid UTF-8 in a TEXT column now surfaces as `Error::Utf8Error(idx, _)`, Display `"… at index N"`, where 0.38 gave `FromSqlConversionFailure(idx, Text, _)`. There is a new `FromSqlError::Utf8Error`. | Nothing matches on these (grep). The only user-visible trace is wording: the iMessage reader logs `"Failed to read message row, skipping: {e}"` and skips the row either way [our-code `crates/openalpaca_connectors/src/imessage/reader.rs:181-185`]. |
| `impl From<ValueRef> for Value` → `TryFrom` (0.38 **panicked** on invalid UTF-8 there) | We build `Value::Text`/`Value::Integer` only as parameters [our-code `crates/openalpaca_storage/src/artifacts/mod.rs:1174-1220`]. Not hit. |
| Savepoint names are now quoted as identifiers (a SQL-injection fix, 0.40.1) | We use no savepoints (grep). Not hit. |
| `Transaction` default drop behaviour | Unchanged: still `DropBehavior::Rollback` [dep-source `transaction.rs`]. |
| `busy_timeout` / busy handler | Refactor only [dep-source `busy.rs`]. |
| VTab API breaking changes, `sqlite-wasm-rs` opt-out, `UtcDateTime`, `set_errmsg`, `SQLITE_PREPARE_FROM_DDL` | We define no virtual tables and use none of the rest. |
| hashlink 0.12 (the `prepare_cached` LRU) | Internal; tests pass. |
| MSRV | rusqlite 0.40.2's notes say "Lower MSRV to 1.88.0"; neither rusqlite nor libsqlite3-sys declares `rust-version`, and hashlink 0.12.2 declares 1.85. **Builds on Rust 1.93.0** [measured, B3]. |

### 3.3 SQLite release-note items 3.51.2 … 3.53.2 that could touch us

Sources: https://sqlite.org/changes.html, https://sqlite.org/releaselog/3_53_0.html, https://sqlite.org/releaselog/3_51_2.html, https://sqlite.org/wal.html (WAL-reset section), and the 3.53.0 → 3.53.2 check-in timeline https://sqlite.org/src/timeline?from=version-3.53.0&to=version-3.53.2&to2=branch-3.53&y=ci [all docs].

| Area | What changed | Effect on us |
|---|---|---|
| **WAL** | 3.51.3 / 3.53.0: "Fixed the WAL-reset database corruption bug". It affects 3.7.0 through 3.51.2 in WAL mode "when there are two or more database connections open on the same file, in separate threads or processes, and when those two connections attempt to write or checkpoint at the same instant". Rated extremely improbable. 3.53.2 also fixes a 32-bit `xShmMap` overflow for WAL files near 25 TB. | **Positive.** Our 3.51.1 is affected. The daemon holds one connection to `state/openalpaca.db` [our-code `crates/openalpaca_storage/src/database/mod.rs:40-57`], but the CLI opens the same file from another process while a daemon may be running (`store::open_home_database()`, used by `apps/openalpaca/src/commands/config.rs:79` and `apps/openalpaca/src/commands/memory.rs:1459`). That meets the stated precondition. |
| **Locking** | 3.51.2: "Fixed an obscure deadlock in broken-POSIX-lock detection logic" (the detection is new in 3.51.0) | **Positive.** Present in our 3.51.1. |
| **Query planner: EXISTS-to-JOIN** | New in 3.51.0. 3.51.2 "Fixed multiple problems" in it, and 3.53.1 fixes two more (the OR-optimisation early exit, and `OFFSET`). 3.53.0 enhances it further. | **Positive.** Our 3.51.1 has the known-wrong-result versions. We have `EXISTS (SELECT …)` in `WHERE` [our-code `repository/learning_cursor.rs:68,90,138`, `followup/mod.rs:207`, `memory_index/search.rs:562`, `artifacts/mod.rs:2597`]. I did not establish whether any of our queries was actually mis-planned under 3.51.1. |
| Query planner, other | 3.53.0: "Always use sort-and-merge for EXCEPT, INTERSECT, UNION", better star-schema join order, omit-noop-join, GROUP BY/ORDER BY. 3.53.1/2: COLLATE and transitive-constraint fixes. | Our one `UNION` has `ORDER BY root` [our-code `artifacts/mod.rs:1659-1665`]. The four query-plan tests pass on 3.53.2: `test_execution_log_counts_use_timestamp_indexes`, `test_llm_call_log_has_a_timestamp_index`, `provider_usage_since_query_plan_uses_the_timestamp_index`, and the INDEXED BY queries at `repository/skill_execution/mod.rs:198,229` [measured, E2]. |
| **INDEXED BY** | Nothing in the notes | Tests pass [measured]. |
| **FTS5 / trigram / unicode61** | No feature or behaviour change in any of the notes. The 3.53.1/3.53.2 check-ins harden fts5 against **corrupt** records (buffer over-read/overwrite fixes, "Improved detection of corrupt fts5 records") and ignore nested porter tokenizers. | Memory-index tests and the 24 invariant fixtures pass (E2). This includes the audit's G23 "prefer → prefers" trigram case, which `tasks/memory-redesign/03-design.md:274` recorded as reproduced on 3.51. The probe's `integrity-check` on `fts_word`/`fts_tri` is `ok` both ways (§3.5). The bench returns the same 1600 hits / 172 partial / 0 empty on both versions (§3.7). |
| **Floating point → text** | 3.53.0: "Rounding is now done by default to 17 significant digits, instead of 15, as was the case for all prior versions" (new `SQLITE_DBCONFIG_FP_DIGITS`). A 3.53.x check-in: "floating-point to text conversions in JSON to the full 17-digits". | See §3.4. **Measured** to change CAST/`printf('%s')`/`quote`/`json_object`/`json_array` output and REALs stored into TEXT-affinity columns. I found no code of ours that does any of these. |
| **JSON** | New `json_array_insert`/`jsonb_array_insert`, plus JSONB hardening | We use no SQL JSON functions (grep over `*.rs`/`*.sql`: none). |
| **ALTER TABLE** | 3.53.0 can add/remove NOT NULL and CHECK constraints (`ADD CONSTRAINT`, `DROP CONSTRAINT`, `ALTER COLUMN … SET NOT NULL`) | Our migrations use none of it. See §3.6 for what an older build does if a future migration does. |
| **Expression indexes** | 3.53.0 adds a "self-healing index" for stale expression indexes, and `REINDEX EXPRESSIONS` | Our one expression index is `idx_file_assets_addr ON file_assets(COALESCE(project_root, ''), rel_path)` [our-code `crates/openalpaca_storage/src/migrations/001_baseline.sql:305`]. `COALESCE` is deterministic and unchanged. Both versions use the index on the other's file: `SEARCH file_assets USING INDEX idx_file_assets_addr (<expr>=? AND rel_path=?)` [measured, §3.5]. |
| CLI / QRF / session / zipfile / rtree / fts3 | Only the `sqlite3` shell, extensions and corruption hardening | Not used. |
| **busy_timeout** | Nothing | — |

### 3.4 The float → text change, measured

`SP/probe/{old,new}` binary `probe fp` [measured, `storage-probe-fp.log`]:

| Expression | 3.51.1 | 3.53.2 |
|---|---|---|
| `CAST(0.1 AS TEXT)`, `0.1 \|\| ''`, `printf('%s', 0.1)`, `json('[0.1]')`, `CAST(3.14 AS TEXT)`, `CAST(2.5e-7 AS TEXT)`, `printf('%.2f', 1.0/3)`, `CAST(round(1.0/3,4) AS TEXT)` | unchanged | unchanged |
| `CAST(0.1 + 0.2 AS TEXT)` | `0.3` | `0.30000000000000004` |
| `CAST(1.0/3 AS TEXT)` / `printf('%s', 1.0/3)` | `0.333333333333333` | `0.33333333333333332` |
| `quote(1.0/3)` | `3.333333333333333148e-01` | `0.33333333333333332` |
| `json_object('c', 1.0/3)` | `{"c":0.333333333333333}` | `{"c":0.33333333333333332}` |
| `json_array(0.1 + 0.2)` | `[0.3]` | `[0.30000000000000004]` |
| f64 `0.1+0.2` bound into a **TEXT-affinity column** | `0.3` | `0.30000000000000004` |
| f64 read of `0.1 + 0.2` (REAL → f64, no text) | `0.30000000000000004` | same |

Does it reach us? Every REAL column is declared `REAL` (`001_baseline.sql:154,155,367,384,391,392,421,446,449`) and bound from and read into `f64` [our-code, `grep f64` over `crates/openalpaca_storage/src`]. rusqlite refuses to read a REAL as `String`: the probe's first version hit `Invalid column type Real` [measured]. So no REAL is converted to text on the read path. A grep of every `*.rs`/`*.sql` for `json_*(`, `jsonb`, `printf(`, `CAST(… AS TEXT)` in SQL finds nothing. The `CAST(… AS REAL)` divisions in `repository/subagent/mod.rs:188-206` and `skill_execution/mod.rs:251-276` stay REAL and are read as f64. **Conclusion:** no on-disk or wire value of ours changes. This holds only while no SQL of ours converts REAL to TEXT. A future query that does will print 17 digits.

### 3.5 File-format compatibility in both directions, measured

`probe write <dir> [crash]` runs the real `Database::open` (migrations to 45 on a fresh file) and writes 200 `llm_call_log` rows with REAL costs, 200 `event_log` rows with non-ASCII text and one `file_assets` row through the expression index. It then runs the real `MemoryIndex::open` + `rebuild` over 3 notes: English, Japanese, and a German alias. `crash` exits with `std::process::exit(0)` and leaves `openalpaca.db-wal` (4,132,392 bytes) and `-shm` un-checkpointed. `probe read <dir>` reopens both with the other build and runs `PRAGMA integrity_check`, the row counts, `sum(cost_usd)`, the expression-index plan, `index_state()`, six searches, FTS5 `integrity-check` on both tables and `quick_check` [measured, `storage-probe-crossversion.log`]:

| Scenario | Result |
|---|---|
| 1. 3.53.2 writes, clean close → 3.51.1 reads | schema_version 45, `integrity_check = ok`, 200/200 rows, `sum = 2030.0`, index used, index `Ready` (schema 3, 3 notes), all searches return the same ids, both FTS5 `integrity-check` ok, `quick_check` ok |
| 2. 3.53.2 writes, **exits with the WAL un-checkpointed** → 3.51.1 opens and replays it | identical to 1 |
| 3. 3.51.1 writes, WAL un-checkpointed → 3.53.2 replays it | identical to 1 |
| 4. 3.51.1 writes → 3.53.2 reads → 3.51.1 reads again | identical to 1 at every step |

So **a database written by the new SQLite is readable by the old one**, and a downgrade to a pre-bump build of the same schema still opens the store. The memory index cache is also portable, and it is disposable in any case: `MemoryIndex::open` deletes and recreates a file that fails `quick_check`, lacks a table or carries another `SCHEMA_VERSION` [our-code `crates/openalpaca_storage/src/memory_index/mod.rs:462-551`]. The schema-42 refusal still reads the committed fixture byte for byte on 3.53.2: `database::tests::the_schema42_fixture_is_refused_without_vec0_and_its_bytes_are_unchanged ... ok` [measured, E2]. That is the re-test `tasks/memory-redesign/06d-evidence-vec0-spike.md:86` asks for "if the `rusqlite` or SQLite version changes".

### 3.6 "An older build refuses a newer schema cleanly": does a 3.53-only feature break that?

The promise works by reading `schema_version` before any other SQL [our-code `database/mod.rs:71-79`, `:111-178`]. If a future migration used 3.53-only syntax, could a 3.51.1 build still read that number? `probe newsyntax` (3.53.2) / `probe opensyntax` (3.51.1) [measured, `storage-probe-newsyntax.log`]:

| 3.53-only statement applied by 3.53.2 | Stored schema text | 3.51.1: `SELECT version FROM schema_version` | 3.51.1: read `t` | 3.51.1: `INSERT INTO t` |
|---|---|---|---|---|
| `ALTER TABLE t ADD CONSTRAINT pos CHECK (x > 0)` | `CREATE TABLE t(x INTEGER, CONSTRAINT pos CHECK (x > 0))` | ok (46) | ok | ok (constraint enforced) |
| `ALTER TABLE t ALTER COLUMN x SET NOT NULL` | `CREATE TABLE t(x INTEGER NOT NULL)` | ok | ok | ok |
| `CREATE INDEX … ON t(json_array_insert(…))` | as written | ok | ok | **fails**: `unknown function: json_array_insert()` |
| `CREATE VIEW … json_array_insert(…)` | as written | ok | ok | ok |

The refusal stays clean, because `schema_version` is readable in every case. A 3.53-only function inside an index or trigger would only break writes to that table in the older build, and a newer schema is refused before any write. I tested four statements, not every 3.53 feature.

### 3.7 Performance: 10K-note memory-index bench, measured

`bench_search_10k_notes_with_verification`, release, same machine (Apple aarch64, 18 threads, load average 12–15 from the other workloads) [measured, `storage-bench10k-{old,new,alternating}.log`]:

| Run | build (s) | search+verify p50 / p95 / max (ms) |
|---|---|---|
| old (cargo) | 4.17 | 6.89 / 13.52 / 18.17 |
| new (cargo) | 8.54 | 10.85 / 20.59 / 32.08 |
| old 1 / new 1 | 9.29 / 7.74 | 11.40 / 21.23 / 26.66 vs 10.06 / 19.55 / 26.91 |
| old 2 / new 2 | 4.96 / 4.42 | 9.03 / 18.68 / 27.25 vs 7.43 / 14.38 / 20.71 |
| old 3 / new 3 | 9.45 / 6.06 | 10.38 / 19.77 / 25.84 vs 8.64 / 17.13 / 23.92 |

The first pair looked like a regression. Alternating runs put the new build no slower (p95 14–20 ms vs 19–21 ms), so the difference is inside machine noise. Every run is far under the plan's 50 ms target. Cache size is identical (109.2 MiB), and every run gave 1600 hits, 0 empty, 172 partial and 1600 verified. I did not run the 100K benches.

### 3.8 The iMessage connector (chat.db)

The reader opens Apple's `chat.db` with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX` and `busy_timeout(5 s)` [our-code `crates/openalpaca_connectors/src/imessage/reader.rs:63-70`]. Messages.app writes that file with the system SQLite. A newer bundled reader of an older-written file is the direction §3.5 shows working. The 4 reader tests (in-memory chat.db-shaped schemas, with and without `date_deleted`) pass [measured, E5]. The real `chat.db` was **not** opened (privacy; not needed). The 5 `unused import` warnings in `crates/openalpaca_connectors/src/startup.rs:3-7` are not from the bump: all five are used only by the `#[cfg(feature = "telegram")]` function at [our-code `startup.rs:106-112`], so an imessage-only build always warns. I did not measure a baseline for this one.

## 4. sysinfo 0.38.4 → 0.39.6

- Changelog [dep-source `REG/sysinfo-0.39.6/CHANGELOG.md`; docs https://github.com/GuillaumeGomez/sysinfo]:
  - 0.39.0 raises the MSRV to **1.95**, adds `NetworkData::operational_state` and `Process::cgroup_limits`, and fixes users/soundness.
  - 0.39.2: "Linux: Correctly set thread information for processes".
  - 0.39.3: "Linux: Improve retrieval of process information if process terminates while doing so".
  - 0.39.4: soundness of group retrieval, plus macOS "Tahoe" naming and `open_files`.
  - 0.39.5: app-store build.
  - 0.39.6: NetBSD.
  - `migration_guide.md` has no 0.38 or 0.39 entry.
- **MSRV:** `rust-version = "1.95"` [dep-source `REG/sysinfo-0.39.6/Cargo.toml:14`]. Cargo refuses the build on 1.93.0 (`sysinfo@0.39.6 requires rustc 1.95`) [measured, C2]. The resolver picks it anyway under the workspace's `rust-version = "1.93"`, printing `(requires Rust 1.95)` [measured]. The MSRV bump coincides with `cfg_if!` → `cfg_select!` in its sources [dep-source `src/common/system.rs`].
- **Our call sites compile unchanged: 0 errors** [measured, C1]:
  - `crates/openalpaca_storage/src/daemon_lifecycle.rs:75-117`: `System::new`, `refresh_processes(ProcessesToUpdate::All, true)`, `process(Pid::from_u32)`, `name()`, `exe()`, `status()`, `ProcessStatus::{Zombie, Dead}`.
  - `apps/openalpaca/src/manager.rs:180-200`: `processes()`, `name()`, `Pid::as_u32`.
- **ProcessStatus is unchanged.** Its variant list is identical in 0.38.4 and 0.39.6: `Idle, Run, Sleep, Stop, Zombie, Tracing, Dead, Wakekill, Waking, Parked, LockBlocked, UninterruptibleDiskSleep, Suspended, Unknown(u32)` [dep-source, awk over both `src/common/system.rs`].
- **Name reporting and zombie detection:**
  - **macOS:** the process diff is only `cgroup_limits` (returns `None`) and moving `open_files` [dep-source `src/unix/apple/macos/process.rs`, `src/unix/apple/process.rs`]. The real-process test spawns `sleep`, reads it live by name (positive control), then leaves an exited child unreaped via `waitid(WNOWAIT)` and asserts it is not live. It **passes** on 0.39.6 [measured, C3, macOS only].
  - **Linux:** `name`, `exe` (including the ` (deleted)` strip) and the status parse are unchanged. The changes are these:
    - A task whose tgid ≠ pid is marked a thread in `All` mode too. That affects `thread_kind()` only.
    - A process that dies mid-refresh no longer has `exe`/`cmd`/`environ`/`cwd`/`root` overwritten with empty. We build a fresh `System` per call, so there is no older value to keep.
    - Out-of-range `/proc/<pid>/stat` fields no longer panic: the status falls back to `Unknown`, and our rule treats `Unknown` as live [dep-source diff of `src/unix/linux/process.rs`].
  - **Linux was not measured here.** CI's ubuntu job runs the same `#[cfg(unix)]` real-zombie test.
- New transitive dependency `objc2-open-directory 0.3.2` (macOS). It comes from sysinfo's default `user` feature, which we do not use. Trimming sysinfo's default features would drop it, but that is an optional follow-up the bump does not force.

## 5. lru 0.12.5 → 0.18.5

- The changelog [dep-source `REG/lru-0.18.5/CHANGELOG.md`] is additive or fixes everywhere except three points:
  - 0.14 uses `NonZeroUsize::MAX`.
  - 0.15 `promote`/`demote` return `bool`.
  - 0.18.0 fixes an unconstrained lifetime in `get_or_insert_mut_ref`.
  - Also relevant: 0.17 moves to hashbrown 0.17 with MSRV 1.85, and 0.18.2 fixes a panic-safety unsoundness in `pop`.
  - We call none of those.
- Our calls are `LruCache::new(NonZeroUsize)`, `get`, `put` and `iter()` [our-code `crates/openalpaca_core/src/compose/mod.rs:224-226, 262, 277-285, 300, 334, 368`]. Their signatures are identical [dep-source: `new` at `lib.rs:222` vs `:231`, `put` at `:317` vs `:342`, `get` at `:435` vs `:460`]. **Compile errors: 0** [measured, D1].
- The miss-attribution scan relies on `iter()` being MRU-first (comment at `compose/mod.rs:273-275`). It still is: `iter` walks from `head.next`, documented "most-recently used order" [dep-source `REG/lru-0.18.5/src/lib.rs:1717-1742`]. The scratch crate measured the same order and eviction on 1.93.0 (D2). Core's `compose` tests pass [measured, E4].
- The lock drops `hashbrown 0.15.5`, `foldhash 0.1.5` and `allocator-api2`.

## 6. Risks: persisted data, secrets, wire formats, user-visible behaviour

- **Persisted data (`state/openalpaca.db`, not disposable).** Readable and writable by both SQLite versions in both directions, including an un-checkpointed WAL written by the other version [measured, §3.5]. No migration, no schema change, and `schema_version` stays 45. The schema-42 refusal is intact [measured]. The upgrade removes three known 3.51.1 defects (WAL-reset corruption, the lock-detection deadlock, EXISTS-to-JOIN wrong results) [docs]. The REAL → TEXT rendering change reaches nothing we store [our-code + measured, §3.4]. The standing advice in CLAUDE.md to copy `state/openalpaca.db` before the first start of a new build still applies. It exists for migrations 044/045, not for this bump.
- **Memory index (`state/cache/memory-index-v1.sqlite`, disposable).** Same compatibility both ways [measured]. No `SCHEMA_VERSION` bump is needed: tables and tokenizers are unchanged.
- **Memory notes, snapshots, `.history/`.** Plain files, not touched by any of these crates.
- **Secrets.** None of the three crates handles secrets. `llm.toml`, the keychain and `.master_key` are not in SQLite. No change.
- **Wire formats.** No HTTP/WS/SSE payload is produced by SQLite text conversion: every REAL goes out through serde_json's Rust formatting of an f64 (§3.4). No change.
- **User-visible.**
  - A rusqlite error on invalid UTF-8 text now reads "… at index N", as `Utf8Error` rather than `FromSqlConversionFailure`. It is only seen in logs, e.g. the iMessage reader's skip warning.
  - `sysinfo` name matching is unchanged on macOS [measured] and on Linux [dep-source].
  - A mis-read of liveness would only mis-word `daemon stop`/`restart`. The factory reset decides by the singleton lock, not by the pid check (CLAUDE.md, *whole-branch review*).
- **Build.** sysinfo 0.39 makes Rust ≥ 1.95 mandatory for every crate that links `openalpaca_storage`: the daemon, the CLI and the **Tauri GUI**, which CI does not compile.

## 7. Proposed commits, in order

The three are independent in code and each touches one manifest line. Keep them as three commits for bisectability. The lockfile hunks of commit 2 are textually on top of commit 1 (`foldhash 0.2.0` → `foldhash`), so apply them in this order or let cargo regenerate the lock. The stacked patches `SP/patches/storage-{1-rusqlite,2-lru,3-sysinfo}.diff` apply in sequence on HEAD's `Cargo.toml`/`Cargo.lock` and reproduce `SP/patches/storage.diff` byte for byte [measured, `patch -p1` in `SP/storage-applycheck`].

1. **`chore(deps): rusqlite 0.38 → 0.40.2 (bundled SQLite 3.51.1 → 3.53.2)`**. Diff: `SP/patches/storage-1-rusqlite.diff`. Contents: `rusqlite = { version = "0.40.2", features = ["bundled"] }`; the lock updates `rusqlite`, `libsqlite3-sys` and `hashlink`. No toolchain prerequisite: it builds on 1.93.0 (B3), so it can land before or after the toolchain commit. The commit body should name the SQLite behaviour change (17-digit REAL → TEXT), the WAL-reset fix, and the schema-42 re-test. Verification:
   - `cargo check --workspace --all-targets --exclude openalpaca_gui`
   - `cargo test -p openalpaca_storage`: 678 passed, which includes the schema42 refusal, the INDEXED BY/query-plan tests and the memory fixtures.
   - `cargo test -p openalpaca_core memory`: 335 passed.
   - `cargo test -p openalpaca_connectors --features imessage`: 70 passed.
   - `cargo check -p openalpaca_gui` by hand.
   - Optionally the cross-version probe (`SP/probe`, §3.5).
2. **`chore(deps): lru 0.12 → 0.18`**. Diff: `SP/patches/storage-2-lru.diff`. Contents: `lru = "0.18"`; the lock updates `lru` and drops `hashbrown 0.15.5`, `foldhash 0.1.5` and `allocator-api2`. No toolchain prerequisite: lru 0.18.5 declares 1.85 and was built on 1.93.0 (D2). Verification: `cargo check -p openalpaca_core --all-targets` and `cargo test -p openalpaca_core --lib compose` (82 passed).
3. **`chore(deps): sysinfo 0.38 → 0.39.6`**. Diff: `SP/patches/storage-3-sysinfo.diff`. Contents: `sysinfo = "0.39.6"`; the lock updates `sysinfo` and adds `objc2-open-directory`. **Depends on the toolchain commit**: `rust-toolchain.toml` must be ≥ 1.95 (planned 1.98.1), and the workspace `rust-version` should be raised to match so the MSRV-aware resolver stops describing a 1.93 workspace. Verification:
   - `cargo check -p openalpaca_storage -p openalpaca --all-targets`
   - `cargo test -p openalpaca_storage daemon_lifecycle`: 9 passed on macOS, and CI's ubuntu job covers the Linux zombie path.
   - `cargo check -p openalpaca_gui` by hand.

## 8. Open questions

- Linux behaviour of sysinfo 0.39's zombie/name detection is established from source only. CI's ubuntu run of `a_real_unreaped_child_is_not_a_live_process` is the measurement.
- Not run: `cargo check -p openalpaca_gui`, `cargo clippy`, the CLI crate's tests, and the 100K benches.
- Whether any of our `EXISTS` queries returned wrong rows under 3.51.1's EXISTS-to-JOIN bugs was not established. The bump fixes it either way.
- Policy, for the owner: should migrations be restricted to SQL that the oldest still-supported build's SQLite (3.51.1 for pre-bump builds) parses? §3.6 shows the clean refusal survives the new ALTER TABLE forms, but a 3.53-only function in an index breaks writes in an older build.
- Optional and not forced: `sysinfo = { version = "0.39.6", default-features = false, features = ["system"] }` would drop the `user`/`disk`/`network`/`component` code and `objc2-open-directory`. It is a separate change and needs its own check that `system` alone still provides the process API.
