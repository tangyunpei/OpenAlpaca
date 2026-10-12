# Audit of design rev 3 and plan rev 3: verified gaps

**Date:** 2026-09-23. **Audited:** [03-design.md](03-design.md) rev 3 and [04-implementation-plan.md](04-implementation-plan.md) rev 3 at `1451edac` (branch `feat/markdown-memory`), against the code at the same commit, with [05-review.md](05-review.md) and [06a–06d](06a-evidence-audit-verification.md) as evidence. **Question answered:** is the design and plan ready to implement as written? **Answer: not yet.** Two blockers and eighteen majors stand between the documents and an implementer who never has to guess; every one is a document fix or a small owner decision, none reopens MD-1…MD-9 or the full-removal instruction, and none needs new design work beyond writing down what the design already implies.

**Method.** Four critics (correctness, contracts, security, delivery) filed 65 gap reports. The synthesizer merged 16 duplicates (the same mechanism reported through two or three lenses), re-verified every retained gap against the source at HEAD, and corrected two evidence claims: PR #33 merged as a **true merge commit** (`4256b77c`, two parents), not a squash, so this branch rebases with two commits rather than being re-cut (G5); and six `config/agents/*.md` templates grant `memory_read`, not seven (G40). Nothing was dropped: no report re-litigates a settled decision, and every report survived verification as at least a minor gap. **49 gaps** remain: **2 blockers, 18 major, 29 minor**. Read-only throughout; the only file written is this one.

**Buckets.** (A) is what must be fixed in the documents before slice 0 starts — either because slice 0 itself depends on it, or because plan §8 makes the owner's approval of the *whole* written design the gate for starting anything, and these two blockers make the design unapprovable as written. (B) is what must be fixed before the slice it names. (C) can be decided while building, by the implementer, with a one-line note in the documents.

**Owner decisions needed (not document fixes):** **G2** (what origin a model-issued memory write gets, and whether `confirm` is reachable from the tool — a hard decision with a security consequence), **G30** (where the Memory view lives in the GUI shell — a small one), and **G12** (what the quality gate compares against when baseline B is unavailable — a small one). **G38** is a scope trim the owner should nod to (drop or build the "inventory `USER.md`" promise). Everything else is a document fix.

---

## (A) Fix in the documents before slice 0 starts — 5 gaps

### G1 — blocker — Slice 1 is told to "freeze" four contracts that rev 3 does not contain

**Where:** 04 §2 slice 1 (the "Freeze the v1 contracts from design rev 3" list); 03 §6 (the snapshot's contents), §7 (the tool table and the surfaces paragraph).

**Statement.** Slice 1's first task is to freeze "the note schema and the control-snapshot schema", "the tool JSON schemas", "the `/v1/memories` shapes" and `MemoryAccessContext` "from design rev 3". Of the six named contracts, only the note schema (§4's illustrative file and field list) and the partition key (§2) exist in a freezable form. The control snapshot is described by what it holds ("binding, `pending`, per-note revision and origin, tombstones with their `supersedes` lists, origin rank and suppressed sources") with no field names, types or example. The tool table (§7) is prose per column — "query, optional kind/scope enum, bounded limit, historical flag" — with no property names, types, enums, required lists or result objects. The routes are one sentence with no methods, paths, bodies, cursor format or error shape. `MemoryAccessContext` is four words (G6). An implementer starting slice 1 has nothing to freeze; they must *author* these, which is design work the plan does not assign, and G6, G7, G10, G11, G17, G18, G19 below are the specific decisions that authoring will have to take.

**Evidence.** 04 slice 1: "Freeze the v1 contracts from design rev 3: the note schema and the control-snapshot schema … the tool JSON schemas … the `/v1/memories` shapes". 03 §6 last paragraph of *Write protocol* ("Snapshots are replaced atomically and stay bounded …") is the only description of the snapshot's shape. 03 §7 table rows and the "Surfaces" bullet: "Edits use `If-Match` or a revision (409 on conflict), with cursor pagination, capped payloads".

**Smallest fix.** Either add a §7a to the design holding the four schemas (the snapshot JSON with an example, the six tool parameter objects and result objects, the route table with methods, bodies and the cursor rule, the `MemoryAccessContext` struct), or reword slice 1 as "**author and freeze** the v1 contracts", listing the decisions it must take (G6, G7, G10, G11, G17, G18, G19, G20) and stating that the owner signs the frozen set before slice 2 starts.

**Blocks:** slice 1 (and through it every later slice).

### G2 — blocker (owner decision) — What origin a model-issued memory write gets, and whether the model can `confirm`, is undefined; read literally, prompt injection mints top authority

**Where:** 03 §4 *Authority* (`user` = "an explicit save or edit through a user-directed surface"; "Confirming a candidate (`memory_update` action `confirm`, a user-directed surface only) makes it `active` with origin `user`"); §6 *Promotion and correction* ("Explicit 'remember' writes are synchronous (origin `user`)"); §7 table row `memory_update` (lists `confirm` as a model/tool input) and §7 *Capabilities* ("The main conversation keeps its user-directed write capability"); 04 §2 commit 3.2 (ships `confirm` as a tool action).

**Statement.** "User-directed surface" is never defined, and the design uses it two incompatible ways. §7 says the main conversation *keeps* its "user-directed write capability", so a main-loop `memory_store` call is user-directed and records origin `user` — the highest rank, which may supersede the user's own notes (§6: "at or above") — and `confirm`, listed as a tool input, promotes an `extracted_other` candidate (by definition inferred from assistant or tool output) to `user` on the model's say-so. §4 says `confirm` is for "a user-directed surface only", which reads as *not* the tool. Today the main-loop turn that holds `memory_store`/`memory_forget` also holds `web_search`, `web_fetch` and every enabled MCP/plugin tool, so untrusted text arrives in the same turn as the write tools, and neither memory tool is annotated or confirmed. Under the literal reading a web page, an MCP result, an uploaded file or a retrieved note can, through the model, mint a `user`-origin note that retires the user's own, defeating the design's stated guarantee ("a lower-authority note never overrides a user-authored one"; "hand-edited frontmatter cannot retire a user's note"). Under the other reading, no chat path ever produces origin `user` and MD-2's "lower authority than an explicit save" has no chat instance. Nothing on `ToolContext` says which surface a call came from, and `MemoryAccessContext`'s "channel" is undefined (G6). Slice 1 cannot freeze "the authority ordering and origins" until this is decided, and it is a product decision with a security consequence, so it is the owner's.

**Evidence.** `crates/openalpaca_core/src/tools/builtins/main_loop.rs:158-184` (`memory_store`/`memory_forget` on the main-loop surface, no confirmation); `crates/openalpaca_core/src/tools/builtins/mod.rs:578-590` (`annotations_for_builtin`: `memory_search` read-only, no arm for `memory_store`/`memory_forget` → `None`); `crates/openalpaca_core/src/security/sandbox/mod.rs:626-643` (the confirmation set is derived from `destructive_hint`, so neither prompts); `crates/openalpaca_core/src/tools/registry/mod.rs:50-100` (`ToolContext` carries `lane_key` :86, `source` :89, `principal` :94 — no user-directed bit); CLAUDE.md *Message Routing* (the main loop's surface carries every enabled extension tool and `web_search`/`web_fetch`).

**Decision the owner must take.** One of: **(1)** a main-loop tool call whose `ToolContext.principal` is `Principal::User` *is* user-directed: `memory_store`/`memory_update edit|archive|supersede` record origin `user`, `confirm` is allowed from the tool, and the mitigation is G11's confirmation prompt on the destructive actions plus the "no `user` supersedes `user` without the target's revision" rule; or **(2)** (recommended by the security critic) a model tool call never mints `user`: `MemoryAccessContext` carries `surface: UserSurface | ModelTool`; only the API, CLI and GUI write origin `user`; a `ModelTool` store or edit records `extracted_user`; `confirm` is refused by the tool with a message naming Settings → Memory and `openalpaca memory confirm`; a `ModelTool` supersede of a `user`-origin note is reported as a conflict, not applied. Option 2 narrows MD-2's "explicit save" to the non-tool surfaces, which is why it is the owner's call. Whichever is chosen: define "user-directed surface" in §4 once, add the chosen rule to the §5 regression row *Malicious Markdown, frontmatter or source text* as "…or tool output cannot mint origin `user`", and freeze it in slice 1 with the origins.

**Blocks:** slice 1 (origins), commit 3.2 (tools), the GUI's confirm flow (3.9).

### G3 — major — 0.5's newer-schema refusal is specified by what it names, not by what it must never name, and the only refusal in the tree names the factory-reset verb

**Where:** 04 §2 slice 0.5; 04 §4 *Rollback*.

**Statement.** 0.5 says the refusal names "the version and the database path". The message an implementer will extend is the legacy refusal at `database/mod.rs:126-139`, whose remedy is `openalpaca config reset --factory`. CLAUDE.md's standing rule is that the four start-up refusals never name each other's remedies. The plan's own rollback scenario puts an older binary (slice 0 onward) and a newer (43) store on one machine; if the older build's newer-schema message inherits the reset verb, the user who follows it deletes the new build's database — exactly what 0.5 exists to prevent. The guard is a one-line comparison, so the message *is* the design of 0.5, and the plan leaves it to the implementer.

**Evidence.** `crates/openalpaca_storage/src/database/mod.rs:126-139` (the `bail!` text: "Run `openalpaca config reset --factory` to delete {file} …"); `:87-92` (`schema_version()` reads `MAX(version)`); `:141-150` (a version above the newest registered migration finds no pending migration and returns `Ok(())` silently); 04 §4: "A pre-cutover build **from slice 0 onward** refuses the new build's 43 database (0.5)".

**Smallest fix.** Write the 0.5 text into the plan: "database `<path>` is at schema `<n>`, newer than this build knows (`<max>`): use the OpenAlpaca build that created it" — no verb — and add to 0.5's test that the message does not contain `reset --factory`.

**Blocks:** slice 0 (0.5).

### G4 — minor — 0.2 "turn the prune step off by default" implies a switch that does not exist

**Where:** 04 §2 slice 0.2.

**Statement.** `spawn_memory_decay` calls `apply_importance_decay` and then `prune_low_importance` unconditionally, and `MemoryDecayConfig` has no enable flag. "Off by default" therefore means either a new config key (registry row, `config/daemon.toml` line, `daemon_config_cli` row, TUI row — all deleted again in 3.6) or deleting the call. The implementer must guess, and the two choices differ in four files.

**Evidence.** `apps/openalpacad/src/background.rs:121-127` (both calls in the loop, no guard); `crates/openalpaca_core/src/daemon_config/orchestrator.rs:253-274` (`MemoryDecayConfig`: `poll_interval_secs`, `half_life_days`, `min_importance`, `soft_cap`, `access_boost` — no `prune_enabled`); `config/daemon.toml:58-62`.

**Smallest fix.** Reword 0.2: "remove the `prune_low_importance` call (and its count) from the decay loop; keep decay; no new key", with the characterization test as stated.

**Blocks:** slice 0 (0.2).

### G5 — minor — The branch premise is stale: PR #33 has merged, and this branch is stacked on it

**Where:** 04 §1 ("on a new branch cut from `main` **after draft PR #33 merges**. Do not stack this work on #33"); §7 ("Draft PR #33 merges first").

**Statement.** PR #33 merged on 2026-09-23 at 19:48 UTC as a **merge commit** (`4256b77c`, two parents — not a squash, contrary to one critic's report). `feat/markdown-memory` is `chore/deps-upgrade-and-simplify` (`7561f152`, the head #33 merged) plus two local commits (`0d490847`, `1451edac`). The local `origin/main` ref is stale at `51e9560e`. So the plan's precondition is met, its "do not stack on #33" rule is moot (the stack is now in `main`), and the branch is usable after a fetch and a two-commit rebase — but the documents still say to wait.

**Evidence.** `gh pr view 33 --json state,mergedAt,mergeCommit` → `MERGED`, `2026-09-23T19:48:29Z`, `4256b77c`; `gh api …/commits/4256b77c` → `parents: 2`; `git ls-remote origin refs/heads/main` → `4256b77c`; local `git log -1 origin/main` → `51e9560e`; `git rev-list --count origin/main..HEAD` → 68 (66 of which are #33's, already in remote `main`).

**Smallest fix.** Record in §1/§7 that #33 merged (merge commit `4256b77c`, 2026-09-23) and that the work branch is `feat/markdown-memory` rebased onto `origin/main` (two commits) before slice 0.

**Blocks:** slice 0 (the first `fix:` commit's base).

---

## (B) Fix before the slice they block — 35 gaps

### Before slice 1 (contracts)

### G6 — major — `MemoryAccessContext` has four undefined words and no table of who builds it on each surface

**Where:** 03 §3 opening paragraph; 04 slice 1 (freeze list), commit 3.1.

**Statement.** "Owner identity, the request workspace, allowed operations and channel" is the whole definition. "Allowed operations" duplicates the sandbox's capability gate for tools and has no source on the API or CLI. "Channel" could be `HandleRequest.source`, `ToolContext.source`, `lane_key` or the `Principal`. The inputs exist per surface but differ: recall reads `ContextRequest { owner_id, scope: MemoryScopeContext }`; tools read `ToolContext { owner_id, workspace_id, request_workspace_root, workspace_path, principal, source, lane_key }`; the lead and subagents carry `created_by` and the task's workspace; `/v1/memories` will have `state.local_user_id` and, by the convention every other route enforces, the `x-workspace-path` header — which the design never names; the CLI `memory` verbs' workspace (its CWD, as `chat` sends it) is unstated, and no `--global`/`--project` override is mentioned. A wrong guess files `fact`/`procedure`/`decision` notes into the wrong partition, because rule 2 defaults them to the project.

**Evidence.** 03 §3: "Every operation receives a typed `MemoryAccessContext` … owner identity, the request workspace, allowed operations and channel" and nothing further. `crates/openalpaca_core/src/prompt_ctx/sources/mod.rs:20-29` (`ContextRequest`); `crates/openalpaca_core/src/tools/registry/mod.rs:50-100` (`ToolContext` fields: `owner_id` :63, `workspace_id` :67, `request_workspace_root` :77, `lane_key` :86, `source` :89, `principal` :94, `workspace_path` :100); `apps/openalpacad/src/routes/tasks.rs:974` ("send the project as the x-workspace-path header, the way every other route takes it"); `apps/openalpaca/src/chat_stream/mod.rs:116,187` (the CLI always sends its canonical CWD).

**Smallest fix.** A per-surface table in §3: surface → owner source, workspace source, principal/surface value (see G2), and whether writes are permitted: main loop = `ToolContext` (`Principal::User`, `request_workspace_root`); lead/subagent = `created_by` + task workspace, read-only; recall = `ContextRequest`; API = `local_user_id` + `x-workspace-path`; CLI = the same header from its CWD, with `--global`/`--project <path>` overrides. Replace "allowed operations" with the resolved capability set or drop it.

**Blocks:** slice 1; commit 3.1.

### G7 — major — Supersede is a *create* in §6 and an *update action* in §7; `memory_store` has no `supersedes` input

**Where:** 03 §6 mutation table row *supersede*; §7 rows `memory_store` and `memory_update`.

**Statement.** §6: "**Create B** with `supersedes: [A]`, given A's id and expected revision. A's file is not touched." §7 puts `supersede` under `memory_update`, whose input is "ID, expected revision, structured patch or body, reason; action … `supersede` (the target's id and expected revision)" and whose result is "Conflict or a new revision" — but B has no id yet, A is not written, and the result carries no new UUID. `memory_store`'s inputs list neither `supersedes` nor `fact_key`. An implementer must guess which tool creates B and what it returns.

**Evidence.** 03 §6 row 2 and §7 rows 3–4 as quoted.

**Smallest fix.** Give `memory_store` optional `supersedes: [{id, expected_revision}]` and `fact_key`, returning the new UUID and revision; remove `supersede` from `memory_update`'s actions — or define `memory_update {id: A, action: supersede, body…}` as "creates B, returns `{id: B, revision}`" and change the result column. One of the two, in both sections.

**Blocks:** slice 1 (tool schemas); commit 3.2.

### G8 — major — The partition key hashes the canonical *project root*, not the canonical *partition directory*, and no component on the path to the partition is checked for links

**Where:** 03 §2 (`<partition-key>` = "first 16 hex of sha256(canonical project root)"; "rejects escaping paths, symlinks and junctions"); §3 rule 1; 04 slice 2 confinement bullet.

**Statement.** Confinement is anchored at the memory directory and applied to files under it. The path *to* it — `<project>/.openalpaca`, `memory`, `owners`, `<owner-key>`, `notes` — can each be a git-committed symlink in a cloned repository; the marker walk and `content_dir` both follow links, and the reference pattern the design names (`confine_to_root`) canonicalizes the root first, so a root reached through a link is accepted as-is (its own doc says so). Two consequences: project writes land wherever the link points, and, because the key hashes the project root rather than the resolved partition directory, two projects whose `.openalpaca` resolve to one directory get two snapshots over one `notes/` — a `pending` recovery in one classifies the other's writes as `external`, and tombstones apply in one view only. The artifact store already solved this by addressing rows through the canonical *store* directory (`project_root_at`); the design diverges from that rule without saying so.

**Evidence.** `crates/openalpaca_storage/src/store/mod.rs:612-624` (`project_root_at` doc: "a `<project>/.openalpaca` symlinked at another store resolves to *that* store's address … `confine_to_root` cannot see that case — the symlink is *at* the root it canonicalizes"); `crates/openalpaca_storage/src/store/artifact.rs:215-270` (`confine_to_root` canonicalizes `root`, checks descendants only); `crates/openalpaca_core/src/memory/workspace.rs:64-77` (`walk_up_for_marker` uses `exists()`, which follows links); `store/mod.rs:586-597` (`content_dir` → `create_dir_all` through a link).

**Smallest fix.** Slice 1: define `<partition-key>`'s project component over the canonical partition directory (what `project_root_at` does for artifacts), not the canonical project root. Slice 2: resolve the partition directory once by walking `<project>` → `.openalpaca` → `memory` → `owners/<key>` → `notes` with `symlink_metadata` on each component (or an `openat` chain with `O_NOFOLLOW|O_DIRECTORY`), and refuse the project partition — fall back to global with one WARN — when any component is a link, a junction or not a directory. Add both to slice 2's "every confinement case".

**Blocks:** slice 1 (the key); slice 2 (confinement).

### G9 — major — The Settings → Persona `USER.md` editor and `openalpaca persona edit user` have no route and no mechanism

**Where:** 03 §6 (*`USER.md`* bullet), §7 *Surfaces*; 04 rows 3.4, 3.8, 3.9, §7 risk table ("the GUI commit (3.9) depends only on the routes in 3.8").

**Statement.** 3.8 lists only `/v1/memories` routes; the router has no persona path; the CLI has no `persona` subcommand; the GUI is a webview whose Tauri capabilities are `core:default` and `dialog:default` (no filesystem), so it cannot edit the daemon's config file without a route or a new Tauri command. `file_write` refuses `USER.md`, and `update_persona` is bootstrap-only (05 row 7). The daemon watches the file and reloads it, which would serve a CLI `$EDITOR` edit — but the plan does not say the CLI edits the file, and if it does, there is no revision check against a concurrent `update_persona` during bootstrap. The GUI editor cannot be built from 3.8 as written, contradicting the risk table.

**Evidence.** `apps/openalpacad/src/router.rs` (`rg -n -i 'persona|memor'` → no match); `apps/openalpaca/src/main.rs:38-74` (subcommands: Daemon, Config, Gui, Connector, Tasks, Agents, Llm, Chat, Sessions, Plugin, Ext, Store); `apps/openalpaca-gui/src-tauri/capabilities/default.json`; `crates/openalpaca_core/src/tools/builtins/file_ops.rs:252-273` (the three persona-file refusals); `apps/openalpacad/src/hot_reload.rs:170-171` (the `USER.md` watcher).

**Smallest fix.** Add `GET /v1/persona/user` and `PUT /v1/persona/user` (raw Markdown; `If-Match` on a content hash or the `persona_version`; 409 on conflict; the same atomic writer `update_persona` uses) to 3.8 and to slice 1's route freeze; route both the CLI verb (GET → `$EDITOR` → PUT with the fetched hash) and the GUI editor through it; regenerate `docs/api/`.

**Blocks:** slice 1 (routes); commits 3.4, 3.8, 3.9.

### G10 — major — "Shows which notes a response used" has no wire contract, no persistence rule and no owning commit

**Where:** 03 §7 *GUI* bullet, §8 (`openalpacad` row); 04 rows 3.3, 3.9, §7 risk table.

**Statement.** `ChatStreamEvent::Done` carries `content, model, tokens_in, tokens_out, duration_ms, attachments_used, attachments_skipped, delegation`; `HandleResult` and `GatewayResponse` carry the same optional trio; the stored assistant row has nothing; `GET /v1/chat/history` serves the row flattened (P1: attachments come back from `content_json`). Commit 3.3 (recall) adds no field, 3.8 lists routes only, and the risk table says 3.9 depends only on 3.8. Undefined: the field's name and shape (id, revision, partition, chunk?), whether it is live-only on `done` (the `attachments_skipped` precedent) or persisted for reload, and whether skill-tier and lead turns report it.

**Evidence.** `crates/openalpaca_core/src/chat/stream_manager/mod.rs:35-49`; `crates/openalpaca_core/src/gateway/router/mod.rs:49-61`; 03 §7: "It also shows which notes a response used"; 04 row 3.9 test column: "'notes used in this answer' on assistant turns".

**Smallest fix.** Add to 3.3: `HandleResult`/`GatewayResponse`/SSE `done.memory_used: [{id, revision, partition_key}]`, omitted when empty, live-only like `attachments_skipped` (stated in `GUI_Manual.md`) — or persisted in the assistant row's `content_json` with the 3.5 baseline rewrite, if reload must show it. Say which in §7, freeze the shape in slice 1, and let 3.9 read it.

**Blocks:** slice 1 (shape); commits 3.3, 3.9.

### G11 — major — Which memory tools carry `destructive_hint` — and so prompt on attended turns and are refused on `unattended` ones — is never stated

**Where:** 03 §6 *Forget contract* ("no confirmation beyond the user's request and existing tool policy"), §7 *Capabilities*; 04 commit 3.2 ("security classification" as a key file, no rule).

**Statement.** "Existing tool policy" is empty for memory: neither `memory_store` nor `memory_forget` is annotated today, so the model deletes memories silently. The main loop has no explicit `require_confirmation_for`, so annotations are its only gate. Whether `memory_forget` and `memory_update`'s `archive`/`supersede`/`confirm` join the derived confirmation set decides whether a prompt-injected forget is stopped by a card or not (it is the mitigation G2 option 1 relies on), and whether an `unattended` turn is refused them. The implementer must guess.

**Evidence.** `crates/openalpaca_core/src/tools/builtins/mod.rs:578-590` (`annotations_for_builtin`); `crates/openalpaca_core/src/security/sandbox/mod.rs:263-267`, `:626-643` (explicit list wins, else `destructive_hint = true`).

**Smallest fix.** State in §7 *Capabilities*: `memory_forget` and `memory_update` (every action but `edit`) carry `destructive_local_annotations()`; `memory_store`, `memory_search`, `memory_read`, `memory_list` do not. 3.2 gains a test that `effective_confirmation_set` on the main loop contains `memory_forget` and `memory_update`, and that an `unattended` turn is refused them.

**Blocks:** slice 1 (freeze with the tool contracts); commit 3.2.

### G12 — major (small owner decision) — Baseline B needs the E5 model, which the plan's own isolation rules keep out of the scratch store; when B is "unavailable" the quality gate has no comparator

**Where:** 04 slice 1 *Baseline B*; §5 *Baselines* and the gate "against **B**"; slice 4 ("This work never reads `~/.openalpaca`"); §6 ("Any daemon or CLI run uses `OPENALPACA_HOME_STORE` … in a scratch directory").

**Statement.** B is "the current `memory_search` tool path, hybrid, with the E5 model when it is already present locally". The only cache the embedder reads is `state/cache/fastembed` *inside* the home store, which under the isolation rules is a fresh scratch directory — so the model is never "already present" and B needs a ~1 GB download into the scratch store. §5 says only that an unrunnable B "is recorded as unavailable" and defines no gate for that case, yet the −5 pp non-inferiority gate is stated against B alone.

**Evidence.** `crates/openalpaca_storage/src/store/mod.rs:337-347` (`embedding_cache_dir()` = `state_dir()/cache/fastembed` under `home_root()`, which honours `OPENALPACA_HOME_STORE`); `apps/openalpacad/src/services/llm.rs:172-233` (resolves that dir for `provider == "local"`; [06b](06b-evidence-embedding-inventory.md) §1b).

**Decision.** Either (a) perform the one-time download into the scratch store during slice 1 and record it as part of B's conditions, or (b) when B is unavailable, gate against A with a stated wider margin. Write the chosen rule into §5.

**Blocks:** slice 1 (B must run before slice 3 deletes the engine); slice 4's gate.

### G13 — major — The evaluation has no harness form, location, reader/judge model rule or cost bound

**Where:** 04 slice 1 *Evaluation set v0*; §5 *Controls*; §6 ("Tests never reach `localhost:11434`").

**Statement.** About 470 held-out plus tuning gold questions in eight categories across English/Chinese/mixed, "with a frozen judge protocol" — but no author or method, no statement of whether the run is a `cargo test`, whether an Ollama judge is allowed (the §6 rule says tests never reach it), where the corpus lives, or what it costs. The one precedent in the tree — the `#[ignore]`d, env-gated steer-routing eval with its corpus under `scripts/eval/` and a whitelisted README — is not cited, and a new eval README is invisible to git until whitelisted.

**Evidence.** `crates/openalpaca_core/tests/steer_routing_eval.rs:12-18,163` (`#[ignore]` + `OPENALPACA_LIVE_EVAL=1` + `--features live-eval` + a resolvable `llm.toml`); `crates/openalpaca_core/Cargo.toml:13-14`; `scripts/eval/steer_routing/`; `.gitignore` (`*.md` blanket, `!scripts/eval/steer_routing/README.md` whitelisted).

**Smallest fix.** State in slice 1: the harness is an `#[ignore]`d, env-gated eval test in the steer-eval shape, data under `scripts/eval/memory/` (plus its `.gitignore` line); reader and judge models come from the eval's own `llm.toml` (cloud keys by default; a local Ollama only through that explicit opt-in, never in default `cargo test`); who authors the questions, how the tuning split is kept apart, and the expected cost per run.

**Blocks:** slice 1; slice 4.

### G14 — minor — The schema-42 fixture's generation, journal mode and test handling are unstated, and a tidy fixture fails the "bytes unchanged" assertion

**Where:** 04 slice 1 *A minimal real schema-42 fixture*; row 3.5 ("its bytes are unchanged").

**Statement.** `Database::open` registers the vec extension, opens, runs `PRAGMA journal_mode = WAL` and only then `run_migrations`. A fixture committed in rollback mode has its header rewritten before the refusal; opening any fixture in place creates `-wal`/`-shm` beside the committed file (both git-ignored). `crates/openalpaca_storage/tests/` does not exist, and the existing refusal test compares schema and rows, not bytes.

**Evidence.** `crates/openalpaca_storage/src/database/mod.rs:40-63` (extension, open, WAL pragmas before `run_migrations`); `.gitignore` (`*.db`, `*.db-shm`, `*.db-wal`); `crates/openalpaca_storage/src/database/tests.rs:56-83`.

**Smallest fix.** State: the fixture is generated by an `#[ignore]`d test through `Database::open` (so it is already WAL), checkpointed and closed cleanly (no `-wal` left), committed under `crates/openalpaca_storage/tests/fixtures/`; the refusal test copies it to a tempdir, hashes, opens the copy, expects `Err`, re-hashes.

**Blocks:** slice 1 (the fixture); commit 3.5 (its test).

### G15 — minor — `imported ≥ external` leaves the derived-supersession rule between those two origins undecided

**Where:** 03 §4 *Authority ordering*; §6 *Derived supersession* ("at or above").

**Statement.** Whether a hand-dropped or hand-edited file may supersede an explicitly imported note depends on `≥` being `=` or `>`. Slice 1 freezes "the authority ordering" and slice 2 tests "a lower-authority link is reported, not honored"; neither can be written until it is one symbol.

**Evidence.** 03 §4: "`user` > `extracted_user` > `imported` ≥ `external` > `extracted_other`".

**Smallest fix.** Replace `≥` with `>` (an external edit cannot retire an import) or `=`, and say so in both sections.

**Blocks:** slice 1.

### G16 — minor — Three sections disagree on whether import/export ship in v1

**Where:** 03 §2 ("can be imported explicitly"; quarantine "until `memory doctor --repair` or an explicit import"), §4 (origin `imported`), §7 *Capabilities* ("`memory_manage` covers import, export, adopt, reindex and doctor"), §7 *CLI* (no `import`/`export` verb), §9 ("Importing an arbitrary Markdown knowledge base is a separate, bounded feature").

**Statement.** An origin and a capability name import and export; the CLI verb list and §9 exclude them; §2 relies on "an explicit import" as one of two ways out of quarantine. Slice 1 cannot freeze whether `imported` has a producer.

**Smallest fix.** Strike import/export from §7's capability text, mark `imported` in §4 as reserved with no v1 producer, make `doctor --repair` the only way out of quarantine in §2, and freeze that in slice 1.

**Blocks:** slice 1.

### G17 — minor — How a title becomes `<short-title>` in the filename is unspecified

**Where:** 03 §2 (`notes/<uuid>--<short-title>.md`; "the filename's title part is presentation only"); 04 slice 2 confinement bullets.

**Statement.** A raw title carries `/`, `..`, NUL, a leading `.`, NFC/NFD variants and Windows device names. The confinement bullets reject `..`/hidden/control names after the fact instead of defining a grammar that cannot produce them, so a retitle could be refused — or, if the check is skipped on the rename path, escape. `slugify` in the same crate already does exactly this for artifacts.

**Evidence.** `crates/openalpaca_storage/src/store/artifact.rs:144-170` (`slugify`: NFKD-fold to `[a-z0-9-]`, byte-bounded, reserved-name guard).

**Smallest fix.** Pin the filename grammar in the slice-1 note contract as `<uuid>--slugify(title, N).md`, with the UUID as the only identity.

**Blocks:** slice 1.

### G18 — minor — `/v1/memories` error envelope, error codes, and `If-Match` vs body revision are unchosen; "management-only `sync`" has no meaning under one bearer token

**Where:** 03 §7 *Surfaces* ("Edits use `If-Match` **or** a revision (409 on conflict) … management-only `sync`"); 04 row 3.8.

**Statement.** The daemon has two envelopes — the shared `api_error` (`{"error":{"code","message"}}`) and the extension routes' flat `{"error":"<word>"}`, "plan §7's single exception" — and the design picks neither, names no codes, and says "or" between the header and a body field. Every owner-scoped route uses `state.local_user_id`; there is no second auth level for "management-only" to mean.

**Evidence.** `apps/openalpacad/src/routes/mod.rs:131` (`api_error`); `apps/openalpacad/src/routes/extensions.rs:23` (the flat envelope, documented as the one exception).

**Smallest fix.** State: the `api_error` envelope; codes `MEMORY_NOT_FOUND` (404), `MEMORY_REVISION_CONFLICT` (409), `MEMORY_QUARANTINED` (409), `MEMORY_INVALID_INPUT` (400); `If-Match: "<revision>"` header only; "management-only" means "not on any model surface", not a second auth level.

**Blocks:** slice 1 (route freeze); commit 3.8.

### G19 — minor — `kind: episode` has no default write scope

**Where:** 03 §4 ("`kind` covers fact, preference, decision, procedure, reference and episode"); §3 rule 2 (lists five kinds).

**Smallest fix.** Add `episode` to rule 2's project-default list (or the global one).

**Blocks:** slice 1 (the scope matrix that 3.1 tests).

### G20 — minor — `memory_manage` "never granted to the model by default" implies it can be granted, while §3 says its verbs are local-only

**Where:** 03 §3 *Resets* (`adopt` "unavailable to the model and the HTTP API"), §6 (offline management takes the store lock), §7 *Capabilities*.

**Statement.** Capabilities become allowlist entries by name, so a template (a plugin-contributed one included) could list `memory_manage`; no tool would provide it. "By default" contradicts "local-only".

**Evidence.** `crates/openalpaca_core/src/security/sandbox/mod.rs:69-72,120-133` (capability strings are allowlist names).

**Smallest fix.** State that no tool provides `memory_manage` (it names CLI-local verbs only), drop "by default".

**Blocks:** slice 1 (capability names).

### Before slice 2 (the storage library)

### G21 — major — `.history` retention is stated two ways, and the recovery row relies on a copy the mutation table never makes

**Where:** 03 §2 layout ("last 5 service-written revisions, current included"), §6 mutation table row 1 ("the prior revision is copied to `.history`" … "the service's own revision is still in `.history`, which keeps every service-written revision"); 04 slice 2 ("bounded to the last 5 … the current one included"), §7 risk table ("every service-written revision kept in `.history`").

**Statement.** Two sections say "last 5", two say "every". Separately, step 2 copies only the *prior* revision, so the just-written `to_rev` is not in `.history` until the next write — yet the "anything else" recovery branch claims the service's own revision is there, and §2 says "current included". Slice 2's "every recovery-table row with a crash injected at each step" cannot be written until both are decided.

**Smallest fix.** State the cap once (last 5, current included) and delete "every"; amend step 2 to "copy `to_rev` into `.history/<id>/<to_rev>.md` (fsync) before the rename, then prune to 5", so the recovery branch's claim holds at every crash point.

**Blocks:** slice 2.

### G22 — major — No serialization canon: the revision is a byte hash, patches must preserve untouched fields, and the storage crate has no YAML parser

**Where:** 03 §4 *Revisions* ("a hash of the file's exact UTF-8 bytes"; "Untouched Markdown and unknown extension fields are preserved during supported patches"), the illustrative file (a YAML comment inside frontmatter); 04 slice 2 ("Parse and serialize notes, preserving unknown fields"; "No new crate and no new feature: `uuid`, `sha2` and `rusqlite` FTS5 are already there").

**Statement.** Without a canon a no-op patch changes the revision, a client's `expected_revision` computed after re-serialization differs, and an external edit that only reformats reads as a content change. Undefined: frontmatter key order, list style, quoting, timestamp format (the example uses `…T10:00:00Z`; chrono's `to_rfc3339()` emits `+00:00` with sub-seconds), `null` spelling, whether comments survive (they will not under `serde_yaml`), the hash algorithm and hex case. The only YAML crate in the workspace is `serde_yaml` 0.9, a dependency of `openalpaca_core` only; `openalpaca_storage`, where `memory_files` lives, has none, and the plan's "no new crate" line omits the parser entirely.

**Evidence.** `Cargo.toml:43` (`serde_yaml = "0.9"`); `crates/openalpaca_core/Cargo.toml:23` (the only consumer); `crates/openalpaca_storage/Cargo.toml` `[dependencies]` (no YAML crate).

**Smallest fix.** Add to §4: revision = lowercase hex SHA-256 of the file bytes; the service writes frontmatter in §4's field order, unknown keys after them in original order, block-style lists, RFC 3339 UTC seconds with `Z`, `null` for absent optionals; comments are not preserved (drop the one in the example or mark it illustrative); a re-serialization of an unchanged parse is byte-identical (a round-trip test). Add `serde_yaml.workspace = true` to the storage crate and say so in slice 2 (one edge, no new crate).

**Blocks:** slice 2.

### G23 — major — Which FTS5 table each term is run against is ambiguous, and the slice-2 query fixture passes under only one reading

**Where:** 03 §5 steps 3–4; 04 slice 2 test list ("'what editor do I prefer?' recalls 'User prefers vim for editing'").

**Statement.** Step 3 searches the `unicode61` table with a union of terms; step 4 adds the trigram companion "for phrases, exact identifiers and CJK or substrings" — which reads as *not* for ordinary English terms. No stemming is mentioned. Reproduced on SQLite 3.51.0: the note (title "Vim", body "User prefers vim for editing") against `"what" OR "editor" OR "do" OR "i" OR "prefer"` returns **0** rows under `unicode61`, 1 under `porter unicode61`, 1 under `trigram` — and the trigram hit comes only from `prefer` inside `prefers` (`editor` does not hit `editing`). The fixture passes only if every ≥3-character term is also ORed against the trigram table, or a stemmer is added; step 4 as written says neither.

**Evidence.** Probe run by the synthesizer (python `sqlite3`, 3.51.0): `u union -> 0`, `p union -> 1`, `t union -> 1`; per-term trigram: `what 0, editor 0, do 0, i 0, prefer 1`.

**Smallest fix.** State the routing rule in §5: every normalized term of ≥3 characters is ORed against both tables (the trigram companion is not CJK-only), with a lower weight for trigram-only hits; or add `porter` to the `unicode61` tokenizer and say why. Note in slice 2 that the fixture depends on it.

**Blocks:** slice 2.

### G24 — major — `doctor --repair` and `adopt` have no pre- or post-conditions

**Where:** 03 §2 (quarantine "until `memory doctor --repair` or an explicit import"; "empty controls are never silently initialized over existing notes"), §3 *Resets* (`adopt <owner-key>` "re-binds an orphaned partition to the current owner"); 04 slice 2 ("Implement design §6's recovery table and orphan detection"), row 3.8.

**Statement.** What repair may write (a fresh snapshot with every note `external`? tombstones empty? `pending` cleared?), whether the partition leaves quarantine afterwards, and what happens to notes whose files parse but whose ids collide are unstated. For `adopt`: rename `owners/<old>` to `owners/<new>` and the snapshot file, or rewrite the binding only; what if the current owner already has a partition at that path (merge or refuse); the home partition only or every `proj-<old>-*` snapshot too. Slice 2 must implement orphan detection and 3.8 ships the verbs.

**Smallest fix.** Add to §6: repair = rebuild the snapshot from the directory (binding from the path, every parsed note `external`, tombstones empty, `pending` absent, `repaired_at` stamped), refuse on duplicate ids, and lift quarantine; adopt = refuse when the target owner-key directory or snapshot exists, else rename directory and snapshot and rewrite `owner_key`/`owner_id`, for the home partition and every `proj-<old>-*` snapshot in one command with a printed list.

**Blocks:** slice 2 (orphan detection); commit 3.8.

### G25 — major — Notes move from a 0700 `state/` to default-mode content directories, and the design sets no file or directory mode

**Where:** 03 §2 layout and the "Path checks alone do not confine…" paragraph; 04 slice 2 `memory_files` confinement bullets.

**Statement.** Every memory row lives today in `state/openalpaca.db`, and `state/` is created 0700. The new `notes/`, `.history/` and the project partition would be created by `create_dir_all` with the umask default (0755/0644), readable by every other local account. The design's only permission statement is about the same OS identity, a different threat.

**Evidence.** `crates/openalpaca_storage/src/store/mod.rs:96-100` (`state_dir` → `create_private_dir`), `:736-746` (0o700), `:586-597` (`content_dir` → plain `fs::create_dir_all`).

**Smallest fix.** Slice 2 creates `memory/`, `owners/<owner-key>/`, `notes/` and `.history/` with `create_private_dir` (0700) and opens note and temp files with mode 0600 (`OpenOptions::mode`), in both scopes; a Unix test asserts the modes. Add the rule to §2 beside the layout.

**Blocks:** slice 2.

### G26 — major — The project `.gitignore` is written once, at partition creation; a pre-existing file with other content is never corrected

**Where:** 03 §0 MD-7 ("written when the partition is created"), §2 layout; 04 slice 2 ("Write the project partition's `.gitignore` (`*`)").

**Statement.** The store's convention for the two files it seeds is write-only-when-absent, so an implementer following it leaves a pre-existing `<project>/.openalpaca/memory/.gitignore` alone. In the normal case `*` ignores itself, so a well-behaved repository never commits one; but any clone with `.git` becomes a partition root on the first `fact`/`decision` write, and a repository that force-added that file with `!*` (or empty) makes the next `git add -A` stage the owner's notes and `.history`. A deeper `.gitignore` overrides every parent rule, so nothing else protects them. This is an adversarial-clone scenario, but the fix is one comparison and MD-7's "works for existing projects" otherwise holds only for stores nobody else touched.

**Evidence.** `crates/openalpaca_storage/src/store/mod.rs:485-511` (README and `.gitignore` written only `if !exists()`); `crates/openalpaca_core/src/memory/workspace.rs:64-77` (any `.git` is a marker).

**Smallest fix.** On every partition open, `memory_files` reads `memory/.gitignore` and rewrites it atomically when its bytes are not the service's text (it is store-owned); slice 2's tests gain "a pre-existing `.gitignore` with other content is replaced". State it in §2.

**Blocks:** slice 2.

### G27 — minor — Two active notes with the same `fact_key` in one partition have no defined winner, and `fact_key` has no grammar

**Where:** 03 §3 rule 3 (project-over-global for the same key only); §5 step 6 ("Same-key overrides are resolved before budgeting").

**Statement.** Two `memory_store` calls, or an `extracted_user` note beside a `user` note, can leave two active same-key notes in one partition with no `supersedes` link; slice 2's filter stage must pick and the design does not say how.

**Smallest fix.** State: within a partition the highest-authority note wins, ties by newest `updated_at`; the loser is excluded from automatic recall and reported as `conflict` by `memory_search` and `doctor`; `fact_key` matches `[a-z0-9][a-z0-9._-]{0,63}`.

**Blocks:** slice 2.

### Before slice 3 (the cutover)

### G28 — major — Commit 3.5 cannot be green on its own, and its stated evidence is producible only at 3.6

**Where:** 04 §2 rows 3.5–3.7; §7 risk table ("Reviewable commits 3.1–3.10 in order, each green").

**Statement.** At 3.5 the baseline no longer creates `memory`/`memory_fts`/`memory_vec`, but every reader of those tables survives until 3.6/3.7: the 18 storage repository tests and 30 core `memory::` tests open a `Database` and touch `memory`; `test_sqlite_vec_available` asserts `memory_vec` exists; the artifact store's workspace inventory, listing UNION and purge run raw `SELECT COUNT(*) FROM memory` and are asserted by artifact and workspace-route tests — all fail with `no such table: memory` until 3.7. And 3.5's evidence, "opened **with sqlite-vec no longer linked**", is impossible at 3.5 because `ensure_vec_extension()` is removed in 3.6.

**Evidence.** `crates/openalpaca_storage/src/database/tests.rs:215` (`test_sqlite_vec_available`); `crates/openalpaca_storage/src/artifacts/mod.rs:1540,1592-1605,1660,1773` (`UPDATE memory`, `SELECT COUNT(*) FROM memory`, the UNION); `artifacts/tests.rs:153,173,1914`; `apps/openalpacad/src/routes/workspaces/tests.rs:244,1034`; `database/tests.rs:257,268`; `database/mod.rs:23-29,40` (`ensure_vec_extension`, removed by the §3 map row).

**Smallest fix.** Fold 3.5, 3.6 and 3.7's SQL removal into one commit (or order them 3.7-SQL → 3.6 → 3.5), and move the "no sqlite-vec" refusal evidence to the commit that deletes `ensure_vec_extension`; if 3.5 stays separate, its test reads "refused with sqlite-vec still linked".

**Blocks:** slice 3 (the commit order).

### G29 — major — The control snapshot is reachable by the daemon's global `file_write` at HEAD, contradicting §2's claim

**Where:** 03 §2 ("memory tools and generic file writes cannot reach it"; the residual paragraph covers shell/MCP subprocesses only); 04 commit 3.2.

**Statement.** The global builtin registry captures `workspace_root = std::env::current_dir()` at boot, and both launchers start the daemon in the home store, so `file_write`'s root *is* `~/.openalpaca`. `validate_workspace_path` refuses only absolute paths and `..`; `resolve_workspace_path_for_write` confines to that root; `file_ops.rs` blocks only the three persona files; `tool_suggest` puts `file_write` on the main-loop surface. A model can therefore `file_write("state/memory-control/home-<key>.json", …)` and rewrite origins (forging `user`), drop tombstones (resurrecting forgotten ids) or plant `pending`; `MEMORY.md` and `.history/` under the home partition are reachable the same way, and `file_read` of the snapshot discloses the owner-id mapping. The design's admitted residual does not cover this builtin.

**Evidence.** `apps/openalpacad/src/services/tools.rs:51`; `crates/openalpaca_core/src/tools/builtins/file_ops.rs:19,90` (fixed `workspace_root` fields, no per-request override), `:252-273` (persona refusals only), `:278`; `crates/openalpaca_core/src/tools/builtins/helpers/mod.rs:11-24,65-97`; `crates/openalpaca_core/src/orchestrator/intent/tool_suggest.rs:49`; `apps/openalpaca/src/manager.rs:218,228-233` and `apps/openalpaca-gui/src-tauri/src/lib.rs:233,269` (daemon CWD = home store; [06c](06c-evidence-identity-scope-learning.md) A8).

**Smallest fix.** In commit 3.2 add a refusal to `file_write`/`workspace_write` for any path whose first component is `state`, and for `.history/`, `MEMORY.md` and the `.gitignore` under `memory/`, with a test — and reword §2 to "`file_write` refuses `state/` and the partition's own files (3.2); shell and MCP subprocesses remain the stated residual". Say whether same-owner `file_read` of the snapshot is accepted or refused.

**Blocks:** slice 3 (3.2); the snapshot's location should be confirmed in slice 1 with this in view.

### G30 — major (small owner decision) — The Memory view has no placement, no DESIGN_SPEC section, an incomplete file list, and no way to learn that a turn stored or forgot a note

**Where:** 03 §7 *GUI* bullet; 04 rows 3.9 (key files: "views/, lib/api/, types") and 3.10 (`DESIGN_SPEC.md:1609` only).

**Statement.** The GUI's spec of record mentions memory twice, both incidental; the shell has exactly four views and eight settings sections, and the plan does not say whether Memory is a fifth top-level view, a Library tab or a Settings section — adding a nav entry is the kind of "widening" the project rule reserves for the owner. The files that must change (`stores/ui.ts`, `NavRail.tsx`, `App.tsx`, `AppShell.test.tsx`, `views/settings/sections.ts`, `lib/query-keys.ts`) are not in 3.9's list. `ServerEvent` has no memory variant and the design names none, while the GUI invalidates queries by WS event domain — so the view cannot learn that the model stored, superseded or forgot a note mid-turn except by polling. The project's own precedent (U5) builds GUI surfaces from a DESIGN_SPEC section.

**Evidence.** `apps/openalpaca-gui/DESIGN_SPEC.md:697,1594` (the only "memory" hits); `apps/openalpaca-gui/src/stores/ui.ts:27` (`View = "chat" | "work" | "library" | "settings"`); `views/settings/sections.ts:28-37`; `crates/openalpaca_api/src/events/mod.rs` (no memory variant); `lib/query-keys.ts` (invalidation by event domain).

**Decision and fix.** The owner picks the placement (nav entry vs Settings section). Then add a DESIGN_SPEC section for the Memory view and the Persona editor (placement, layout, empty/conflict/undo copy) or rule in 3.9 that design §7's list is the spec; list the shell, store, section and query-key files under 3.9; and decide whether a `memory_changed` `ServerEvent` (with its `event_bridge.rs` and `events/persistence.rs` arms and a `docs/api/` regen) ships in 3.8, or the view polls.

**Blocks:** slice 3 (3.8 if the event ships; 3.9).

### G31 — minor — No table of which memory config keys go, stay or are new; the checked-in `daemon.toml` and its release template are not in the removal map; the completeness regex misses `decay`/`supersession`

**Where:** 03 §6 ("bounded per turn and per run by count as well as cost" — no numbers, no keys); 04 §3 map row `daemon_config/orchestrator.rs` and the completeness search; rows 3.4, 3.6.

**Statement.** `MemoryConfig` mixes conversation-summary and profile-confidence knobs that must stay with the supersession/Jaccard/decay knobs that go; `fts_jaccard_threshold` is never named; the map's registry and CLI line ranges cover only part of the rows; `config/daemon.toml` and `scripts/release/templates/config/daemon.toml` still carry `supersession_distance_threshold`, `[orchestrator.memory.decay]` and `[server.embedding_indexer]`, and `DaemonConfig` has no `deny_unknown_fields`, so they would be silently ignored (the owner's instruction forbids a warning, so they must be removed from the shipped files); the surviving extraction keys (`extract_every_n_turns`, `extract_max_daily_cost_usd`, `task_extract_*`, `memory_confidence_threshold`) are not said to be kept, renamed or replaced, and the new per-turn and per-run **count** bounds have no key or default.

**Evidence.** `crates/openalpaca_core/src/daemon_config/orchestrator.rs:202-230` (`MemoryConfig`), `:253-274` (decay), `:282-290` (extraction keys); `config/daemon.toml:50-62,173`; `crates/openalpaca_storage/src/config_schema/registry.rs:393-459,751-764,823-853` (the map cites `:391-404,750-775,821-862`); `apps/openalpaca/src/commands/daemon_config_cli/mod.rs:141-168,279` (the map cites `:139-144,277-287`); `crates/openalpaca_core/src/daemon_config/mod.rs:71-73`.

**Smallest fix.** A config-key table in 3.4 (kept / removed / new, with defaults and registry rows, e.g. `extract_max_items_per_turn`, `task_extract_max_items_per_run`); add both `daemon.toml` files to the removal map with the exact keys; add `decay|supersession_distance|fts_jaccard` to the completeness regex.

**Blocks:** slice 3 (3.4, 3.6); the count defaults belong in the slice-1 freeze.

### G32 — minor — Whether a factory reset deletes `state/memory-control/` and `state/cache/memory-index-v1.sqlite` is unstated, and `adopt` depends on the answer

**Where:** 03 §2 (snapshot-loss rules), §3 *Resets*; 04 row 3.7 ("the reset preview names memory files as surviving"), §4.

**Statement.** Both new files sit under `state/`, "the machine's", beside the database the reset deletes, so an implementer may reasonably delete them — after which every orphaned partition has no snapshot, is quarantined, loses its tombstones, and `adopt` has no owner mapping to re-bind (§2: empty controls are never initialized over existing notes). The preview's *Untouched* block still calls `state/cache/` "the local embedding model (~1 GB)", and the survivor tests pin nothing about the new entries.

**Evidence.** `apps/openalpaca/src/commands/config_factory_reset.rs:98-100` (deletes the DB trio and two config files), `:195-209` (the preview's *Untouched* list); `config_factory_reset/tests.rs:233,281`.

**Smallest fix.** State in §3 and 3.7 that the reset leaves `state/memory-control/` and the index untouched, name both in the preview (rework the `state/cache/` line), say the index is rebuilt on next start, and have the survivor test assert a seeded snapshot survives.

**Blocks:** slice 3 (3.7); the sentence belongs in §3 now.

### G33 — minor — The project-store README says memory is "deliberately *not* git-ignored", contradicting MD-7, and the plan refreshes only the home README

**Where:** 04 row 3.7 and §3 map row `store/mod.rs` (home README + `home-6.txt` digest only); 03 §0 MD-7 ("The seeded README text is refreshed").

**Statement.** The project README row for `memory/` says the opposite of MD-7; project READMEs are written only when absent and have no superseded-digest tripwire, so every existing project keeps the wrong sentence after the cutover.

**Evidence.** `crates/openalpaca_storage/src/store/mod.rs:925` ("| `memory/`, `skills/`, `config/` | reserved … deliberately *not* git-ignored |"); `:485-487` (write-if-absent); `superseded_readmes/` holds `home-1..5` only.

**Smallest fix.** 3.7 rewrites the project README row too, and either adds a project-digest tripwire or states that existing projects keep the old text.

**Blocks:** slice 3 (3.7).

### G34 — minor — The workspace move's snapshot re-key has no ordering relative to `rename(2)` and no recovery rule

**Where:** 03 §2 (key over the canonical root; "A workspace move renames the snapshot along with the rebinding"), §3 *Workspace move*; §6 recovery table (no move row).

**Statement.** `PATCH /v1/workspaces` is preflight → one DB transaction → `rename`. A crash between the rename and the snapshot rewrite leaves a partition at the new root with no matching snapshot (quarantined, never re-initialized over notes) and an orphaned old-key snapshot; neither the recovery table nor `doctor --repair` names the case.

**Evidence.** `apps/openalpacad/src/routes/workspaces.rs:499-504` (`rebase_project` then `move_project_store`); `crates/openalpaca_storage/src/store/project_move.rs:77` (`fs::rename`).

**Smallest fix.** Specify: write the snapshot under the new key before the rename and delete the old key after; give `doctor` a rule for a snapshot whose bound root is gone while its directory exists at the new root.

**Blocks:** slice 3 (3.7).

### G35 — minor — "The profile is injected once" does not say which of the two injections survives, and the compose-layer files the query-text cache lives in are not in 3.3

**Where:** 03 §5; 04 row 3.3 (key files: `prompt_ctx/sources/memory.rs`, `manager.rs`, `runner/lead_agent/mod.rs`, `user_profile.rs`).

**Statement.** The profile is rendered by the persona layer into the static system prompt (`user_budget`, priority High) and again by `UserProfileSource` (a budgeted `SystemMessage` section, priority Normal); they differ in budgeting and position, and the lead passes `user_document: None`, so the choice changes the lead's prompt. The Layer-3 cache keyed on the fingerprint with `memory_retrieval_hash` zeroed lives in `compose/`, which 3.3 does not list. Two critics recommended opposite survivors, which is the point: the document must choose.

**Evidence.** `crates/openalpaca_core/src/compose/persona.rs:108-111,172-181`; `crates/openalpaca_core/src/prompt_ctx/sources/user_profile.rs:57-63`; `crates/openalpaca_core/src/compose/mod.rs:389-431`; `memory_retrieval_hash` zeroed at `orchestrator/query_handler/simple_query_handler.rs:550,1050`, `runner/lead_agent/prompt.rs:244`, `runner/lead_agent/tools.rs:506` (see `workflow_context.rs:6`).

**Smallest fix.** Name the survivor in 3.3 and list `compose/persona.rs`, `compose/mod.rs`, `compose/dynamic_context.rs` and the zeroing callers among its key files.

**Blocks:** slice 3 (3.3).

### G36 — minor — "The user's own words" must be pinned to the typed message, not the model input that carries attachment text

**Where:** 03 §0 MD-2, §6 (an active `extracted_user` note from "the user's own words"); 04 row 3.4 (input unspecified; no attachment test).

**Statement.** The model-facing message on an attachment turn carries the files' extracted text (U2). Today's extraction happens to read the typed text only; a rewrite that feeds it what the model saw lets an uploaded or pasted document mint an *active* preference note.

**Evidence.** `crates/openalpaca_core/src/orchestrator/handlers.rs:28-31,49,352,389-393` (`request.content` is the intent source; extraction receives `intent_source_content`); `crates/openalpaca_core/src/orchestrator/handler_attachments.rs:284` (extracted text inlined for the model).

**Smallest fix.** 3.4 states the extraction input is `request.content` and adds a test where an attachment's extracted text contains a preference and yields at most a `candidate`.

**Blocks:** slice 3 (3.4).

### G37 — minor — Nothing refuses the `"system"` sentinel (or a `provider:id` string) as an owner, so API-originated runs would share one ownerless partition

**Where:** 03 §3 (owner identity "from the authenticated request or agent runtime"; rule 4); 04 commit 3.1 scope matrix.

**Statement.** A `/v1/command process` turn runs as `Principal::System`; `start_workflow` stamps `task.created_by = "system"`; the lead and every subagent carry `owner_id: Some("system")`, and task-output extraction writes for that owner. Under the design the service would mint `home-<sha256("system")>`, one partition written by extraction and readable by any worker with `memory_read` on such a run, belonging to no human.

**Evidence.** `crates/openalpaca_core/src/tools/registry/mod.rs:149-158` (`created_by()`); `apps/openalpacad/src/routes/command.rs:73-98`; `crates/openalpaca_core/src/tools/builtins/start_workflow.rs:119`; `crates/openalpaca_core/src/runner/lead_agent/mod.rs:257-263`, `runner/lead_agent/tools.rs:338-344`; `crates/openalpaca_core/src/orchestrator/dispatcher/memory.rs:13-59`, called from `dispatcher/lead_agent.rs:1098`; `001_baseline.sql:15` (`global_user` is the identity table).

**Smallest fix.** `MemoryAccessContext` construction accepts only an id that is a `global_user` row; anything else yields "no owner" (no recall, no extraction, tools refuse), and 3.1's scope matrix gains "a System-principal workflow and its subagents have no partition".

**Blocks:** slice 3 (3.1).

### G38 — minor (scope nod) — The design promises a cutover step that inventories `USER.md` and proposes moving extracted lines to notes; no commit builds it

**Where:** 03 §6 *`USER.md`* bullet ("Cutover inventories preferences already extracted into `USER.md` and **proposes** moving them to notes"); 04 row 3.4.

**Statement.** Row 3.4 covers the writer stopping, the CLI verb and onboarding only; the surface an inventory would need is unnamed. `USER.md` survives the factory reset, so the owner's extracted lines will exist after the switch-over.

**Evidence.** `apps/openalpaca/src/commands/config_factory_reset.rs:207` ("the rest of your configuration — … orchestrator/").

**Smallest fix.** Either assign the inventory to 3.4 as a CLI listing (`openalpaca persona show user --extracted`) or delete the bullet and add one line to the §4 switch-over note telling the owner to review `USER.md` by hand.

**Blocks:** slice 3 (3.4).

### G39 — minor — "Recently learned" and `undo` have no window, ordering or edit rule

**Where:** 03 §0 MD-2, §6, §7 (`recently-learned` route, CLI `undo`, GUI list); 04 row 3.8.

**Statement.** The route, the verb and the list are all defined by the phrase "a recently learned list with one-step undo": no window (days or count), no ordering, and no rule for a note the user has since edited (does an edited `extracted_user` note become `user` and leave the list, and may `undo` still forget it?).

**Smallest fix.** Define: recently learned = active notes with origin `extracted_user`, newest first, `?since=` (default 7 days) or `?limit=` (default 50); an edit through a user-directed surface re-records origin `user` and removes it; `undo` = forget by id + expected revision, 409 once the origin is no longer `extracted_user`.

**Blocks:** slice 3 (3.8, 3.9).

### G40 — minor — If 3.2 edits the shipped agent templates, the owner's post-reset install never receives them

**Where:** 04 row 3.2 (key files include `config/agents/*.md`); §4 switch-over note.

**Statement.** Content seeding is gated per directory — an existing `config/agents/` is skipped whole — and the factory reset leaves `config/agents/`, `config/skills/` and `config/orchestrator/` untouched. Six templates grant `memory_read` today (not seven, as one report said). If 3.2 renames or adds capability grants, the owner's real install keeps the old files and the switch-over note does not say so.

**Evidence.** `apps/openalpacad/src/bootstrap/content.rs:105`; `apps/openalpaca/src/commands/config_factory_reset.rs:207`; `config/agents/{lead_agent,general_agent,code_agent,planning_agent,review_agent,explore_agent}.md`.

**Smallest fix.** Either state in 3.2 that no template changes (`memory_read` keeps its name and meaning), or add a step to the §4 note: delete `config/agents/` before the first start so the new build re-seeds it.

**Blocks:** slice 3 (3.2, 3.10).

---

## (C) Decide while building — 9 gaps

### G41 — minor — The completeness regex forbids `MemoryKind` and `MemoryScope`, the obvious names for the new kind/scope enums

**Where:** 04 §3 completeness search; 03 §7 ("optional kind/scope enum").

**Smallest fix.** Remove `MemoryKind|MemoryScope` from the regex (the old types die with `models/memory.rs`, which `MemoryRepository` already covers) or name the new enums in §4 (e.g. `NoteKind`, `NoteScope`).

### G42 — minor — The slice-2 exit gate "passes the bench" names a benchmark runner the repository does not have

**Where:** 04 slice 1 *Benchmark harness*, slice 2 exit, §5 *Warm retrieval*.

**Evidence.** No `benches/` directory, no `criterion`/`divan`/`[[bench]]` in any manifest; `cargo bench` with `#[bench]` needs nightly and the toolchain is pinned to 1.93.0 stable.

**Smallest fix.** Define it as an `#[ignore]`d test under `memory_index`, run with `cargo test --release -p openalpaca_storage -- --ignored bench_`, printing p50/p95 and the machine; no new dependency.

### G43 — minor — Raw SQL against `memory` survives the completeness search, and the `memories` count's GUI/CLI consumers are not in any commit row

**Where:** 04 §3 completeness search and the `artifacts/mod.rs:1502-1552` row; rows 3.7, 3.9.

**Evidence.** `crates/openalpaca_storage/src/artifacts/mod.rs:1592-1605,1660,1773` (workspace inventory, listing UNION, purge — outside the cited range; they compile clean and fail at runtime with `no such table`); `apps/openalpaca-gui/src/lib/api/workspaces.ts:25,57`, `views/settings/ConnectionSection.tsx:631-678` ("{memories} memories still point there"), `views/settings/SettingsView.test.tsx:56,571-580`; `apps/openalpaca/src/commands/store.rs:112-125`.

**Smallest fix.** Add `\bFROM memory\b|\bINTO memory\b|UPDATE memory\b` to the search, cite the three extra ranges in the row, and list the four GUI/CLI files under 3.7 so the field, copy and test go in the same commit.

### G44 — minor — The 3.5 refusal text must keep the phrases the existing test pins, and its memory-redesign wording is true only for version 42

**Where:** 04 §4 *Refusal text*; row 3.5.

**Evidence.** `crates/openalpaca_storage/src/database/tests.rs:59-110` asserts "Unsupported legacy schema version {version}", the absolute path, and "schema version {BASELINE_VERSION}"; `database/mod.rs:126` is one guard for every version in `1..BASELINE`, so after the re-squash a 41 database would be told "the memory storage was replaced", which is not why it is refused.

**Smallest fix.** Keep "schema version 43" in the reworded text (or say the test changes), and branch the message on `current_version == 42` with the existing legacy wording below it.

### G45 — minor — The eleven CLI verbs have no syntax

**Where:** 03 §7 *CLI*; 04 row 3.8.

**Statement.** What `add` takes (flags, stdin or `$EDITOR`), how `edit` and `forget` obtain the mandatory `expected_revision`, where `doctor` without `--repair` runs (no `/v1/memories` doctor route is listed), and whether `sync` calls the route or the local index.

**Smallest fix.** A verb/flag table in §7: `add --kind --scope --title [--alias…] [body|-]`; `edit <id>` (GET, `$EDITOR`, PUT with the fetched revision); `forget <id>…` (shows title + revision, confirms); `doctor` local read-only beside a running daemon; `doctor --repair`/`adopt` take the singleton lock; `sync` = the POST route.

### G46 — minor — Partition-key and owner-key derivation inputs are underspecified

**Where:** 03 §2 ("sha256(canonical project root)"; "first 32 hex characters of sha256(owner_id)").

**Evidence.** `crates/openalpaca_core/src/memory/workspace.rs:80-86` (`workspace_id_from_root` = `canonicalize().to_string_lossy()`, which on Windows carries `\\?\`).

**Smallest fix.** Define both as lowercase hex SHA-256 over the UTF-8 bytes of, respectively, the canonical partition-directory string (per G8) and the owner id; first 16 and first 32 characters.

### G47 — minor — The 0.3 stub `Embedder` must emit 768-dimensional vectors, or the characterization test passes on the unchanged code

**Where:** 04 slice 0.3.

**Evidence.** `crates/openalpaca_llm/src/embedder.rs:20-24` (the trait); `memory_vec` is `float[768]`; `crates/openalpaca_core/src/memory/task_extraction/mod.rs:401-418` swallows a failed KNN with `unwrap_or_default()`, so a non-768 stub yields "no similar" and both notes stay active before the fix. The plan's negative-control rule would catch it, but only after a wasted cycle.

**Smallest fix.** State that the stub emits 768-dim vectors, lives `#[cfg(test)]` in `memory/task_extraction/tests.rs`, and that the negative control first asserts `PersistResult::Superseded` on the unchanged code.

### G48 — minor — "Outbound networking denied" names no mechanism and no evidence to record

**Where:** 04 slice 3 gates, slice 4.

**Smallest fix.** Name the tool (a `sandbox-exec` profile with `(deny network-outbound)` wrapping `openalpacad`, or a pf rule) and the evidence to keep (the daemon log showing no download, the CLI round-trip transcript).

### G49 — minor — Plan §7 points at a document that exists only on this machine

**Where:** 04 §7 ("The paused LLM-providers design (`tasks/llm-simplify/design.md`) … Regenerate that design's survey maps afterwards").

**Evidence.** `git check-ignore -v tasks/llm-simplify/design.md` → `.gitignore:98 *.md`; `git ls-files tasks/llm-simplify` is empty; the directory holds `design.md`, `map-callside.md`, `map-router.md` untracked.

**Smallest fix.** Whitelist `tasks/llm-simplify/*.md` in the next docs commit, or state the collision (which files, which decision) in §7 itself and drop the regenerate instruction.

---

## Summary by bucket

| Bucket | Count | Blocker | Major | Minor | Ids |
|---|---|---|---|---|---|
| (A) before slice 0 | 5 | 2 | 1 | 2 | G1–G5 |
| (B) before their slice | 35 | 0 | 17 | 18 | G6–G40 (slice 1: G6–G20; slice 2: G21–G27; slice 3: G28–G40) |
| (C) while building | 9 | 0 | 0 | 9 | G41–G49 |
| **Total** | **49** | **2** | **18** | **29** | |

Majors by id: G3 in (A); G6–G13 (slice 1), G21–G26 (slice 2), G28–G30 (slice 3) in (B).

**Where an owner decision is needed rather than a document fix:** G2 (hard), G30 and G12 (small), G38 (a scope nod). Everything else can be written by the design's author from what the design already implies and the code at HEAD.

---

## The four critics' ready verdicts

The reports relayed to the synthesizer carried the 65 gaps and no separate verdict field, so no critic's verdict can be quoted verbatim here. What each lens's reports amount to, counted as filed (before merging):

- **Correctness** (12 reports: 0 blocker, 3 major, 9 minor): not ready — the three majors are the two unserved GUI promises (notes used, the Persona editor) and the unplaced Memory view; the rest are removal-map and reset-preview omissions.
- **Contracts** (23 reports: 1 blocker, 10 major, 12 minor): not ready — "slice 1 is told to freeze contracts that rev 3 does not contain", with the snapshot, tool, route and access-context shapes, `.history` retention, the serialization canon, FTS term routing and the supersede shape as the majors.
- **Security** (12 reports: 1 blocker, 5 major, 6 minor): not ready — "a main-loop tool call is an explicit remember and gets origin `user`", so prompt injection reaches the top authority; plus the unannotated destructive tools, default file modes, the write-once `.gitignore`, unchecked path components and the 0.5 message's remedy.
- **Delivery** (18 reports: 0 blocker, 6 major, 12 minor): not ready — commit 3.5 cannot be green alone, baseline B cannot run under the isolation rules, the evaluation has no harness, and the GUI commit has no spec; the branch premise is stale.

All four lenses answer the question the same way: the design and plan are close, internally consistent on the settled decisions, and not yet implementable as written.
