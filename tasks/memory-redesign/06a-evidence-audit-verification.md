> Evidence appendix to [05-review.md](05-review.md), rev 2 review of 2026-09-23. Produced by an independent read-only verification pass at HEAD `0d490847` (the deliverable was written at `7561f152`; only `tasks/lessons.md` changed between them). Kept verbatim as the record; the review states which items change the design.

# Verification of tasks/memory-redesign/01-code-audit.md

Checked against HEAD `0d490847` (audit written at `7561f152`; only `tasks/lessons.md` differs, so line numbers match).
Read-only. Tests re-run: `cargo test --offline -p openalpaca_storage repository::memory` → 18 passed / 488 filtered; `cargo test --offline -p openalpaca_core memory::` → 30 passed / 1635 filtered (outputs in `test-storage.txt`, `test-core.txt` beside this file). FTS5 probe: `fts_probe.py` (Python sqlite3 3.51.0, default unicode61 tokenizer, same external-content shape and the exact `escape_fts5_query` / `find_similar_fts_fallback` query construction).

Path shorthand as in the audit: `repo/` = `crates/openalpaca_storage/src/repository/memory/`, core paths under `crates/openalpaca_core/src/`.

Verdicts: **C** = CONFIRMED, **CWC** = CONFIRMED-WITH-CORRECTION, **R** = REFUTED.

## Current architecture

| # | Claim | Verdict | Evidence | Note |
|---|---|---|---|---|
| A1 | `memory` table fields, kinds fact/preference/kb_chunk, scopes global/workspace/lane; FTS5 external-content with insert/delete/content-update triggers; `memory_vec` vec0 `float[768]` | CWC (line nit) | `001_baseline.sql:274-340`; `models/memory.rs:5-123` | `models/memory.rs` has 123 lines, not 128. Content correct. |
| A2 | Uniqueness is `(owner_id, scope, scope_id, content_hash)`; repo and KB comments still say owner/hash | CWC (lines) | `001_baseline.sql:292`; stale comments at `repo/mod.rs:90-91` and `memory/kb.rs:56` | Audit cites `mod.rs:87-89` (that is `Self { db }`) and `kb.rs:57`. |
| A3 | add; transactional delete/clear of vector+row; supersede inserts successor, old row importance .1, old vector deleted | CWC (lines) | `repo/mod.rs:90-130` (add), `:205-277` (clear/delete/delete_for_owner), `:279-352` (supersede, `.1` at :342, vector delete :347) | Ranges slightly off; behaviour exact. |
| A4 | One SQLite connection behind a mutex; module comment says "connection pooling" | C | `database/mod.rs:3`, `:31-35`, `:183-205` | |
| R1 | whitespace split, quote every term, implicit AND, `ORDER BY rank`; no stop words, rewriting, CJK segmentation, score or span | C | `repo/search.rs:13-24`, `:27-86`; baseline creates `memory_fts` with no `tokenize=` (unicode61) | Probe adds weight: a natural query `what's my favorite editor?` returns 0 rows against "The user's favorite editor is Neovim"; `favorite editor` returns it. CJK substring `中文回答` returns 0 rows against `用户喜欢简洁的中文回答` (unicode61 keeps a CJK run as one token). |
| R2 | vector KNN over the whole vec table, k = 3×limit, post-filtered by owner/scope/id; threshold 1.5; candidate starvation | C | `repo/search.rs:91-151`; `repo/mod.rs:18,22`; threshold passed only by `search_hybrid` at :183 | `search_vec` itself takes `Option`; 1.5 is the hybrid default. |
| R3 | "hybrid" = FTS rows then vec rows, dedup by id, stop at limit; no RRF | C | `repo/search.rs:156-203` | |
| R4 | cascade workspace first then global; Lane not in cascade | C | `repo/search.rs:213-257`; `memory/scope_context.rs:211-217` | Stronger: nothing in production ever writes `MemoryScope::Lane` (grep: only the label in `memory_ops/mod.rs:56`). |
| R5 | central source passes `embedding=None`, limit 10, bytes/4, untrusted user message, relevance = importance; manager sorts by it | C | `prompt_ctx/sources/memory.rs:41-48` (literal `None`), `:59`, `:66-71`; `prompt_ctx/manager.rs:56-60` | "Always passes None" is true by construction (hard-coded). Registered once at `orchestrator/mod.rs:305-322`; used by SimpleQuery (`simple_query_handler.rs:533-545`) and SkillInvocation (`skill/invocation.rs:561-570`), not by the lead. |
| R6 | lead injection: inline embed, 5 results, touch/boost, 300-char cut, untrusted wrap, own budget | C | `runner/lead_agent/mod.rs:414-489` | |
| R7 | `memory_search` embeds inline, cascades, boosts; output lacks provenance/score; description promises score/metadata | C | `tools/builtins/memory_search.rs:70-110`, `:112-126`, `:138-143` | Description also promises "past conversations", which the table does not hold. |
| W1 | explicit save → Preference/Conversation/.9/1.0; description says fact or preference; default global; `--workspace` substring; silent fallback to global with WARN | C | `tools/builtins/memory_ops.rs:48-72`, `:79-82`; `orchestrator/memory_ops/mod.rs:17-76`, `:126-143` | Missed: the explicit save goes through `persist_memory_item`, i.e. similarity supersession (see F7). `text.contains("--workspace")` also mangles content that merely mentions e.g. `--workspace-path`. |
| W2 | trait extraction every N qualifying turns per lane (5), current turn only, 800/400 chars, profile/memory targets, thresholds | C | `orchestrator/extraction.rs:43-62`, `:77-78`, `:80-109`, `:242-245`; default `extract_every_n_turns: 5` at `daemon_config/orchestrator.rs:308` | Counter is in-process (resets on restart). Prompt says memory ≥ 0.5, config threshold is 0.65. |
| W3 | memory extractions forced Global Preference .6/.7, extracted confidence discarded, no evidence ids | C | `orchestrator/extraction.rs:325-356` | |
| W4 | USER.md read/parse/render, fixed `.md.tmp` + rename, in-memory update, version bump; no CAS / writer lock | C | `orchestrator/extraction.rs:358-368`, `:379-405`, `:528-569` | Another USER.md writer exists (`update_persona`, capability `persona_write`) with no shared lock. |
| W5 | `USER.md` injected as a system message by `UserProfileSource` | C | `prompt_ctx/sources/user_profile.rs:25-63` | Missed: the same document is also rendered by the persona layer (`compose/persona.rs:108-111,172-181`) into the static system prompt (`compose/static_prompt.rs:131`), so on SimpleQuery/Skill turns it appears twice — once budgeted, once unbudgeted (`compose/dynamic_context.rs:86-92`, `compose/assembly.rs:39-41`). Source-traced, not run. |
| W6 | task extraction enabled by default, 500/2000 chars, ≤5 items, confidence < .5 dropped, batch embed, workspace-or-global scope, metadata = task_id + source_path | C | `memory/task_extraction/mod.rs:61-139`, `:226-229`, `:239-320`; enable flag checked at `orchestrator/dispatcher/memory.rs:29-32`, default true at `daemon_config/orchestrator.rs:310` | Also: `model: None` (:118) — ignores `[orchestrator.costs] extraction_model`; LLM `importance`/`confidence` are not clamped (:245-263). Module doc still says "DAG, or pipeline" (deleted topologies). |
| W7 | extraction fire-and-forget, errors logged, no queue or drain | C | `orchestrator/dispatcher/memory.rs:13-60`; `orchestrator/handlers.rs:331-395` | |
| W8 | closest Fact/Preference is superseded on similarity alone | C | `memory/task_extraction/mod.rs:401-434`; `repo/search.rs:265-321` | |
| W9 | KB import: recursive `.md`, ~500 chunk, additive, symlink-following walk | CWC | `memory/kb.rs:17-105` | Behaviour true, but **`ingest_directory` has no production caller** — grep finds only its own tests. The `/v1/memory/kb/ingest` route (and `/v1/memory`, `/v1/memory/{id}`, `/reindex`, `/status`) was deleted in `5ec1db1d` (2026-08-30 dead-code sweep). No `kb_chunk` row can be created by a current build; the symlink traversal is unreachable. |
| W10 | chunker mixes byte/char lengths; long sentence exceeds limit; no fences/spans | C | `memory/chunker/mod.rs:35`, `:98-125` | Reachable only from the dead KB import. |
| M1 | local fastembed, default `intfloat/multilingual-e5-base`, ~1 GB comment, sync `.embed()` in async fn | C | `openalpaca_llm/src/embedder.rs:137-216` (default :171, comment :145, sync call :196-199) | No `query:`/`passage:` prefixes are added, which E5 models expect. |
| M2 | `fastembed 4.9.1` pulls hf-hub/ort/ort-sys/tokenizers; feature wiring | C | `Cargo.lock:1744-1758` (also `image`, `ndarray`, `rayon`); `apps/openalpacad/Cargo.toml:43`; `crates/openalpaca_llm/Cargo.toml:14,44` | |
| M3 | OpenAI embedder 10 s/30 s; sends text to endpoint; no Ollama backend though config advertises one | C | `embedder.rs:49-56`, `:71-128`, `:242-274`; `config_schema/registry.rs:836` | Also: `build_embedder` passes `None` for runtime config (:230), so a configured `openai_embeddings` endpoint override is never used. |
| M4 | embedder built synchronously at service init, failure → None; schema advertises enabled/local/E5/768; Rust config does not instantiate defaults; "do not assert every install has embeddings" | CWC (understated) | `apps/openalpacad/src/services/llm.rs:172-233`, `services/mod.rs:141`; `config_schema/registry.rs:822-864`; `EmbeddingsConfig.enabled` serde-default false (`openalpaca_llm/src/config/llm_config/router_config.rs:50-56`) | The seeded template every daemon writes on first start **enables** it: `scripts/release/templates/config/llm.toml:46-50` (`enabled = true`, `provider = "local"`, e5-base, 768), embedded via `apps/openalpacad/src/bootstrap/config.rs:64`. So embeddings are the default for every seeded install, and the first boot downloads ~1 GB synchronously inside startup. |
| M5 | indexer scans missing embeddings; decay+prune worker; defaults hourly/30 d/.05/500/.05; decay rewrites `last_accessed_at` | C | `apps/openalpacad/src/background.rs:13-76`, `:81-136`; `daemon_config/orchestrator.rs:264-274`; `repo/decay.rs:75-83` | Indexer spawned only with an embedder; decay always (`main.rs:677-685`). No switch disables decay/pruning. |
| M6 | fixed 768 vs config 64..4096; vector errors swallowed | C | `001_baseline.sql:316`; `registry.rs:853-864`; `repo/search.rs:177-188` (`unwrap_or_default`) | Missed consequence: a dimension mismatch makes the indexer re-embed the same ≤50 rows every 30 s forever (see missed #10). |
| M7 | retain TokenManager (credentials) and capacity estimator | C (line nit) | `services/llm.rs:235-255`; `routing/router/capacity.rs:255-282` | |

## Prioritized findings

| # | Claim | Verdict | Evidence | Note |
|---|---|---|---|---|
| F1 | Lead injection with no workspace runs an unscoped hybrid search → every scope of the owner | **C** | `runner/lead_agent/mod.rs:425-447` (`search_hybrid(created_by, …, None, None, None)`) | Reachable in normal operation. The lead's `workspace` is the turn's `MemoryScopeContext` (via `start_workflow`) or `for_request(task.workspace_id)` on relaunch (`orchestrator/task_launch.rs:153`). It is `None` whenever the request carried no project and the daemon CWD folds to the home store — both launchers run the daemon in the home root (`scope_context.rs:108-116`, `src-tauri/src/lib.rs:269`, `apps/openalpaca/src/manager.rs:218`). That covers connector lanes (Telegram/Discord/iMessage), scheduled skills, GUI turns with no project selected, and CLI turns from a directory with no `.git`/`.openalpaca` marker. Retrieved rows are also `touch_accessed`-boosted (:450-455). Owner boundary holds. In a dev `cargo run` from the repo the CWD makes every such turn scope to the repo instead. |
| F2 | `memory_search` keeps a caller `workspace_id` when `ctx.workspace_id` is None; validator accepts unknown keys; `strict` is not enforced locally | **C** (reachability narrow) | `tools/builtins/memory_search.rs:29-42`, `:65-68`, `:84-93`; `tools/registry/mod.rs:1306-1370` (property loop :1338-1367 skips unknown keys); `validate_tool_arguments` runs before `execute_with_context` (`registry/mod.rs:618,635`) | Every production `ToolContext` copies the turn's scope (`simple_query_handler.rs:140-150`, `skill/invocation.rs:723-731,1227-1233`, `lead_agent/mod.rs:257-263`, subagents `lead_agent/tools.rs:338-344`), so `ctx.workspace_id` is None in exactly the F1 cases. Surfaces: main loop (always-core, `main_loop.rs:179-183`), lead (`lead_agent/mod.rs:157-159`), and subagents granted `memory_read` (`config/agents/{review,explore,code,general,planning}_agent.md`), plus the skill tier. **The model is never told about the parameter**: the schema lists only `query`/`limit` (:145-158), the description does not mention it, and results show `scope` but not `scope_id`. The provider layer does not enforce the schema either — Anthropic never sends `strict` (`providers/anthropic/request.rs:209-223`); OpenAI-compatible forwards `strict: true` (`providers/openai/request.rs:198-200`) without `additionalProperties: false`. So exploitation needs the model to invent an undocumented key holding another project's exact canonical path (hallucination or prompt injection). Same owner only. On the lead path F1 already exposes everything without a tool call. The context-less `execute` (which reads `owner_id` from args) has no production caller — all tool calls go through `SandboxManager` → `execute_with_context` (`security/sandbox/mod.rs:476-480`). |
| F3 | superseded rows stay in FTS; vector deleted; indexer re-embeds; retrieval can boost them | **C** | `repo/mod.rs:340-347`; `repo/embedding.rs:21-38` (LEFT JOIN … IS NULL); `apps/openalpacad/src/background.rs:43-63`; `repo/decay.rs:10-30` | Default indexer poll is 30 s (`daemon_config/server.rs:68`), so re-activation is quick. Un-accessed `.1` rows reach the `.05` floor after one half-life (~30 d) and are pruned. Missed consequence: once re-embedded, a stale predecessor can be chosen again as "closest" by `find_similar_for_supersession`, so a new item supersedes the old row while the actual current row stays live — two current heads. Inferred from code, not run. |
| F4 | fallback joins terms with ` AND `, quoting turns it into a literal token; probe returns 0 rows | **CWC** (slightly overstated) | `repo/search.rs:340-363` + `:13-24`; probe: `"Project" "AND" "database" "AND" "uses" "AND" "PostgreSQL"` → `[]`; intended query → hit | Reproduced. Caveats: (a) it still matches when the old memory happens to contain the word "and" (probe: "Project database uses PostgreSQL and Redis" matches), since unicode61 lowercases; (b) the fallback runs only when there is no embedding for the new item (`task_extraction/mod.rs:402-418`). On the seeded default config (embeddings on) the vector path runs, so F4 affects embedding-disabled or embedder-failed installs. |
| F5 | `kind_filter` applied to FTS only, not vec | **CWC** (latent) | `repo/search.rs:167-174`, `:177-189`, `:191-201` | True, but **every** production hybrid caller passes `kind_filter = None` (`prompt_ctx/sources/memory.rs:46`, `lead_agent/mod.rs:433,445`, `memory_search.rs:92`). The only kind-filtered search is forget's FTS-only `search_fts` (`memory_ops/mod.rs:92-100`). No production effect today; "mostly" should be "all". |
| F6 | KB import never retires stale/deleted content | **CWC** (latent) | `memory/kb.rs:17-80` | Behaviour true for the function, but it is dead code (see W9): no route, CLI verb, tool or boot step calls it since `5ec1db1d`. |
| F7 | similarity supersession can replace explicit knowledge; forget deletes best FTS Preference hit across scopes, no id/scope/ambiguity, Facts/KB unforgettable; decay/cap prune explicit preferences | **CWC** (understated) | `task_extraction/mod.rs:401-434`; `orchestrator/memory_ops/mod.rs:88-115`; `repo/decay.rs:44-91`, `:98-146` | Understated in three ways. (1) `persist_memory_item` is shared: explicit `memory_store` (`memory_ops/mod.rs:36-51`) and trait extraction (`extraction.rs:336-351`) also supersede on similarity, so a new explicit save can demote a different explicit preference, and auto-extraction can demote an explicit save. The successor may change kind: a Fact can supersede a Preference. (2) With the default E5 embedder the threshold is probably far too loose. The E5 model card says cosine scores sit around 0.7–1.0 even for unrelated text; for unit vectors that is L2 ≤ ~0.77, under the 1.0 supersession threshold (`daemon_config/orchestrator.rs:237`) and the 1.5 retrieval threshold. Not measured here. (3) The central retrieval path never calls `touch_accessed`, so a preference that only ever reaches the model through the central source still decays: 0.9 reaches the 0.05 floor after log2(18) ≈ 4.2 half-lives (~125 days) and is deleted. Forget also crosses workspaces: a Telegram turn can delete a project-scoped preference. |
| F8 | Markdown alone doesn't guarantee correctness/latency; central FTS already model-free | C | `prompt_ctx/sources/memory.rs:44`; inline embeds at `lead_agent/mod.rs:417-424`, `memory_search.rs:71-79` | The audit understates how weak the central path is. It feeds the whole user message into an implicit-AND FTS query, so natural-language turns and CJK text usually retrieve nothing (see R1 probe). The baseline is "almost no recall", not "keyword recall". |

## Existing tests and coverage limits

| # | Claim | Verdict | Evidence | Note |
|---|---|---|---|---|
| T1 | 18 storage / 30 core tests pass with those filters | C | re-run, identical counts | |
| T2 | coverage description; listed gaps | CWC (understated) | `repo/tests.rs` (18 fns); `memory/task_extraction/tests.rs` (5 parser tests); grep for callers in any test file | There are **zero tests anywhere** for `supersede`, `apply_importance_decay`, `prune_low_importance`, `find_similar_for_supersession`, `find_similar_fts_fallback` or `persist_memory_item`. The "registry owner-injection test" (`tools/registry/tests.rs:608-649`) exercises a mock `ContextCaptureTool`, not `MemorySearchTool`, so the real anti-spoofing and the F2 path are untested. `orchestrator/memory_ops/tests.rs` covers only `parse_remember_scope`. |

## Schema retirement trap

| # | Claim | Verdict | Evidence | Note |
|---|---|---|---|---|
| S1 | baseline 42, single registry entry | C | `migrations/mod.rs:3-5`, `:14-22` | |
| S2 | `Database::open` registers sqlite-vec before opening | C | `database/mod.rs:13-29`, `:39-40` (`sqlite3_auto_extension`, process-wide) | Also linked into the CLI (`store::open_home_database`). |
| S3 | refusal of 1..41 before any SQL | C | `database/mod.rs:126-139` | |
| S4 | a changed baseline at the same version would silently accept an old 42 DB | C | `database/mod.rs:141-149` (no pending → return) | |
| S5 | DB holds tasks/messages/artifacts/usage; the "factory-reset message" documents losses (:129-135) | CWC (minor) | `database/mod.rs:128-135` | That text is the legacy-schema **refusal** message that names the reset verb. It lists task/message/memory and orphaned artifact/upload files, not usage rows. The reset preview proper is `config_factory_reset.rs:176-212`. |
| S6 | dropping a vec0 table without the module is unsafe; not executed | C (hedge accurate) | — | Consistent with SQLite behaviour ("no such module"). Not executed here either. `memory_vec` is the only vec0 table and `memory_fts` the only FTS5 table in the baseline. |

## Surrounding app contracts

| # | Claim | Verdict | Evidence | Note |
|---|---|---|---|---|
| C1 | no dedicated memory CRUD in router/CLI/GUI | CWC | `apps/openalpacad/src/router.rs` (no `/v1/memory*`); routes removed in `5ec1db1d` | True for CRUD, but memory is exposed and written elsewhere. **Writer:** `ArtifactStore::rebase_project` rewrites `memory.scope_id` with raw SQL on `PATCH /v1/workspaces` (`crates/openalpaca_storage/src/artifacts/mod.rs:1533-1549`). **Readers:** `project_roots` unions memory scope ids for `store purge --all` (`:1653-1668`); workspace inventory and purge plans count memories (`:1585-1615`, `:1772-1775`), served by `GET /v1/workspaces` / the PATCH preview (`routes/workspaces.rs:301-307`, `:930`), shown by the GUI (`apps/openalpaca-gui/src/lib/api/workspaces.ts:25,57`) and the CLI (`apps/openalpaca/src/commands/store.rs:112-126`). **Config:** `daemon.orchestrator.decay_*`, `extract_*`, `task_extract_*`, `daemon.server.embedding_*`, `ai.embeddings.*` in the config schema and `openalpaca config` (`daemon_config_cli/mod.rs:24-32,157-167,279`). |
| C2 | purge keeps workspace memories and reserved `memory/` | C | `routes/workspaces.rs:628-636` | |
| C3 | `project_move.rs:35-90` coordinates the store move | C | `store/project_move.rs:35-90` | The memory re-key for the same operation lives in `artifacts/mod.rs:1539-1543`, not here (C1). |
| C4 | factory-reset preview says every memory goes, model cache survives | C | `config_factory_reset.rs:190`, `:203` | Also: `config/orchestrator/` (USER.md, the profile half of "memory") is listed as untouched (:207). |
| C5 | README reserves `memory/`; project memory deliberately not git-ignored | C | `store/mod.rs:904`, `:925` | |

## Integration / deletion-map claims

| # | Claim | Verdict | Evidence | Note |
|---|---|---|---|---|
| D1 | `memory/preferences.rs` `PreferenceBackend` has no implementors or callers | C | whole-workspace grep: `PreferenceBackend`, `PreferenceStore`, `PreferenceValue` appear only in `memory/preferences.rs` (108 lines, no test module) | |
| D2 | embedder consumers are memory-only | C | `.embed(` only in `memory_search.rs:73`, `task_extraction/mod.rs:279,393`, `lead_agent/mod.rs:418`, `background.rs:52` | `AppState.embedder` (`apps/openalpacad/src/state.rs:48`, set at `main.rs:714`) is never read. |
| D3 | `ContentKind::Memory` → `memory`, reserved and uncreated | C | `store/mod.rs:406-445`, `:586-594` | Only referenced in `ContentKind::ALL`. |
| D4 | memory workspace vs request root distinction | C | `memory/scope_context.rs:66-85`, `:153-178` | |

## Tally

53 rows (A1–A4, R1–R7, W1–W10, M1–M7, F1–F8, T1–T2, S1–S6, C1–C5, D1–D4):

- CONFIRMED: 41 (M7 has a line nit; F2 is confirmed, but only narrowly reachable)
- CONFIRMED-WITH-CORRECTION: 12. A1, A2, A3 and S5 are line or wording fixes. W9, M4, F4, F5, F6, F7, T2 and C1 are substantive.
- REFUTED: 0

## Missed by the audit (ordered by importance)

1. **Central retrieval has almost no recall.** SimpleQuery and the skill tier pass the user's whole message to an implicit-AND FTS query (`prompt_ctx/sources/memory.rs:41-48` → `repo/search.rs:13-24`). Every token must appear in one memory, punctuation included in the phrase, and CJK runs are single tokens. Probe: `what's my favorite editor?` → 0 rows; `中文回答` → 0 rows. F8's "central retrieval already skips embeddings" undersells that it rarely retrieves anything.
2. **KB import is dead code.** `ingest_directory` has no caller since `/v1/memory*` routes were removed in `5ec1db1d`. The same commit left dead repository API behind: `list_paginated`, `recent`, `get`, `get_for_owner`, `delete_for_owner`, `clear`, `embedding_stats`. `MemoryScopeContext::default_write_scope` has no production caller and `MemoryScope::Lane` is never written. F6/W9/W10 are latent.
3. **Embeddings are on by default.** The seeded `llm.toml` enables local E5 at 768 (`scripts/release/templates/config/llm.toml:46-50`), so a stock install downloads ~1 GB at first boot, synchronously in service init (`services/mod.rs:141`), and runs ONNX inference on tokio workers (lead start, `memory_search`, extraction, the 30 s indexer).
4. **Thresholds vs E5 distribution** (unmeasured): L2 1.0 supersession and 1.5 retrieval cutoffs are likely non-selective for E5 (cosine ~0.7+ even for unrelated text). The result would be near-universal supersession of the nearest same-scope Fact/Preference, and a retrieval cutoff that filters nothing. No E5 `query:`/`passage:` prefixes.
5. **Supersession is shared by all writers** (`persist_memory_item`): explicit `memory_store` and trait extraction too, not only task extraction. Kind can change. Stale predecessors re-embedded by the indexer can be superseded again, leaving two live heads.
6. **Central path never touches `last_accessed_at`.** Memories surfaced only through it still decay; an explicit 0.9 preference is pruned after ~125 days at defaults.
7. **Second SQL writer and readers of `memory` outside the repository**: `ArtifactStore::rebase_project` (`artifacts/mod.rs:1533-1549`), `project_roots` (:1653-1668), and workspace inventories (:1585-1615, :1772). These are contracts for `PATCH /v1/workspaces`, purge and `store purge --all` that a redesign must carry.
8. **USER.md is injected twice** on SimpleQuery/Skill turns: persona layer plus the `UserProfileSource` system message (the second copy unbudgeted).
9. **Stale Layer-3 cache**: the per-lane dynamic-context cache is keyed on query text with `memory_retrieval_hash` zeroed at every caller (`simple_query_handler.rs:550`, `skill/invocation.rs:575`, `compose/mod.rs:389-431`). A repeated identical query on a lane reuses the previous memory/profile sections even after a `memory_store`. The code already acknowledges this in `query_handler/workflow_context.rs:4-8`.
10. **Indexer retry starvation**: `list_missing_embeddings` has no ORDER BY and no failure mark (`repo/embedding.rs:26-37`). Rows that fail (dimension mismatch, embed error) are re-embedded every 30 s forever and can block other rows.
11. **Tests**: zero coverage of supersede/decay/prune/find_similar*/persist_memory_item; the "owner-injection" registry test uses a mock.
12. Smaller items:
    - task extraction ignores `extraction_model` (`model: None`);
    - LLM importance/confidence are unclamped;
    - `--workspace` substring mangling;
    - the OpenAI embedder ignores the configured endpoint;
    - `AppState.embedder` is unread;
    - the `memory_search` description promises "past conversations" and a "relevance score";
    - the `tool_helpers.rs:117` "no results" hint can never fire (empty results are `[]`, not an error);
    - CLAUDE.md's "Memory search is hybrid … cascading scope" is only true for the lead and the tool, not the central prompt path.

## Stale claims

None of the audit's claims is refuted, and none is stale relative to HEAD. The stale text is in the code:
- the owner/hash comments in `repo/mod.rs:90-91` and `memory/kb.rs:56`;
- the "DAG, or pipeline" wording in `memory/task_extraction/mod.rs:4-5,38`;
- the "connection pooling" comment in `database/mod.rs:3`.
