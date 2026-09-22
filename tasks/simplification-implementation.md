# Crate simplification implementation — 2026-09-21

Base: `97a42a39bd59de6f00d1b51f6fd1150ab1fe08c2`, branch
`chore/deps-upgrade-and-simplify`. The owner authorized implementation of the
crate-by-crate review and corresponding Obsidian updates. The app has never
been distributed. Changes are local and uncommitted.

## Review disposition

- [x] **Storage S1–S5:** removed unused Agent/Memory public APIs; simplified
  optional query binding, shared conversation inserts, LIKE/timestamp helpers;
  retired old app-root/asset-path/upload import code. Current project-store
  moves, permissions, ownership, artifact/session recovery and real disk tests
  remain. Pure repository fixtures now explicitly use an in-memory database.
- [x] **Core C01–C10:** safe Unicode byte previews; shared persona/body/JSON
  parsing; shared usage persistence/events with caller-specific pricing;
  shared skill tool resolution/registration with security policies preserved;
  common recipient checks including Discord; owned per-turn metadata instead
  of three global maps; typed internal spawn calls preserving partial batches.
  Removed the unused cascade wrapper, retaining real repository cascade search.
- [x] **LLM L1–L3:** one configured-key resolver and pool strategy; shared
  provider construction with startup/runtime defaults and error policies kept;
  typed pending Anthropic completion event instead of synthetic SSE.
- [x] **Plugins P1–P3:** shared feature-independent OpenAI response codec and
  TOML conversion; pending RPC registrations now clean up when a call future is
  cancelled, including during writer backpressure. Dormant provider/connector
  integration is still dormant.
- [x] **Connectors C1–C4:** shared confirmation, Unicode-safe chunking and
  keyed rate-limit helpers; synchronous Telegram start; dispatcher-owned
  confirmation listener with eager subscription and shared running guard.
- [x] **Apps D01–D03 / CLI C01–C04 / GUI G01:** required services in AppState,
  shared persona reload and optional-LLM lookup, shared CLI truncation and agent
  requests, validated daemon-config batch saves, one config-list resolution
  policy, shared Tauri command builder, owned temporary test directories.
- [x] **API A1 / Wake W1 / MCP M1–M2 / PL1–PL2:** removed unused direct
  dependencies and two platform scaffolds; callback-owned debounce state;
  common MCP retry policy and removal of unimplemented resource/prompt methods.
- [x] Final validation and documentation application.

The 39 primary opportunities in the review are implemented. Small optional
ideas that would add policy or feature scope were left alone: MCP lifecycle
`next_at` diagnostics, typed plugin-agent cross-crate protocol changes, broader
connector trait redesign, plugin-provider registration and GUI onboarding or
daemon ownership. No repository table/column was removed as part of API cleanup.

## Storage compatibility

The owner subsequently requested clean numbering: `001_baseline.sql` now starts
at version **1**, with future migrations **2, 3, ...**. The runner creates
`schema_migrations` and records versions in the same transaction as each SQL
migration; SQL files contain no version inserts. Applied history must exactly
match a prefix of the registry. Untracked nonempty databases, empty or invalid
ledgers and versions beyond the current build are refused without schema/data
or journal-mode changes. The old development `schema_version` ledger is not
accepted, including old versions 1 and 42. A fresh version-1 database reopens
normally; future migrations append to it.

Only the current home/project disk layout is supported. Old development
application-data roots and content-addressed upload layouts are no longer
imported automatically. The owner also authorized removing the existing database. No database was
found in the default home store, repository or old standard app-data locations,
and no daemon was running; no store files have been removed. Existing `store rebase` and current-store recovery stay active.

## Rust line counts

Physical Rust source lines, including comments and blank lines, measured with
the same `syn` AST counter before and after. Inline `#[cfg(test)]`, dedicated
test files, test-only helpers and `test-utils` items count as **testing**.
Examples are separate. SQL, manifests, TypeScript and docs are outside this
Rust comparison. The earlier 42-script squash is already in the starting
commit, so its savings are not counted again.

| Crate | Production before | Production after | Change | Tests before | Tests after | Change |
|---|---:|---:|---:|---:|---:|---:|
| `openalpaca_api` | 568 | 568 | +0 | 336 | 336 | +0 |
| `openalpaca_core` | 49,875 | 49,448 | -427 | 52,194 | 52,669 | +475 |
| `openalpaca_llm` | 11,495 | 11,285 | -210 | 9,049 | 9,316 | +267 |
| `openalpaca_mcp` | 1,278 | 1,239 | -39 | 1,191 | 1,316 | +125 |
| `openalpaca_storage` | 15,271 | 14,031 | -1,240 | 14,173 | 13,182 | -991 |
| `openalpaca_wake` | 486 | 483 | -3 | 374 | 374 | +0 |
| `openalpaca_platform` | 4 | 0 | -4 | 10 | 0 | -10 |
| `openalpaca_platform_macos` | 4 | 0 | -4 | 10 | 0 | -10 |
| `openalpaca_connectors` | 3,410 | 3,276 | -134 | 1,487 | 1,522 | +35 |
| `openalpaca_plugins` | 6,086 | 6,038 | -48 | 4,210 | 4,321 | +111 |
| `openalpacad` | 22,849 | 22,459 | -390 | 17,920 | 18,189 | +269 |
| `openalpaca` | 11,999 | 11,978 | -21 | 3,798 | 4,149 | +351 |
| `openalpaca_gui` | 187 | 184 | -3 | 0 | 23 | +23 |
| **Total** | **123,512** | **120,989** | **-2,523** | **104,752** | **105,397** | **+645** |

Examples remain 74 lines. Two removed scaffolds account for only 8 production
and 20 test lines. Deleted tests cover retired layout/API paths or duplicate
algorithms; new regressions cover preserved behavior and cancellation/lifecycle
boundaries. Test-code reductions are not counted as production-code savings.

## Initial simplification validation (before version-1 follow-up)

| Gate | Result |
|---|---|
| `cargo test --offline --workspace --exclude openalpaca_gui --no-fail-fast` | 3,463 passed; 0 failed; 5 existing ignored tests |
| `cargo test --offline -p openalpaca_gui --lib` | 1 passed |
| `cargo test --offline -p openalpaca_llm --lib` (default features) | 240 passed |
| `cargo test --offline -p openalpacad --bin openalpacad routes::settings::` (after final helper cleanup) | 24 passed |
| `cargo test --offline -p openalpaca_connectors --lib common::delivery::tests` (default features, after final assertion) | 2 passed |
| `cargo clippy --offline --workspace --exclude openalpaca_gui --all-targets` | Completed successfully; existing repository warnings remain |
| `cargo check --offline -p openalpaca_plugins -p openalpaca_connectors --all-targets` (default features) | Passed; feature-disabled connector imports still warn |
| GUI `bun run check` / `bun run test` / `bun run format:check` / `bun run build` | All passed; 82 test files / 973 tests |
| `python3 scripts/gen_api_docs.py --check` / `git diff --check` | Passed |

The workspace test count does not include the additional focused reruns above.
The full suite preceded the final lint/helper cleanup; affected settings and
connector tests were rerun, and the final Rust sources passed workspace clippy
and default-feature compilation (the last edit adds only a test assertion).
Tests used isolated stores and local mock transports. Live provider, connector,
and packaged-GUI acceptance were not exercised. Validation logs are retained
under `/private/tmp/openalpaca-simplify-*.log` for this session.

Independent reviews compared
current source with HEAD for storage/move safeguards, MCP policy, metadata and
attachment ownership, skill security, spawn partial failure, daemon wiring and
provider/key precedence. The provider factory's first draft changed runtime
precedence; that was corrected and pinned by a mock-server regression before
completion. No review finding remains open in the implemented scope.

## Clean version-1 follow-up

The baseline reset additionally moves version bookkeeping from SQL into the
runner, validates the complete applied history, and defers persistent WAL mode
until after schema acceptance. Regressions cover old version-1/41/42 refusal,
newer/empty/gapped histories, version 2 applying once, and rollback of schema,
data and ledger on failure. The full-suite rerun exposed seven old fixture helpers
that discarded their `TempDir` too early: four pure repository helpers now use
the shared in-memory database, while connector attachment, daemon notification and core KB tests
retain their real temporary directories and disk assertions.

Follow-up validation:

- Storage: **465 passed**, including **24 database tests** for creation, reopening,
  rejection, future migration 2, rollback and final-schema behavior.
- Workspace rerun: 3,457 passed, 12 fixture failures and 5 ignored. After the
  fixture corrections, both affected library suites passed in full (**core
  1,656**, **connectors 94**), and the daemon notification suite passed
  (**24 tests**, including all 4 failed cases). All failures from the workspace
  run are resolved; across that run and the targeted reruns, all **3,469**
  non-ignored workspace tests passed. The original failed run remains in the
  log rather than being relabelled as a successful single invocation.
- Daemon status endpoint: **16 passed**, including the database-version response.
- Storage `cargo clippy --offline -p openalpaca_storage --all-targets`: completed
  with existing warnings. `cargo check --offline -p openalpaca_storage`: passed.
- API generator tests: **17 passed**; generated API `--check` and diff whitespace
  checks passed.

Logs: `/private/tmp/openalpaca-baseline1-{storage-tests,workspace-tests,fixture-rerun,notification-tests,status-tests,clippy,api-tests}.log`.
The core/connector rerun used `cargo test --offline -p openalpaca_core -p
openalpaca_connectors --lib --features openalpaca_connectors/telegram,openalpaca_connectors/discord,openalpaca_connectors/imessage,openalpaca_core/live-eval`;
the daemon rerun used `cargo test --offline -p openalpacad --bin openalpacad
notification::tests::`. Live external services and the owner's database were
not used by the tests.

## Corresponding documentation

Repository README/CLAUDE, installation and daemon manuals, and generated API
references describe the current crates, store opening and retired imports.
All 13 project notes under `/Users/tangyunpei/Valuts/Main/Projects/OpenAlpaca`
were updated and checked against the prepared copies, including ADR-040,
baseline/future migration rules, runtime ownership, shared helpers and
production/test counting. The original notes were backed up to
`/private/tmp/openalpaca-obsidian-backup-2026-09-21` before application.
The baseline-1 follow-up updated 11 of those notes, backed up separately under
`/private/tmp/openalpaca-baseline1-vault-stage/before`, including the amended
ADR-040 and migration-2 instructions.

## Separate existing issue observed

`crates/openalpaca_storage/src/repository/memory/search.rs::find_similar_fts_fallback`
joins terms with `AND`, then passes the string to `search_fts`, which quotes
all terms, including `AND`. This can prevent the intended similarity match
unless the stored text also contains that word. The Unicode supersession
regression uses fixed embeddings so it tests the changed preview code rather
than depending on this existing fallback. Correcting FTS query composition is
a separate behavior fix; it was not silently folded into this refactor.
