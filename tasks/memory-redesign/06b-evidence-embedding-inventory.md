> Evidence appendix to [05-review.md](05-review.md), rev 2 review of 2026-09-23. Read-only inventory of every embedding, vector and model-download consumer at HEAD `0d490847`. Note: its "Migration order" paragraph (drop `memory_vec` only in a build that still links sqlite-vec) is superseded by the stub-module result in [06d](06d-evidence-vec0-spike.md), which drops it without sqlite-vec.

# Embedding stack: every consumer

Repo `/Users/tangyunpei/dev/OpenAlpaca`, branch `chore/deps-upgrade-and-simplify`, HEAD `0d490847`, 2026-09-23.
This was a read-only pass. No repo file was edited, `~/.openalpaca` was not touched and no daemon or CLI was started. Scratch probes were built and run only in this scratchpad.

## Verdict

The audit's claim holds: **every consumer of the embedder or of vectors is memory.** No non-memory feature uses the stack. Skill routing, tool selection, intent parsing, artifact search, session search, plugins, MCP, connectors and wake were each checked, and none of them embeds anything.

Only five call sites actually compute an embedding (`.embed()`), and all five are memory. The rest of the `Option<Arc<dyn Embedder>>` threading only carries the embedder through to one of those five.

Some things are not consumers but still need care when the stack is deleted, because they would go stale or break (§6):
- a schema-migration ordering constraint, which a probe here confirmed;
- the signal-handling rationale in the daemon;
- the `state/cache` store layout and its README-supersession chain;
- the factory-reset copy;
- a GUI comment;
- the ONNX Runtime code that is statically linked into both release binaries, the CLI included.

---

## 1. Embedder trait, instances and `Option<Arc<dyn Embedder>>` plumbing

### 1a. Definition and export
| Where | What |
|---|---|
| `crates/openalpaca_llm/src/embedder.rs:1-371` | `EmbedError`, `trait Embedder { embed, dimensions }`, `OpenAiEmbedder` (feature `openai`, :28-133), `LocalEmbedder` (feature `local-embeddings`, fastembed, :137-216), `build_embedder` / `build_embedder_with_runtime` (:224-274), `resolve_openai_key` (:277-295), 4 tests (:297-371, one `#[cfg(feature="local-embeddings")]`) |
| `crates/openalpaca_llm/src/lib.rs:4` (`pub mod embedder`), `:32` (re-exports), `:22` (`EmbeddingsConfig` re-export) | public surface |
| Only `impl Embedder for` in the tree | `embedder.rs:70` (OpenAI) and `:189` (Local). **No test mock exists**, so no test exercises `openalpaca_core`'s vector path with `Some(embedder)`. |

### 1b. Construction and ownership (daemon)
| Where | Role |
|---|---|
| `apps/openalpacad/src/services/llm.rs:172-233` | `build_embedder`: reads `[embeddings]`, resolves `store::embedding_cache_dir()` only for `provider == "local"`, and logs `Embedder initialized` or `Embeddings disabled`. The call is synchronous, and the model download happens inside it. |
| `apps/openalpacad/src/services/mod.rs:31` (field), `:133-141` (build), `:215` (to the tool registry), `:275` (into `Services`) | owner |
| `apps/openalpacad/src/services/tools.rs:17, :57` | passes it to `builtin_tools_with_persona_context` → global `memory_search` |
| `apps/openalpacad/src/main.rs:468` | `Orchestrator::new(..., svcs.embedder.clone(), ...)` |
| `apps/openalpacad/src/main.rs:677-684` | `background::spawn_embedding_indexer` (only when `Some`) |
| `apps/openalpacad/src/state.rs:48` + `main.rs:714` | `AppState.embedder`: **written, never read** (no route or handler reads it). Dead. |

### 1c. The five real consumers (each one calls `.embed()`)
| # | Call site | Feature | Class | Without an embedder today |
|---|---|---|---|---|
| 1 | `crates/openalpaca_core/src/tools/builtins/memory_search.rs:71-79` → `search_hybrid_cascade` :87 | `memory_search` tool (global registry + lead surface) | memory search | FTS only (query embedding is `None`) |
| 2 | `crates/openalpaca_core/src/runner/lead_agent/mod.rs:416-445` | lead agent's proactive `### RETRIEVED MEMORY ###` injection | memory, lead injection | FTS only |
| 3 | `crates/openalpaca_core/src/memory/task_extraction/mod.rs:390-396` (`persist_memory_item_with_embedding`), then `find_similar_for_supersession` :403 or the FTS-Jaccard fallback :413, then `insert_embedding` :437/:468 | the one memory writer: supersession plus vector insert | memory store/extraction | FTS-Jaccard supersession (`fts_jaccard_threshold`), no vector row |
| 4 | `crates/openalpaca_core/src/memory/task_extraction/mod.rs:276-296` | batch embed for task-output extraction | memory extraction | per-item path with `None` |
| 5 | `apps/openalpacad/src/background.rs:9-75` (`spawn_embedding_indexer`; `.embed` :52, `.dimensions` :55) | back-fills `memory_vec` for rows missing a vector | memory index | not spawned |

Callers that reach consumer #3 (all memory):
- `memory_store` tool: `tools/builtins/memory_ops.rs:24,31,36,65` → `orchestrator/memory_ops/mod.rs:19,36-50` (`store_memory`)
- user-trait extraction: `orchestrator/handlers.rs:347,390` → `orchestrator/extraction.rs:31,336-350`
- task extraction: `memory/task_extraction/mod.rs:56,299,338-360`, reached from `orchestrator/dispatcher/memory.rs:16,44,49` ← `dispatcher/lead_agent.rs:1097-1110`

### 1d. Pass-through only (holds or forwards the embedder, embeds nothing)
- `crates/openalpaca_core/src/orchestrator/mod.rs:182` (field), `:294` (param), `:341` (to `TaskDispatcher::new`), `:367`
- `crates/openalpaca_core/src/orchestrator/dispatcher/mod.rs:65,94,109`
- `crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs:489,661` (into `run_lead_agent`), `:1101`
- `crates/openalpaca_core/src/runner/lead_agent/mod.rs:82` (param). It is **not** forwarded to subagents.
- `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs:200` → `tools/builtins/main_loop.rs:121,166`. It reaches only `MemoryStoreTool`; `memory_forget` and `memory_search` on the main loop come from the global registry.
- `crates/openalpaca_core/src/tools/builtins/mod.rs:306-313,350` (`builtin_tools` → `memory_search_tool`), `:387,394`
- `crates/openalpaca_core/src/tools/builtins/memory_search.rs:12,132,165`
- `crates/openalpaca_core/tests/steer_routing_eval.rs:249`: passes `None`

### 1e. Places where a non-memory use was looked for and not found (evidence)
| Candidate | Where | Finding |
|---|---|---|
| `tool_selection` (`core_union` / `full`) | `simple_query_handler.rs:210-228`; `daemon_config/orchestrator.rs:136,176`; `validation.rs:168-175` | `full` is `registered_tool_names()` filtered by `extension_is_available`. `core_union` is the base picks from `orchestrator/intent/tool_suggest.rs:62` (regex and substring flags) plus the per-request set. No vectors. |
| SkillRouter weighted scoring | `orchestrator/skill/router/mod.rs:1-7, 116-157`; `docs/Skill_Template_Reference.md:141-158, 388-401` | `base + intent substring + keyword ratio + recency - negative keyword`. Purely lexical. |
| Intent routing | `orchestrator/intent/skill_match.rs` (slash command, then router), `intent/tool_suggest.rs` (regex), social fast path (exact phrase) | lexical |
| Per-turn prompt memory (`MemorySource`) | `prompt_ctx/sources/memory.rs:41-44`, wired at `orchestrator/mod.rs:307` | passes `None` ("FTS-only search"). The chat prompt's automatic memory is **already model-free**. |
| KB ingestion (`KbChunk`) | `memory/kb.rs:17` | no embedder, and **no production caller** (only its own test at :118). Rows it wrote would have been back-filled by the indexer. |
| Artifact search | `openalpaca_storage/src/artifacts/mod.rs:1193` | `LIKE` via `escape_like` |
| Session search | `repository/conversation/mod.rs:658`, `routes/sessions.rs:392` | `LIKE` |
| Plugins, MCP, connectors, wake, `openalpaca_api` | whole crates | zero references |
| HTTP routes | `apps/openalpacad/src/routes/**` | zero references. No memory route exists; `GET /v1/status` has no embedding or indexer field. |
| Direct use of `ort`, `tokenizers`, `hf_hub`, `ndarray`, `rayon` | all crates and apps | none. `fastembed::` appears only in `embedder.rs:139-177`. |
| Ollama "embedding-only" filter | `providers/ollama.rs:117,233`; `routing/router/ollama_tests.rs:354-420` | model-discovery logic that skips embedding models from Ollama's catalogue. It is **unrelated** to the embedder: keep it. |

### 1f. Already dead inside the stack
- `AppState.embedder` (`state.rs:48`): never read.
- `MemoryRepository::embedding_stats` (`repository/memory/embedding.rs:40-56`): test-only (`memory/tests.rs:326`).
- `build_embedder_with_runtime` is only ever called with `runtime_config = None` (`embedder.rs:230`), so `EndpointsConfig.openai_embeddings` (`openalpaca_llm/src/config/llm_config/runtime.rs:61-62,71-72,80`) is never read in production.

### 1g. Signature fallout: call sites that lose one positional argument (tests included)
`builtin_tools(` 17 · `builtin_tools_with_persona_context(` 5 · `Orchestrator::new(` 26 · `TaskDispatcher::new(` 6 · `main_loop_tool_set(` 3 · `run_lead_agent(` 9 · `memory_search_tool(` 2 · `MemoryStoreTool::new(` 5 · `persist_memory_item(` 4 · `persist_memory_item_with_embedding(` 3 · `extract_task_memories(` 3 · `spawn_task_memory_extraction(` 2 · `store_memory(` 3 · `extract_user_traits_background(` 6 · `build_tool_registry(` 2. That is roughly 95 occurrences, definitions included (from `rg -c`).

---

## 2. sqlite-vec / vec0 / `memory_vec`

| Where | What |
|---|---|
| `Cargo.toml:104` (`sqlite-vec = "0.1.6"`, lock resolves 0.1.9); `crates/openalpaca_storage/Cargo.toml:19` | non-optional dependency of the storage crate, so it compiles into the daemon, the CLI and the GUI backend |
| `crates/openalpaca_storage/src/database/mod.rs:8,13-29` (`ensure_vec_extension`, `sqlite3_auto_extension`), `:40` | registered on **every** `Database::open`. That covers the daemon and the CLI's `store::open_home_database()` (`apps/openalpaca/src/commands/config.rs:79`). The GUI backend never opens the database. |
| `crates/openalpaca_storage/src/migrations/001_baseline.sql:272` (comment), `:312-317` | `CREATE VIRTUAL TABLE memory_vec USING vec0 (memory_id INTEGER PRIMARY KEY, embedding float[768])`. vec0 also creates the shadow tables `memory_vec_chunks`, `memory_vec_info`, `memory_vec_rowids`, `memory_vec_vector_chunks00` and 2 autoindexes (observed in the probe below). The FTS triggers do not touch `memory_vec`. |
| `repository/memory/embedding.rs:8-18` (insert), `:21-38` (`list_missing_embeddings`, indexer only), `:41-55` (stats, dead) | vector CRUD |
| `repository/memory/search.rs:88-151` (`search_vec`, KNN `MATCH … AND k`), `:176-189` (hybrid merge), `:259-321` (`find_similar_for_supersession`), `:323+` (FTS fallback, stays) | vector reads |
| `repository/memory/mod.rs:15-21` (`DEFAULT_VEC_DISTANCE_THRESHOLD`, `VEC_OVER_FETCH_FACTOR`), `:209-211` (clear), `:224-225` (delete), `:265-267` (scoped delete), `:346-347` (supersede) | `DELETE FROM memory_vec` inside the memory row transactions |
| `repository/memory/decay.rs:137-140` | orphan `memory_vec` cleanup |
| Tests: `database/tests.rs:215-245` (`test_sqlite_vec_available`); `repository/memory/tests.rs:237, 275, 326, 369, 409, 496, 552` | 8 tests |
| `scripts/gen_api_docs.py:1000-1016` | **no vec0-specific handling.** Its `CREATE (VIRTUAL) TABLE name (…)` regex needs `(` straight after the name, so any `USING <module> (` table, `memory_fts` and `memory_vec` alike, is skipped silently. `docs/api/database/schema.md` lists neither, so deleting `memory_vec` changes nothing in `docs/api/database/`. This is a pre-existing gap in the generator. |
| `docs/api/crates/openalpaca_llm.md:15,26,28` | generated. It lists the `embedder` module and its re-exports and changes on regeneration. |

**Migration-order probe.** This was run here against a *real* vec0 table built by sqlite-vec 0.1.9's own C source, the version the workspace locks, compiled in the scratchpad against system SQLite 3.51 (`vecprobe.c`). Results:
- Without the module registered:
  - `SELECT` from `memory`, `PRAGMA integrity_check` and `VACUUM` all succeed.
  - Any reference to `memory_vec`, including `DROP TABLE IF EXISTS memory_vec`, fails with `no such module: vec0`.
  - The shadow tables are ordinary tables and can be dropped one by one, which leaves a broken virtual-table row behind.
- With the module registered: `DROP TABLE memory_vec` removes the table, all four shadow tables and their autoindexes cleanly.

What this means for the deletion:
- Migration 043's `DROP TABLE memory_vec` must run in a build that still registers sqlite-vec.
- The five `DELETE FROM memory_vec` statements must be removed in the same change as the registration. Otherwise every memory delete or supersede transaction fails once the module is gone.
- The rusqlite build is `bundled` (`Cargo.toml:97`). This behaviour belongs to SQLite core, so it is not specific to one build.
- The audit's `01-code-audit.md:139` says its own probe "supplies no evidence". This one does, although it used a disposable database and not a real schema-42 store.

---

## 3. Config, CLI, GUI and API surfaces

### llm.toml `[embeddings]`
- Type: `crates/openalpaca_llm/src/config/llm_config/router_config.rs:14` (field), `:49-56` (`EmbeddingsConfig { enabled, provider, model, dimensions }`). Re-exports at `llm_config/mod.rs:14` and `lib.rs:22`.
- Seeded template `scripts/release/templates/config/llm.toml:46-50`: `enabled = true`, `provider = "local"`, `model = "intfloat/multilingual-e5-base"`, `dimensions = 768`. **Every install seeded from it downloads the model.** The repo's `config/llm.toml:31` is git-ignored and local.
- Not a key in the template, but in the type: `[endpoints] openai_embeddings` (dead, §1f).
- **Stale-key hazard.** The M3 in-place writer (`llm_config/edit.rs:16-19,126-133`) keeps any key "these types never knew about". Once `EmbeddingsConfig` leaves the struct, an existing `[embeddings]` table survives every GUI or CLI write, silently ignored. The repo convention for a retired key is a boot WARN (`crates/openalpaca_core/src/daemon_config/mod.rs:71-106`, `warn_on_removed_keys`), and `llm.toml` has no such probe yet. Adding one, or removing the table on write, is a decision.

### config_schema registry (`crates/openalpaca_storage/src/config_schema/registry.rs`)
- `:391-404` `daemon.orchestrator.supersession_distance_threshold` ("L2 distance threshold for semantic supersession")
- `:750-775` `daemon.server.embedding_poll_interval_secs`, `daemon.server.embedding_batch_size`
- `:821-862` `ai.embeddings.enabled|provider|model|dimensions` (category `Agents`, subcategory `Embeddings`)
- Existing inaccuracies, which become moot on deletion:
  - `provider` advertises "local, openai, or ollama", but there is no Ollama backend (`embedder.rs:242-273`).
  - `dimensions` accepts 64–4096 while `memory_vec` is fixed at `float[768]`.
  - The default says `true`, but a missing block means "disabled" in code (`services/llm.rs:228-230`).
- Tests: `config_schema/tests.rs:147, 213-216, 245, 277-282, 366-371, 386-387`.

### daemon.toml
- `[server.embedding_indexer]`: `config/daemon.toml:173-175`. It is not in the release template `scripts/release/templates/config/daemon.toml`. Type at `crates/openalpaca_core/src/daemon_config/server.rs:21,33,56-72`.
- `[orchestrator.memory] supersession_distance_threshold`: `config/daemon.toml:56`, `daemon_config/orchestrator.rs:213-215,237`, clamp `validation.rs:44-49`.
- `fts_jaccard_threshold` stays: it drives the model-free fallback. Its doc comment ("when the embedder is unavailable", `orchestrator.rs:216-218`) becomes stale.
- The decay knobs `[orchestrator.memory.decay]` are memory, not embedding, and belong to the redesign's own scope.

### CLI (`apps/openalpaca/src/commands/`)
- `ai_config.rs:131-150` (unset), `:238-241` (`list_ai_entries` rows), `:310` (`clear_ai_config_at` sets `embeddings = None`)
- `ai_config_helpers.rs:6` (import), `:195-240` (`apply_to_config` set), `:385-396` (`read_from_config` get)
- `daemon_config_cli/mod.rs:139-144` (supersession mapping), `:277-287` (indexer mappings)
- `config_tui.rs:1618-1651` draws the `Agents` subcategories from the registry, and `:945-971` does the same for `Daemon`. The "Embeddings" screen disappears by itself when the registry rows go.
- `config_factory_reset.rs:203` prints "Untouched: {root}/state/cache/ — the local embedding model (~1 GB)". Test `config_factory_reset/tests.rs:233-240` seeds `state/cache/fastembed/model.onnx`.

### GUI (`apps/openalpaca-gui/`)
- No setting, no API type, no route call, no Tauri command.
- Only prose: `src/views/chat/first-run.ts:21`, the rationale for `=== undefined` ("a first boot answers slowly (the embedding model is downloaded before anything else)"), and `DESIGN_SPEC.md:1609`. The logic stays correct either way, but the reason goes stale.

### API
- No route exposes embedding or indexer state. There is no memory route at all.
- `GET /v1/status` has no field for it, and the GUI's `DaemonLlmStatus` has none either.

### Store layout (`crates/openalpaca_storage/src/store/mod.rs`)
- `:329-347` `embedding_cache_dir()` has one caller, `services/llm.rs:191`.
- `:349` `CACHE_DIR` has no other user, so `state/cache/` has no other producer.
- `HOME_README` `:888` ("the embedding cache") and `:897` (the `state/cache/` row).
- Changing `HOME_README` means appending its current text as `superseded_readmes/home-6.txt` and bumping `SUPERSEDED_HOME_READMES` (`:944-950`). The tripwire tests are at `store/tests.rs:349-393`. `home-5.txt:14` already carries the fastembed row.
- Tests `store/tests.rs:171-189` (`embedding_cache_dir`) and the `:318` comment; `apps/openalpacad/src/routes/workspaces/tests.rs:1239` (comment).
- `.gitignore:134-138` (`.fastembed_cache`).
- Per the audit's own rule, clean up an existing `~/.openalpaca/state/cache/fastembed` only with a targeted removal, never a sweep of `state/`.

### Other stale rationale
`apps/openalpacad/src/shutdown.rs:12` and `apps/openalpacad/src/main.rs:172-175` justify installing signal handlers before any thread pool starts by naming "ONNX Runtime thread pool". The mechanism is still correct because tokio has pools too. Only the example goes stale.

---

## 4. Packaging and first-run docs

**What ships and what downloads.**
- None of the release scripts or installers mentions the model or its cache: `scripts/release/package-{macos,linux}.sh:55/48`, `package-windows.ps1:58`, `install*.{sh,ps1}`, `uninstall*`.
- The Tauri bundle does not either: `tauri.conf.json:37` `externalBin: ["bin/openalpacad"]`, and `scripts/prepare-sidecar.ts`.
- **Nothing ships or pre-fetches the model.** It is downloaded at runtime by hf-hub on the first daemon boot.

**Why the first boot blocks.**
- The daemon binds its listener and writes `discovery.json` early (`main.rs:202,214`).
- It builds services, embedder included, at `main.rs:359`, and only starts `axum::serve` at `:741`.
- So on first boot a client finds a discovery file for a daemon that does not answer until the roughly 1 GB download ends.

**Build-time ONNX Runtime.**
- `ort-sys` is built with feature `download-binaries`, reached through `fastembed` feature `ort-download-binaries`. `cargo build` fetches a prebuilt static `libonnxruntime.a` (67 MB here, in `~/Library/Caches/ort.pyke.io/dfbin/aarch64-apple-darwin/…`).
- The library is linked statically. `otool -L target/release/openalpacad` shows no onnxruntime dylib.
- The release daemon is 75.5 MB.
- **The release CLI `target/release/openalpaca` (32.6 MB) also contains ONNX Runtime**: `OrtGetApiBase` and about 23.5k `onnxruntime` symbols, with 0 `fastembed` symbols.
  - This is feature unification: the packaging scripts build `-p openalpaca -p openalpacad` together, and CI builds with `--workspace`.
  - Deleting fastembed shrinks both binaries. The exact MB saving was not measured, because that would need a rebuild.

**Docs that promise the download or semantic search (to rewrite):**
- `README.md:52` ("The first start is slow… about 1 GB… for memory search"), `:140`, `:154`, `:170`
- `docs/Installation_Manual.md:233-238`, `:420-427` (the whole §6 "The embedding model", and the anchor at :235)
- `docs/QuickStart_Manual.md:55-59`
- `docs/Daemon_Manual.md:76` (README refresh wording), `:78-83` (cache paragraph), `:910` ("embedding indexer" background task)
- `docs/CLI_Manual.md:179` (factory reset leaves "the local embedding model, ~1 GB")
- `docs/Skill_Template_Reference.md:994`; `docs/tools/TECHNICAL.md:660, 686, 698`
- Strings the model reads, which change its behaviour and not only documentation:
  - `tools/builtins/memory_search.rs:138-143` ("hybrid search combining full-text search and vector similarity … relevance score")
  - `tools/builtins/memory_ops.rs:82` ("Semantically similar existing memories are superseded automatically")
- `CLAUDE.md:74, 108, 163, 267, 269, 275, 276, 341, 348`; `apps/openalpaca-gui/DESIGN_SPEC.md:1609`
- Historical records, not to be rewritten: `tasks/api-fix-plan.md` (10 hits), `tasks/research/*` (about 27), `tasks/extension-enable-design.md` (3), `tasks/lessons.md` (2)
- Owner's Obsidian KB, under the mirror-decisions rule: notes `01`, `04`, `05`, `09`, `10`, `11`, `12` mention embeddings

---

## 5. Dependency tree (`cargo tree --offline -p openalpacad -e normal -i …`)

- `fastembed v4.9.1`: only `openalpaca_llm`, and only through `openalpacad`'s `local-embeddings` feature (`apps/openalpacad/Cargo.toml:39-44`, `crates/openalpaca_llm/Cargo.toml:14,43-44`, `Cargo.toml:105`). Under `--workspace`, feature unification also puts it into the `openalpaca` CLI's build.
- `ort v2.0.0-rc.9`: only `fastembed`.
- `ort-sys v2.0.0-rc.9`: only `fastembed` and `ort`. Features: `download-binaries`, `ureq`, `tar`, `flate2`, `sha2`.
- `tokenizers v0.21.4`: only `fastembed` (feature `onig`, which compiles the C library oniguruma).
- `hf-hub v0.4.3`: only `fastembed` (features `ureq`, `native-tls`).
- `sqlite-vec v0.1.9`: only `openalpaca_storage`, so it is in all of daemon, CLI and GUI. Its only other dependency is the `cc` build dependency, which other crates share.
- `-p openalpaca` and `-p openalpaca_gui` on their own resolve no `ort`.
- **Nothing outside the embedding path needs ort, tokenizers or hf-hub.** No code references `ort::`, `tokenizers::`, `hf_hub::`, `ndarray::` or `rayon::`.

**Size of the removal.**
- `openalpacad` has 465 unique normal-dependency packages. With `--prune fastembed` it has 369, so **96 packages are reachable only through fastembed**. Pruning `sqlite-vec` as well leaves 368.
- The fastembed-only set: ort, ort-sys, tokenizers, hf-hub, onig/onig_sys, esaxx-rs, ndarray, rayon/rayon-core/rayon-cond, image and a whole image-codec stack (rav1e, ravif, exr, tiff, png, gif, zune-jpeg, avif-serialize, av1-grain…), ureq, indicatif, console, two webpki-roots versions, num-complex/num-rational/num-bigint, half, matrixmultiply, and others. The full list is in `fastembed_only.txt` in this scratchpad.

---

## 6. Non-memory consumers: none. What breaks or goes stale anyway, and the model-free answer

| Item | What breaks or goes stale if the embedder, sqlite-vec and cache are deleted | Model-free replacement |
|---|---|---|
| `memory_vec` in existing schema-42 databases | `DROP TABLE memory_vec` fails once vec0 is not registered (proved above), and so do the five `DELETE FROM memory_vec` statements | Migration 043 that drops it while sqlite-vec is still linked, then remove the crate; or the redesign's export-then-convert utility (`01-code-audit.md:134-139`). Do not hand-drop shadow tables. |
| Supersession (`find_similar_for_supersession`) | the vector branch goes | the existing FTS + Jaccard fallback (`search.rs:323+`, `fts_jaccard_threshold`), or the redesign's explicit active/superseded state |
| `memory_search` and lead injection | the vector half of the hybrid search goes | FTS only, which is what `prompt_ctx/sources/memory.rs:44` already does on every chat turn |
| `ai.embeddings.*`, `[server.embedding_indexer]`, `supersession_distance_threshold` | keys are silently ignored in existing files (the M3 writer keeps them) | boot WARN, following `warn_on_removed_keys` |
| `state/cache/` | nothing writes it any more; README row and factory-reset copy go stale | drop `embedding_cache_dir` and `CACHE_DIR`; add a README text through `home-6.txt`; do a targeted one-time cleanup of the fastembed directory |
| First-boot wait (docs, GUI comment) | the promise of a ~1 GB wait becomes false | rewrite the docs; the GUI logic stays |
| Signal-handler comment | the ONNX example goes stale | reword it; keep the mechanism |
| CLI and daemon binaries | both lose the statically linked ONNX Runtime, and the build no longer fetches from the pyke CDN | none needed |
