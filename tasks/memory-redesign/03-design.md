# Proposed native Markdown memory design

**Status: rev 7.8 (2026-09-27): LQ17's revision answers its review ([09](09-learning-design.md) rev 3.37) — §5 step 4's routing is amended (the word table's text and phrases split at script boundaries; no prefix for a term carrying an unsegmented character or for a stop word; the prefix terms searched again as whole words in the header when the word table's query fills its cap; the body trigram table searched only for terms carrying an unsegmented character), and §7c Q9 and Q12 with it; one of the eight proxy totals loses a row, for the lead to rule on; rev 7.7 (2026-09-27): LQ17's index layout revision ([09](09-learning-design.md) §6.5, rev 3.36) — §5 step 4's term routing (a `unicode61` prefix query; trigram over titles, aliases, tags and the text of scripts `unicode61` cannot split), §7a's `MatchReason` gains `prefix`, §7c Q9 and Q12 amended; designed and measured on a prototype, not yet built; rev 7.6 (2026-09-27): the L8 product slice's repair round 2 — the chat model's edit suggestion cites what the note it copies cites and is dated the day it was saved, a forget withdraws the suggestions naming the forgotten note (§4, §6), confirming in chat is stated as rule 5 reads since LQ11 (§4, §7), §7a's error codes gain `MEMORY_INDEX_REBUILDING`, and rev 7.4's record is corrected ([09](09-learning-design.md) rev 3.35); rev 7.5 (2026-09-27): the L8 product slice's repair round 1 corrects and completes rev 7.4 — §4 and §7a state what a chat model's store meets under a saved note's key, what stays direct, an exact duplicate, a suggestion's empty `superseded` and the in-chat card's wording ([09](09-learning-design.md) rev 3.34); rev 7.4 (2026-09-27): the owner's decisions of 2026-09-26 on the L8 evaluation — LQ11 makes a chat model's replacement of a note the user saved a **suggestion** (§0, §4, §7, §7a `memory_store`/`memory_update`, §7c R4 on the tool path), LQ13 unwraps an extraction answer, LQ14 keeps the chat model's own keys from the extractor, LQ16 keeps R3 with a bulk stat — and the scale slice's open items (a rebuild reads each partition under its writer slot; no read a person or a model waits on waits out a rebuild, the timeline refusing `index_rebuilding`); see the revision log and [09](09-learning-design.md) rev 3.33; rev 7.3 (2026-09-26): the scale slice's repair round makes two §5 mechanisms of rev 7.2 hold as written — damage met by any index read owes the rebuild, and a read overlapped by a rebuild reads again — with no contract or ruling changed; rev 7.2 (2026-09-26): three §5/§6 mechanisms change shape (S3's sweep of forgotten files, S6's rebuild timing, S7's time box on the reply), no contract or ruling — see the revision log; rev 7.1 (2026-09-25): §7a's `memory_confirm` contract names its `invalid_input` refusal; slice 4 is in progress, not "not started". Rev 7 (2026-09-25): Learning v2 is folded — every row of [09](09-learning-design.md) §11 (slices L1–L5, L10a–L10c and the residue slice's R6, with the lead's two rulings of 2026-09-25 in §7a), each at the section it amends and citing the 09 revision that introduced it; the Learning v2 slices are implemented (09's status line) and its evaluation (L8) is running; rev 6 (2026-09-25): §3, §4, §6 and §7a gain the chat memory commands' user-directed surface (MD-13, [09](09-learning-design.md) §12, slice L9); rev 5 (2026-09-24): Learning v2 in design (MD-10…MD-12, [09](09-learning-design.md)); the owner has answered MD-1…MD-9 (§0) and the three decisions the rev-3 audit raised (G2, G30, G38); slices 0–3 are implemented (see the [implementation plan](04-implementation-plan.md)'s revision log), slice 4 is in progress — its evaluation report, [08](08-evaluation-report.md), is a draft (D measured, the gate passed; the post-fix D′/E′ runs are pending) and records the bundle inspection; the packaged, network-denied verification and handing the owner the switch-over note are not done; the slice-3 rulings are §7c's last table (R1–R9).** Rev 4 applies the [audit of rev 3](07-audit-gaps.md) — 49 verified gaps, G1–G49 — and adds **§7a, the frozen contracts** that slice 1 could not freeze from rev 3. Rev 3 applied the owner's answers and the instruction that the old engine be removed completely. Rev 2 applied the [review](05-review.md). Every change traces to evidence in [06a–06d](05-review.md#method) or to a gap in 07, and the revision log at the end lists which gaps each change closes. See also [the audit](01-code-audit.md), [framework research](02-framework-research.md), [filesystem research](02-filesystem-research.md) and the [implementation plan](04-implementation-plan.md).

## 0. Owner decisions

Rev 1 adopted these silently. They are product decisions, so they went to the owner (project rule: a default that reverses an accepted design or widens a component's job is a decision).

**Answered by the owner on 2026-09-23.** Seven followed the recommendation: MD-1, MD-2, MD-3, MD-4, MD-5, MD-7 and MD-8. Two did not, and the rows below record what was chosen:
- **MD-6: refuse and reset.** Existing data is not converted.
- **MD-9: everything at once.** The GUI ships with the cutover.

**Also decided (owner, 2026-09-23):** "the old memory system with the 768 sql design should be completely removed too, since the app is never released." Nothing of the old engine survives the cutover:
- `memory`, `memory_fts` and `memory_vec float[768]`, and their triggers
- the repositories, models, KB importer and chunker
- the embedder, the indexer and decay
- their config keys, CLI options and docs

There is **no compatibility code** for it: no conversion, no leftover-key warnings, no legacy-cache cleanup command. The factory reset that MD-6 requires already deletes `llm.toml` and restores `daemon.toml`, so no old key can survive to be warned about.

**Decided on the rev-3 audit (owner, 2026-09-23).** Three of the audit's gaps were decisions rather than document fixes. Each is written into the section it governs; this is the record:
- **G2 — a model tool call never mints origin `user`.** `MemoryAccessContext` carries `surface: UserSurface | ModelTool` (plus the internal `Extraction`, §3); only the API, the CLI and the GUI write origin `user`; a `ModelTool` `memory_store` or edit records `extracted_user`; `confirm` is refused by the tool with a message naming the app's Memory view and `openalpaca memory confirm`; a `ModelTool` supersede of a `user`-origin note is reported as a conflict, not applied. "User-directed surface" is defined once, in §4. *(Decided by the owner on 2026-09-23. Amended by LQ11, 2026-09-26, rev 7.4: such a supersede — and a store under that note's `fact_key`, and an edit of it — is written as a **suggestion** the user confirms; it is still not applied, and a tool call still never mints `user` or confirms.)*
- **G30 — the Memory view is a fifth top-level view** in the side nav, beside Chat, Work, Library and Settings, **and a `memory_changed` `ServerEvent` ships with the routes** (3.8) — persisted nowhere, like `daemon_shutting_down` — so the view invalidates its queries on it instead of polling. *(Decided by the owner on 2026-09-23.)*
- **G38 — no `USER.md` inventory command.** The cutover does not inventory or propose moving the lines the background writer extracted into `USER.md`; the switch-over note (plan §4) tells the owner to review the file by hand. *(Decided by the owner on 2026-09-23.)*

**Ruled by the session lead (2026-09-23), recorded as a ruling, not an owner decision — G12:** baseline B is measured in slice 1 in the developer's scratch store with a one-time, explicitly authorized download of the E5 model into that scratch store's `state/cache/fastembed`; the download is part of B's recorded conditions; it is a developer benchmark, not a test, so "tests never reach `localhost:11434`" and "no network in tests" are untouched. If the download is refused, the gate is against A with a −10 pp margin and the report says so (plan §5).

| Id | Question | Recommendation (what rev 2 is written for) | Alternatives and their cost |
|---|---|---|---|
| **MD-1** | Retire embeddings and sqlite-vec with **no** optional semantic path in v1? | **Yes.** Every consumer is memory ([06b](06b-evidence-embedding-inventory.md)). The removal drops 97 of the daemon's 465 dependency packages (96 for `fastembed`, 1 for `sqlite-vec`), the ONNX Runtime library that `ort-sys` fetches at build time (about 67 MB unpacked), and the ~1 GB first-boot model download that the seeded `llm.toml` switches on for every install. | Keep an optional embedder behind a flag. That preserves paraphrase recall but keeps the whole dependency chain in the build. |
| **MD-2** | What does **automatic** learning produce? | **Decide by source role.** A preference or fact stated in the **user's own words** becomes an **active** note with origin `extracted_user`. It has lower authority than an explicit save, never overrides a user-authored note, and appears in a *recently learned* list with one-step undo. Anything inferred from assistant text or tool output stays a **candidate**, outside recall until confirmed. | Rev 1: every extraction is a hidden candidate until reviewed. That is safer, but the assistant stops learning until a review surface ships ([06c](06c-evidence-identity-scope-learning.md) A6). |
| **MD-3** | What happens to the background `USER.md` writer? | **It stops writing learned facts into `USER.md` at cutover.** Learned preferences become memory notes. In the **same** slice, an explicit `USER.md` editor ships (`openalpaca persona edit user` and Settings → Persona), and onboarding completion stops depending on the background writer. `update_persona` stays during bootstrap. | Keep the writer. Two stores then hold learned preferences, and `USER.md` keeps its system-prompt authority for inferred text. |
| **MD-4** | Group chats: rev 1 blocked recall in "group lanes". | **v1 keeps today's lane model.** No group lane exists: a connector lane mixes every DM and group on that platform, and core cannot tell them apart ([06c](06c-evidence-identity-scope-learning.md) A1–A3). Group privacy becomes its own *connector audience* design, covering `USER.md`, transcript, notices and memory together. | Block recall on every connector lane. That also switches recall off in the owner's own DMs. |
| **MD-5** | Which workspace scopes memory? | **The project root found from the request workspace**, by the marker walk memory uses today (`.openalpaca`, else `.git`; `crates/openalpaca_core/src/memory/workspace.rs`). The project store is created lazily on the first project write, as artifacts do. A directory with no marker is global. The daemon's own CWD never selects a project. The CLI sends its CWD on every turn, so rev 1's rule would scatter partitions into arbitrary directories. This **extends ADR-031**, whose CWD-derived `workspace_id` "keeps doing memory scoping". | Rev 1: any explicit request workspace, the CLI's CWD included. An earlier rev-2 draft required an *existing* store, which splits one project's facts before and after its first artifact creates the store (`artifact_write.rs:53-57`). |
| **MD-6** | What happens to existing schema-42 memories (the owner's real store has them)? | **Answer: refuse and reset.** The baseline is re-squashed to 43 without memory tables, and a schema-42 database is refused like 1–41. The owner runs `openalpaca config reset --factory`, which also removes tasks, sessions, usage and `llm.toml` with its API keys. The refusal still works without sqlite-vec ([06d](06d-evidence-vec0-spike.md) Q1). Plan §4 has the refusal text and the switch-over note. | *Recommended, not chosen:* migration 043 in the new build (backup, export to Markdown, stub-`vec0` retirement), keeping every non-memory row. |
| **MD-7** | Is project memory git-ignored? | **Yes, by a self-contained `.gitignore` (`*`) inside `<project>/.openalpaca/memory/`**, written when the partition is created and re-asserted on every open (§2). It works for existing projects and never edits the user's ignore rules. The seeded README text is refreshed. | Today's stated stance: memory is committed with the project. |
| **MD-8** | Fix the current engine's **data-loss** behaviour before the replacement lands? | **Yes, as slice 0**, when the cutover is more than about two weeks away. The fixes: <br>- stop decay pruning <br>- turn off similarity-based replacement for **every** caller, keeping exact-hash dedup only <br>- scope the lead's injection to global when there is no workspace (F1) <br>- stop `config reset` deleting the owner id (a bug) <br>- refuse a database whose schema is newer than the build knows | Wait. Memories in the owner's live store keep being pruned or replaced in the meantime ([05](05-review.md) finding 2). |
| **MD-9** | Which management surfaces ship with the cutover? | **Answer: everything at once.** The API, the CLI and the GUI Memory view (with the Settings → Persona editor) all ship in the cutover. | *Recommended, not chosen:* API and CLI with the cutover, and the GUI in the next slice. |

### Learning v2 — owner decisions of 2026-09-24

After slice 3, the owner judged the system "good enough as a prototype" and asked for automatic learning to become the priority, because users change memory through chat, not by hand. The group-chat audience work (MD-4) stays out. Evaluation runs only once everything below is in place. Answered 2026-09-24, all as recommended:

| Id | Decision |
|---|---|
| **MD-10** | **Cadence:** every turn is queued; extraction runs over *all* turns since its last run when the conversation has been idle about 2 minutes, when 10 new turns have accumulated, when the user switches conversations, and before the daemon stops. Nothing is skipped, messages are no longer cut to 800/400 characters, and a reply is never delayed. |
| **MD-11** | **Replacement:** when learning finds that a new statement replaces an older note, the replacement is **automatic when the older note was itself learned** (shown in *recently learned* with one-click undo), and a **suggestion the user confirms when the older note was saved by the user** — G2 holds: nothing learned silently overrides a user-authored note. |
| **MD-12** | **Confirming a suggestion in chat:** the assistant may raise a suggested replacement, and the user answers on the same Approve/Deny card tool confirmations use; the Memory view and the CLI keep it too. The confirmation is the human's click, not the model's say-so. |
| **MD-13** | **Chat memory commands** (owner, 2026-09-24): `/remember`, `/forget`, `/memories`, handled by the daemon with no model, on attended app/CLI turns only; `/remember` saves with origin `user` because the human typed it. No memory skill. Design: [09 §12](09-learning-design.md). |
| **MD-14** | **Layered memory tools** (owner, 2026-09-24, from claude-mem's `mem-search`): `memory_search` returns a compact index by default with date filters and a sort order; `memory_read` fetches up to 10 ids at once; a new `memory_timeline`. [09 §13.1](09-learning-design.md). |
| **MD-15** | **Index-first recall each turn**: pinned notes, the top 2 excerpts and an index of up to 8 more; `recall_mode` keeps excerpt recall selectable and the evaluation measures both. [09 §13.2](09-learning-design.md). |
| **MD-16** | **Conversation summaries** as `episode` notes after a quiet period, a switch or shutdown; findable by search and timeline, never injected automatically; forgotten with their conversation. [09 §13.3](09-learning-design.md). |

### L8 — owner decisions of 2026-09-26

Taken from the L8 evaluation's diagnosis ([09](09-learning-design.md) §10, where LQ11–LQ17 are listed with their evidence; built in the L8 product slice, 09 rev 3.33):

| Id | Decision |
|---|---|
| **LQ11** | **A chat model's replacement of a note the user saved, edited by hand or imported is a suggestion** the user confirms (§4). Before it, a replacement of a `user` note was `authority_conflict` — and the model then stored a second, contradicting active note — while one of an `external` or `imported` note was applied at once (rev 7.5). |
| **LQ13** | **An extraction answer is unwrapped**: `<think>` blocks and prose around it stripped, a bare array of items read as the items; nothing is accepted that a well-formed answer would not carry, and a cut answer is still reported as cut. |
| **LQ14** | **A `fact_key` only the chat model's `memory_store` coined is not offered to the extractor**; the note stays searchable and recallable. |
| **LQ16** | **Ruling R3 stays** (a stat scan before every read), made cheaper by a bulk directory stat where the platform has one (§7c R3). |

LQ12 (learning metrics by surface) is the evaluation's; LQ15 (the design point is about 30K notes, 100K later) and LQ17 (an index-layout revision, a slice of its own) are recorded in 09 §10. LQ17's revision is [09](09-learning-design.md) §6.5 (revs 7.7–7.8: §5 step 4, §7a `MatchReason`, §7c Q9 and Q12).

Also asked by the owner and adopted as scope: the extractor proposes **aliases (including the other language) and a `fact_key`**, reusing existing keys; every agent template gets memory **search** (`memory_read`); the recall header shows validity dates; a subagent's notes reach *recently used*; parsed partitions are cached; a 100K-note bench and the index size are examined; and a **learning evaluation** (conversations in, a different judge model) is added. Design and plan: [09-learning-design.md](09-learning-design.md).

## 1. Decision and scope

Replace the persistent `MemoryRepository`/vector-memory subsystem with a native Rust `MemoryService`:
- **Markdown files** hold canonical knowledge.
- **One small control snapshot per partition** holds what must not live in editable prose: revisions, origin and tombstones.
- **A separate SQLite FTS5 database** holds disposable search data only.

SQLite stays for conversations, tasks, identities, usage, artifacts and other application state.

The default memory path has no embedding model, reranker, learned tokenizer assets, Python/Node sidecar, model download or network call. Unicode term splitting and substring indexing are ordinary algorithms. Optional extraction uses the existing configured conversation model and its normal cost accounting; it is never required to store, browse, search or delete a note.

This removes the memory model's startup and dependency burden (measured in [06b](06b-evidence-embedding-inventory.md)). Better answer accuracy is a hypothesis to test. That said, today's per-turn recall barely works ([05](05-review.md) finding 1), so the bar is low for the automatic path and real for the tool path. The main quality improvements sought:
- consistent scope enforcement
- explicit corrections
- reliable forgetting (no silent pruning, no similarity replacement)
- source evidence
- predictable context selection

Low-overlap paraphrase and cross-language recall remain the known weakness of lexical search.

Retain conversation compaction, summaries, session logs, task state, skills, `SOUL.md`, `IDENTITY.md` and `USER.md`; they are separate systems. Do not delete the credential `TokenManager`: it manages authentication, not text tokenization. Do not turn retrieved notes into agent instruction files. The KB importer (`memory/kb.rs`, `memory/chunker/`) has **no production caller** since `5ec1db1d` deleted the `/v1/memory*` routes ([06a](06a-evidence-audit-verification.md)), so it is deleted, not ported.

## 2. Fit the existing store

Use `store::content_dir(StoreScope, ContentKind::Memory)`. That content kind already exists, and the home/project READMEs reserve `memory/`. Add helpers to the existing store module rather than joining literal paths throughout core.

```text
<home-store>/memory/owners/<owner-key>/
  MEMORY.md                          generated navigation index; rebuildable
  notes/<uuid>--<slug>.md            canonical notes (active, archived, candidate)
  .history/<uuid>/<revision>.md      the last 5 service-written revisions, the current one included; never auto-retrieved

<project>/.openalpaca/memory/
  .gitignore                         "*", written with the first partition and re-asserted on every open (MD-7)
  owners/<owner-key>/                ...same shape as the home partition...

<home-store>/state/memory-control/<partition-key>.json
                                     one control snapshot per partition (§6, §7a)
<home-store>/state/cache/memory-index-v1.sqlite
                                     disposable catalog, chunks, FTS indexes
```

- `<home-store>` means `store::home_root()`, including `OPENALPACA_HOME_STORE`. No home path is hardcoded.
- **`<owner-key>`** is the first **32** lowercase hex characters of SHA-256 over the UTF-8 bytes of `owner_id`. Owner ids can be arbitrary strings: `link_generate` accepts any `user_id` ([06c](06c-evidence-identity-scope-learning.md) A5). The mapping back to the owner id is recorded in the control snapshot.
- **`<partition-key>`** is `home-<owner-key>` for the home partition, or `proj-<owner-key>-<dir-key>` for a project partition, where `<dir-key>` is the first **16** lowercase hex characters of SHA-256 over the UTF-8 bytes of the **canonical partition directory** string — `<project>/.openalpaca/memory/owners/<owner-key>` after `canonicalize()`, the way `store::project_root_at` addresses artifact rows by the resolved store directory rather than by the path the caller named (`crates/openalpaca_storage/src/store/mod.rs:612-624`). Two projects whose `.openalpaca` resolve to one directory therefore share one partition and one snapshot; keying on the project root would give them two snapshots over one `notes/`. On Windows the canonical string carries the `\\?\` prefix `canonicalize()` returns; it is hashed as returned, so the key is stable on that machine and not portable across machines (no key is). A workspace move renames the snapshot along with the rebinding (§3, §6).
- **The filename is `<uuid>--<slug>.md`**, where `<slug>` is `store::artifact::slugify(title, 48)` (`store/artifact.rs:144-170`: NFKD-folded to `[a-z0-9-]`, byte-bounded, Windows reserved names guarded; it returns `artifact` when nothing is left, and `memory_files` substitutes `note` for that one value). The grammar cannot produce `/`, `..`, a leading `.` or a control character, so a retitle can never be refused or escape. The slug is presentation only: **the UUID is the identity**, and a retitle renames the file in one atomic rename.
- Store helpers exclude `.history`, `.gitignore` and temporary files from note scans.
- Control snapshots sit under machine state, outside the editable content roots. The snapshot binds partition id, owner key and canonical root, and a workspace move updates that binding (§3).
- **Modes.** `memory/`, `owners/<owner-key>/`, `notes/` and `.history/` are created with `store::create_private_dir` (0700 on Unix, `store/mod.rs:736-746`) in both scopes, and every note, temp and history file is opened with mode 0600 (`OpenOptions::mode`). Today every memory row lives in `state/openalpaca.db` under a 0700 `state/`; a note must not become readable by other local accounts by moving to a content directory that `content_dir` creates with the umask default (`store/mod.rs:586-597`). A Unix test asserts the modes.
- **The project `.gitignore` is store-owned.** On every partition open, `memory_files` reads `<project>/.openalpaca/memory/.gitignore` and rewrites it atomically when its bytes are not the service's text (`*\n`). The store's other seeded files are write-when-absent (`store/mod.rs:485-511`), which would leave a cloned repository's force-added `!*` in place and let the next `git add -A` stage the owner's notes. A test replaces a pre-existing `.gitignore` with other content.

**Why a control snapshot is necessary.** Knowledge lives in Markdown, but three things must survive deletion of the search cache: a forgotten-source tombstone, the service's record of which revision it wrote, and the origin of each note. That is one small JSON file per partition (§7a has the schema), not a second knowledge database. The memory tools cannot reach it by construction, and the daemon's generic file tools **refuse** it: `file_write`, `workspace_write` and `file_read` refuse any path whose first component is `state`, and `file_write`/`workspace_write` also refuse a partition's own control files — `.history/`, `MEMORY.md` and `memory/.gitignore` (plan 3.2). The builtin registry captures the daemon's CWD as its workspace root, and both launchers start the daemon in the home store, so without that refusal `file_write("state/memory-control/…")` would reach the snapshot and `file_read` would disclose the owner-id mapping.

Path checks alone do not confine shell or MCP subprocesses running with the same OS identity. Existing unrestricted host execution is equivalent to a trusted administrator for this guarantee, and nothing here claims resistance to the machine owner. Shell and MCP subprocesses are the stated residual; the builtin file tools are not.

A complete backup or export includes notes **and** the control snapshot. If a partition's snapshot is missing or invalid:
- automatic recall, extraction and replay for that partition are quarantined until `memory doctor --repair` (§6, *Management operations*) — the only way out;
- explicit reads still work, and every note is labelled `unverified`;
- empty controls are never silently initialized over existing notes.

A fresh empty partition is initialized atomically. A copied `.md` dropped into `notes/` by hand is picked up by the next reconciliation scan as origin `external` (§4): human-readable, searchable, low authority. It never inherits identity, authority or forget history, and there is no import verb in v1 (§9).

**A factory reset does not touch memory.** `openalpaca config reset --factory` deletes the database trio and the two config files and leaves `state/memory-control/` and `state/cache/memory-index-v1.sqlite` untouched, so an orphaned partition keeps its snapshot — its tombstones and its owner-id mapping — for `adopt` (§3). The index is validated and rebuilt on the next start if it does not match the partitions. The reset preview names both entries (plan 3.7).

Do not create one enormous memory file or copy every conversation into a journal. The DB holds the conversation text, and the session log holds only previews ([06c](06c-evidence-identity-scope-learning.md) A11). Start with a shallow `notes/`, and add topic folders only when browsing tests show a need. `MEMORY.md` lists IDs, titles, descriptions and relative links; it is not another authoritative copy of facts.

## 3. Ownership and scope are enforced by the service

Every operation receives a typed `MemoryAccessContext` from the authenticated request or agent runtime. Its fields — owner, project root, surface, capabilities, attribution — and **who constructs it on each surface** are in §7a (*`MemoryAccessContext`*). None of them ever comes from LLM tool arguments or a Markdown header. Identity resolution is reused; no parallel owner system is invented. Two rules of construction:
- **The owner is a `global_user` row.** Construction accepts an id only when `SELECT 1 FROM global_user WHERE id = ?1` finds it (`001_baseline.sql:15`); anything else — the `"system"` sentinel a `/v1/command process` turn and every workflow it starts carry (`ToolContext::created_by()`, `tools/registry/mod.rs:149-158`), or a `provider:id` string of an unlinked external identity — yields **no owner**: nothing is recalled, nothing is extracted, and the tools refuse. Without this, every API-originated run would share one ownerless `home-<sha256("system")>` partition written by extraction and readable by any worker with `memory_read`.
- **The surface is set by the constructor, never by the caller of a tool.** `UserSurface` is exactly the API, the CLI, the GUI and a chat memory command typed on an attended app or CLI chat turn (§4); `ModelTool` is every tool call, whichever loop made it; `Extraction` is the background extractor.

Rules:

1. **The project partition is the project root found from the request workspace** by the marker walk (`.openalpaca`, else `.git`; MD-5). The store is created lazily on the first project write, as artifacts do. A request with no workspace, or a workspace with no marker above it, reads and writes only the owner's global partition. The daemon's own CWD never selects a project. The partition directory itself is resolved once per open by walking `<project>` → `.openalpaca` → `memory` → `owners/<owner-key>` → `notes` with `symlink_metadata` on every component; when any component is a symlink, a junction or not a directory, the project partition is **refused** and the operation falls back to the global partition with one WARN naming the component. The marker walk and `content_dir` both follow links (`memory/workspace.rs:64-77`, `store/mod.rs:586-597`), and `confine_to_root` canonicalizes the root first, so a git-committed symlink at any of those levels would otherwise place the owner's notes wherever it points.
2. **With a project store**, reads cover global plus that project for the same owner. Default write scope depends on kind:
   - `preference` defaults to **global**, because a preference stated in a project chat is still the user's;
   - `fact`, `decision`, `procedure`, `reference` and `episode` default to the **project**;
   - an explicit `NoteScope` (§4) overrides the default;
   - **learning** ([09](09-learning-design.md) §2.5): a learned note whose `fact_key` subject is `user` (`user.*`) and that replaces nothing is **global**, whatever its kind (LR17), and a learned replacement is written in its target's partition (LR10) — 09 rev 3.3.
3. A project note outranks a global one only for the same explicit `fact_key`, and only where authority permits (§4). **Within one partition**, two active notes with the same `fact_key` and no `supersedes` link between them are resolved the same way: the highest-authority note wins; among notes of equal authority, the newest `observed_at` wins when every one of them has one; after that (or first, when any lacks one) the note first written most recently (the newest `created_at`, which no edit moves), then the newest `updated_at`, then the lowest id (§7c R9); the loser stays on disk, is excluded from automatic recall, and is reported as `conflict` by `memory_search`, by `doctor` and in the GUI. Textual similarity never hides a global note, and a lower-authority note never overrides a user-authored one. A **conversation summary** (§4) that a model's edit gave a `fact_key` still groups by that key here — `conflict` in a search, `same_fact_key` in `doctor` — but holds no key for learning: learning names no target by it and never lists it to the extractor ([09](09-learning-design.md) rev 3.19; whether such an edit should be refused instead is owner question LQ10).
4. **Scheduled skills** are global: none has a persisted workspace binding (`apps/openalpacad/src/scheduled_skills.rs:208` sends no workspace). **Follow-ups** replay their stored `workspace_path` through rule 1. **Subagents** inherit a narrowed context and the existing denied-section and tool rules. Fresh searches enforce the same rules as parent-context distillation.
5. **Audience:** v1 keeps today's lane model (MD-4). Memory adds no exposure that `USER.md` and the shared transcript do not already have. Connector audience (DM vs group, non-owner senders) is a separate design, and the iMessage non-owner-as-owner defect is its own task ([05](05-review.md)). Markdown can never set or widen audience.
6. **Unknown fields are rejected.** Tool input carrying `owner_id`, `workspace_id`, an absolute path or trust fields is refused. The tool registry's validator today accepts unknown keys (F2), so the memory tools validate strictly themselves (`additionalProperties: false` on every schema in §7a). Every read-by-ID and every mutation re-checks ownership and scope, even after a search.

Note IDs are UUIDs, and citations use partition-relative paths; absolute workspace paths are not baked into records.

**Workspace move.** No move lock exists today ([06c](06c-evidence-identity-scope-learning.md) A7). `PATCH /v1/workspaces` is preflight → one DB transaction → `rename(2)` (`routes/workspaces.rs:499-504`, `store/project_move.rs:77`), and the memory service is threaded through it in this order:
1. The preflight calls `MemoryService::quiesce(partition)`, which refuses the move while a memory write for that project is pending and holds new writes until step 5.
2. The service writes the snapshot under the **new** partition key — the binding is the new canonical directory, and `moved_from` names the old key — before anything is renamed.
3. The route runs its DB transaction (`rebase_project`, minus the memory SQL that goes with the table).
4. `move_project_store` renames the directory.
5. The service deletes the old-key snapshot, invalidates the project's catalog entries and releases the hold.

A crash between any two steps leaves a state `doctor` can name (§6, recovery table, row *workspace move*).

**Resets.**
- A plain `openalpaca config reset` deleting `identity.local_user_id` is a **bug**, fixed in slice 0 (plan §2): config reset must not delete `identity.*`.
- A factory reset deletes the database, so the next boot mints a new owner id with a different owner key. The old partitions survive as files **with their snapshots** (§2), and are listed by `memory doctor` as *orphaned*.
- `openalpaca memory adopt <owner-key>` re-binds an orphaned partition to the current owner (pre- and post-conditions in §6, *Management operations*). It is a local management command: no tool provides it and no route serves it. Nothing is inherited automatically.
- Update the factory-reset preview, which today says memories go with the database, and test it.

## 4. A small, versioned note format

Illustrative v1 file, in the service's canonical serialization (below); the UUIDs and ids are examples, not existing data:

```markdown
---
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: Build and test commands for this workspace
kind: procedure
status: active
fact_key: build.validation
aliases:
  - checks
  - validation
  - 测试
  - 编译
tags:
  - development
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
observed_at: 2026-09-23T09:55:00Z
valid_from: null
valid_to: null
review_after: null
pinned: false
sources:
  - type: task
    id: 7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90
    observed_at: 2026-09-23T09:55:00Z
supersedes: []
---
# Build and test commands for this workspace

## Knowledge
Run the documented focused checks for the component being changed.

## Evidence and limits
The cited run checked one component. It did not certify the entire application.

## Recheck when
The build configuration or test entry points change.
```

**Fields.**
- Required: `schema`, `id` (UUID), `title`, `kind`, `status`, `created_at`, `updated_at` and a Markdown body.
- Optional: `aliases`, `fact_key`, `tags`, `observed_at`, `valid_from`, `valid_to`, `review_after`, `pinned`, `sources` and `supersedes`.
- `kind` is the enum **`NoteKind`**: `fact`, `preference`, `decision`, `procedure`, `reference`, `episode`. Scope is the enum **`NoteScope`**: `global`, `project`. (The old `MemoryKind`/`MemoryScope` die with `models/memory.rs`; the completeness search in plan §3 forbids the old names, so the new ones differ.)
- `status` is **`NoteStatus`**: `active`, `candidate`, `archived`. **Superseded is not a stored status.** It is derived from links (§6), so replacing a note never rewrites the old file.
- `fact_key` matches `[a-z0-9][a-z0-9._-]{0,63}`; anything else is rejected at write time and treated as absent when found in an external file.
- `sources` entries are `{type: message | task, id, observed_at?}` — DB ids, never session-log offsets.
- `supersedes` in the **file** is a list of note ids (a user may edit it); the tool and API inputs that create a link take one `{id, expected_revision}` (§7a).

**Canonical serialization** (the revision is a byte hash, so the bytes must be reproducible):
- Frontmatter keys are written in the order shown above; unknown keys follow them in the order they were read. Lists are block style (`- item`), one item per line; scalars are unquoted unless YAML requires quoting; timestamps are RFC 3339 UTC to the second with `Z` (never `+00:00`, never sub-seconds); an absent optional is written as `null`. YAML comments are **not** preserved — a parse-and-write of a hand-commented file loses the comment and changes the revision once, after which it is stable.
- The service parses frontmatter with `serde_yaml` (already in the workspace at `Cargo.toml:43`; the storage crate gains `serde_yaml.workspace = true`, one edge, no new crate) and writes it through its own emitter in the form above, so the canon does not depend on `serde_yaml`'s output style. The body is written byte-for-byte as read or as given.
- A parse followed by a write of an unchanged note is **byte-identical**, and a no-op patch does not change the revision. A round-trip test pins this on the example above, on a note with unknown keys, and on CJK aliases.

**Authority** is not in the frontmatter. The partition decides owner and scope. The control snapshot records, per note, the revision the service last wrote and its **origin**. **A user-directed surface is exactly the HTTP API (`/v1/memories*`, `/v1/persona/user`), the CLI verbs that call it, the GUI, and a deterministic memory command (`/remember`, `/forget`, `/memories`) typed on an attended app or CLI chat turn** (MD-13, [09](09-learning-design.md) §12) — `MemoryAccessContext.surface == UserSurface`. A tool call is never one, whichever loop made it and whatever the model was told (G2, owner decision). A memory command is one because no model is on its path: the orchestrator's deterministic tier answers it with a fixed template, and the text written is exactly what the person typed; any other turn — a connector's, an unattended one, a scheduled skill, a follow-up, `/v1/command process` — is refused and builds no context. **One more act is user-directed** (MD-12, [09](09-learning-design.md) §4.2, rev 3.7): *a confirm of a suggestion whose approval is the human's Approve on the confirmation card for that exact call, delivered through `POST /v1/chat/confirmations/{id}`* — the `memory_confirm` tool (§7) confirms as `UserSurface` for that one write and nothing else, only with that approval, which the sandbox records on no other branch (no cache hit, no `auto_approve`, no *Always allow*). Origins, and who can record each:
- `user`: an explicit save, edit, confirm or supersede through a **user-directed surface** — and nothing else
- `extracted_user`: extracted from the user's own words (MD-2) by the background extractor, **or written by a model tool call** (`ModelTool` `memory_store` and `memory_update edit`): the model saying "the user told me" is the same trust level as the extractor saying it
- `extracted_other`: inferred from assistant or tool output, always a candidate — except a **conversation summary** (MD-16, [09](09-learning-design.md) §13.3, revs 3.17–3.18): an `episode` of origin `extracted_other` that is **not** a candidate, written active by learning (`ExtractionProvenance::Summary`), findable by search, timeline and list, and never delivered by automatic recall (§5 step 5). An edit or archive of a summary by anyone but the user keeps its origin `extracted_other`, so it stays a summary (rev 3.18; the alternative, refusing it, is owner question LQ10); a user-directed edit records `user` and makes it the user's note
- `imported`: **reserved**, with no producer in v1 (an import verb is a separate feature, §9); it keeps its rank so the ordering does not move when one ships
- `external`: the file's bytes no longer match the recorded revision, or the service never wrote the file (dropped in by hand)

Confirming a candidate makes it `active` with origin `user`. It is a user-directed act: `POST /v1/memories/{id}` with `action: confirm`, the GUI's confirm button, `openalpaca memory confirm <id>`, and — only for a suggestion that cites a message the user typed ([09](09-learning-design.md) §4.2 rule 5: learning's, or since LQ11 the chat model's own; rev 7.6) — the Approve on `memory_confirm`'s card (above). **`memory_update` refuses it**, and **before** any card ([09](09-learning-design.md) §4.1, rev 3.7): `memory_update` with `action: confirm` from a `ModelTool` context answers *"Confirming a remembered candidate is done by you, not by me: open the Memory view in the app, or run `openalpaca memory confirm <id>`."* — or, on an attended `gui` turn for a suggestion `memory_confirm` would take, names `memory_confirm` instead — and writes nothing.

**Suggestions** (MD-11, [09](09-learning-design.md) §3, revs 3.5–3.6). When learning finds that the user's words change a note, it replaces a learned (`extracted_user`) note at once and writes a **suggestion** for any other: a candidate of origin `extracted_user` whose `supersedes` names the note, which stays active and recalled until the user answers (LR9). So **a candidate may name any target in its partition, a `user` note included** (S1): the refusal of a non-user-directed `supersedes` of a `user` note applies only when the new note would be active, and a candidate retires nothing while it stays one. **A suggestion is never archived** on any surface (`invalid_input`, S9), and **a model tool never edits one** (`authority_conflict`, S10), so the text a confirmation shows is the one its writer wrote. An inferred change (origin `extracted_other`) is a suggestion too, confirmable only in the Memory view and the CLI. **Since LQ11 a chat model's replacement of a note the user saved is one too** (below): origin `extracted_user`, no `observed_at` — the extractor's alone, which is how the Memory view, the CLI and (rev 7.5) the in-chat card and the `<memory_suggestions>` line say the assistant suggested it, since its text is the chat model's and not the extractor's — and confirmable in chat when it cites a row the user typed — its turn's, or for an edit one the note it copies cites (rev 7.6); the card and the line date it by the day it was saved, as the Memory view and the CLI do (rev 7.6).

Authority ordering: `user` > `extracted_user` > `imported` > `external` > `extracted_other` — five ranks, strictly ordered: an external edit cannot retire an import. An external edit **downgrades** a note to `external` until a service write re-records it. Sources in the note are readable claims, not proof. An LLM-produced confidence number is not calibrated evidence and is not stored as authority.

**What a model tool call may do to a note the user saved** (owner decision LQ11, rev 7.4). A `ModelTool` replacement of a note of origin `user`, `external` or `imported` — a `memory_store` whose `supersedes` names it, a `memory_store` with no `supersedes` whose `fact_key`'s R9 winner is such a note in the partition the store may land in (the one `scope` names, else the request's project partition and then home, the first holding a saved winner; rev 7.5), a `memory_store` whose `supersedes` names a note that is not saved while its `fact_key`'s R9 winner in that note's partition is such a note (the suggestion then names that winner; rev 7.5), and a `memory_update edit` of one that changes its title, body, aliases, tags, key or validity — is written as a **suggestion**: a candidate of origin `extracted_user` naming the note, through learning's path (`SuggestUnlessLearned`, S1–S4), in the note's partition when no `scope` is named (ruling LR10 on the tool path), citing the turn's own row the user typed when it has one — and an edit's suggestion, whose text is the note's own, then what the note cites, at most eight (rev 7.6), so a forget of a row the note learned from takes the copy out as it would the note (§6); the note is untouched and stays in force until the user confirms. (Applying the change would re-record the note as `extracted_user`, a silent downgrade of the user's own words; before LQ11 a replacement of a `user` note was refused with `authority_conflict`, and the model then stored the new value as a second active note, while one of an `external` or `imported` note was applied at once.) An exact duplicate of such a note — the same kind, title, body, key, aliases and tags — is that note (`dedup: existing`), never a suggestion (rev 7.5). A `ModelTool` `memory_update archive` of a `user` note, and an edit of one that changes nothing a suggestion carries (nothing, or only `review_after`), are still **refused** with `authority_conflict` naming the note and the two user-directed ways to do it. **Unchanged and direct** (rev 7.5, as before LQ11): archive of an `external` or `imported` note, and an edit of one that changes only `review_after`, which re-records it as `extracted_user` — after which a replacement of it stands; and a `memory_update edit` of a learned note that sets the key of a saved note, which then meets it under R9. **Recorded, not built** (rev 7.6): a `memory_store` whose `supersedes` names a saved note W while its `fact_key`'s R9 winner in W's partition is another saved note W2 is a suggestion naming W alone; once the person confirms it, it is `user` and outranks W2 under R9 (an `external` or `imported` W2 by rank, a `user` W2 by the newer `created_at`), and no card or listing names W2. The confirm is the person's act, so G2 holds, and learning's `replaces` has the same shape (it consults the key's winner only without `replaces`). A `ModelTool` `memory_update edit` that becomes a suggestion still raises `memory_update`'s confirmation card first (§7, *Capabilities*: `memory_update` is destructive), so on an attended turn the person approves the edit and then finds it waiting as a suggestion (rev 7.5). A replacement of a learned (`extracted_user`) note stands, as learning's does. A `ModelTool` `memory_forget` of a `user` note is allowed **under its confirmation prompt** (§7, *Capabilities*): the card is the user's own act, and a forget records no origin.

**Times.** `valid_from`/`valid_to` define a half-open effective-time interval only when evidence supports it; unknown bounds stay null. Current recall excludes notes whose interval has ended, and a historical query evaluates the requested date. `review_after` only requests a recheck; it is never an expiry or deletion.

**Revisions.** A revision is the **lowercase hex SHA-256 of the file's exact UTF-8 bytes** (64 characters), returned by the service, never embedded in the file. The snapshot records the one the service last wrote; a file whose current hash differs is `external`.
- `expected_revision` is mandatory for update, archive, confirm, supersede and forget.
- Untouched Markdown and unknown extension fields are preserved during supported patches (the canon above is what makes "untouched" mean byte-identical).
- Duplicate required keys, invalid timestamps, duplicate IDs and oversized records are rejected with a visible diagnostic. Malformed external files stay on disk, excluded from automatic context, and are never rewritten or deleted during recovery.

**Initial limits** (tune through evaluation):

| Limit | Value |
|---|---|
| Note size | 32 KiB |
| Chunk size | about 1–2 KiB, on heading/paragraph boundaries |
| Combined automatic memory context, pinned notes and wrappers included | 8 KiB |
| Retrieved chunks | at most 8 |
| Explicit tool read response | 32 KiB |
| Extracted notes per extraction call / per task run | 12 / 10 (`[orchestrator.costs] extract_max_items_per_call`, `task_extract_max_items_per_run`; plan 3.4, then [09](09-learning-design.md) §1.10, rev 3.1: the per-turn 5 became per call — a small model's output budget can lower it, §1.6) |

These are byte limits, and the existing budgeting system reserves the headroom. Replace the language-insensitive estimate for this section with a conservative one, and test it on Chinese, emoji and long identifiers.

## 5. Read path: one implementation for every agent

```mermaid
flowchart LR
    R[Authenticated request] --> A[Resolve owner, partitions, audience]
    A --> S[MemoryService recall]
    M[Canonical Markdown notes] --> I[Rebuildable lexical index]
    I --> S
    M --> V[Verify revision, status, links, tombstones]
    S --> V
    V --> C[Bounded cited context]
    C --> P[Main loop, lead or subagent]
```

The same `MemoryService::recall` serves `prompt_ctx::MemorySource`, the lead-agent bootstrap and `memory_search`. The lead agent's direct repository query is removed.

**The profile is injected once per turn**, and the survivor is the persona layer's static system-prompt block: `compose/persona.rs` `user_document_block` (priority `High`, budgeted by `[orchestrator.prompt_budgets] user_profile_budget`, cached with the rest of the static layer). `prompt_ctx::sources::UserProfileSource` — the second, `Normal`-priority `SystemMessage` section — is **deleted**, and the lead, which passes `user_document: Arc::new(None)` to the persona layer today (`runner/lead_agent/prompt.rs:213`) and received the profile only through the deleted source, passes its user document to the persona layer instead. **The memory section leaves the cached dynamic-context layer.** Today it is cached by query text with `memory_retrieval_hash` zeroed ([05](05-review.md) finding 3; `simple_query_handler.rs:550,1050`, `lead_agent/prompt.rs:244`, `lead_agent/tools.rs:506`); after the cutover the service builds it per turn — it is cheap (§5's 50 ms bound) — and appends it outside the fingerprinted layer, so `DynamicContextInput.memory_retrieval_hash` and its four zeroing sites go. Retrieved-data trust labels are preserved in all prompts and handoff packages.

Deterministic retrieval steps:

1. **Resolve allowed partitions** before searching (§3). An unknown or unauthorized root fails closed.
2. **Select a few active pinned notes** within a separate sub-budget (initially 2 KiB of the 8 KiB). `MEMORY.md` is optional navigation, not unbounded startup content. Pinned notes are still untrusted reference data.
3. **Search.** Normalize the query and keep exact identifiers and paths alongside Unicode-normalized terms. Search titles, aliases, tags and body with FTS5/BM25 over a **union of terms**. Today's per-turn path requires every word of the whole message and matches almost nothing ([06c](06c-evidence-identity-scope-learning.md) A6). Build query syntax from escaped literal terms (an AST), never from caller-supplied `MATCH` expressions. This removes the F4 class of defect.
4. **Term routing.** *(Revised by LQ17, revs 7.7–7.8 — [09](09-learning-design.md) §6.5. Until then every normalized term of three or more characters was ORed against a `unicode61` table and a trigram companion over every chunk, and the trigram companion was what found `prefer` inside a body's `prefers` (07 G23, reproduced on SQLite 3.51: 0 rows `unicode61`, 1 row trigram).)*
   - **The `unicode61` table** takes every normalized term. Its text, and the phrases it is queried with, are split at script boundaries: a space goes where a character of a script `unicode61` cannot split into words meets a letter or digit of another script, so `和commit` holds the word `commit`. A term is a **prefix query** when its last token has three or more characters, unless the term carries such a character or that token is a stop word. So `prefer` matches `prefers` and `preference`, `config` matches `configure`, and `memory_file` matches `memory_files`; `用户喜欢` and `and` match exactly. An exact word or identifier is its own prefix.
   - **When that query fills its candidate cap**, the terms it sent as prefixes are searched again as whole words in titles, aliases and tags, with a cap of their own, so a word beginning shared by many longer words cannot crowd a whole-word match in a note's header out of the candidates.
   - **The trigram tables** hold titles, aliases and tags for every note, and the body only of a chunk carrying a character of a script `unicode61` cannot split into words: CJK, Thai, Lao, Myanmar and Khmer. Every term of three or more characters searches the first. Only a term that carries such a character searches the second, and only such a term is ranked by it.
   - **Weighting.** A word-beginning hit has a trigram-only hit's coverage weight, below a whole-token hit, and is reported as `prefix`.
   - **What no table finds** is a term **inside** a word of a body written in a space-separated script — `presso` in `espresso`, `book` in `runbook`, and `now` in `knowledge` in a Chinese note too. A title, an alias or a tag still finds it, and aliases are the remedy for a wording the note lacks.
   - **No stemmer is added.** `porter` is English-only and changes what "exact" means for identifiers and CJK; a prefix query changes no token.
   - **Phrases and exact identifiers** are matched as phrases on the `unicode61` table (the last token a prefix, as above) and as literals on the header trigram table — and on the body trigram table when they carry an unsegmented character.
   - **One- and two-character queries** use a bounded scoped substring pass. Return `partial: true` when a scan cap is hit, and do not claim complete Chinese retrieval from `unicode61`.
5. **Filter, then limit.** Union and deduplicate the candidates. Filter owner, scope, kind, status, **derived supersession**, tombstones, same-`fact_key` losers (§3 rule 3), audience and effective validity **before** the result limit. Automatic recall also leaves out every **conversation summary** (§4; a pinned one is still delivered, the pin being the user asking — [09](09-learning-design.md) revs 3.17–3.18); an explicit search returns them. Preserve ranking evidence through context assembly, never replacing it with generic importance.
6. **Rank** by exact title/alias hits, lexical relevance, authority (§4), explicit validity and a modest recency tie-break. Pin and test the weights, and do not claim they are optimal. Superseded and archived notes are excluded unless the user explicitly asks for history. Same-key overrides are resolved before budgeting.
7. **Verify.** Reopen the selected files with confined handles, re-hash them, and check status, links and tombstones. Return current excerpts with note ID, relative path, heading, line range, revision, sources, match reason and freshness. A dirty or deleted note is never served from cached text.
8. **Budget and deliver.** `[orchestrator.memory] recall_mode` (MD-15, [09](09-learning-design.md) §13.2, revs 3.15–3.16) says how: **`index`**, the default, delivers the pinned notes, the **top 2** recalled notes as excerpts, and an index block of up to **8** further notes, one line each — `<id> <date> <kind> “<title>” — <reason>`, the full id (owner question LQ8) — under `More notes that may be relevant (read with memory_read):`, all inside the same 8 KiB; **`excerpts`** delivers up to 8 items, pinned included, each as an excerpt. A file-based skill's turn is recalled in `excerpts` unless the skill can call `memory_read`. Each excerpt's header carries the re-read note's `valid_from` and `valid_to` when a bound is set, then `timing=upcoming` when the index row — the same bytes the re-read found — says the window has not begun ([09](09-learning-design.md) §6.1, rev 3.8; residue R1). Budget whole chunks when possible and mark truncation. Deliver as quoted data in user or tool context, never as a system instruction. A citation resolves to the revision actually read. After eviction or forgetting it reports `revision_unavailable` or `deleted`, and a source whose DB evidence is gone reports `evidence_unavailable`. **What was delivered is reported**: the turn's `HandleResult`/`GatewayResponse`/SSE `done` carry `memory_used: [{id, revision, reason}]` (§7a), omitted when empty and persisted nowhere, and the service keeps an in-memory ring of recent deliveries that `GET /v1/memories/recently-used` reads.

The default recall/search path invokes no model. When lexical evidence is weak, the running agent may issue up to two revised searches within its existing loop and cost budget, then abstain or explain the gap. There is no hidden query-expansion agent or neural reranker. Cross-language aliases come from the user or from an already-budgeted write.

**Freshness without a watcher.** Correctness does not depend on filesystem events. The wake watcher cannot add paths after start, and core may not depend on it ([06c](06c-evidence-identity-scope-learning.md) A13). Freshness comes from four sources:
- **Service writes** update the index synchronously, giving read-after-write consistency. The index is a disposable cache, so an index failure after a write landed does not fail the write (§6 step 4).
- **Reconciliation** is a bounded stat scan (mtime + size + inode) of the partition's `notes/`. It runs at service start, every 30 s, before management reads, and on `sync`. Changed files are re-hashed and re-indexed, and a detected external change publishes `memory_changed` with `change: external` (§7a).
- **Verification** of every selected file (step 7) stops a stale deletion or edit from being served before reconciliation sees it.
- A missed **new** external file may be absent until the next scan, and freshness is reported.

A `notify` watcher in the daemon is an optional latency improvement, measured before it is added. Cold start opens the index if it is valid and schedules reconciliation on a blocking worker. A missing or corrupt index triggers a rebuild, and search meanwhile serves a bounded scoped file scan with `index_state: rebuilding`.

## 6. Writes, corrections, deletion and recovery

### Write protocol

There is one daemon-owned writer queue per partition. CLI and GUI mutations go through the service. Offline management (`doctor --repair`, `adopt`) takes the store lock and refuses while the daemon runs. File access uses confined no-follow handles and rejects escaping paths, symlinks and junctions, hardlink aliases where enforceable, and hidden or control-file targets; the path *to* the partition is checked component by component (§3 rule 1). The atomic write builds on `store::write_atomic` (`crates/openalpaca_storage/src/store/mod.rs:754`), hardened with a no-follow open, mode 0600, a file `fsync` and a directory `fsync` where the platform supports it. The artifact store's crash-tested put path (`crates/openalpaca_storage/src/artifacts/`) is the reference.

**Every mutation touches at most one note file.** That keeps recovery to a single pending field instead of an operation log:

| Mutation | Steps (each step atomic) | Recovery at service start |
|---|---|---|
| **create / update / archive / confirm** | 1. Snapshot: set `pending = {id, from_rev, to_rev}` (`from_rev` null on create) <br>2. Copy the **new** bytes to `.history/<id>/<to_rev>.md` (`fsync`), then write the note (temp, `fsync`, rename), then prune `.history/<id>/` to the newest 5 <br>3. Snapshot: record `to_rev` and origin, clear `pending` <br>4. Update index and `MEMORY.md`; publish `memory_changed`. An index failure here does not fail the write (lead ruling, 2026-09-25; §7a) | File equals `to_rev`: finish step 3. <br>File equals `from_rev` or is absent: clear `pending`; the `.history` copy of `to_rev` is left (it is bounded and harmless). <br>Anything else: an external edit raced us. The note becomes `external`, and the service's own `to_rev` is in `.history`, which is why step 2 copies it **before** the rename. |
| **supersede** (replace note A with B) | A `memory_store` (or `POST /v1/memories`) carrying `supersedes: {id: A, expected_revision}`. **Create B** with `supersedes: [A]` in its frontmatter, as the create row above. A's revision is checked, not written; a changed A is a conflict and nothing is created. A's file is **not touched**: A is superseded because a qualifying note names it (below). The result is B's new UUID and revision. | As for create. |
| **forget** (ids with expected revisions) | 1. Snapshot: append tombstones (id, `supersedes` list, origin, suppressed source ids) — a candidate's carries no `supersedes` ([09](09-learning-design.md) S3), and a **withdrawal**'s neither links nor suppressed sources (*Forget contract*) <br>2. Delete the note files and their `.history` <br>3. Purge index and navigation; publish `memory_changed` (an index failure does not fail it, as above) | A tombstoned id whose file still exists: delete it. Idempotent. |
| **workspace move** (§3, five steps) | 1. quiesce <br>2. write the new-key snapshot (`moved_from` = old key) <br>3. DB transaction <br>4. `rename` <br>5. delete the old-key snapshot | Handled by `doctor`, not at service start, because the service cannot tell a move from a lost directory: a snapshot whose bound directory does not exist is **stale** when another snapshot names it in `moved_from` and that one's directory exists (`--repair` deletes the stale one — the move completed), **abandoned** when its own `moved_from` names a snapshot whose directory exists (`--repair` deletes it — the move never happened), and **orphaned** otherwise (listed, never deleted). |

**`.history` holds the last 5 service-written revisions of a note, the current one included**, in both scopes, and nothing else — not every revision. Forget deletes the note's `.history` with it.

**Derived supersession.** A note is superseded when an **active or archived note, or a tombstone**, lists it in `supersedes` and three conditions hold:
- **Same partition.** A link across partitions is ignored.
- **Enough authority.** The superseder's origin must rank **at or above** the target's (§4; `imported > external`, so a hand-dropped file cannot retire an import). A lower-authority link never hides a higher-authority note, so hand-edited frontmatter cannot retire a user's note. Such a link is reported by `doctor` and the GUI as a conflict, and both notes stay active.
- **Not a candidate.** A candidate supersedes nothing until it is confirmed.

Archiving or forgetting a replacement therefore does **not** revive what it replaced, because tombstones keep their `supersedes` lists and their origin for exactly this. Restoring an old note is an explicit update that removes the link. Only the single `pending` field needs crash recovery. A multi-note change is expressed as independent single-note mutations whose intermediate states are all valid, never advertised as atomic.

External editors do not honor app locks. The revision check prevents stale overwrites between **service** writers, and an external edit is detected and downgraded (§4). The final race with an arbitrary in-place editor is surfaced as a conflict. It is not claimed to be impossible.

Snapshots are replaced atomically and stay bounded: completed pending fields are cleared, duplicate source-suppression entries are compacted, and a live tombstone is never discarded to meet a size cap. Tests cover a corrupt or truncated snapshot, a missing file, and a crash at each step in the table.

### Management operations

- **`memory doctor`** (read-only, local or through `GET /v1/memories/doctor`) lists, per partition: quarantine (snapshot missing or invalid), orphans (a partition whose owner key is not the current owner's), conflicts (a lower-authority `supersedes` link; two active same-`fact_key` notes), unverified and malformed files, and stale/abandoned move snapshots (table above). It changes nothing.
- **`memory doctor --repair`** (local, singleton lock, refuses while the daemon runs) rebuilds a **missing or invalid snapshot** from the directory and lifts the quarantine. Preconditions: the directory's `<owner-key>` is the current owner's (otherwise it is an orphan, and `adopt` is the verb); every `.md` under `notes/` either parses or is listed as malformed and skipped; **no two parsed notes share an id** — a duplicate refuses the repair and names both files, because the service cannot choose. Postconditions: the binding is taken from the path; every parsed note is recorded at its current hash with origin `external`; `tombstones` is empty, `pending` absent, `repaired_at` stamped; the index entries for the partition are rebuilt. It also deletes stale and abandoned move snapshots. It never rewrites, moves or deletes a note file.
- **`memory adopt <owner-key>`** (local, singleton lock) re-binds every partition of an orphaned owner key to the current owner in one command: the home partition and every `proj-<old>-*` snapshot. Precondition: for every partition it would touch, neither `owners/<current-key>` nor the current-key snapshot exists in that scope — one collision refuses the whole command, naming it, and nothing is partially adopted (merging two owners' notes is not a v1 operation). Postconditions: each `owners/<old-key>` directory is renamed to `owners/<current-key>`, each snapshot file is renamed to its new key with `owner_key` and `owner_id` rewritten, and the printed list names every partition adopted. The current owner id is read from the database's `identity.local_user_id` under the lock.
- **`memory sync`** (`POST /v1/memories/sync`) runs one reconciliation scan now and reports what changed. "Management-only" means it is on no model surface; it is under the same bearer token as every other route, because the daemon has one auth level.

### Promotion and correction

- **Explicit "remember"** writes are synchronous, acknowledged only after durable success. Through a user-directed surface — the chat's own `/remember` included — they record origin `user`; through the model's `memory_store` they record `extracted_user` (§4).
- **Automatic extraction** follows MD-2. User-stated preferences and facts become active notes with origin `extracted_user`, listed as *recently learned* with undo. Inferences from assistant text or tool output become `candidate`s. Failed or cancelled tasks and assistant-only completion claims never become facts. **The extractor's input is the message as typed** — never the model input, which on an attachment turn carries the files' extracted text (U2); an uploaded document therefore cannot mint an `extracted_user` note. An item under `memory_confidence_threshold` is dropped, and the number itself is never stored. It runs cancellably with task attribution, with no fire-and-forget jobs. **Chat learning** is Learning v2's (MD-10, [09](09-learning-design.md) §1, revs 3.1–3.3):
  - **Cadence.** Every turn is queued; a session's turns past its durable cursor (`learning_cursor`, migration 044) are extracted together when the session has been idle `[orchestrator.learning] idle_secs` (120), when `turns_per_flush` (10) of the user's turns have accumulated, when the user switches conversations, and in a 3 s stop drain whose durable half is a boot catch-up. A reply never waits for it.
  - **The window** is read from the database, `conversation_messages.content` only; assistant rows with no `request_id`, `/`-lines and their answers stay out (LR2). **Each user row learns into its own turn's root**, stored on the row as `workspace_root` — so §7a's Extraction row holds per row — and a chunk never mixes roots (LR1).
  - **The byte budget** scales with the extraction model's context window ([09](09-learning-design.md) §1.6), a user row over 8 KiB keeps its first 6 KiB and last 2 KiB, and on a keyed provider a chunk's whole request is capped at 40 % of the key's TPM.
  - **Bounds.** At most `extract_max_items_per_call` (12) notes per call and `task_extract_max_items_per_run` (10) per run; **durable daily caps** read `llm_usage_daily` for the UTC date — `extract_max_calls_per_day` (200) and `extract_max_daily_cost_usd` — and a binding cap queues, never drops (LR8).
  - **What holds a window**: an unavailable model, a quarantined partition and a store error hold it with backoff (LR20); only a reachable model's unusable answer counts as an attempt, and the third in a row skips the chunk with one WARN. A deleted or cleared conversation's unlearned turns are dropped (LQ4), and so are the turns of a project directory that no longer exists (owner question LQ5).
  - **Learned notes carry `aliases` and a `fact_key`**, and `observed_at` ([09](09-learning-design.md) §2, rev 3.3); the extractor reads the notes it is shown through `learning_view` only (§7a).
- **Recently learned** = active notes with origin `extracted_user` that automatic recall can use — a same-key loser or a `source_forgotten` note is not listed ([09](09-learning-design.md) S7) — newest `created_at` first, over a window (`?since=`, default 7 days) or a count (`?limit=`, default 50), each item with `supersedes` (S8). An edit or confirm through a user-directed surface re-records the note as `user` and removes it from the list. **`undo` withdraws** ([09](09-learning-design.md) §3.3, LR12, rev 3.5): by id and expected revision, it removes the note with a tombstone that carries no links and suppresses no source, so a note it replaced is effective again and whatever else the same message taught stays learned. It also takes **any candidate**, which is how a suggestion is dismissed (*Keep mine*, a Deny on the card, and a confirm's dismissal of the other suggestions for the same note, S6). Anything else — a note the user has since made theirs — is `409 MEMORY_NOT_UNDOABLE`, and a plain forget is the verb for that.
- **Replacing through a suggestion.** Its confirm binds the replaced note's revision (`target_revision`, S5): a moved target is a revision conflict, a missing one `not_found`, and one no longer effectively active is `invalid_input` naming undo ([09](09-learning-design.md) rev 3.6). A confirm dismisses every other pending suggestion for the same note, each as its own withdrawal after the write (S6).
- **Deduplication** is exact, within one owner and partition; a pending candidate with an equal `supersedes` is covered too, so a restated change mints no second suggestion ([09](09-learning-design.md) S4). Similarity can suggest a relation, never prove equivalence or contradiction, and **nothing is replaced by similarity**. A replacement is an explicit supersede with the same `fact_key`, or a user correction.
- **Updating a note** uses its ID and expected revision. Observation time is kept separate from effective time.
- **`USER.md`** (MD-3): it is reserved for deliberately configured identity and interaction instructions. The background profile writer stops writing learned facts into it; they go to memory notes instead.
  - An explicit editor ships in the same slice: `openalpaca persona edit user` and Settings → Persona (MD-9), both through **`GET /v1/persona/user`** and **`PUT /v1/persona/user`** (§7a): raw Markdown, `If-Match` on the file's SHA-256, 409 on conflict, the same backup-and-atomic-write path `update_persona` uses (`tools/builtins/update_persona/mod.rs:121-134`). A concurrent bootstrap `update_persona` changes the hash, so a stale editor gets a 409 rather than overwriting it.
  - Onboarding completion stops depending on the background writer.
  - The lines the old writer already extracted into `USER.md` are the owner's to keep or remove by hand (G38, owner decision); the switch-over note says so.
  - A persona instruction that conflicts with a memory note is surfaced for an explicit persona edit, not silently overridden.
  - A forget result names retained persona or transcript copies when known.
- **Decay and automatic pruning of durable knowledge are removed.** `review_after` marks staleness, not permission to delete, and archiving is explicit.

### Forget contract

`memory_forget` takes explicit IDs and expected revisions, never "delete the best lexical match". A natural-language request first searches and selects, and ambiguous matches are resolved before deletion. On an attended turn the tool's `destructive_hint` raises the ordinary confirmation card (§7, *Capabilities*), which is the user's say-so; on an `unattended` turn it is refused before it runs. Nothing beyond that is asked.

Forget removes the canonical notes, their `.history` copies, navigation entries, index rows and any live memory-context cache. It records durable tombstones and suppresses extraction replay for the affected source ids. **A withdrawal is not a forget** ([09](09-learning-design.md) §3.3, LR12): undo, *Keep mine*, a Deny on a suggestion's card and a confirm's dismissals remove the one note with a tombstone that carries no links and suppresses no source. **A forget of a note withdraws the pending suggestions that name it** (rev 7.6), each so: none can be confirmed any more, and the chat model's edit suggestion is a copy of the forgotten note's text, which a candidate left behind would hand back to `memory_list` and `memory_read`. A suggestion whose target goes any other way — archived, retired, deleted by hand — stays a candidate, unlisted ([09](09-learning-design.md) §3.5). **Forgetting a conversation summary** (§4) is likewise a withdrawal's tombstone, so the conversation's facts stay learned; a fact forget still marks a summary citing the same message `source_forgotten`, and a forgotten message is left out of every later summary. **Deleting or clearing a conversation, or purging its project, forgets its summaries** — those whose cited messages all belong to it ([09](09-learning-design.md) rev 3.17, LQ4's three deletes). A note whose `sources` cite a forgotten note is **derived** `source_forgotten` at read time and kept out of automatic recall until it is edited. No other file is rewritten, so the one-file rule holds. **The hard guarantee:** no retrieval or replay resurrects the known records or source ids, including after an index rebuild or a restart that recovers a `pending` write.

**Limit:** tombstones live in the partition's snapshot.
- A **lost or corrupt** snapshot quarantines the partition (§2), so nothing is recalled rather than something resurrected.
- An **older snapshot restored from a backup** lacks tombstones written after that backup, so a restore is an explicit management action whose preview lists the notes it would bring back.

Transcript messages, session logs and external backups are separate data. The forget result states whether they remain and points at their existing deletion controls; it does not claim the memory action erases every copy. Raw source text still visible in the active conversation can still influence an answer, so memory sections are refreshed and the boundary is stated. Semantic re-expression from new evidence cannot be blocked by hashes, and a new explicit user instruction may intentionally reintroduce a fact. Historic sessions are not re-extracted by default. Tombstones store ids and source suppression, never the forgotten plaintext. There is no automatic Git history or cloud sync.

## 7. Tools, API and app surfaces

These are provider-neutral contracts; §7a is the frozen wire form. Familiar names are kept where useful, and old argument formats break cleanly (the app is unreleased).

| Operation | Model/tool input (full schema in §7a) | Result and enforced behavior |
|---|---|---|
| `memory_search` | `query`; optional `kind`, `scope`, `limit` (1–50, default 10), `detail` (`index` \| `snippets`), `date_from`, `date_to`, `order_by`, `historical`, `as_of` | An **index** by default — one row per note, no body — or, with `detail: snippets`, cited snippets with revision, match reasons, freshness, completeness and validity bounds; active-only by default; same-key losers reported as `conflict` ([09](09-learning-design.md) §13.1, rev 3.14) |
| `memory_read` | `ids` (1–10); optional `expected_revision`, `section` (single id only) | Bounded current Markdown and provenance per id, in the order asked, sharing 32 KiB; an unreadable id answers in its place; the service re-checks access |
| `memory_timeline` | exactly one of `anchor` (a note id), `query`, `date`; optional `before`, `after` (0–20, default 5) | Index rows around the anchor in date order, superseded, archived and ended notes included and marked, candidates never ([09](09-learning-design.md) §13.1) |
| `memory_store` | `title`, `body`, `kind`; optional `scope`, `aliases`, `tags`, `fact_key`, `sources` (verified DB handles), `valid_from`/`valid_to`, **`supersedes: {id, expected_revision}`** | New UUID and revision; origin `extracted_user` from a tool call. No raw path, ownership or trust accepted from the client. A `supersedes` whose revision has moved is a refusal and nothing is created; one whose target the user saved, edited by hand or imported — or a store under such a note's `fact_key` — is written as a suggestion (§4, LQ11). |
| `memory_update` | `id`, `expected_revision`, `action` ∈ `edit | archive | confirm`, `patch`, `reason` | Conflict or a new revision. `confirm` is refused from a tool call (§4). An `edit` of a note the user saved, edited by hand or imported is written as a suggestion carrying the edit (§4, LQ11); `archive` of a `user` note is refused from a tool call. **Supersede is not an update action**: it is `memory_store` with `supersedes`. |
| `memory_forget` | `targets: [{id, expected_revision}]`, `reason` | Per-ID status plus the stated evidence and history scope; prompted on attended turns, refused unattended |
| `memory_list` | `kind`, `status`, `scope`, `cursor`, `limit`, `include_history`, `include_candidates` | Metadata; superseded history and candidates only on explicit request |
| `memory_confirm` | `id`, `expected_revision`, `target_revision` | The Approve on its card confirms one suggestion that cites a message the user typed ([09](09-learning-design.md) §4.2 rule 5; rev 7.6), as a user-directed write (§4); offered only on attended `gui` main-loop turns ([09](09-learning-design.md) §4, LR13, rev 3.7) |

**Capabilities.**
- `memory_read` covers `memory_search`, `memory_read`, `memory_list` and `memory_timeline` ([09](09-learning-design.md) §13.1). It keeps its name and meaning. **All nine shipped templates name it**, and `AgentTemplate::to_subagent` grants it **ambiently to every internal template** that neither names it nor denies it, so the owner's post-reset install — whose `config/agents/` the seeder skips whole and the factory reset never touches — needs no re-seed; **a plugin-contributed template gets it only by naming it**, since its tool results go to third-party code (LQ3, [09](09-learning-design.md) §5, rev 3.8). The lead's hand-built surface carries `memory_search`, `memory_read` and `memory_timeline` (rev 3.14; `runner/lead_agent/mod.rs:173`).
- `memory_write` covers `memory_store` and `memory_update`. No shipped template grants it, and the lead does not either: workers and the lead read; only the main conversation writes (below). `memory_confirm` is a per-request tool of the main loop's attended `gui` turns, never a template's.
- `memory_forget` is destructive.
- `memory_manage` names the CLI-local verbs (`doctor --repair`, `adopt`) and nothing else. **No tool provides it**, so listing it in a template's `capabilities` grants nothing; it is not "never granted by default", it is not grantable.

**Tool annotations decide the confirmation prompt** (`annotations_for_builtin`, `tools/builtins/mod.rs:578-590`; the sandbox derives its confirmation set from `destructive_hint` when a policy lists nothing explicit, `security/sandbox/mod.rs:626-643`, and the main loop lists nothing). They are static per tool name, so a prompt cannot depend on an argument:
- `memory_forget`, `memory_update` and `memory_confirm` carry `destructive_local_annotations()`: a confirmation card on an attended turn, and an immediate refusal on an `unattended` one (`sandbox/mod.rs:307`, M6) — a scheduled skill or a piped `openalpaca chat` cannot forget or archive.
- **`approval_is_per_call`** ([09](09-learning-design.md) §4.2 rule 2, LR14, rev 3.7) is true for `memory_confirm` alone: its card is always raised — no approval-cache hit, no `auto_approve`, whatever `require_confirmation_for` lists — its answer is never cached, and the event says `per_call: true`, so no client offers *Always allow*.
- **A preview hook** (`confirmation_preview`) lets a tool refuse before any card or attach a daemon-written preview: `memory_confirm`'s card shows both notes' text from the service's own read (never the model's words), and `memory_update` with `action: confirm` is refused there, before the card.
- `memory_store`, `memory_search`, `memory_read`, `memory_list` and `memory_timeline` carry `read_only_annotations()` or none. **A `memory_store` with `supersedes` does not prompt**: the annotation is per tool, and prompting every plain save is not acceptable; what bounds a tool-issued supersede is authority instead — it can never retire a `user` note (§4), so the notes it *can* retire are the model's or the extractor's own (`extracted_user`) and a conversation summary; since LQ11 (rev 7.4) its replacement of a note the user saved, edited by hand or imported is only a suggestion, which the user confirms.

Map them through tool annotations, platform classification, the sandbox policy, the default agent allowlists in `config/agents/*.md` and deny settings. Workers get no write access through a globally available tool. The main conversation keeps `memory_store`, `memory_update` and `memory_forget` on its per-request surface (`tools/builtins/main_loop.rs`), as `ModelTool` calls.

**Surfaces, all shipping with the cutover (MD-9):**
- **API, with the cutover.** Typed `/v1/memories*` routes — list, search, `timeline`, read, create, update (edit/archive/confirm), forget, undo, history, `recently-learned`, `suggestions`, `recently-used`, `sync` and the read-only `doctor` — and `GET|PUT /v1/persona/user`, under the existing bearer auth, in the shared `api_error` envelope (§7a has the table, the cursor rule and the error codes). Mutations take **`If-Match: "<revision>"`** — the header only, never a body field — and answer 409 on conflict. Cursor pagination, capped payloads and escaped Markdown rendering. Regenerate `docs/api/`.
- **CLI, with the cutover.** `openalpaca memory list|search|timeline|show|add|edit|suggestions|confirm|undo|forget|sync|doctor|adopt` and `openalpaca persona show|edit user`, sharing the daemon's semantics (syntax in §7a). Every daemon-backed verb takes the CLI's CWD as the project (the `x-workspace-path` header `chat` already sends) unless `--global` or `--project <path>` says otherwise. `doctor --repair` and `adopt` are local-only.
- **GUI, with the cutover.** A **Memory view** — the fifth top-level view in the side nav, after Library and before Settings (G30, owner decision) — and a Settings → Persona `USER.md` editor (the ninth settings section). The Memory view has a scope selector (global / this project), search, source evidence, raw Markdown and diff against `.history`, candidates with confirm, *recently learned* with undo, revision-conflict handling and forget. Empty copy: *"Nothing remembered yet. Ask Alpaca to remember something, or add a note."* Conflict copy: *"This note changed since you opened it. Reload to see the current version — your text is kept."* A recently-learned row reads *"Learned from what you said · Undo"* (a replacement adds *"replaced “A”"*), and a **Suggestions** card offers *Replace* or *Keep mine* for each suggestion ([09](09-learning-design.md) §3.5). It shows which notes a response used, from the `done` frame's `memory_used` (live only; a reload reads `recently-used`). It invalidates its queries on `memory_changed` (§7a) and never polls. No model settings or cache internals appear in the editing flow.

## 7a. Contracts (frozen in slice 1)

Slice 1 freezes these as written; a change after that is a design revision, not an implementation detail. JSON property names are exact. Every object is closed (`additionalProperties: false`) unless a row says otherwise.

### The control snapshot

`<home-store>/state/memory-control/<partition-key>.json`, replaced atomically, one per partition:

| Field | Type | Meaning |
|---|---|---|
| `schema` | integer | `1` |
| `partition_key` | string | `home-<owner-key>` or `proj-<owner-key>-<dir-key>` (§2) |
| `scope` | `"home"` \| `"project"` | |
| `owner_key` | string (32 hex) | |
| `owner_id` | string | the `global_user.id` the key was derived from |
| `root` | string | the canonical partition directory (`…/memory/owners/<owner-key>`) |
| `created_at`, `updated_at` | RFC 3339 `Z` | |
| `moved_from` | string \| null | the previous `partition_key` while a workspace move is in flight; null once step 5 (§3) has run |
| `repaired_at` | RFC 3339 \| null | stamped by `doctor --repair` |
| `pending` | object \| null | `{id, from_rev, to_rev}` — `from_rev` null on create; the one field crash recovery reads (§6) |
| `notes` | object | `<uuid>` → `{revision (64 hex), origin, written_at}`; `origin` ∈ `user | extracted_user | extracted_other | imported | external` |
| `tombstones` | array | `{id, origin, supersedes: [uuid], suppressed_sources: [string], forgotten_at}`; a suppressed source is `message:<rowid>` or `task:<id>` |

```json
{
  "schema": 1,
  "partition_key": "proj-3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7-0a1b2c3d4e5f6071",
  "scope": "project",
  "owner_key": "3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7",
  "owner_id": "u_01J8Z3Q9K7X2M4N6P8R0S2T4V6",
  "root": "/Users/x/dev/app/.openalpaca/memory/owners/3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7",
  "created_at": "2026-09-23T10:00:00Z",
  "updated_at": "2026-09-23T10:05:00Z",
  "moved_from": null,
  "repaired_at": null,
  "pending": null,
  "notes": {
    "55e811b6-3220-4758-a47b-034bd1023f38": {
      "revision": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
      "origin": "extracted_user",
      "written_at": "2026-09-23T10:05:00Z"
    }
  },
  "tombstones": [
    {
      "id": "0c7b2e4a-9d13-4c8e-b1f5-6a2d3e4f5a6b",
      "origin": "user",
      "supersedes": ["d4e5f6a7-b8c9-4d0e-9f1a-2b3c4d5e6f70"],
      "suppressed_sources": ["message:4812", "task:7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90"],
      "forgotten_at": "2026-09-23T10:02:00Z"
    }
  ]
}
```

A snapshot that fails to parse, whose `schema` is unknown, or whose `root` is not the directory it was read for, is *invalid* and quarantines the partition (§2).

### `MemoryAccessContext`

```rust
pub struct MemoryAccessContext {
    /// A `global_user.id` row (§3); never from a tool argument.
    pub owner_id: String,
    /// Derived: first 32 hex of sha256(owner_id).
    pub owner_key: String,
    /// The MD-5 marker root of the request workspace; `None` = global partition only.
    pub project_root: Option<PathBuf>,
    /// Who is asking. Set by the constructor, never by a caller.
    pub surface: Surface,             // UserSurface | ModelTool | Extraction
    /// What this call may do; the service refuses anything else even if a tool was routed to it.
    pub capabilities: Capabilities,   // { read: bool, write: bool, forget: bool }
    /// For events, `memory_used` and the recently-used ring.
    pub attribution: Attribution,     // { lane_key: Option<String>, task_id: Option<String>, request_id: Option<Uuid>, source: Option<String> }
}
```

`surface` decides the origin a write records (§4): `UserSurface` → `user`; `ModelTool` → `extracted_user`; `Extraction` → `extracted_user` for the user's own words, `extracted_other` (candidate) for assistant or tool output, and `extracted_other` **active** for a conversation summary (`ExtractionProvenance::Summary`, refused for any other kind, a `fact_key`, a `supersedes` or a replace policy — [09](09-learning-design.md) rev 3.17). `capabilities` replaces rev 3's "allowed operations": for `UserSurface` all three are true; for `ModelTool` it is the sandbox-resolved capability set of the calling agent (`memory_read`/`memory_write`/`memory_forget`), so the service is a second check behind the gate; for `Extraction` only `write` is true. Who constructs it:

| Surface | Owner from | Project root from | `surface` | Writes |
|---|---|---|---|---|
| GUI/CLI chat turn — main loop and skill tier, tool calls | `ToolContext.principal` = `Principal::User { global_id }` (else no owner) | `ToolContext.request_workspace_root` (R22: the `x-workspace-path` header, never CWD-derived) | `ModelTool` | store → `extracted_user`, or a suggestion (LQ11, §4; rev 7.5); update `edit` (a suggestion for a note the user saved, §4) /`archive`; forget (prompted); `confirm` refused — except `memory_confirm`, whose one write runs as `UserSurface` with the human's Approve on its card ([09](09-learning-design.md) §4.2 rule 5) |
| The same turn's automatic recall | `ContextRequest.owner_id` | `ContextRequest.scope.request_workspace_root` | `ModelTool` (read only) | none |
| Background extraction of a conversation (Learning v2) | the session's owner | each row's stored `workspace_root` — the turn's `request_workspace_root` when it ran ([09](09-learning-design.md) LR1) | `Extraction` | `extracted_user` active or a suggestion, `extracted_other` candidate, a conversation summary. **Reads through `learning_view` only**: metadata and bounded excerpts of the notes the chunk brings up, not tool-reachable, never recorded in `memory_used` or the ring; capabilities stay write-only ([09](09-learning-design.md) rev 3.3); the keys it offers and resolves leave out a key held only by the chat model's notes — origin `extracted_user` with no `observed_at` (LQ14, rev 7.4) |
| Task-output extraction (`dispatcher/memory.rs`) | the task's `created_by`, if a `global_user` row | the task's workspace | `Extraction` | `extracted_other` candidates only (tool output) |
| Connector lanes (Telegram, Discord, iMessage) | the linked owner's `Principal::User`; `Principal::External` → no owner | none → global | `ModelTool` | as a chat turn |
| Scheduled skills | `Principal::User { local_user_id }` (`scheduled_skills.rs:203-205`) | `workspace_path: None` → global | `ModelTool`, `unattended` | forget/archive refused |
| Follow-ups | the claimed item's owner | the item's stored `workspace_path` (`followup.rs:44,152,227`) through rule 1 | `ModelTool`, `unattended` per the row | as declared |
| Lead agent | `created_by` — no partition for `system` | `workspace.request_workspace_root` (`lead_agent/mod.rs:257-263`) | `ModelTool` (read only) | none |
| Subagents | inherited from the lead, narrowed | inherited | `ModelTool` (read only) | none |
| `/v1/memories*`, `/v1/persona/user` | `state.local_user_id` | the `x-workspace-path` header (percent-encoded UTF-8, R81, `routes/mod.rs:327`), resolved by the same marker walk; absent → global | `UserSurface` | everything, origin `user` |
| Chat memory commands — `/remember`, `/forget`, `/memories` on an attended GUI/CLI chat turn (source `gui`/`cli`, not `unattended`; [09](09-learning-design.md) §12) — built by `MemoryService::context_for_chat_command`, which refuses any other turn | the turn's `Principal::User { global_id }` (else no owner) | the turn's `request_workspace_root` (R22) | `UserSurface` | store (origin `user`), forget; reads for `/forget` and `/memories` |
| CLI management verbs | through the API | the CLI's canonical CWD as that header (`chat_stream/mod.rs:116,187`); `--global` sends none; `--project <path>` sends that path | `UserSurface` | everything |
| `doctor --repair`, `adopt` | `identity.local_user_id` read under the store lock | the directory being repaired | none — offline, no service | see §6 |

### Tool contracts

Shared enums: `NoteKind` = `fact | preference | decision | procedure | reference | episode`; `NoteScope` = `global | project`; `NoteStatus` = `active | candidate | archived`; `Origin` as above; `Freshness` = `verified | stale | unverified`; `MatchReason` = `title | alias | fact_key | term | prefix | trigram | phrase` (`prefix`, a term matched as the beginning of a word, added by LQ17, rev 7.7 — [09](09-learning-design.md) §6.5.3). `uuid` is the canonical hyphenated lowercase form; `revision` is 64 lowercase hex.

**`memory_search`**

| Property | Type | Required | Notes |
|---|---|---|---|
| `query` | string, 1–512 chars | yes | |
| `kind` | `NoteKind` | no | |
| `scope` | `NoteScope` | no | default: every partition the context allows |
| `limit` | integer 1–50 | no | default 10 ([09](09-learning-design.md) rev 3.14; automatic recall and learning still ask the index for 20) |
| `detail` | `index` \| `snippets` | no | default `index` |
| `date_from`, `date_to` | RFC 3339 or `YYYY-MM-DD` | no | inclusive, on the note's date (`observed_at`, else `created_at`); a bare date is that whole UTC day; `date_from` after `date_to` is `invalid_input` |
| `order_by` | `relevance` \| `newest` \| `oldest` | no | default `relevance` |
| `historical` | boolean | no | default false; true includes superseded and archived notes and, without `as_of`, notes whose validity window has ended |
| `as_of` | RFC 3339 | no | evaluate effective validity at this instant |

Result (MD-14, [09](09-learning-design.md) §13.1, rev 3.14): with `detail: index`, `{ "results": [{ "id", "date", "kind", "scope", "status", "title", "reason", "size", "conflict" }], "count", "partial", "index_state" }` — no body; `status` ∈ `active | superseded | archived | ended`, `size` the body's bytes ÷ 4. With `detail: snippets`, `{ "results": [{ "id", "revision", "title", "kind", "scope", "status", "origin", "excerpt", "heading", "line_range": [from, to], "match": [MatchReason], "freshness", "conflict": boolean, "sources": [...], "valid_from", "valid_to" }], "count", "partial": boolean, "index_state": "ready" | "rebuilding" }` (`valid_from`/`valid_to` RFC 3339 or null, rev 3.8).

**`memory_read`**

| Property | Type | Required | Notes |
|---|---|---|---|
| `ids` | [uuid], 1–10, distinct | yes | the old `id` argument is `invalid_input` |
| `expected_revision` | revision | no | with a single id only; a mismatch is that entry's `revision_conflict` with the current revision |
| `section` | string | no | with a single id only; a heading; returns that section only |

Result ([09](09-learning-design.md) §13.1, rev 3.14): `{ "notes": [...], "count", "budget_bytes" }`, one entry per id in the order asked — the read object `{ "id", "revision", "title", "kind", "scope", "status", "origin", "frontmatter": {...}, "body", "truncated": boolean, "path", "sources", "supersedes": [uuid], "superseded_by": [uuid], "freshness" }`, or `{ "id", "error": "not_found" }` / `{ "id", "error": "revision_conflict", "current_revision" }` in its place. The bodies share 32 KiB, split evenly in the max-min sense (a short body is never cut to make room); another owner's id answers exactly as an id no note ever had.

**`memory_timeline`** (MD-14, [09](09-learning-design.md) §13.1, rev 3.14)

| Property | Type | Required | Notes |
|---|---|---|---|
| `anchor` | uuid | one of three | the timeline around that note; a candidate is `invalid_input`, another owner's `not_found` |
| `query` | string, 1–512 chars | one of three | the first hit of a historical search is the anchor; no hit draws an empty timeline with `anchor: null` |
| `date` | RFC 3339 or `YYYY-MM-DD` | one of three | notes dated before it, then at or after it |
| `before`, `after` | integer 0–20 | no | default 5 each |

Result: `{ "anchor": {"id"} | {"date"} | null, "results": [index row], "count", "index_state" }` in date order, each row's `reason` `before`, `anchor` or `after`; superseded, archived and ended notes are listed and marked by `status`, candidates never; every partition the context reads. Refusal (rev 7.4): `index_rebuilding` while a whole rebuild of the search index is owed or running, or starts under the read — at once, never after waiting for it.

**`memory_store`**

| Property | Type | Required | Notes |
|---|---|---|---|
| `title` | string, 1–200 chars | yes | |
| `body` | string, ≤ 32 KiB | yes | |
| `kind` | `NoteKind` | yes | |
| `scope` | `NoteScope` | no | default by kind (§3 rule 2) |
| `aliases` | [string], ≤ 16 | no | |
| `tags` | [string], ≤ 16 | no | |
| `fact_key` | string, `[a-z0-9][a-z0-9._-]{0,63}` | no | |
| `sources` | [{ `type`: `message` \| `task`, `id` }], ≤ 8 | no | verified against the DB; an unknown id is refused |
| `valid_from`, `valid_to` | RFC 3339 | no | |
| `supersedes` | { `id`: uuid, `expected_revision`: revision } | no | creates the successor (§6) |

Result: `{ "id", "revision", "scope", "origin", "superseded": [uuid] }` — `superseded` the notes this one retired; for a suggestion (LQ11, rev 7.4) the tool's result also carries `"suggestion": true`, `"replaces"` (the note it names) and `"note"`, a sentence telling the model the change waits for the user and how the user confirms it, and its `superseded` is `[]`, as `memory_update`'s is for the suggestion an edit becomes: nothing is retired until the user confirms (rev 7.5). An exact duplicate of a note the user saved answers that note, `dedup: existing` (rev 7.5). Refusals: `revision_conflict` (the target moved), `not_found`, `invalid_input` (a target in the other partition than the `scope` named — ruling R4). `pinned` is not a tool input. *(Before rev 7.4: `authority_conflict` for a `ModelTool` supersede of a `user` note.)*

**`memory_update`**

| Property | Type | Required | Notes |
|---|---|---|---|
| `id` | uuid | yes | |
| `expected_revision` | revision | yes | |
| `action` | `edit` \| `archive` \| `confirm` | yes | |
| `patch` | { `title?`, `body?`, `aliases?`, `tags?`, `fact_key?`, `valid_from?`, `valid_to?`, `review_after?` } | with `edit` | untouched fields are preserved byte-for-byte |
| `reason` | string, ≤ 500 chars | no | |

Result: `{ "id", "revision", "status", "superseded": [uuid], "dismissed": [uuid] }` — the note a confirmed suggestion retired and the other suggestions for it that the confirm withdrew, empty for every other update ([09](09-learning-design.md) §3.5). A `ModelTool` edit of a note the user saved, edited by hand or imported answers the **suggestion's** `id`, `revision` and `status: candidate`, and the tool's result adds `"suggestion": true`, `"replaces"` and `"note"` (LQ11, rev 7.4). Refusals: `revision_conflict` (with `current_revision`), `confirm_refused` (`ModelTool`, before any card), `authority_conflict` (`ModelTool` `archive` of a `user` note, an edit of one that changes nothing a suggestion carries, or `edit` of a suggestion — S10), `invalid_input` (`archive` of a suggestion on every surface — S9), `not_found`.

**`memory_confirm`** (MD-12, [09](09-learning-design.md) §4.1, rev 3.7)

| Property | Type | Required | Notes |
|---|---|---|---|
| `id` | uuid | yes | a candidate of origin `extracted_user` with non-empty `supersedes` and a user-authored cited row |
| `expected_revision` | revision | yes | the suggestion's, as listed in `<memory_suggestions>` |
| `target_revision` | revision | yes | the replaced note's, as listed |

Result: `memory_update`'s. Refusals: `revision_conflict`, `not_found`, `invalid_input` (a target no longer in force — nothing is left to replace, undo the suggestion instead — or a malformed `expected_revision`/`target_revision`), `not_a_suggestion` (an inferred or non-suggestion candidate: the Memory view and `openalpaca memory confirm` take it), `approval_required` (no human Approve for this call). Every refusal but `approval_required` is given by the preview hook, before any card (`memory/service/in_chat.rs`: `suggestion_preview` and `confirm_suggestion` check the revisions, `check_revisions`, and `suggestion_preview` the target's effective status; the file layer checks it again at the confirm).

**`memory_forget`**

| Property | Type | Required | Notes |
|---|---|---|---|
| `targets` | [{ `id`: uuid, `expected_revision`: revision }], 1–20 | yes | |
| `reason` | string, ≤ 500 chars | no | |

Result: `{ "results": [{ "id", "status": "forgotten" | "revision_conflict" | "not_found", "current_revision"? }], "retained": { "transcript": boolean, "session_logs": boolean, "note": string } }`.

**`memory_list`**

| Property | Type | Required | Notes |
|---|---|---|---|
| `kind` | `NoteKind` | no | |
| `status` | `NoteStatus` | no | default `active` |
| `scope` | `NoteScope` | no | |
| `cursor` | string | no | opaque, from a previous page |
| `limit` | integer 1–100 | no | default 50 |
| `include_history` | boolean | no | superseded notes; default false |
| `include_candidates` | boolean | no | default false |

Result: `{ "items": [{ "id", "revision", "title", "kind", "scope", "status", "origin", "updated_at", "pinned", "supersedes": [uuid] }], "next_cursor": string | null }` (`supersedes`: [09](09-learning-design.md) S8, rev 3.5; the same item in `recently-learned`).

### `/v1/memories` and `/v1/persona/user`

All under the bearer middleware, all in the shared `api_error` envelope `{"error":{"code","message"}}` (`routes/mod.rs:131`) — the extension routes' flat envelope stays the one exception. A mutation's **`If-Match: "<revision>"`** header is the only place a revision is sent; a body `expected_revision` is refused as `MEMORY_INVALID_INPUT`. A successful read or write answers with `ETag: "<revision>"`. `x-workspace-path` selects the project partition as on `/v1/chat`.

| Method | Path | Body / query | Response | Errors |
|---|---|---|---|---|
| GET | `/v1/memories` | `?kind&status&scope&cursor&limit&include_history&include_candidates` | `memory_list` result | |
| GET | `/v1/memories/search` | `?q&kind&scope&limit&historical&as_of&detail&date_from&date_to&order_by` | `memory_search` result; `detail` defaults to `snippets` here ([09](09-learning-design.md) rev 3.14) | |
| GET | `/v1/memories/timeline` | `?anchor` \| `?q` \| `?date`, `&before&after` | `memory_timeline` result ([09](09-learning-design.md) §13.1) | `404` (unknown anchor), `400`, `503 MEMORY_INDEX_REBUILDING` (rev 7.4) |
| GET | `/v1/memories/recently-learned` | `?since` (RFC 3339 or `7d`, default 7 days) `&limit` (default 50) | `{ "items": [list item…] }` | |
| GET | `/v1/memories/suggestions` | `?limit` (default 20, max 100) | `{ "items": [{ "id", "revision", "title", "knowledge", "reason", "created_at", "observed_at", "origin", "scope", "sources", "older_count", "target": { "id", "revision", "title", "excerpt", "origin", "scope" } }] }` — newest first, one per note it would replace; a suggestion whose target is gone or not effectively active is not listed ([09](09-learning-design.md) §3.5, rev 3.5) | |
| GET | `/v1/memories/recently-used` | `?limit` (default 50, max 200) | `{ "items": [{ "id", "revision", "reason", "delivered_at", "lane_key", "task_id" }] }` | in-memory ring, cleared at restart |
| GET | `/v1/memories/doctor` | | the read-only report (§6) | |
| POST | `/v1/memories` | `memory_store` body | `201` + `memory_store` result, origin `user` | `409 MEMORY_REVISION_CONFLICT` |
| POST | `/v1/memories/forget` | `{ "targets": [...] , "reason"? }` | `memory_forget` result | |
| POST | `/v1/memories/sync` | | `{ "scanned", "changed", "index_state" }` | |
| GET | `/v1/memories/{id}` | `?section` `?revision` (a `.history` revision) | `memory_read` result + `ETag` | `404` |
| GET | `/v1/memories/{id}/history` | | `{ "revisions": [{ "revision", "written_at" }] }` | `404` |
| POST | `/v1/memories/{id}` | `If-Match`; `{ "action": "edit" \| "archive" \| "confirm", "patch"?, "reason"?, "target_revision"? }` — `target_revision` with `confirm` only, **required** to confirm a suggestion (S5) | `memory_update` result + `ETag` | `409` (the candidate's or the target's revision moved; the moved note's revision as `ETag`), `428`; `400` without `target_revision` for a suggestion, for a target no longer effectively active ([09](09-learning-design.md) rev 3.6) and for a candidate naming several notes; `404` for a target that is gone |
| POST | `/v1/memories/{id}/undo` | `If-Match` — an `extracted_user` note, or any candidate | `{ "id", "status": "withdrawn", "restored": [uuid] }` (lead ruling, 2026-09-25, below) | `409 MEMORY_NOT_UNDOABLE` |
| DELETE | `/v1/memories/{id}` | `If-Match` | `{ "id", "status": "forgotten", "retained" }` | `409`, `428` |
| GET | `/v1/persona/user` | | `{ "content" }` + `ETag: "<sha256>"` | |
| PUT | `/v1/persona/user` | `If-Match`; `{ "content" }` | `{ "etag" }` | `409 PERSONA_REVISION_CONFLICT`, `428`, `400 PERSONA_INVALID` |

Error codes: `MEMORY_NOT_FOUND` (404), `MEMORY_REVISION_CONFLICT` (409; the message carries the current revision and the response carries it as `ETag`), `MEMORY_QUARANTINED` (409; the partition's snapshot is missing or invalid), `MEMORY_NOT_UNDOABLE` (409), `MEMORY_INDEX_REBUILDING` (503; the timeline while the index is rebuilt — rev 7.4, listed here in rev 7.6), `MEMORY_INVALID_INPUT` (400), `MEMORY_IF_MATCH_REQUIRED` (428), `PERSONA_REVISION_CONFLICT` (409), `PERSONA_IF_MATCH_REQUIRED` (428), `PERSONA_INVALID` (400; the same validation `update_persona target=user mode=replace` applies). **Cursor rule:** `next_cursor` is an opaque, URL-safe base64 of `(updated_at, id)` of the last item, valid for the same filter set; a cursor from another filter set is `MEMORY_INVALID_INPUT`. **The persona writer** is the `update_persona` path — `backup_if_exists`, `prune_backups`, `atomic_write`, then `SystemEvent::UserProfileUpdated` — and the route swaps the daemon's `user_document` itself after the write, so a `GET` after a `PUT` is consistent before the file watcher fires. **`/v1/memories/doctor`, `recently-used` and `sync` are on no model surface**; that is all "management-only" means.

**Undo answers a withdrawal (lead ruling, 2026-09-25; [09](09-learning-design.md) rev 3.5).** The frozen row said `{ "id", "status": "forgotten" }` and the first build also carried the forget's `retained`, whose note says the cited sources "will not be re-extracted". An undo is a withdrawal, which suppresses nothing, so `retained` would always read false. It answers `{ "id", "status": "withdrawn", "restored": [uuid] }`, where `restored` names the notes the withdrawn one had retired and that are effective again (empty for a suggestion). `memory_changed` still says `change: "forgotten"` for a withdrawal — the note is gone, and the `change` enum has no other word.

**An index failure after a landed write no longer fails the write (lead ruling, 2026-09-25; residue R6, `7d5174e9`).** §6 step 4 runs after the durable step 3, and the index is a disposable cache (§2). So a failure there is logged — one WARN, the partition key and the error, no note text — the partition's cached fingerprints are dropped so the next reconciliation re-indexes it whole, and the next search's stat scan re-indexes first; the caller gets `Ok` with what was written, and `memory_changed` is published. This holds for store, update (confirm included), forget and withdraw; `sync` and the reconciliation tick still return an index error, because they are the re-index and retrying them is idempotent.

### Events and the `done` frame

- **`memory_changed`** (`SystemEvent::MemoryChanged` → `event_bridge.rs` arm → `ServerEvent::MemoryChanged`; `events/persistence.rs` arm returns `Ok(0)`, persisted nowhere, like `daemon_shutting_down`): `{ "type": "memory_changed", "partition_key", "note_id", "change": "created" | "updated" | "archived" | "confirmed" | "forgotten" | "external", "revision": string | null, "origin", "surface": "user" | "model_tool" | "extraction", "ts" }`. Published by every service write and by a reconciliation scan that detects an external change. The GUI maps it to `[qk.memory.all()]` in `invalidationKeysFor`.
- **`memory_used`** on `HandleResult`, `GatewayResponse` and SSE `done`: `[{ "id", "revision", "reason": "pinned" | "recall" | "index" | "tool" }]`, omitted when empty, **live only** — like `attachments_skipped`, nothing persists it, and `GET /v1/chat/history` does not serve it. `pinned`, `recall` and `index` are the automatic path — `index` a note the recall only listed, its line and not its body ([09](09-learning-design.md) §13.2, rev 3.15); `tool` is a note the model fetched through `memory_search`, `memory_timeline` or `memory_read` in that turn (an index row counts). Main-loop and skill-tier turns report it; a lead or subagent run has no `done` frame and appears only in the `recently-used` ring, with its `task_id`.
- **`ToolConfirmationRequested`** (`SystemEvent`, `ServerEvent` and the SSE `confirmation_requested` frame) and each row of `GET /v1/chat/confirmations` carry `preview` — the daemon-written text of a `memory_confirm` card, `null` on every other card — and `per_call` ([09](09-learning-design.md) §4.4, rev 3.7). Both are **live only**: the `event_log` row keeps ids, so a later-forgotten note's title never survives there. No event variant was added.

### CLI syntax

Every daemon-backed verb accepts `--global` or `--project <path>`; the default is the CLI's CWD. Ids may be abbreviated to a unique prefix.

```text
openalpaca memory list    [--kind K] [--status S] [--scope global|project] [--history] [--candidates] [--limit N]
openalpaca memory search  <query> [--kind K] [--scope S] [--limit N] [--historical] [--as-of RFC3339]
                          [--since DATE] [--until DATE] [--order relevance|newest|oldest]   snippets; --limit default 10
openalpaca memory timeline [<id-prefix> | --query Q | --date DATE] [--before N] [--after N]   default 5 each
openalpaca memory show    <id> [--revision <rev>] [--raw]
openalpaca memory add     --kind K --title T [--scope S] [--alias A]... [--tag T]... [--fact-key KEY]
                          [--supersedes <id>] [BODY | -]        body: the argument, stdin ("-"), or $EDITOR when neither
openalpaca memory edit    <id>                                    GET → $EDITOR → POST with the fetched revision;
                                                                  on 409 prints both revisions and names the temp file holding your text
openalpaca memory suggestions [--limit N] [--format json]         the pending suggestions, one line each
openalpaca memory confirm <id>                                    POST action=confirm with the fetched revision; for a suggestion
                                                                  also the replaced note's revision, read at that moment
openalpaca memory undo    <id>                                    POST undo with the fetched revision (withdraws; dismisses a suggestion)
openalpaca memory forget  <id>... [--yes]                         prints title and revision per id, asks unless --yes, then DELETE each
openalpaca memory sync                                            POST /v1/memories/sync
openalpaca memory doctor  [--repair]                              without --repair: GET /v1/memories/doctor when a daemon runs, else a local read-only scan;
                                                                  --repair: local, takes the singleton lock, refuses while a daemon runs
openalpaca memory adopt   <owner-key>                             local, singleton lock
openalpaca persona show   user
openalpaca persona edit   user                                    GET → $EDITOR → PUT with If-Match; on 409 as `memory edit`
```

`--supersedes <id>` fetches the target's current revision first and sends it as `expected_revision`, so a race is a visible 409, never a silent overwrite.

## 7b. Rulings pinned by the slice-1 fixtures

The invariant fixtures (`crates/openalpaca_storage/tests/fixtures/memory/`, slice 1) had to take one reading wherever rev 4 was silent. Each reading below is adopted as a design ruling on 2026-09-23 by the session lead, because none reverses an owner decision or widens a component's job; the owner may send any of them back as a design revision, in which case the fixture changes with it. Slices 2 and 3 implement them as written.

| # | Reading | Fixture |
|---|---|---|
| P1 | An explicit `scope: project` from a request that resolves no project root is refused `invalid_input`, not written to global | 02 |
| P2 | `as_of` alone never includes superseded or archived notes; `historical: true` is the only switch; a bound is half-open at the instant | 11 |
| P3 | A search result's `conflict: true` marks the note that **carries** an un-honored `supersedes` link, and the same-`fact_key` **loser**; the winner and the link's target read `conflict: false` | 10, 18, 24 |
| P4 | `memory_search` returns a same-`fact_key` loser flagged `conflict: true`; automatic recall excludes it (§3 rule 3 and §5 step 5 read together) | 18, 24 |
| P5 | The substring pass for 1–2 character queries has no `MatchReason`; those steps check ids only | 12, 23 |
| P6 | `source_forgotten` has no §7a wire field; the fixtures read it from a search result's `sources_status` | 16 |
| P7 | A `ModelTool` write from a context whose `capabilities.write` is false is refused `fixture:capability_denied`, naming `memory_write` | 20 |
| P8 | Exact dedup (same owner, same partition, same bytes) answers with the existing note's id rather than creating a second file; the `dedup` expectation reads it | 09 |
| P9 | The `authority_conflict` and `confirm_refused` messages name the Memory view and the `openalpaca memory` verb | 10, 18 |
| P10 | An extraction on a daemon with no configured model reports `unavailable` with one WARN and the turn is unaffected | 19 |
| P11 | `doctor` names a stale or abandoned move snapshot by the directory it is bound to | 17 |
| P12 | A `forget` whose target id no longer exists answers `not_found`, not `revision_conflict`, whatever revision was sent | 16 |
| P13 | A `memory_changed` published by a reconciliation scan (`change: external`) carries no `surface`: §7a's enum is `user \| model_tool \| extraction` and a scan is none of them; the fixture omits the field until the design adds a value or makes it optional | 13 |

Two consequences for §7a: `sources_status` is a field of a search result (P6), and `memory_changed.surface` is optional, absent for a change a reconciliation scan discovered (P13).

## 7c. Rulings pinned by the slice-2 library

Slice 2 (`crates/openalpaca_storage/src/memory_files/` and `memory_index/`, landed 2026-09-23) took these readings where §4–§7b were silent or, in one case, too narrow. Adopted as rulings by the session lead on the same basis as §7b; slice 3 builds on them as written.

| # | Ruling | Where |
|---|---|---|
| Q1 | `pending` carries `origin` beside `{id, from_rev, to_rev}`: recovery's "finish step 3" must record the post-write origin (fixture 15). §7a's `pending` row is widened accordingly. | `memory_files/snapshot.rs` |
| Q2 | The flat §4 frontmatter is parsed and serialized by a hand-written canon (strings, lists, timestamps, null, booleans, the small `sources` objects); unknown keys are preserved verbatim; no YAML crate. | `memory_files/note.rs` |
| Q3 | The library has its own hardened filesystem primitives (`fsx.rs`): the store's `write_atomic`/`create_private_dir` are private to `store/mod.rs`. `state/` is created private (0700) by `store::state_dir()` before the first snapshot. | `memory_files/fsx.rs` |
| Q4 | Every path component below the store root is checked, `owners` and `.history` included; a refused path returns `PathRefused` naming the component and the library logs nothing — the service logs the one WARN. | `memory_files/partition.rs` |
| Q5 | A retitle writes onto the current path and then renames, so there is never a duplicate-id window; recovery repairs a stale slug. | `memory_files/partition.rs` |
| Q6 | `repair` on a healthy partition checks its preconditions (a duplicate id is refused naming both files) and rebuilds nothing. `adopt` refuses an owner key that is not 32 lowercase hex, and `snapshot_path_for` refuses anything but `home-<32hex>` / `proj-<32hex>-<16hex>`. `adopt` re-binds a project snapshot whose root is gone; the CLI runs `classify_move_snapshots` first. | `memory_files/partition.rs`, `mod.rs` |
| Q7 | `source_forgotten` = a suppressed source whose tombstone `forgotten_at` is **at or after** the note's `written_at`; an edit lifts it. `doctor` reports one conflict per pair. | `memory_files/authority.rs` |
| Q8 | Exact dedup (P8) keys on kind + title + body + `fact_key` + aliases + tags among effectively active notes, since two fresh UUIDs never share bytes. | `memory_files/partition.rs` |
| Q9 | Query routing: a term of three or more characters is searched in both FTS tables; a one- or two-character term is substring-searched when it is CJK or when the query has no longer term; English stop words are dropped from routing only when another term remains; an identifier's parts are a fallback used only when the literal matches nothing. *(Revised by LQ17, revs 7.7–7.8 — [09](09-learning-design.md) §6.5.2. The `unicode61` side takes every term, its text and phrases split at script boundaries, as a prefix query when its last token has three or more characters, unless the term carries an unsegmented character or that token is a stop word; when that query fills its cap, the prefix terms are searched again as whole words in titles, aliases and tags. The trigram side is two tables, one over titles, aliases and tags, which every term of three or more characters searches, and one over the bodies of unsegmented-script chunks, which only a term carrying an unsegmented character searches. The rest of this ruling holds.)* | `memory_index/query.rs` |
| Q10 | Candidates are never returned by search. `partial` is also set when the 256-chunk FTS candidate cap is hit — slice 3 must not read `partial` as "search again". `same_key_loser` is a separate result field from `conflict`. `MAX_TERMS` is 32. | `memory_index/search.rs` |
| Q11 | Verification (§5 step 7: re-read and re-hash the selected files) is the service's job, not the index's; `reconcile` reports differences and the caller applies them. | `memory_index/reconcile.rs` |
| Q12 | The cache runs with a 64 MiB page cache and mmap, and `rebuild` runs FTS `optimize` plus a WAL checkpoint; measured at 10K notes on an Apple M5 Max: search + verification p50 8–14 ms, p95 17–27 ms, cache ≈ 8× the corpus (the trigram index), rebuild ≈ 8 s in release — off the request path. *(LQ17, revs 7.7–7.8 — [09](09-learning-design.md) §6.5.5. The revised layout, measured on a prototype (rev 7.8's form), is 40 MiB (3.3×) at 10K with p95 9 ms, 113 MiB (3.0×) at 30K with p95 17 ms, and 367 MiB (2.9×) at 100K with p95 47 ms, where the current layout is 109 MiB and 11 ms, 321 MiB and 29 ms, and 1,066 MiB and 149 ms; with gold-like headers, 504 MiB and 62 ms at 100K against 1,225 MiB and 136 ms. The build measures it again.)* | `memory_index/` |

### Rulings pinned by slice 3

Adopted on the same basis as §7b/§7c (plan §7a S3-R6, folded in at rev 4.4):

| # | Ruling | Where |
|---|---|---|
| R1 | Pinned notes are delivered oldest-first. | `memory/service/` recall |
| R2 | The 2 KiB pinned sub-budget counts note text; wrappers count against the 8 KiB total. | recall |
| R3 | `Freshness::Stale` means the pre-read stat scan found the file changed; a stat scan runs before every read, stricter than the 30 s reconciliation. *(Kept by owner decision LQ16, rev 7.4: the scan lists `notes/` with one bulk directory stat where the platform has one — `getattrlistbulk` on macOS — and `readdir` + `lstat` elsewhere, the same entries and fields.)* | recall, `memory_files/dirstat.rs` |
| R4 | A `supersedes` target in the other partition is `invalid_input`. *(Rev 7.4, LQ11: on the `ModelTool` surface a replacement that names no `scope` goes to its target's partition, as learning's does — ruling LR10; with a `scope` naming the other partition, and on every other surface, R4 holds.)* | mutations |
| R5 | `memory_store` answers `dedup` (with the existing id) for an exact duplicate. | mutations, §7b P8 |
| R6 | Mutations return `MemoryChange` records; the daemon publishes `memory_changed` from them (3.8). | `service/`, `event_bridge.rs` |
| R7 | A body the read cut at 32 KiB is never written back: the GUI's editor is disabled for it and `openalpaca memory edit` refuses it before opening the editor, naming the file. The update route cannot receive one: a note over 32 KiB does not parse, so its read, edit and delete are `404` and `doctor` lists the file as `oversized` (route test pins both directions). | 3.9 and its follow-up `5aa4c1a5`, `d48e427a` |
| R8 | `GET /v1/memories/recently-used` has no GUI reader yet (the *notes used* note comes from `done.memory_used` live); DESIGN_SPEC §5.7 records it as not built. | GUI |
| R9 | A same-`fact_key` tie on authority (§3 rule 3) is not broken by `updated_at` first: any edit bumps it, so fixing a typo in the older note made it the winner again. Among the notes tied on authority, the newest `observed_at` wins when **every** one of them has one (the clock is chosen once for the tied set, because "when both have one" is not an order over three notes and the winner would depend on file order); after that (or first, when any lacks one) the newest `created_at` (the note first written most recently), then the newest `updated_at`, then the lowest id. A consequence for the owner: `confirm` records a candidate as `user` and moves its `updated_at`, never its `created_at`, so unless both notes carry `observed_at` a confirmed candidate loses a same-key tie to a `user` note written after the candidate and before the confirmation — the confirmation is not the newer knowledge. Prompted by the D evaluation's source-conflict misses ([08](08-evaluation-report.md) §3.3). | `memory_files/authority.rs` (`same_key_winner`) |

## 8. Module boundaries and cost controls

| Location | Responsibility |
|---|---|
| `openalpaca_storage::memory_files` (new) | Note parse/serialize (canon, §4), confined atomic writes, `.history`, revisions, control snapshot (§7a), recovery, owner and partition keys, the `.gitignore` assertion, modes; no LLM dependencies |
| `openalpaca_storage::memory_index` (new) | Separate cache DB, deterministic chunking, FTS5 `unicode61` + trigram with the §5 term routing, the literal query builder, reconciliation scan, rebuild, the `#[ignore]`d bench |
| `openalpaca_storage::migrations` | The baseline re-squashed to 43 without any memory table, and the 1–42 refusal (plan §4). There is no conversion module. |
| `openalpaca_core::memory::service` (new) | `MemoryAccessContext` (§7a), recall, corrections, origin/authority, extraction policy (MD-2), the recently-used ring, `memory_used`, tool-facing API |
| `openalpaca_core::prompt_ctx::sources::memory` | The single bounded injection adapter, consuming service relevance and citations, outside the cached dynamic-context layer |
| `openalpacad` services / background / routes / events | Build one service; run the reconciliation timer; the move-quiesce hook; `/v1/memories*` and `/v1/persona/user`; the `memory_changed` bridge arms. No embedder, no indexer, no decay worker. |

Prefer existing crates over a new framework or crate graph: `uuid`, `sha2`, `serde_yaml` (one new edge into the storage crate, already a workspace dependency), the store path helpers, `write_atomic`, `slugify`, the artifact store's write discipline, and cost accounting. `notify` stays out of core. No new crate and no new feature is needed. Record the following without logging note bodies:
- search time
- candidate and chunk counts
- index state
- scope
- injection bytes
- dropped-note reasons
- extraction cost and count

## 9. First-release exclusions

The first release has none of these:
- mandatory Git
- remote synchronization
- an embedding fallback
- a graph service
- automatic repository-wide document ingestion
- all-agent autonomous writers
- autonomous folder reorganization
- time-based deletion of durable notes
- a connector-audience model (MD-4, its own design)
- import and export verbs (origin `imported` is reserved for the import feature below; a hand-copied file is `external`)

Importing an arbitrary Markdown knowledge base is a separate, bounded feature with a visible source snapshot and a re-import policy. The dead KB importer is deleted, not reused.

These exclusions keep the replacement reviewable. Expand only after the evaluation in the plan identifies a concrete deficiency.

## Revision log

- **rev 7.8 (2026-09-27), LQ17's revision answers its review ([09](09-learning-design.md) rev 3.37; design text only, measured on a prototype, not yet built):** §5 step 4 is amended again, and §7c Q9 and Q12 with it. §7a is unchanged: `MatchReason` keeps `prefix`.
  - **§5 step 4.**
    - The `unicode61` table's text, and the phrases it is queried with, are split at script boundaries, so a Latin word glued to Chinese (`和commit`) is a word. Rev 7.7 said a body trigram hit counts in ranking only for a term carrying an unsegmented character, but its prototype searched that table for every term and credited a Latin substring in any chunk carrying one CJK character — `now` inside the template heading `knowledge` then matched every Chinese note and no English one.
    - The body trigram table is now searched only by a term carrying an unsegmented character.
    - Such a term, and a stop word, is never a prefix, so a CJK term's reason no longer depends on where in a run it stands.
    - When the word table's query fills its candidate cap, the terms it sent as prefixes are searched again as whole words in titles, aliases and tags. At 30,000 notes a prefix shared by many longer words (`work*`) otherwise crowded two relied-on notes, found only by a tag, out of the candidates.
    - What no table finds now includes a Latin term inside a Latin word of a Chinese body, so 09 §6.5.4's count is 928 term–note matches, 722 of them the template's headings, where rev 7.7 counted 641 and 452.
  - **§7c Q9 and Q12** carry the same routing and the rerun figures. At 30K notes the prototype's p95 is 17 ms against the current layout's 29 ms; at 100K, 47 ms against 149 ms.
  - **The measured case** (09 §6.5.5):
    - the retrieval proxy, row for row as rev 7.7's prototype but for a known tie;
    - the proxy's questions among 30,000 distractors, where no relied-on note leaves the candidates;
    - the learning lookup and L8's coverage search, where nothing is lost;
    - a bench mix with gold-like headers.
  - **Rev 7.7's "no net loss" is corrected:** index tuning's listed-or-delivered loses one row (tq-0032), which LQ17's rule makes the lead's to rule on.
- **rev 7.7 (2026-09-27), LQ17's index layout revision ([09](09-learning-design.md) §6.5, rev 3.36; design text only, measured on a prototype, not yet built):** a **contract change** to §7a and a design revision of §5 step 4. §7c Q9 and Q12 are amended in place.
  - **§5 step 4.** The rule "every term of three or more characters is ORed against both tables" (07 G23) no longer holds for bodies.
    - The `unicode61` table takes every term, as a prefix query when its last token has three or more characters.
    - The trigram tables hold titles, aliases and tags, and the bodies of chunks in scripts `unicode61` cannot split.
    - A term inside a word of a space-separated body is no longer found. 09 §6.5.4 measures what that drops: 641 term–note matches over the evaluation questions, 452 of them the note template's own headings (`now` in `knowledge`, `check` in `recheck`).
  - **§7a.** `MatchReason` gains `prefix`. Every surface prints the reason through `as_str`, and the GUI types `match` as `string[]`.
  - **§7c Q9 and Q12.** Q9 gains the revised routing and Q12 the prototype's figures; the rest of both rulings holds. No route, event, field or migration changes, and the disposable cache moves to index schema 4.
  - **The measured case** is 09 §6.5.5. On the retrieval proxy the prototype shows no net loss against the current layout, in both recall modes and over both question sets, and every lost row is explained. *(Corrected in rev 7.8: index tuning's listed-or-delivered falls from 95 to 94 — one row, tq-0032 — which is a net loss on that total, for the lead to rule on.)* At 100K notes (average mix) the cache goes from 1,066 MiB to 367 MiB and the p95 of search + verification from 145 ms to 40 ms.
- **rev 7.6 (2026-09-27), the L8 product slice's repair round 2 ([09](09-learning-design.md) rev 3.35, `04df5bce..1a6a69b8`):** §4 and §6 gain two rules for the chat model's suggestions, §4 and §7 state confirming in chat as rule 5 reads since LQ11, §7a's error-code list is completed, and rev 7.4's record is corrected. No field or route changes.
  - **§4, the chat model's edit suggestion cites what the note it copies cites**, after the turn's typed row, at most eight. Its text is the saved note's, verbatim but for the patch, and it cited the turn's row alone, so a forget of a sibling citing the note's row left the confirmed copy of that row's text in automatic recall, where a user-directed edit of the note, keeping its sources, would have gone out with it. The card and the `<memory_suggestions>` line now date the assistant's suggestion by the day it was saved — as the Memory view and the CLI already did — since its newest cited typed row may be the copied note's, older.
  - **§6, a forget withdraws the pending suggestions that name the forgotten note.** None can be confirmed any more (`not_found` on the target), and the chat model's edit suggestion left behind was the forgotten note's text, which `memory_list {status: candidate}` and `memory_read` handed back to the model. Each is a withdrawal: no source is suppressed. [09](09-learning-design.md) §3.5's "stays a candidate" now holds for a target gone any other way.
  - **§4 and §7's `memory_confirm` row:** confirming in chat is for a suggestion that cites a message the user typed — learning's, or the chat model's own — no longer "from the user's own words only", which the model-facing refusals also said: `memory_update`'s confirm now calls it "a suggestion waiting for the user", and `not_a_suggestion` states rule 5.
  - **§4, recorded, not built:** a store naming saved note W under the key of another saved note W2 outranks W2 when confirmed, and nothing names W2; shared with learning's `replaces`.
  - **§7a's error codes gain `MEMORY_INDEX_REBUILDING` (503)**, which the timeline's route row has carried since rev 7.4, and rev 7.4's last bullet no longer says "no contract changed"; the Extraction row of §7a's per-surface table states LQ14, as rev 7.4's bullet said it did. Rev 7.4's range left out `30689e20`, a one-line P8 change after its documentation commit ([09](09-learning-design.md) rev 3.35 records it).
- **rev 7.5 (2026-09-27), the L8 product slice's repair round 1 ([09](09-learning-design.md) rev 3.34, `f4a1e5e6..76127cbf`):** §4 and §7a are corrected and completed; one field of a §7a result changes for a suggestion.
  - **Rev 7.4's "where it was `authority_conflict`" held for a `user` note only.** Before LQ11 a `ModelTool` replacement of an `external` or `imported` note was applied at once (`partition.rs` refused `Origin::User` alone), so LQ11 turned a refusal into a suggestion for the one and an applied change into a suggestion for the other; §0's LQ11 row and §4 now say so. **What stays direct** is written down in §4: archive of an `external` or `imported` note, an edit of one that changes only `review_after` (which re-records it as `extracted_user`, after which a replacement of it stands), and an edit of a learned note that sets a saved note's key — so rev 7.4's "never retires a saved note" holds for the replacements §4 lists, not for these.
  - **§4, the note a store meets under its key** is the R9 winner of the partition it would land in — R9 is per partition — the first partition holding a saved winner, in the order LR11 reads: rev 7.4 stopped at the first partition holding any winner, so a learned note in the project hid a saved note at home and the tool's note landed at home under the saved note's key. And a `supersedes` naming a note that is not saved names instead the saved R9 winner of the store's key in that note's partition, when there is one.
  - **§4, §7a: an exact duplicate of a saved note is that note** (`dedup: existing`), as `memory_store`'s description says; rev 7.4 made it a "replace X with X" suggestion.
  - **§7a `memory_store`'s result:** for a suggestion `superseded` is `[]` — `replaces` names the note — as `memory_update`'s is for an edit's suggestion; the field was the frontmatter's link, which a model could read as a retirement.
  - **§4, the in-chat card:** the chat model's suggestion is said to be the assistant's there too ("suggested by the assistant on <date>"), and a `memory_update` edit that becomes a suggestion is prompted first, as `memory_update` always is.
  - **LQ14 (rev 7.4's second bullet):** whether a key is learning's is decided over every note of the partitions learning reads together, whatever its status ([09](09-learning-design.md) §2.3 step 4); no contract changes.
- **rev 7.4 (2026-09-27), the owner's decisions of 2026-09-26 on the L8 evaluation, built in the L8 product slice ([09](09-learning-design.md) rev 3.33, `6120812b..79f00f49`):** a **contract change** to §7a and a ruling amended in §7c.
  - **LQ11 (§0 G2, §4, §7, §7a `memory_store` and `memory_update`, §7c R4).** A chat model's replacement of a note the user saved, edited by hand or imported is a **suggestion**, written through learning's path, where it was `authority_conflict` *(for a `user` note; one of an `external` or `imported` note was applied at once — corrected in rev 7.5)*: a `supersedes` naming such a note, a store with no `supersedes` whose `fact_key`'s R9 winner is one, and an edit of one that changes what a suggestion carries. `memory_store` loses its `authority_conflict` refusal; `memory_update` keeps it for archive of a `user` note and an edit that changes nothing a suggestion carries, and answers the suggestion's id for an edit that becomes one; the tool results add `suggestion`, `replaces` and `note`. The suggestion cites the turn's own row the user typed, so §4.2 rule 5 of 09 lets the in-chat card confirm it. On the tool path a replacement with no `scope` goes to its target's partition (LR10), amending R4 there only. G2 holds: no `user` is minted, nothing active changes until a user-directed confirm.
  - **LQ14 (§7a's Extraction row reads through `learning_view`; the row itself states it since rev 7.6).** The keys `learning_view` offers and the key targets it resolves leave out a key held only by notes the chat model wrote — origin `extracted_user` with no `observed_at`, which only the extractor sets; no field is added to the snapshot or the note.
  - **LQ13** changes the extractor's reading of an answer (09 §2.4 rule 1), and **LQ16** keeps ruling R3 with a bulk directory stat (§7c R3): neither changes a contract.
  - **The scale slice's open items** *(one contract change — the timeline's refusal below; this bullet said "no contract changed" until rev 7.6)*. §5 *Freshness without a watcher*: a rebuild reads each partition under its writer slot, so a write during it is never indexed with the stale derived facts of an earlier snapshot, and a completed rebuild makes the next tick re-derive every partition. "Other index reads wait" now reads: **only background learning's lookups wait for a rebuild**; recall and search answer from the files also when a rebuild starts under them, the suggestions list and a deleted conversation's summaries are read from the files, and `memory_timeline` refuses at once with `index_rebuilding` (`503 MEMORY_INDEX_REBUILDING` on its route) — a new refusal class on a read, which §7a's `memory_timeline` row gains.
- **rev 7.3 (2026-09-26), from the L8 scale slice's repair round 1 ([09](09-learning-design.md) rev 3.31):** two mechanisms rev 7.2 describes now hold as written, and one is bounded against a panic; **no contract of §7a and no ruling of §7b/§7c changes.** (1) §5 *Freshness without a watcher*, "an SQLite corruption error owes a rebuild the next tick runs": rev 7.2's build marked damage only where a stat scan, a search, an index pass or the tick met it, so damage in a B-tree only another read reaches (a pinned note's chunk count, the citations behind learning's lookup, the timeline's dates) was never rebuilt, not even by a restart; now every index read on a request's path marks it. (2) "Other index reads wait" while a rebuild runs: the wait was checked before the read, so a rebuild starting in between left the read an emptied index; such a read — learning's lookups, the timeline, the suggestions list, a deleted conversation's summaries — now reads again once the rebuild ends, and a turn's suggestions are left out, as while any rebuild runs. (3) A rebuild that panics stays owed and holds no reader, where it left every waiting read blocked for the life of the process. The idle tick's FTS merge, which rev 7.2 records, merged nothing until this round (a positive FTS5 `merge` never found a level to merge); it now leaves one segment per index.
- **rev 7.2 (2026-09-26), from the L8 scale slice ([09](09-learning-design.md) rev 3.30):** three mechanisms of §5 and §6 change shape; **no contract of §7a and no ruling of §7b/§7c changes.**
  - **§6, the forget row's recovery (S3).** "A tombstoned id whose file still exists: delete it" now holds wherever a scan runs, not only at open: the stat scan before every read and at every reconciliation tick, and the whole read behind every listing, write, index pass and `MEMORY.md`, leave out a file carrying a forgotten id — by name or, under another name, by frontmatter — and delete it with its `.history` (a read-only open leaves it out and deletes nothing). So a forgotten note's file put back while the service runs is never listed, read, indexed, re-recorded `external` or navigated — the hard guarantee of the *Forget contract*, which recovery at open alone did not give a partition already open. Recovery's own sweep (a `notes/` listing, a `.history` stat per tombstone) then runs once per partition per process through the service, and again after a forget that failed after its tombstone; without the service's cache (the CLI, `doctor --repair`'s process) it runs at every open as before. It is exactly as safe: the sweep deleted (1) note files, which every scan now deletes, (2) `.history` directories, which nothing reads for a forgotten id and which the reconciliation tick now deletes every 30 s, and (3) snapshot records of forgotten ids, which reads no directory and still runs at every open with the `pending` recovery.
  - **§5, *Freshness without a watcher*: the rebuild's timing (S6).** "Cold start opens the index if it is valid … A missing or corrupt index triggers a rebuild, and search meanwhile serves a bounded scoped file scan with `index_state: rebuilding`" is now what the daemon does: its open runs no integrity check and returns at once, a missing, recreated or schema-bumped cache is rebuilt on a blocking worker after the daemon starts (retried by the tick if it fails), and until it is done recall and search answer from the files — at most 10,000 notes, newest first, with the index's filters and ranking, `partial` when capped — while other index reads wait and a turn's suggestions are left out. Corruption is found on use, not at boot (an SQLite corruption error owes a rebuild the next tick runs). §7c Q12 holds, and the idle tick now also merges FTS segments and truncates the WAL, and rebuilds a cache grown past twice its rebuilt size (S5).
  - **§5, the per-turn recall's bound (S7).** Step 8's delivery, and the suggestions a `memory_confirm` turn is told about, are bounded at 1.5 s: past it the turn continues without them, exactly as it does on a memory error, with one WARN (partition keys, elapsed time, no note text). The "cheap (§5's 50 ms bound)" sentence is a 10K target, not this deadline.
- **rev 7.1 (2026-09-25), from the documentation pass's fact-check review:** §7a's `memory_confirm` refusals gain `invalid_input` — a target no longer effectively active, or a malformed revision — which the tool answers before any card (`memory/service/in_chat.rs`, `suggestion_preview` and `check_revisions`) and which rev 7 left out, though §6 and the route row already stated the rule for the route. The status line says slice 4 is in progress ([08](08-evaluation-report.md) is its draft report) rather than not started. No other change.
- **rev 7 (2026-09-25), Learning v2 folded (MD-10…MD-16, [09](09-learning-design.md) §11 at its rev 3.22):** each change at the section it amends, citing the 09 revision that introduced it. **L1:** §4's limit is per extraction call (`extract_max_items_per_call`, 12); §6's automatic extraction states the cadence, the database window reading `content` only, per-row `workspace_root`, the byte budget, the durable daily caps and what holds a window, and drops the sentence calling the daily caps counters since start. **L2:** §3 rule 2 gains LR17 (a learned `user.*` note that replaces nothing is global) and LR10; §7a's Extraction row reads through `learning_view` only and learns into each row's root; learned notes carry aliases, a key and `observed_at`. **L3:** §4 — suggestions, a candidate may name a `user` target (S1), never archived (S9), never edited by a model tool (S10); §6 — recently learned lists recallable notes with `supersedes`, undo **withdraws** and takes any candidate, a confirm binds and checks the target (S5, rev 3.6's retired target) and dismisses the others (S6), dedup covers candidates (S4), a candidate's tombstone carries no links (S3), and the forget contract says a withdrawal is not a forget; §7a — list items gain `supersedes`, the route table gains `/suggestions`, confirm's `target_revision` and its errors, and undo's new response (**lead ruling**); `memory_update`'s result gains `superseded`/`dismissed` and its refusals S9/S10. **L4:** §4 — the human's Approve on `memory_confirm`'s card is a user-directed act, and `memory_update`'s confirm is refused before the card; §7 — `memory_confirm` in the tool table, `approval_is_per_call`, the preview hook; §7a — its contract, its row in the per-surface table, and `preview`/`per_call` on the confirmation event and route. **L5:** §7 *Capabilities* — all nine templates name `memory_read`, internal templates get it ambiently, a plugin template only by naming it; §5 step 8 — the header's `valid_from`/`valid_to`/`timing=upcoming`; §7a — search results' `valid_from`/`valid_to`. **L10a:** the tool table, `memory_search` (`detail`, date bounds, `order_by`, 1–50 default 10, the index row), `memory_read` (`ids`, per-id errors, one shared 32 KiB), `memory_timeline`, the route table and the CLI syntax. **L10b:** §5 step 8's `recall_mode` and §7a's `memory_used` reason `index`. **L10c:** conversation summaries in §3 rule 3, §4, §5 step 5, §6's forget contract and §7a's writers. **Residue R6:** §5 and §6 step 4 — an index failure after a landed write does not fail the write (**lead ruling**). Two contract amendments are the lead's rulings of 2026-09-25, recorded in §7a: undo's `{id, status: "withdrawn", restored}` and R6. No other contract changes; the owner questions LQ5–LQ10 (09 §10) stay pending, and the text says where each touches.
- **rev 6 (2026-09-25), slice L9 (MD-13, [09](09-learning-design.md) §12):** the user-directed surfaces gain one member — a deterministic memory command (`/remember`, `/forget`, `/memories`) typed on an attended app or CLI chat turn. §3's construction rule and §4's definition name it, with why G2 holds (no model on its path; the text written is exactly what was typed); §6's explicit-remember bullet includes it; §7a's per-surface table gains its row (`MemoryService::context_for_chat_command`: the turn's principal, the turn's `request_workspace_root`, `UserSurface`, every capability, and a refusal for any other turn). No other contract changes: no route, no tool, no event field. The rest of 09 §11 is not folded here.
- **rev 4 (2026-09-23):** applies [07-audit-gaps.md](07-audit-gaps.md). Gap ids in brackets.
  - **§0:** the three audit decisions the owner took (G2 option 2, G30 fifth view + `memory_changed`, G38 no inventory) and the session lead's G12 ruling are recorded. MD-7's row says the `.gitignore` is re-asserted [G26].
  - **§2:** the partition key hashes the canonical partition *directory* (`project_root_at`'s rule), both derivations are pinned to lowercase hex over UTF-8 bytes [G8, G46]; the filename is `<uuid>--slugify(title, 48).md` [G17]; `.history` is the last 5, current included, stated once [G21]; directories 0700 and files 0600 [G25]; the project `.gitignore` is re-asserted on every open [G26]; the builtin file tools refuse `state/` and the partition's control files, and shell/MCP subprocesses are the only residual [G29]; `doctor --repair` is the only way out of quarantine and a hand-copied file is `external`, so no import [G16]; the factory reset leaves `state/memory-control/` and the index untouched [G32].
  - **§3:** `MemoryAccessContext` is defined in §7a with a per-surface table; "allowed operations" and "channel" become `capabilities` and `surface` [G6]. The owner must be a `global_user` row, so `system` and `provider:id` runs have no partition [G37]. Each path component to the partition is checked with `symlink_metadata` [G8]. `episode` defaults to the project [G19]. Same-`fact_key` notes in one partition: highest authority wins, ties newest, the loser reported as `conflict` [G27]. The workspace move is five ordered steps with the new-key snapshot written before the rename [G34]. `adopt` and `doctor --repair` point at §6's pre/post-conditions [G24]; `memory_manage` is not grantable [G20].
  - **§4:** the example is in the canonical serialization (block lists, no comment, `sources` as DB ids) and the canon is written down — field order, block lists, RFC 3339 `Z`, `null`, comments dropped, byte-identical round-trip, `serde_yaml` into the storage crate [G22]; the enums are `NoteKind`/`NoteScope`/`NoteStatus` [G41]; `fact_key` has a grammar [G27]; "user-directed surface" is defined once as the API/CLI/GUI, the origin table says who may record each, `confirm` is refused from a tool, and a `ModelTool` supersede/edit/archive of a `user` note is `authority_conflict` [G2]; `imported` is reserved with no v1 producer [G16]; the ordering is strict, `imported > external` [G15]; the revision is lowercase hex SHA-256 [G22].
  - **§5:** the profile survivor is the persona layer's block, `UserProfileSource` goes, the lead passes its document, and the memory section leaves the cached layer with `memory_retrieval_hash` [G35]; term routing — every ≥3-character term against both tables, no stemmer — is stated with the probe that decided it [G23]; step 8 reports `memory_used` [G10]; reconciliation publishes `memory_changed` [G30].
  - **§6:** step 2 copies `to_rev` into `.history` before the rename so the recovery branch's claim holds at every crash point, and "every revision" is gone [G21]; supersede is a `memory_store` with `supersedes: {id, expected_revision}` creating B, and the row says what it returns [G7]; a workspace-move row with `doctor`'s stale/abandoned/orphaned rule [G34]; a *Management operations* subsection gives `doctor`, `doctor --repair`, `adopt` and `sync` their pre- and post-conditions [G24, G18]; extraction's input is the typed message, its count bounds have keys and defaults, and the confidence threshold is an acceptance floor never stored [G36, G31]; recently learned and `undo` have a window, an ordering and an edit rule [G39]; the `USER.md` editor has its route and the inventory bullet is deleted [G9, G38]; the forget contract names the prompt and the unattended refusal [G11].
  - **§7:** the tool table matches §7a — `supersedes` on `memory_store`, `memory_update` = `edit | archive | confirm`, `confirm` refused from a tool [G7, G2]; the capability text drops import/export and says no tool provides `memory_manage` [G16, G20]; no template changes, six templates named [G40]; annotations: `memory_forget` and `memory_update` carry `destructive_hint`, `memory_store` does not — **07's smallest fix for G11 and the instruction's "`memory_store` with `supersedes`" cannot be expressed, because `annotations_for_builtin` is per tool name and `effective_confirmation_set` reads only `destructive_hint`; the design bounds a tool-issued supersede by authority instead and says so** [G11]; the API bullet names the envelope, `If-Match`-only and the management-only meaning [G18]; the CLI bullet names the workspace overrides [G6, G45]; the GUI bullet places the view, adds the Persona section, gives the empty/conflict/undo copy, reads `memory_used` and `memory_changed` [G30, G10].
  - **§7a (new):** the frozen contracts — the snapshot schema with an example, `MemoryAccessContext` with its per-surface table, the six tool schemas, the route table with the cursor rule and error codes, the persona routes, `memory_changed`, `memory_used`, and the CLI syntax [G1, G6, G7, G9, G10, G17, G18, G19, G20, G39, G45].
  - **§8:** modules list the ring, the bridge arms, the persona routes and the `serde_yaml` edge [G22, G30]. **§9:** import/export are listed as excluded [G16].
  - **Where rev 4 departs from 07.** G11, as above. G7: the instruction's single `supersedes: {id, expected_revision}` object is used rather than 07's list; the file field stays a list. G2's refusal message names the Memory *view* rather than "Settings → Memory", because G30 placed the view in the side nav. G6 and G2 add a third `surface` value, `Extraction`, for the background extractor: MD-2 already gives it its own origins, so a two-valued surface could not say which of `extracted_user`/`extracted_other` an extraction write records; no owner-decided rule changes.
- **rev 4.8 (2026-09-24):** R9 states a consequence of its tie-break for the owner: a confirmed candidate keeps its `created_at`, so it loses a same-key tie to a `user` note written between the candidate and the confirmation, where the old newest-`updated_at` rule let the confirmation win. No rule change.
- **rev 4.7 (2026-09-24):** §7a's `memory_search` row says what `historical` includes: superseded and archived notes and, without `as_of`, notes whose validity window has ended — what the index does (`memory_index/search.rs`, `Filter::build`) and what the tool's own description says since `50168294`. The CLI help and the CLI manual say the same.
- **rev 4.6 (2026-09-24):** wording only, no rule change. §3 rule 3, R9 and the rev 4.5 entry said "`observed_at` when every one has one, otherwise `created_at`", which reads as if `created_at` were consulted only when a note lacks `observed_at` — so that equal observations would fall to `updated_at` and an edit could flip the winner again. `same_key_winner` compares `observed_at` (when every tied note has one), then `created_at`, then `updated_at`, then the id; the three places now say "after that (or first, when any lacks one) the newest `created_at`", and a `memory_files` test pins equal observations deciding on `created_at`.
- **rev 5 (2026-09-24):** §0 gains Learning v2's owner decisions MD-10–MD-12 and scope; the design lives in 09-learning-design.md.
- **rev 4.5 (2026-09-24):** §3 rule 3's same-`fact_key` tie-break is no longer "the newest `updated_at`" — an edit of the older note flipped the winner — but the newest `observed_at` when every tied note has one; after that (or first, when any lacks one) the newest `created_at`, then `updated_at`, then the lowest id; §7c gains R9, which records it.
- **rev 4.4 (2026-09-24):** §7c gains the slice-3 rulings R1–R8 (plan §7a S3-R6 folded in; R7 and R8 from the 3.9 review).
- **rev 4.3 (2026-09-24):** the status line only. Slices 0–3 are implemented (plan rev 4.2), so it no longer reads "not implemented; awaiting the owner's approval". This revision does not fold the slice-3 rulings (plan §7a, S3-R6) into §7c; §7b/§7c still hold the slice-1 and slice-2 rulings only.
- **rev 4.2 (2026-09-23):** §7c records the twelve rulings the slice-2 library pinned (Q1–Q12); §7a's `pending` gains `origin` (Q1).
- **rev 4.1 (2026-09-23):** §7b adopts the thirteen readings the slice-1 fixtures pinned (P1–P13) as rulings; §7a gains `sources_status` and an optional `memory_changed.surface`.
- **rev 3 (2026-09-23):** the owner answered MD-1…MD-9.
  - **MD-6 (refuse and reset)** removes the `imported_legacy` origin, the `legacy:` block, `migrate --dry-run` and the `legacy_memory` module.
  - **MD-9 (everything at once)** puts the GUI and the Persona editor in the cutover.
  - **The owner's instruction** removes the old engine completely, with no compatibility code.
- **rev 2 (2026-09-23):** applies [05-review.md](05-review.md).
  - **§0 (new):** owner decisions MD-1…MD-9, which rev 1 had adopted silently.
  - **§2:** control records collapse to one snapshot per partition, with no operation records. Owner key = a hash of the owner id. The project `.gitignore` lives inside the memory directory (MD-7).
  - **§3:** the group-lane rule is replaced by today's lane model plus a separate connector-audience design (MD-4). The workspace rule uses the marker-found project root with lazy store creation (MD-5, extending ADR-031). Preferences default to global. A move-quiesce hook replaces the nonexistent move lock. Resets become the config-reset bug fix, orphan listing and an explicit `adopt`.
  - **§4:** superseded becomes derived, not stored. Authority moves from frontmatter to the snapshot's origin field, with an explicit ordering. Sources cite DB ids, not session logs.
  - **§5:** union-of-terms retrieval, the per-turn defect called out, one profile injection, and freshness by reconciliation scan instead of a required watcher.
  - **§6:** every mutation touches at most one note file, with a single `pending` field for recovery, and forget is tombstone-first. Nothing is replaced by similarity. `USER.md` handling follows MD-3.
  - **§7:** capabilities named and surfaces staged (MD-9).
  - **§8:** module for migration 043 added; `notify` kept out of core.
  - The KB importer is deleted as dead code.
  - **After a consistency review** ([05](05-review.md#consistency-review-of-rev-2)): <br>- MD-5 uses the marker-found project root, with lazy store creation <br>- scheduled skills are global <br>- derived supersession requires the same partition and at least equal authority <br>- supersede checks the target's revision <br>- dependents of a forgotten note are derived, not rewritten <br>- `.history` keeps service-written revisions <br>- the partition key and the `confirm`/`undo` operations are defined <br>- the snapshot-loss limit is stated <br>- `migrate --dry-run` ships with the cutover <br>- `rusqlite`/`uuid` features named <br>- MD-8 covers every caller and the newer-schema guard
- **rev 1 (2026-09-23):** initial proposal.
