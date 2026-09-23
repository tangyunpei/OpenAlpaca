# OpenAlpaca memory implementation audit (2026-09-23)

Source snapshot: `7561f152712c01912436ad04a6fd57974be2bcab`, branch `chore/deps-upgrade-and-simplify`, schema baseline 42.

Read-only source audit. Line numbers refer to the checkout inspected today. No application code was changed. Focused offline tests passed; this is not end-to-end runtime or retrieval-quality certification.

Path shorthand: `repository/memory/`, `models/`, `database/`, `migrations/`, `config_schema/` and `store/` are under `crates/openalpaca_storage/src/`; `prompt_ctx/`, `runner/`, `tools/`, `orchestrator/`, `memory/` and `daemon_config/` are under `crates/openalpaca_core/src/`. `embedder.rs` means `crates/openalpaca_llm/src/embedder.rs`; `background.rs` means `apps/openalpacad/src/background.rs`. A bare filename/line continuation refers to the fully identified module in that bullet or paragraph. The implementation map groups companion filenames under the first full path in each row.

## Current architecture

The live design is already a mixture of Markdown and database memory. `USER.md` stores the profile; conversation summaries and durable memories are separate stores. The proposed replacement should retire the durable memory subsystem without deleting task state, conversation history, profile identity, context budgeting, credential management, or the generic SQLite database.

### Authoritative durable memory

- `crates/openalpaca_storage/src/migrations/001_baseline.sql:274-340`: `memory` is authoritative; its fields are owner, kind, scope/id, source, content, SHA-256, importance/confidence, creation/update/access dates, metadata and supersession predecessor. Kinds are `fact`, `preference`, `kb_chunk`; scopes are `global`, `workspace`, `lane` (`models/memory.rs:5-128`). FTS5 is an external-content index synchronized by insert/delete/content-update triggers. `memory_vec` is a `vec0` virtual table with fixed `float[768]` embedding.
- Actual uniqueness is `(owner_id, scope, scope_id, content_hash)` at baseline:292. Repository `mod.rs:87-89` and KB `kb.rs:57` comments still say owner/hash only. **Do not report a cross-scope dedup bug; the real schema fixed that.**
- `crates/openalpaca_storage/src/repository/memory/mod.rs:90-134` adds rows; :212-275 deletes/clears vector plus row transactionally; :286-351 supersedes by inserting a successor and lowering old importance to .1, removing its vector.
- `crates/openalpaca_storage/src/database/mod.rs:31-35,183-200`: all database repositories share a single SQLite connection behind a mutex. This is more specific than the file's “connection pooling” module comment.

### Retrieval

- `repository/memory/search.rs:13-24,26-86`: whitespace split, quote every term, FTS implicit AND, rank ordering. There is no stop-word removal, query rewriting, CJK-specific segmentation, result score or source span returned by the repository.
- :91-151: vector nearest-neighbor candidates are selected across the vector table, over-fetched 3x, then filtered by owner/scope/id. Distance threshold defaults to 1.5. Correct owner isolation exists, but candidate starvation can occur when another scope dominates nearest neighbors.
- :156-202: “hybrid” means FTS results first, then vector results, dedup by ID until the limit. It is not reciprocal-rank fusion or normalized score combination. Full FTS results can completely exclude vector results.
- :213-256: cascade searches workspace first, then global. Lane exists in schema but is not in the normal global/workspace cascade.
- `prompt_ctx/sources/memory.rs:25-74`: central context source always passes `embedding=None`, searches up to 10, estimates tokens by UTF-8 bytes/4, and injects memory as untrusted user content. It uses importance as section “relevance”; `prompt_ctx/manager.rs:55-76` then sorts sections by that value, potentially replacing search order with importance order.
- `runner/lead_agent/mod.rs:411-487`: a separate proactive injection path generates an embedding inline, retrieves 5, boosts access, truncates each content to 300 Unicode characters, and wraps it as untrusted memory. It has separate budget logic and inconsistent scope behavior (findings below).
- `tools/builtins/memory_search.rs:70-108`: the tool also embeds inline, cascades scopes, and boosts importance. Tool results expose ID/kind/scope/content/importance/date but omit underlying provenance and retrieval score (:112-126), despite description promising score/metadata (:138-143).

### Writes and extraction

- `tools/builtins/memory_ops.rs:48-71` routes explicit saves to `orchestrator/memory_ops/mod.rs:17-75`. All saves are classified `Preference`, source `Conversation`, importance .9, confidence 1.0, even though the tool description says fact or preference. Default scope is global; a literal `--workspace` substring in content selects workspace and is removed (:126-142). If no workspace is detected, it silently degrades to global with a warning.
- `orchestrator/extraction.rs:43-108`: trait extraction occurs after a response, every N qualifying turns per lane (default 5), but only the current turn is inspected, with user truncated to 800 characters and assistant to 400. This is not a summary of all intervening turns. LLM output chooses `profile` or `memory`; threshold fields exist.
- :325-350: memory extractions are forced global preference and stored with hard-coded .6 importance/.7 confidence; the original extracted confidence is discarded and no conversation/message evidence IDs are persisted.
- :358-367,390-405,528-568: profile extractions modify `USER.md`, parse/render it, write a fixed `.md.tmp` then rename, update the in-memory document and bump persona version. This is an existing Markdown pipeline to reuse selectively, but it does not provide compare-and-swap or one writer lock around read/modify/write.
- `prompt_ctx/sources/user_profile.rs:25-61` injects `USER.md` content as a system message. Unlike retrieved memories, automatically derived profile text is elevated into instruction context. Treat this as a trust-boundary design risk, not a demonstrated prompt-injection exploit.
- `memory/task_extraction/mod.rs:61-139,239-315`: successful task output extraction is enabled by default, truncates description/output to 500/2000 chars, asks an LLM for at most 5 facts/preferences, filters confidence < .5, embeds batches, and saves scope workspace if available or global. It records task ID and extraction source path, not evidence ranges or quoted supporting material.
- `orchestrator/dispatcher/memory.rs:25-59`: extraction is fire-and-forget and errors are logged. It has no durable pending queue or shutdown-drain guarantee.
- `memory/task_extraction/mod.rs:389-455`: similarity to the closest Fact/Preference is sufficient to supersede; no same-claim key, contradiction classification, authority comparison, or human review protects older explicit assertions.
- `memory/kb.rs:17-112`: directory import recursively reads every `.md`, chunks at roughly 500 units using custom heading/paragraph splitting, stores `KbChunk` with file/section/index metadata. It is additive: edits add new chunks and deletion/moves do not retire old content. The recursive walk follows `is_dir()` through symlinks without root confinement, cycle prevention, limits or ignore rules. Pathological traversal/exposure is a code risk; no actual file escape was exercised.
- `memory/chunker/mod.rs:35,98-125`: implementation mixes byte lengths and character-count hard splitting, and an individual long sentence can exceed the nominal limit. Fence-aware Markdown parsing, stable block IDs and source line spans are absent.

### Models, dependencies and maintenance

- `crates/openalpaca_llm/src/embedder.rs:137-211`: local fastembed backend loads the configured model; fallback model is `intfloat/multilingual-e5-base`. Source comments estimate roughly 1 GB download. This is an embedding model with tokenizer assets, not a dedicated token-counting model. The synchronous model `.embed()` call occurs inside an async method without `spawn_blocking` (:190-199).
- `Cargo.lock:1743-1758`: `fastembed` 4.9.1 pulls `hf-hub`, `ort`, `ort-sys`, `tokenizers` and supporting libraries. `apps/openalpacad/Cargo.toml:43` enables `local-embeddings`; `crates/openalpaca_llm/Cargo.toml:14,44` gates the optional dependency.
- `embedder.rs:49-55,71-127`: remote OpenAI embedding backend has 10s connect/30s request timeouts and sends memory/query text to the configured embedding endpoint. There is no Ollama embedding backend in the factory (:242-273), although configuration description advertises one.
- `apps/openalpacad/src/services/llm.rs:172-233`: embedder creation is synchronous during service initialization when enabled. Failure falls back to no embedder. `config_schema/registry.rs:823-859` advertises enabled/local/E5/768; the Rust optional config does not automatically instantiate these defaults if no embeddings block is present. Do not assert that every running installation has embeddings enabled without reading its effective config.
- `apps/openalpacad/src/background.rs:13-75`: background missing-embedding scans and batch writes. :81-135 runs importance decay and pruning. `daemon_config/orchestrator.rs:264-272` defaults to hourly polls, 30-day half-life, .05 floor, 500 non-KB memories and .05 retrieval boost. `repository/memory/decay.rs:53-85` overwrites `last_accessed_at` during decay even without retrieval, so that field is really both decay reference and access metadata.
- Fixed DB dimension 768 conflicts with config allowing 64..4096 (`config_schema/registry.rs:853-859`); repository explicitly delegates dimension enforcement to the table. Retrieval swallows vector errors (`search.rs:177-188`), which can make bad configuration look like ordinary keyword results.
- **Retain** `TokenManager` (`apps/openalpacad/src/services/llm.rs:235-250`): it is credential/OAuth token discovery and refresh, unrelated to NLP tokenization. Retain request capacity estimation (`openalpaca_llm/src/routing/router/capacity.rs:263-297`), context manager budgets and conversation compaction. These already use rough estimators, not a local tokenizer model.

## Prioritized findings

### F1 — Confirmed scope violation in lead-agent injection

`runner/lead_agent/mod.rs:425-447`: with workspace, cascade is used. Without workspace, unscoped hybrid search reads every scope for that owner. A global/connector task can therefore receive another workspace's memories. Owner boundary is still enforced. Change all entrypoints to one typed retrieval service with explicit permitted scope set; “no workspace” must mean global only, never wildcard.

### F2 — Confirmed same-owner workspace argument bypass in search tool

`tools/builtins/memory_search.rs:29-40` clones caller arguments and always overwrites owner ID but only overwrites workspace ID when trusted context is Some. With `ctx.workspace_id=None`, an unrecognized caller `workspace_id` is preserved and used at :65-93. Registry validator `tools/registry/mod.rs:1337-1367` silently accepts unknown keys: it only validates properties it recognizes. `strict: true` in the model-facing definition is not a local enforcement boundary. This is source-confirmed bypass of intended workspace authority for the same owner, not a cross-owner issue or live model exploit test. A new service should accept owner/workspace only as typed context, reject unknown identity fields, and make context-less `execute` reject execution.

### F3 — Confirmed superseded content remains/reappears in search

`repository/memory/mod.rs:340-347` marks old row .1 and deletes only its vector. FTS queries never exclude predecessors. `repository/memory/embedding.rs:28-31` marks that old row as missing its embedding; indexer `background.rs:43-56` regenerates it. Thus a superseded fact is still available lexically and becomes available semantically again until eventual pruning. Retrieval can boost it back up. New design needs explicit `active/superseded/archived` state; default queries exclude inactive blocks, including index rebuilds.

### F4 — Confirmed FTS supersession fallback requires literal “AND”

`repository/memory/search.rs:340-363` joins extracted terms using ` AND `, then `search_fts` quotes every whitespace token (:13-24), turning SQL FTS syntax into literal “AND”. Reproduced in an in-memory SQLite FTS5 probe with exactly this query construction: content `Project database uses PostgreSQL`, generated query `"Project" "AND" "database" "AND" "uses" "AND" "PostgreSQL"` returns zero rows; intended quoted terms return the row. Exact-content duplication remains handled by hash, but nonidentical updates usually bypass intended fallback supersession. Replace with an explicit literal-query builder; do not preserve this code.

### F5 — Confirmed kind filter absent from vector branch

`search_hybrid` applies `kind_filter` only to FTS (:167-174); `search_vec` has no kind argument (:91-99), and the merged vector rows are not filtered (:191-201). A filtered hybrid query may return other kinds. This is an API contract defect, although current central/tool call sites mostly pass None. Replacement candidate filters must happen before selection and be verified at service boundary.

### F6 — Confirmed KB stale-copy behavior

`memory/kb.rs:17-85` only adds content hashes; there is no source manifest, reconciliation, file hash, deletion reconciliation or version tombstone. A changed file keeps obsolete content retrievable, and deleting the file does not revoke its chunks. A Markdown canonical store removes this dual authority if indexes are always derived from current files and scan manifests explicitly retire disappeared blocks.

### F7 — Destructive heuristic semantics (design risk)

Similarity only shows relatedness. `task_extraction/mod.rs:401-434` makes the closest related fact/preference a predecessor; model-generated output can downgrade/replace explicit user knowledge without checking source authority. Forget (`orchestrator/memory_ops/mod.rs:91-114`) searches all scopes for Preferences and deletes the best FTS hit, with no exact ID, explicit scope or ambiguity handling; Facts and KB cannot be forgotten through this tool. Decay/500-row pruning deletes even explicit durable preferences (`decay.rs:107-131`). Replace similarity supersession with explicit same-subject/claim-key revisions, default archive, and tombstones preventing extraction from restoring forgotten facts.

### F8 — Markdown alone does not solve correctness or latency

Current SQL FTS is already model-free; central prompt retrieval already skips embeddings. Removing embeddings eliminates a proven dependency and critical-path model calls, but neither faster total retrieval nor higher recall is established without benchmarking. Pure grep/FTS can lose paraphrase and multilingual recall. Plan aliases and structured subject/claim keys, multilingual lexical handling, bounded search expansion, source citations, temporal validity, and abstention. Learn from modern systems' memory lifecycle instead of copying their model/vector dependency.

## Existing tests and coverage limits

Executed successfully on 2026-09-23:

```text
cargo test --offline -p openalpaca_storage repository::memory -- --nocapture
18 passed, 0 failed, 488 filtered out (0.24s test execution)

cargo test --offline -p openalpaca_core memory:: -- --nocapture
30 passed, 0 failed, 1635 filtered out (0.04s test execution)
Integration binaries selected by Cargo: 0 matching tests executed.
```

Storage tests cover add/get/hash dedup, FTS filtering, owner isolation, basic vector retrieval, missing-index/stats, hybrid ID dedup, FTS triggers, vector cleanup, pagination, touch boost and workspace/global cascade. They use toy constant vectors and do not certify real-model semantic quality. `repository/memory/tests.rs:409-444` tests dedup, not hybrid ranking/filter correctness.

Core memory tests cover chunking, basic KB import/reimport dedup, workspace marker/canonicalization/home-store rules, five JSON-response parser cases, and a mocked extraction no-thinking contract. Additional tools tests exist (`tools/builtins/memory_ops.rs:160+`, registry owner-injection test `tools/registry/tests.rs:608+`) but were not included in this focused run. No test found in these memory-specific files for supersession+indexer reactivation, FTS fallback's AND quoting, real retrieval relevance, provenance-backed factuality, KB edit/delete reconciliation, or concurrent Markdown saves. These omissions do not imply the entire repository was exhaustively searched for every behavioral equivalent.

## Integration and deletion map

Replace or delete:

- Durable DB models/repository and old three-table memory schema/triggers.
- `openalpaca_llm/src/embedder.rs`, exports, `EmbeddingsConfig`, daemon embedder state/construction and all threaded `Option<Arc<dyn Embedder>>` plumbing (or retain only if a separately audited nonmemory consumer needs it; current search found memory consumers).
- Embedding indexer, memory decay/pruning worker, associated config/schema/UI controls, `state/cache/fastembed` code (explicit targeted cleanup of old cache only, never broad state cleanup).
- `fastembed` and `sqlite-vec` workspace dependencies, local-embeddings feature and resulting unused ONNX/HF/tokenizers lockfile dependencies. Re-resolve dependencies and verify transitive packages, rather than hand-delete arbitrary lock entries.
- Current FTS/vector supersession, query-ranked forget, KB importer/chunker as canonical storage mechanism, duplicate proactive lead retrieval.
- Unused `memory/preferences.rs` abstraction if still no callers; `rg` found only self definitions, no implementations of `PreferenceBackend`.

Retain/adapt:

- `openalpaca_storage::store::{StoreScope, ContentKind, content_dir}`; `ContentKind::Memory` already maps to `memory` (`store/mod.rs:406-445,586-594`). Existing home/project stores reserve `.openalpaca/memory` without auto-creating it. Home/project ownership and purge semantics must be extended consistently.
- Workspace resolution and distinction between memory workspace and actual request-supplied root (`scope_context.rs:66-84`). A CWD fallback is not evidence to write project-owned files. Prefer request root for new writes; specify compatibility deliberately.
- Owner authentication and tool execution context; context manager trust wrappers, byte/token budgets, summary/compaction, dialogue/task storage, profile parsing/change notification and cost logging.
- One `MemoryService` shared by retrieval source, lead and simple-query paths, tools, API, GUI, connector/scheduled workflows and extraction queue. Canonical `.md` files + derived lexical index is compatible with keeping SQLite for app state; model-free FTS is not a tokenizer model.
- Keep `USER.md` identity/communication configuration under a clear separate role or migrate it under one explicit decision. Do not allow automatic experience files to become unrestricted system instructions merely because the extension is `.md`.

Suggested capabilities: `memory_read` for bounded list/search/read with allowed scopes; `memory_write` for explicit create/revise with revision precondition and provenance; `memory_forget` (destructive) for tombstone plus cache/source-derived purge, explicit ID/scope; `memory_manage` for import/export/restore/reindex and policy changes. Source-derived candidate generation should have `memory_propose` only, with promotion policy controlled by the service. A plugin must receive granted capabilities and scope set, never raw arbitrary memory directory write access. Match actual core sandbox capability conventions when implementing; this is proposed taxonomy, not existing API.

## Schema retirement trap and bounded pre-release path

Current baseline is **42**, not old migration 040 notes: `migrations/mod.rs:3-5,14-22`. `Database::open` registers sqlite-vec before opening (:22-40); `run_migrations` refuses versions below baseline before applying SQL (`database/mod.rs:116-139`). A changed baseline at the same version would silently accept an old 42 DB unchanged. Do not do that.

Recommended fresh baseline 43 without durable memory tables or vec extension registration. Existing 42 DB should receive a precise refusal before normal service initialization, with actual DB path and documented preservation/reset alternatives. No silent reset: this database also holds tasks, messages, artifacts and usage. The existing factory-reset message itself documents those losses (:129-135).

If keeping local development data matters, use a **one-shot retirement utility built from the old dependency set**, not a permanent feature flag retaining the legacy memory engine:

1. Stop the daemon and prevent concurrent writers. Back up with SQLite's backup/checkpoint-aware mechanism; copying only `.db` while WAL is live is insufficient.
2. Open using the old sqlite-vec registration, require exact schema42, inventory all durable memory plus scope/source/metadata/lifecycle fields. Produce an export manifest and staged Markdown tree; preserve source authority and do not pretend inferred facts are confirmed. Mark predecessor chains inactive using supersedes links; .1 importance alone is not definitive supersession status.
3. Validate parse round trips, counts, IDs/content hashes, owner/scope mapping and file confinement before committing. Stage Markdown with resumable phases and a receipt; filesystem writes and DB transactions are not one atomic transaction.
4. While the vec module still exists, drop memory FTS triggers, FTS table, vec virtual table and memory table in an explicit transaction; preserve all unrelated tables/rows. Register schema43 only after successful retirement and durable manifest publication. Prove idempotent resume/crash behavior and compare nonmemory data hashes/counts.
5. Open the converted DB with the new build without sqlite-vec, rebuild only the derived lexical memory index and verify end-to-end read/search/write/forget. Remove the utility from normal runtime once complete.

Why module order matters: virtual-table lifecycle operations can require their module implementation; simply removing sqlite-vec and then expecting `DROP TABLE memory_vec` to succeed is unsafe. This audit did not execute an actual old vec0 database with module registration removed. A synthetic in-memory missing-module probe was blocked by local SQLite defensive `sqlite_schema` write protection and supplies no evidence. Require a targeted compatibility test using an actual disposable schema42 DB before committing the migration implementation. Do not edit `sqlite_schema` or hand-drop extension shadow tables as a shortcut. This caveat is bounded to retirement tooling, not an argument for retaining embeddings in the released app.

Alternative: explicit development reset after export/backup, chosen by the operator. Major code deletion authorization does not itself authorize silent destruction of all unrelated local application records.

## Surrounding app contracts checked in this pass

- `apps/openalpacad/src/router.rs` and the CLI/GUI memory-reference inventory contain no dedicated memory CRUD management surface. Current memory access is mainly agent tools; new management API/CLI/GUI work must be budgeted rather than described as a backend-only replacement.
- `apps/openalpacad/src/routes/workspaces.rs:628-636` explicitly keeps workspace memories and the reserved `memory/` directory when purging project run/history data. Preserve this contract; implement explicit memory erasure separately.
- `crates/openalpaca_storage/src/store/project_move.rs:35-90` already coordinates moving the project store and refuses ambiguous/cross-volume moves. New watchers and index partitions must participate in the same lifecycle.
- `apps/openalpaca/src/commands/config_factory_reset.rs:178-208` currently says every memory is removed with the database while the model cache survives. Canonical Markdown changes that statement: update the reset preview, identity-reclamation behavior and tests; never silently transfer preserved files to a newly created identity.
- `store/mod.rs:904,925` reserves memory as user content and currently deliberately does not git-ignore project memory. The proposed per-owner private-memory subtree changes that default and must update seeded READMEs/ignore rules carefully.

The [design](03-design.md) and [implementation plan](04-implementation-plan.md) translate these findings into a replacement. No audited defect was patched in this documentation task.
