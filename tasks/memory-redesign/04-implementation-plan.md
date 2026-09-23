# Implementation, removal and evaluation plan

**Status: rev 3 (2026-09-23): the owner has answered MD-1…MD-9 (design §0); nothing is implemented.** Rev 3 applies those answers to rev 2, which applied the [review](05-review.md). Two answers reshape the plan:
- **MD-6, refuse and reset:** existing schema-42 databases are **refused**, not converted, the way 1–41 already are. The conversion machinery is gone.
- **MD-9, everything at once:** the GUI ships **with** the cutover.

The owner also instructed that **the old memory system be removed completely**: the `memory`/`memory_fts`/`memory_vec float[768]` design and everything built on it, with **no compatibility code**, because the app was never released.

Source: `7561f152` (reviewed at `0d490847`). The app is unreleased, so breaking internal APIs and deleting the old engine is allowed. The owner has chosen, knowingly, that moving to the new build means a factory reset of their current store (§4).

## 1. Delivery strategy

- **Shippable slices, one PR each**, on a new branch cut from `main` **after draft PR #33 merges**. Do not stack this work on #33. Every slice lands with all CI gates green, and the app works at every slice boundary.
- **No dual backend.** The old engine serves memory until the cutover slice (3). Slice 3 switches every consumer, the GUI included (MD-9), and deletes the old engine in the same PR. There is no selectable legacy mode and no dual-write.
- **Characterization tests first.** Each behavioural change is preceded by a test written against the unchanged code, and shown to fail with the change reverted.
- **Measure, don't assert.** Speed and accuracy claims come only from slice 4's report.

| Slice | What lands | Depends on | Relative size |
|---|---|---|---|
| **0** | Stop data loss in the current engine; config-reset bug; newer-schema guard | MD-8 ✓ | S (5 small commits) |
| **1** | Contracts, a minimal real schema-42 fixture, invariant fixtures, evaluation set and baseline B | design approved | M |
| **2** | `memory_files` + `memory_index` storage library, no consumers | 1 | L |
| **3** | Cutover: service, tools, recall, extraction, `USER.md` editor, **baseline re-squash to 43 with 42 refused**, old-engine and dependency removal, API, CLI **and GUI**, docs | 2 | XL |
| **4** | Evaluation report, packaged and offline verification, owner switch-over notes | 3 | M |

## 2. Slices and acceptance evidence

### Slice 0: stop data loss in the current engine (MD-8)

The cutover is weeks away, and the owner's live store keeps losing memories meanwhile ([05](05-review.md) finding 2). Because the switch-over resets the store (MD-6), fixes **0.2** and **0.3** protect memories only for that interim period. **0.1**, **0.4** and **0.5** stand on their own. Five small changes follow, each with a characterization test and a negative control. Nothing else in the old engine gets fixed: F2, F4, F5, the per-turn AND query and the double profile injection are all replaced in slice 3.

- [ ] **0.1 `config reset` keeps the owner.** Plain `openalpaca config reset` and the TUI's "Reset All Config" run `DELETE FROM system_config`, which removes `identity.local_user_id` and orphans every row ([06c](06c-evidence-identity-scope-learning.md) A4: `apps/openalpaca/src/commands/config_handlers.rs:466-476`, `config_tui.rs:84-96`, `crates/openalpaca_storage/src/repository/config.rs:84-90`). Exclude `identity.*`. Test: the owner id is unchanged after a reset.
- [ ] **0.2 No automatic pruning of durable memories.** Turn the prune step of the decay worker off by default (`apps/openalpacad/src/background.rs:81-135`, `repository/memory/decay.rs:107-131`). Test: an explicit preference last accessed more than 200 days ago survives a maintenance cycle.
- [ ] **0.3 No similarity-based replacement, for any caller.**
  - **Why every caller.** `persist_memory_item` (`memory/task_extraction/mod.rs:389-455`) and task extraction's `find_similar_for_supersession` (`repository/memory/search.rs:259-300`) have no source filter ([06a](06a-evidence-audit-verification.md) F7). So supersession by similarity is switched off everywhere, and only exact-hash dedup remains.
  - **The test needs a deterministic stub `Embedder`** returning fixed vectors; none exists ([06b](06b-evidence-embedding-inventory.md) §1a). Without it only the Jaccard fallback runs, which F4 makes almost never match, so the test could not fail on the old code.
  - **Test:** two distinct related preferences whose stub vectors sit within the old threshold both stay active.
- [ ] **0.4 Lead injection with no workspace reads global only** (F1, `runner/lead_agent/mod.rs:425-447`). Test: a workflow started with no workspace receives no workspace-scope memory.
- [ ] **0.5 Refuse a database newer than the build.** Today `run_migrations` refuses only versions 1..41 and returns early when nothing is pending (`crates/openalpaca_storage/src/database/mod.rs:126-151`). A build therefore opens a newer-schema database silently and fails later on a missing table. Refuse any `schema_version` above the newest registered migration, naming the version and the database path. Test: a database stamped 99 is refused and left unchanged. With this in place, a pre-cutover build cannot damage a database created by the new one (§4).

Exit: storage and core tests plus clippy pass; `CLAUDE.md` notes the five changes.

### Slice 1: contracts, fixtures and baseline (no product change)

- [ ] **Freeze the v1 contracts** from design rev 3:
  - the note schema and the control-snapshot schema
  - `<partition-key>` and `MemoryAccessContext`
  - the authority ordering and origins, including a never-service-written file (`external`) and a confirmed candidate (`user`)
  - the derived-supersession rule: same partition, at-least-equal authority, the target's expected revision
  - the tool JSON schemas, the `confirm`/`undo` operations and the capability names
  - the `/v1/memories` shapes
- [ ] **A minimal real schema-42 fixture.** Generate it with the **current** build, while sqlite-vec is still linked, through `Database::open`. It must contain `memory_vec` with a few vectors, plus one task and one chat message. Commit it as `crates/openalpaca_storage/tests/fixtures/schema42.db`.
  - **Git:** `.gitignore:90` ignores `*.db`, so the same commit adds `!crates/openalpaca_storage/tests/fixtures/*.db` (confirm with `git check-ignore`).
  - **Why it's needed:** slice 3's refusal test opens it **with sqlite-vec no longer linked**, to prove the refusal works and changes nothing. The spike shows it will ([06d](06d-evidence-vec0-spike.md) Q1: only statements naming `memory_vec` fail). The file cannot be produced once sqlite-vec is gone.
- [ ] **Invariant fixtures.** Note trees, queries and expected results for every row of §5's regression matrix, as data files that slices 2–3 load. No model is involved.
- [ ] **Evaluation set v0.** About 470 **held-out** application questions plus a separate tuning split (§5), with gold notes and a frozen judge protocol.
- [ ] **Baseline B.** The current `memory_search` tool path, hybrid, with the E5 model when it is already present locally; the developer decides any download. Baseline A, the per-turn FTS path, is recorded as a floor. Record machine, build, lockfile hash and effective config. **This must run before slice 3**, which deletes the engine it measures.
- [ ] **Benchmark harness.** A synthetic note generator (10K and 100K notes, 1 KiB average, with large-note and CJK mixes) and a timing harness.

Exit: the fixtures are committed, and the baseline numbers are recorded with their conditions. No research-pass number may appear as a measurement.

### Slice 2: the storage library (no consumers)

**`openalpaca_storage::memory_files`**
- [ ] Parse and serialize notes, preserving unknown fields and untouched Markdown. Validate UUIDs. Owner key = the first 32 hex characters of `sha256(owner_id)`, with the mapping kept in the snapshot.
- [ ] Confinement: no-follow opens, and rejection of symlinks, junctions, `..`, hidden and control files, and hardlink aliases where enforceable. Build a hardened atomic write on `store::write_atomic` (`store/mod.rs:754`): file `fsync`, then rename, then directory `fsync` where supported.
- [ ] Keep `.history`, bounded to the last 5 **service-written** revisions, the current one included. Write the project partition's `.gitignore` (`*`) (MD-7).
- [ ] The control snapshot holds partition binding, `pending`, per-note revision and origin, and tombstones with their `supersedes` lists, origin rank and suppressed sources. Replace it atomically and keep it bounded.
- [ ] Implement design §6's recovery table and orphan detection.

**`openalpaca_storage::memory_index`**
- [ ] A cache DB at `state/cache/memory-index-v1.sqlite`, holding catalog, chunks, FTS5 `unicode61` and an FTS5 trigram companion.
- [ ] A heading- and paragraph-aware chunker with line spans, byte-bounded and cut on char boundaries.
- [ ] A **literal query builder**: AST to escaped terms, a union of terms with BM25, plus exact identifier and alias matches.
- [ ] A bounded short-CJK substring pass that sets `partial`.
- [ ] Derived-supersession (same partition, at-least-equal authority), tombstone, `source_forgotten`, status, kind and validity filters, applied **before** the limit.
- [ ] A reconciliation stat scan, a full rebuild, and `index_state` reporting.

Tests, written with the code, no model, no network:
- round-trip parsing, including unknown fields and malformed files kept but excluded
- every confinement case
- **every recovery-table row with a crash injected at each step**
- tombstones surviving index deletion and pending-write recovery; a lost snapshot quarantines the partition
- derived supersession: a forgotten or archived replacement does not revive its predecessor; a lower-authority link is reported, not honored; a cross-partition link is ignored
- the query fixtures: "what editor do I prefer?" recalls "User prefers vim for editing"; F4's literal-`AND` case; CJK queries of 1, 2 and 3+ characters
- filters applied before the limit
- rebuild equivalence
- stat-scan detection of an external edit or deletion

Bench: 10K notes, warm p95 at or under 50 ms including file verification (§5).

Exit: the library passes its tests and the bench. Nothing calls it yet. No new crate and no new feature: `uuid`, `sha2` and `rusqlite` FTS5 are already there, and `notify` is not needed.

### Slice 3: cutover (one PR, reviewable commits, every consumer and surface at once)

| # | Commit | Key files | Tests first / evidence |
|---|---|---|---|
| 3.1 | `MemoryService` + `MemoryAccessContext`: the MD-5 marker-root partition rule with lazy store creation, kind-based default scope, authority, quiesce | `crates/openalpaca_core/src/memory/service/` (new) | The scope matrix from slice 1: no marker means global; the daemon's CWD never selects a project; a git repo gets its partition before and after its first artifact; unknown identity fields are refused |
| 3.2 | Tools (`memory_search/read/store/update/forget/list`, with `update` actions `edit`/`archive`/`supersede`/`confirm`), strict input validation, capability mapping, agent templates, rewritten tool descriptions | `tools/builtins/memory_*.rs`, `main_loop.rs`, `tools/registry/`, security classification, `config/agents/*.md` | A spoofed `workspace_id` or `owner_id` is refused (F2); forget by ID plus revision only; supersede checks the target's revision; `memory_search.rs:140` and `memory_ops.rs:82` no longer promise semantic search |
| 3.3 | Recall: `prompt_ctx::MemorySource` and the lead injection go through the service. One profile injection, no query-text cache. | `prompt_ctx/sources/memory.rs`, `manager.rs`, `runner/lead_agent/mod.rs`, `user_profile.rs` | The per-turn natural-question fixture now recalls; the lead gets no cross-partition memory (F1); the profile appears once |
| 3.4 | Extraction per MD-2, bounded by count and cost, cancellable, attributed. The `USER.md` writer stops writing learned facts (MD-3). `openalpaca persona edit user`. Onboarding completion no longer needs the background writer. | `orchestrator/extraction.rs`, `memory/task_extraction/`, `dispatcher/memory.rs`, `orchestrator/bootstrap.rs`, CLI `commands/` | A user-stated preference becomes an active `extracted_user` note; an assistant claim becomes a candidate; a failed task produces nothing; bootstrap completes via `update_persona` with no background writer |
| 3.5 | **Schema: re-squash the baseline to 43 and refuse 42** (MD-6, §4) | `migrations/001_baseline.sql`, `migrations/mod.rs`, `database/mod.rs` (the refusal text) | The slice-1 `schema42.db`, opened **with sqlite-vec no longer linked**, is refused with the new text, and its bytes are unchanged. A fresh database opens at 43 with no memory tables. The existing refusal test (`database/tests.rs:59`) now covers 1–42. The fresh-version assertions (`:11`, `:34`, `:50`, `:930`) read the new `BASELINE_VERSION`. |
| 3.6 | **Delete the old engine and the embedding stack completely** (§3 map). **No compatibility code**: no leftover-key warnings (the factory reset that MD-6 requires deletes `llm.toml` and restores `daemon.toml`) and no legacy-cache cleanup command. | see §3 | `cargo tree -p openalpacad -i <crate>` is empty for `fastembed`, `ort`, `ort-sys`, `tokenizers`, `hf-hub`, `sqlite-vec`; the **completeness search** (§3) finds nothing; the package count and CLI binary size delta are recorded |
| 3.7 | Workspace move quiesce; purge and listing read file partitions; the factory-reset preview; the home README text and the `superseded_readmes/home-6.txt` digest | `routes/workspaces.rs`, `store/project_move.rs`, `artifacts/mod.rs` (the `rebase_project` memory SQL goes), `config_factory_reset.rs`, `store/mod.rs` | A move while a memory write is pending is refused; purge keeps memory; the reset preview names memory files as surviving (the memory directory is content, not `state/`) |
| 3.8 | `/v1/memories` routes (list, search, read, create, update, forget, `recently-learned`, management `sync`) and `openalpaca memory list/search/show/add/edit/confirm/undo/forget/sync/doctor/adopt` | `apps/openalpacad/src/routes/`, `router.rs`, `apps/openalpaca/src/commands/`, `openalpaca_api` | Route tests, including 409 on a revision conflict; the CLI round-trip in an isolated store; `docs/api/` regenerated |
| 3.9 | **GUI (MD-9)**: the Memory view (scope selector, search, source evidence, raw Markdown and diff, candidates with confirm, *recently learned* with undo, revision conflicts, forget); "notes used in this answer" on assistant turns; Settings → Persona `USER.md` editor; safe rendering of external Markdown | `apps/openalpaca-gui/src/views/`, `lib/api/`, types | GUI unit tests; all four `bun run` gates; a live GUI round-trip on an isolated store |
| 3.10 | Docs: every manual from [06b](06b-evidence-embedding-inventory.md) (`README.md:52,140,154,170`, `Installation_Manual.md:233-238,420-427`, `QuickStart_Manual.md:55-59`, `Daemon_Manual.md:76,78-83,910`, `CLI_Manual.md:179`, `Skill_Template_Reference.md:994`, `docs/tools/TECHNICAL.md`), `DESIGN_SPEC.md:1609`, the `first-run.ts:21` comment, `GUI_Manual.md` (the Memory view), `CLAUDE.md` (Storage, Data directory, Feature Flags, Code Navigation "Memory"), plan §0 decisions taken, and an Obsidian ADR that supersedes vault note 05's memory section and extends ADR-031. Also the **switch-over note** (§4). | docs | `python3 scripts/gen_api_docs.py --check` passes |

Gates, in full (§6): the workspace tests, clippy, `cargo check -p openalpaca_gui` and the four `bun run` gates. Plus a **fresh isolated-store daemon start with outbound networking denied**: `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in a scratch directory, CWD outside the repo. It must show no model download, a working memory round-trip through the CLI, and the same through the GUI.

Exit: the main chat, lead workflow, worker, connector and scheduled-skill smoke tests all use the service, and no `MemoryRepository`, `Embedder`, `fastembed` or `sqlite-vec` remains in the shipping build.

### Slice 4: quality gate and release verification

- [ ] Run the matched evaluation (§5) and write the report, with no claim beyond the measurements.
- [ ] Packaged daemon and Tauri flow on an empty store with outbound networking blocked.
- [ ] Inspect the bundle for any model, tokenizer or ONNX artifact.
- [ ] Hand the owner the switch-over note (§4) before they first start a slice-3 build on their real store. This work never reads `~/.openalpaca`.

Exit: the hard invariants pass; measured quality and latency are documented. If lexical recall misses the gate, improve aliases, ranking or the bounded follow-up search; do not quietly restore a required embedding.

## 3. Removal and integration map

Re-run symbol searches at implementation time. Do not delete every file whose name contains "memory".

| Current area | Action |
|---|---|
| `crates/openalpaca_storage/src/repository/memory/`, `models/memory.rs` | Delete. File-note DTOs with UUID ids replace them. |
| `crates/openalpaca_storage/src/migrations/001_baseline.sql`, `migrations/mod.rs`, `database/mod.rs` | Remove the memory block (`001_baseline.sql:274-341`) and re-squash the baseline to version 43; remove the sqlite-vec `auto_extension` registration; update the refusal text (§4) |
| Root, storage, LLM and daemon `Cargo.toml`; `Cargo.lock` | Remove `sqlite-vec`, `fastembed`, the `local-embeddings` feature and its daemon enablement; let the lockfile re-resolve (97 packages leave, per [06b](06b-evidence-embedding-inventory.md) §5) |
| `crates/openalpaca_llm/src/embedder.rs`, `lib.rs`, `config/llm_config/mod.rs`, `config/llm_config/router_config.rs` (`[embeddings]`, `EmbeddingsConfig`), `config/llm_config/runtime.rs:61-71` (`openai_embeddings_url` in `EndpointsConfig`), `config/settings_types.rs`, `scripts/release/templates/config/llm.toml:46-50` | Delete, with no leftover-key warning (see 3.6) |
| `apps/openalpacad/src/services/llm.rs`, `services/mod.rs`, `services/tools.rs`, `state.rs` (`AppState.embedder`, never read), `main.rs` | Construct the memory service; keep the credential `TokenManager` |
| `apps/openalpacad/src/background.rs` | Delete the embedding indexer and the decay/prune worker; add the reconciliation timer |
| `crates/openalpaca_core/src/memory/task_extraction/`, `orchestrator/extraction.rs`, `dispatcher/memory.rs` | Rewrite to the MD-2 policy through the service |
| `crates/openalpaca_core/src/memory/kb.rs`, `chunker/`, `preferences.rs` | **Delete.** The KB importer has no production caller, and `PreferenceBackend` has no implementors. |
| `orchestrator/memory_ops/`, `tools/builtins/memory_ops.rs`, `memory_search.rs`, `main_loop.rs` | Typed scope, service APIs, ID-plus-revision mutations; drop the `--workspace` content parsing and best-match deletion |
| `prompt_ctx/sources/memory.rs`, `user_profile.rs`, `section.rs`, `package.rs`, `manager.rs`, `runner/lead_agent/mod.rs` | One recall path, UUID/revision source keys, one profile injection |
| `memory/scope_context.rs`, `memory/workspace.rs`, `tools/registry/`, security classification, `config/agents/` | The MD-5 partition rule; strict memory-tool inputs; `memory_read`/`write`/`forget`/`manage` grants |
| `daemon_config/orchestrator.rs` (decay knobs, `supersession_distance_threshold`), `daemon_config/server.rs` (`EmbeddingIndexerConfig`, `[server.embedding_indexer]`), `config_schema/registry.rs:391-404,750-775,821-862` | Delete, with no removed-key warning (see 3.6) |
| `apps/openalpaca/src/commands/ai_config.rs`, `ai_config_helpers.rs`, `daemon_config_cli/mod.rs:139-144,277-287`, `config_factory_reset.rs:203`, `config_handlers.rs`, `config_tui.rs` | Remove the embedding options; fix config reset (slice 0); add `memory …` and `persona edit user` |
| `apps/openalpacad/src/routes/workspaces.rs`, `crates/openalpaca_storage/src/artifacts/mod.rs:1502-1552`, `store/project_move.rs` | Stop counting and re-keying SQL memories; add move quiesce; keep the purge-keeps-memory contract |
| `crates/openalpaca_storage/src/store/mod.rs` (`embedding_cache_dir`, `CACHE_DIR`, README texts) | Delete `embedding_cache_dir`. **Keep `CACHE_DIR`**, which now holds `memory-index-v1.sqlite`, and repoint its README row. New README text plus the `superseded_readmes/home-6.txt` digest. No code touches an existing `state/cache/fastembed`. The switch-over note tells the owner it can be deleted by hand, because the factory reset leaves `state/cache` alone (D-B). |
| `apps/openalpaca-gui/src/lib/api/`, views, types; `openalpaca_api` | New memory API, Memory view and persona editor (3.9) |
| `README.md`, `docs/*.md`, `docs/api/`, `DESIGN_SPEC.md`, `CLAUDE.md`, the Obsidian vault | See 3.10 |

**Completeness search** (a 3.6 gate). This must find nothing in `crates/`, `apps/`, `scripts/`, `config/` and `docs/` outside `docs/api/` history notes:

```sh
rg -n -i 'memory_vec|memory_fts|vec0|sqlite.vec|fastembed|Embedder|embedding_indexer|EmbeddingsConfig|EmbeddingIndexerConfig|openai_embeddings_url|MemoryRepository|MemoryKind|MemoryScope|KbChunk|kb_chunk|768'
```

**Keep-list** (unrelated matches that must survive):
- the 32768 size limits (`attachment_adapt.rs`, `task_state/workspace.rs`, `security/gate.rs`, `security/sanitizer/mod.rs`, `tools/builtins/mod.rs`, `read_result.rs`)
- the AppleScript "embedding" comment (`imessage/sender.rs:145`)
- Ollama's embedding-only-model skip in discovery (`providers/ollama.rs:117,233`), which keeps non-chat models out of the catalogue and has nothing to do with memory

Preserve:
- SQLite itself, and the conversation, task and identity schema
- credential and OAuth token handling
- request token estimates, active-context compaction and session logs
- artifact and upload storage
- persona editors
- existing trust and confirmation boundaries

## 4. Data transition: refuse and reset (MD-6)

The owner chose not to convert existing data. A schema-42 database is **refused**, exactly as 1–41 are today: before any migration runs, and with its schema and rows left intact.

**Schema change (3.5).**
- Remove the memory block (`memory`, `memory_fts`, `memory_vec`, triggers `memory_ai`/`memory_ad`/`memory_au`: `001_baseline.sql:274-341`) from the baseline.
- Change its final `INSERT INTO schema_version` to 43, and register it as `Migration { version: 43, name: "baseline", … }`, so `BASELINE_VERSION` becomes 43 ("oldest openable schema").
- `run_migrations`'s existing guard (`database/mod.rs:126`) then refuses 1–42, and slice 0.5 refuses anything above 43.
- A fresh database gets the new baseline directly and **never needs sqlite-vec**. The spike showed a fresh database cannot even run today's baseline without it ([06d](06d-evidence-vec0-spike.md)).
- There is no conversion code, no stub module, no backup routine and no dry-run command.

**The refusal still works on an old database without sqlite-vec.** It reads only `schema_version`, and the spike showed that only statements naming `memory_vec` fail when the module is absent ([06d](06d-evidence-vec0-spike.md) Q1). The slice-1 fixture proves it in a test.

**Refusal text.** The existing message (`database/mod.rs:126-139`) is reworded for this case. It must name:
- the database path
- the reason: the memory storage was replaced, and this build cannot read the old format
- the verb: `openalpaca config reset --factory`
- **everything** the reset removes, taken from the verb's own preview: every task, message, session record and memory in the database, **`llm.toml` with its API keys and keychain entries**, and `daemon.toml` returned to defaults
- what survives: `artifacts/`, `uploads/` and `sessions/` files as orphans, plus `.master_key`, `logs/` and `backups/`
- that a build from before the memory redesign still opens the database, for anyone who wants to look something up first

**The owner's switch-over note** ships in 3.10, is repeated in the release notes, and is handed over in slice 4:
1. Before starting the new build, optionally keep a copy of `state/openalpaca.db` with its `-wal` and `-shm` files, while no daemon is running. A pre-memory-redesign build can open that copy later for reference.
2. Start the new build. It refuses with the text above.
3. Run `openalpaca config reset --factory` and type the confirmation word.
4. Start again, then re-enter the API keys (Settings → Models & keys, or `openalpaca llm keys add`).
5. Anything worth remembering can be saved again with the new memory tools.
6. Optionally delete `~/.openalpaca/state/cache/fastembed` (about 1 GB). It belonged to the removed embedding model, and neither the reset nor the new build touches it.

**Rollback** for development: stop the new build and run a pre-cutover build against the kept copy. A pre-cutover build **from slice 0 onward** refuses the new build's 43 database (0.5), so pointing the wrong binary at the wrong database cannot corrupt it. Builds from before slice 0 lack that guard, so the switch-over note says never to run one against a new store.

## 5. Evaluation: "faster" and "more accurate" measured separately

### Baselines

| Id | Baseline | Why it matters |
|---|---|---|
| **B** | The current `memory_search` tool path (hybrid, E5 when present locally), measured in slice 1 before the engine is deleted | The **real** baseline; if B cannot run without a new download, it is recorded as unavailable |
| **A** | The current per-turn FTS path | A **floor** only: it requires every word of the message and barely recalls ([05](05-review.md) finding 1) |
| **C** | One bounded `MEMORY.md` | |
| **D** | The new service | |
| **E** | D plus at most two model-directed follow-up searches | |
| — | No memory | Context-cost reference |

Controls: pin the corpus, the reader model and its settings, prompts, the retrieved byte budget, the tool-call cap and the judge. Evaluate retrieval with identical gold-authored notes first, then run end to end with each system's own extraction. Hold out the question, answer and evidence labels from ingestion and tuning. Vendor numbers are context, never a baseline.

### Sample size and the non-inferiority gate

Rev 1 required a 95 % CI lower bound above −2 pp from 300 paired questions. With paired binary outcomes and a plausible 15 % discordant-pair rate, the standard error of the paired difference at n questions is about √(0.15/n).

A system exactly as good as B passes a margin m with probability Φ(m/SE − 1.96), so 80 % power needs SE ≈ m/2.8:
- **−2 pp** needs about **2,940** held-out questions: not feasible to hand-author.
- **−5 pp** needs about **470**.
- At n = 300, an equal system passes −5 pp only about 61 % of the time.

The plan therefore sets:
- **About 470 held-out questions**, stratified by category, **plus a separate tuning split** that never counts toward the gate. The categories:
  - exact recall
  - paraphrase
  - English/Chinese/mixed
  - changed facts and time
  - project procedures
  - rejected or failed actions
  - abstention
  - source conflicts
- **Gate:** the paired-difference 95 % CI lower bound above **−5 pp** against **B**, with the discordant rate reported so that the power can be checked. Plus a per-category inspection: any category whose point estimate is worse than −10 pp blocks the release until explained, with paraphrase and CJK watched most closely.
- **"More accurate" is claimed** only if the paired difference is positive with its uncertainty reported. Otherwise the claim is "simpler and model-free, with comparable measured quality".
- Public suites (cleaned LongMemEval, LoCoMo QA, a LongMemEval-V2 workflow subset) are optional context, reported with exact versions and never gates. Adversarial and lifecycle fixtures sit outside the aggregate score.

### Targets (proposed, not measured)

| Dimension | Gate / measurement |
|---|---|
| Model and network independence | 0 memory-specific downloads, and 0 network or model calls for create, read, search, update, forget or rebuild, with outbound networking denied |
| Scope and authority | 0 forbidden retrievals or writes across the owner, partition, tool-argument and CLI-CWD fixtures |
| Forget and corrections | 0 known deleted or superseded notes returned after restart, cache deletion, extraction retry or pending-write recovery; a forgotten or archived replacement does not revive its predecessor |
| Persistence | No lost acknowledged service writes; revision conflicts surfaced; an old database refused byte-for-byte unchanged |
| Source resolution | 100 % of returned citations resolve to the revision and span read; factual support measured separately |
| Quality | As in the sample-size section above |
| Warm retrieval | p95 at or under 50 ms for 10K notes averaging 1 KiB, top-8 chunks, including file verification and serialization, release mode, on a declared machine |
| Scale stress | 100K notes, large notes, changed or deleted files, an empty or corrupt cache: time, RSS, index size, partial-result rate, and **reconciliation stat-scan duration**. No unbounded work on the async executor. |
| Startup | An empty store adds at most 100 ms p95 of service initialization; populated-cache reconciliation and time to full recall are measured separately |
| Build and footprint | The dependency-package delta (expected −97), build time without the `ort-sys` fetch, release daemon and CLI binary sizes, bundle size, first-boot time with no model download |
| End to end | Query-to-answer p50/p95, tokens, extra tool or model rounds, and extraction cost and count, all reported separately |

### Essential regression matrix

| Scenario | Required outcome |
|---|---|
| Global or connector request, daemon started by hand inside project A | No A memories: the daemon's CWD never selects a partition |
| CLI turn in a directory with no `.openalpaca` or `.git` above it | Global only; no partition is created there |
| CLI turn inside a git repository with no `.openalpaca/` yet | The repository's project partition; the store is created lazily, the same before and after the first artifact |
| Any opener meets a schema-42 database | Refused with the §4 text, before any statement touches memory tables; bytes unchanged; works with sqlite-vec absent |
| The model supplies hidden owner, workspace or path parameters | Refused; the trusted context is the sole authority |
| The same phrase in two owners or projects | No cross-partition dedup, override or disclosure |
| A user correction vs. a model-inferred related fact | The authority order applies; similarity replaces nothing; a lower-authority `supersedes` link is reported, not honored |
| "Used X before June, now Y" | The current answer is Y; a historical query cites X with the right validity bounds |
| A short Chinese term, a mixed code identifier, negation, punctuation | Valid query handling and measured recall; no whole-message AND failure |
| Save and then immediately ask; manual edit or delete; an external new file | Read-after-write holds; stale selected files are suppressed; the new file appears by the next scan, with freshness reported |
| Two agent saves; an edit conflict; a large note; a duplicate UUID | No lost service update; a clear conflict or diagnostic; bounded resources |
| A crash at each step of design §6's recovery table | One consistent state, no duplicate |
| Forget, erase the cache, re-run source extraction; forget a replacement | Suppressed and not revived; notes citing the forgotten one are derived `source_forgotten`; the transcript-retention boundary is stated |
| Workspace move while writing; purge; factory reset; config reset | Move refused, then rebound; purge keeps memory; factory reset orphans but never reassigns; **config reset keeps the owner** |
| Malicious Markdown, frontmatter or source text | Stays reference data: cannot choose an owner, traverse a path, gain authority or authorize a tool |
| No configured LLM, offline | Management and retrieval work; extraction reported unavailable |
| Compaction, lead/worker handoff, a cancelled task | Provenance, trust labels and budgets preserved; failed output never promoted |

## 6. Validation commands

Run gates with `CARGO_TERM_COLOR=never`, full output redirected to a file, and each command's own exit code checked (`tasks/lessons.md`). Run the focused module tests during each slice, then the full set once per slice:

```sh
cargo test --offline -p openalpaca_storage
cargo test --offline -p openalpaca_core
cargo test --workspace --exclude openalpaca_gui
cargo clippy --workspace --exclude openalpaca_gui --all-targets
cargo check -p openalpaca_gui
python3 scripts/gen_api_docs.py --check
```

From `apps/openalpaca-gui`: `bun run check`, `bun run test`, `bun run format:check`, `bun run build`.

After slice 3, run `cargo tree -p openalpacad -e normal -i <crate>` for each of `fastembed`, `sqlite-vec`, `ort`, `ort-sys`, `hf-hub` and `tokenizers`; every one must be absent. Any daemon or CLI run uses `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in a scratch directory with its CWD outside the repository. Tests never reach `localhost:11434`.

## 7. Sequencing, collisions and risks

- **Draft PR #33** merges first; this work is a new branch from `main`.
- **The paused LLM-providers design** (`tasks/llm-simplify/design.md`) edits the same crate. Slice 3 deletes `EmbeddingsConfig` and `[embeddings]` from `openalpaca_llm`'s config types (`router_config.rs`) and the embedder from `lib.rs`, files that the provider-table design also reshapes. Land **memory slice 3 first**, because it removes code the provider work would otherwise have to carry. Regenerate that design's survey maps afterwards.
- **The iMessage owner defect** (the separate task) and the **connector-audience** design (MD-4) are independent of this plan and may land in any order.
- **Owner decisions:** T1–T33 in plan §0 do not collide. MD-1…MD-9 were answered on 2026-09-23 (design §0). They are recorded in `tasks/api-fix-plan.md` §0 as *taken* rows by the commit that applies each one, per that table's convention.

| Risk | Mitigation |
|---|---|
| Lexical recall regresses on paraphrase or CJK | Aliases, the union query, the trigram companion, the bounded follow-up search, and the evaluation gate with per-category blocking |
| The switch-over loses the owner's data | Chosen deliberately (MD-6). Mitigated by the refusal text and the switch-over note: what goes, what survives, keeping a copy first, and that a pre-redesign build can still open that copy. |
| A wrong build meets a wrong database | 0.5 refuses newer schemas and 3.5 refuses 42; both tested; the switch-over note warns against builds from before slice 0 |
| External edits race service writes | Revision checks, downgrade to `external`, every service-written revision kept in `.history`, surfaced conflicts |
| The stat scan is slow at 100K notes | Measured in slice 2's bench; the optional daemon-side watcher is added only if the measurement says so |
| Slice 3 is large, with the GUI in the same PR (MD-9) | Reviewable commits 3.1–3.10 in order, each green; the GUI commit (3.9) depends only on the routes in 3.8 |

## 8. Commit boundaries

- **Slice 0:** five `fix:` commits.
- **Slice 1:** contracts; the fixture DB with its `.gitignore` line; fixtures and evaluation set; baseline report.
- **Slice 2:** `memory_files`; `memory_index`; bench.
- **Slice 3:** commits 3.1–3.10 in order, with the characterization tests before each behavioural commit.
- **Slice 4:** the evaluation report and the packaged verification record.

Each substantial commit ends with a line naming what was verified (`CLAUDE.md` commit style). Nothing in this plan is implemented until the owner approves the written design and this plan.

## Revision log

- **rev 3 (2026-09-23):** applies the owner's answers to MD-1…MD-9.
  - **MD-6 (refuse and reset).** §4 is rewritten: the baseline is re-squashed to 43 without memory tables, and 42 is refused like 1–41, with a refusal text and a switch-over note that name everything the factory reset removes, API keys included. Migration 043's exporter, receipt, backup, stub `vec0` module, `migrate --dry-run` and the `rusqlite` `vtab`/`backup` and `uuid` `v5` features are all removed. Slice 1's fixture shrinks to a minimal real schema-42 file for the refusal test.
  - **MD-9 (everything at once).** The GUI (the former slice 4) joins the cutover as commit 3.9; the former slice 5 becomes slice 4.
  - **MD-8.** The interim-only value of 0.2 and 0.3 is noted.
  - **Full removal (owner instruction).** The old engine is removed completely, with no compatibility code: no leftover-key warnings and no legacy-cache cleanup command. The removal map gains `openai_embeddings_url` and `EmbeddingIndexerConfig`, a completeness search and a keep-list of unrelated matches.
  - The other answers match rev 2's recommendations.
- **rev 2 (2026-09-23):** applies [05-review.md](05-review.md).
  - **Slices.** Rev 1's big-bang P0–P5 becomes shippable slices, each its own PR.
  - **Slice 0 (new).** Stop data loss; fix the config-reset bug; the newer-schema guard.
  - **Tests first.** A commit table with characterization tests first.
  - **Migration 043.** The data transition became migration 043 in the new build, per the [06d](06d-evidence-vec0-spike.md) spike. Superseded by rev 3.
  - **§5.** B is the real baseline and A a floor; about 470 held-out questions at −5 pp. Build and footprint targets added.
  - **§7 (new).** Sequencing: after PR #33, before the LLM-providers work.
  - **Removal map.** The KB importer, `AppState.embedder`, the `rebase_project` SQL, config and CLI rows, and the stale-key WARN.
  - **Consistency review.** The 16 fixes from the consistency review ([05](05-review.md#consistency-review-of-rev-2)).
- **rev 1 (2026-09-23):** initial plan.
