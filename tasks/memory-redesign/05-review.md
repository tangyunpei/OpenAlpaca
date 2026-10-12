# Review of the memory-redesign deliverable (rev 2 driver)

**Date:** 2026-09-23. **Reviewed:** documents 01–04 and README as written at `7561f152`, against the code at `0d490847` (only `tasks/lessons.md` differs). **Outcome:** [03-design.md](03-design.md) and [04-implementation-plan.md](04-implementation-plan.md) are revised to **rev 2**. Each carries a revision log that points back here. No application code, configuration or live store was changed. Every check below was read-only apart from the isolated spike in [06d](06d-evidence-vec0-spike.md), which touched only synthetic databases outside the repository.

## Method

Four independent checks ran in parallel, each writing its full evidence to an appendix:

| Check | Appendix | Result in one line |
|---|---|---|
| Re-verify every factual claim of the audit | [06a](06a-evidence-audit-verification.md) | 53 claims: **41 confirmed, 12 confirmed with a correction, 0 refuted**. Both focused test runs reproduced (18 storage, 30 core). |
| Inventory every embedding / vector / model-download consumer | [06b](06b-evidence-embedding-inventory.md) | **Every consumer is memory.** Skill routing, tool selection, intent, artifact and session search are already model-free. |
| Spike: retire the vec0 table without sqlite-vec | [06d](06d-evidence-vec0-spike.md) | **Possible with a stub `vec0` module**. The separate "utility built from the old dependency set" is unnecessary. |
| Map owners, workspaces, audience and automatic learning | [06c](06c-evidence-identity-scope-learning.md) | **13 of the design's assumptions do not match the code**. The largest: there is no group lane, and one owner identity already covers several people. |

## What the deliverable got right

The audit is careful and honest. None of its claims was refuted, and its evidence grading (measured vs. read vs. inferred) is consistent. The research notes separate vendor claims from measurements, and the design's invariants all survive the review:
- scope enforced by the service, never by tool arguments
- deletion with tombstones
- revision-checked writes
- disposable index
- no model on the retrieval path
- retrieved text as untrusted data

Rev 2 keeps them all.

## Corrections to the audit (detail in 06a)

- **The knowledge-base importer has no production caller.** The `/v1/memory*` routes, KB ingest included, were deleted in `5ec1db1d`. So F6 (stale KB copies) and the symlink-walk risk cannot occur on a current build. `memory/kb.rs` and its chunker are dead code to delete, not a design driver to port.
- **Embeddings are on by default, not "depending on config".** The seeded `llm.toml` (`scripts/release/templates/config/llm.toml:46-50`) enables the local E5 embedder, so every install downloads about 1 GB during its first boot.
- **F4 is slightly narrower.** The fallback still matches when the old memory contains the word "and", and it only runs for an item with no embedding.
- **F5 has no effect today.** Every hybrid caller passes no kind filter.
- **F7 is wider.**
  - The similarity replacement also applies to explicit `memory_store` saves and to profile extraction.
  - It can change a row's kind.
  - Supersede, decay, prune, both similarity searches and `persist_memory_item` have **no tests at all**.
- **Line numbers.** A handful are off by a few lines. The ":129-135" citation is the old-database refusal text, not the factory-reset preview.
- **One more SQL writer.** Memory has no CRUD route, but `ArtifactStore::rebase_project` rewrites `memory.scope_id` with raw SQL (`artifacts/mod.rs:1539-1543`). Workspace listing, purge and `store` counts also read the table.

## Findings the audit missed (they change priorities)

1. **The per-turn memory recall almost never finds anything.** `prompt_ctx::MemorySource` searches with the *whole user message*, every token quoted, which FTS5 treats as an implicit AND. It uses `unicode61` with no stemming, and a run of CJK characters is one token. A probe matched **0** rows for "what editor do I prefer?" against "User prefers vim for editing", and 0 for "简洁" against "用户喜欢简洁的回答". Recall that works today comes from the model calling `memory_search` and from the lead's injection, both vector-assisted. The rev-1 evaluation's baseline "A: current FTS-only" is therefore a floor, not a fair baseline.
2. **Saved memories are silently destroyed.**
   - The per-turn path never records access, so a preference found only there decays below the floor and is **pruned after about 125 days** at the defaults.
   - The similarity thresholds (1.5 distance, and the supersession threshold) are probably far too loose for E5 scores. That is inferred, not measured: nearly any new memory could replace its nearest neighbour, explicit saves included.
   - A replaced row is re-embedded within 30 s and can be selected again, leaving **two current versions**.
   - The owner's real store is accumulating memories under these rules now.
3. **The profile is injected twice** on main-loop and skill turns, and the memory section is cached by query text, so a repeated identical query reuses a stale section.
4. **Failed embeddings are retried every 30 s forever** and can block other rows. `AppState.embedder` is written and never read. Several repository methods lost their only callers with the deleted routes.
5. **Removing the model stack is a large, measurable win.** 96 of the daemon's 465 dependency packages exist only for `fastembed`, and one more for `sqlite-vec`, so 97 leave. `ort-sys` fetches an ONNX Runtime library at build time (about 67 MB unpacked in the cache) and links it statically. The release CLI (32.6 MB) contains it too, because it is built together with the daemon.

## Where the design's assumptions do not hold (detail in 06c)

| Design rev 1 said | The code does | Consequence for rev 2 |
|---|---|---|
| No recall in "shared/group connector lanes" | **No group lane exists.** A connector's lane is `{owner}:{platform}`, mixing every DM and group on that platform, and core never learns which chat a message came from. | The rule cannot be implemented without first making connectors pass chat identity to core. Group privacy is a **connector-audience** problem, not a memory one: `USER.md`, the shared transcript and run notices already reach groups. It moves to its own work item, and v1 memory keeps today's lane model. |
| "Identity equality is not sufficient" (implies owner = one human) | iMessage runs **every** processed message as the owner, including a stranger's `/ask` DM and any group member's prefixed message (`imessage/routing.rs:132-150`, `handler.rs:219`). Every linked Telegram/Discord account is the owner. | This is a security issue outside memory. It was spun off as its own task ("Fix iMessage connector treating any sender as the owner"). |
| Factory reset must not hand partitions to the next user | A **plain** `openalpaca config reset` (and the TUI's "Reset All Config") deletes `identity.local_user_id` while promising data is preserved. The next boot mints a new owner and orphans every row. `USER.md` survives and is injected for the new owner. | This is a **bug fix** that belongs in slice 0: config reset must not delete `identity.*`. Reclamation machinery shrinks to "partitions are keyed by owner key; an orphaned key is listed by `memory doctor`". |
| Safe owner keys needed | `link_generate` can mint extra owners and accept **arbitrary strings** as owner ids. | Confirmed need. Owner key = a fixed-length hash of the owner id, and the mapping is recorded in the partition's control file. |
| Workspace moves "under the existing move lock" | **There is no move lock.** `PATCH /v1/workspaces` is a preflight, one DB transaction, then `rename(2)`. | The memory service must be **quiesced** by the move preflight: it refuses the move while a memory write is pending, and holds new writes until the rename completes. |
| Retire the daemon-CWD fallback | It is already inert in shipped launches, because both launchers start the daemon in `~/.openalpaca`. But **the CLI always sends its own CWD** as the workspace, so every CLI turn is project-scoped wherever it runs. | Rev 2 changes the rule: the project partition is the **project root found from the request workspace by the marker walk** (`.openalpaca`, else `.git`), created lazily on the first project write, as artifacts do. A directory with no marker is global, so memory is not scattered into random directories, and the daemon's own CWD never selects a project. It is recorded as an extension of ADR-031. |
| Keep `USER.md`'s explicit editor; remove the background writer | **After onboarding there is no editor.** `update_persona` exists only while bootstrapping, `file_ops` refuses `USER.md`, and the GUI and CLI have none. Bootstrap completion depends on `USER.md` content, which the background writer can supply. | The background writer can be removed only together with an explicit `USER.md` editor and an onboarding path that does not depend on it (owner decision MD-3). |
| Ignore owner memory in the project's `.gitignore` | Project `README.md` and `.gitignore` are written **only when absent**. Existing projects never receive a new rule. | Rev 2 writes a self-contained `.gitignore` (`*`) **inside** `<project>/.openalpaca/memory/` when the partition is created. It works for existing projects and never touches the user's own ignore file. |
| The session log is the evidence archive | Session records hold a message id and a **preview**. The text is in the DB, and sessions are swept at 2 GB and orphaned by a factory reset. | Sources cite DB message and task ids. A citation whose evidence is gone reports `evidence_unavailable`; it never breaks the note. |
| Reuse `notify`, UUIDs in storage/core | Only `openalpaca_wake` depends on `notify`, and core may not depend on wake. The wake watcher cannot add paths after start. `tempfile` is a dev-dependency in storage. | v1 needs no watcher for correctness. Freshness comes from service writes plus a stat-scan reconciliation. An optional watcher lives in the daemon. The atomic write reuses `store::write_atomic` (`store/mod.rs:754`) and the artifact store's crash-tested path. |
| Cost controls "behind existing router budgets" | The "daily" extraction caps are counters since daemon start, never reset at midnight, and never bind on a local ($0) model. | Pre-existing, recorded. Extraction in rev 2 is bounded by count per turn and per run as well as by cost. |
| Rev-1 control layout: manifest, attestations, tombstones, operation records | *(design choice, not a code conflict)* | Rev 2 simplifies it: **supersession is derived from links**, so a replacement is a single-file write. There is **one control snapshot per partition**, and forget is ordered tombstone-first and idempotent. The operation-record machinery goes. See 03 rev 2 §6. |

## The schema transition, settled by the spike

> **Superseded by the owner's answer to MD-6 (refuse and reset, 2026-09-23).** Rev 3 of the plan does not convert data. It re-squashes the baseline to 43 and refuses 42. The spike still matters: it shows the refusal works on an old database without sqlite-vec (Q1), and that a fresh database can no longer run the old baseline. The paragraphs below record the recommendation as it was made.

Rev 1 planned a one-shot utility "built from the old dependency set", because dropping a vec0 table without its module was unproven. The spike ([06d](06d-evidence-vec0-spike.md)) proved something simpler:
- Without the module, the database stays fully usable. Only statements naming `memory_vec` fail.
- A stub `vec0` module, registered only on the migration connection, lets one ordinary transaction drop `memory_vec`, its four shadow tables, the FTS index, the triggers and `memory`.
- `memory` is an ordinary table, so the **new build can export it to Markdown itself** before that transaction.

Rev 2 therefore replaces the utility with **migration 043**, which runs in the new build:
1. It takes a pre-migration backup.
2. It exports the legacy rows to Markdown with a durable receipt.
3. It retires the tables under the stub.

It also removes the memory tables from `001_baseline.sql`, so a fresh database never needs the module. This keeps the owner's tasks, sessions, artifacts and usage, which a refuse-42-and-reset path would destroy. The owner's real store is exactly such a database.

## Evaluation gate made attainable

Rev 1 asked for a paired non-inferiority margin of −2 percentage points with at least 300 questions. With paired binary outcomes and a plausible 15 % discordant rate, a system exactly as good as the baseline passes a margin m with probability Φ(m/SE − 1.96), where SE ≈ √(0.15/n). For 80 % power:
- **−2 pp** needs about **2,940** held-out questions.
- **−5 pp** needs about **470**.

At n = 300 even −5 pp passes an equal system only about 61 % of the time; the first rev-2 draft missed this, and the consistency review caught it. Rev 2 sets about 470 held-out questions plus a separate tuning split, a −5 pp margin, and per-category inspection. It compares against the current **`memory_search` tool path** (hybrid) as the real baseline, and uses the per-turn FTS path only as a floor.

## Consistency review of rev 2

A fresh reviewer read the first rev-2 draft against its own evidence and the code, and reported 16 verified problems: 1 blocker, 9 major, 6 minor. All 16 are fixed in the rev 2 now on disk:

| # | Severity | Problem in the first rev-2 draft | Fix |
|---|---|---|---|
| 1 | **blocker** | Migration 043 ran inside `Database::open`, which the CLI calls without the daemon's lock (`config.rs:79` → `store/mod.rs:126`). A CLI command beside a live daemon could export, drop `memory` under it and lose rows written meanwhile. Two openers could also race, and the unconditional runner would drop tables even after a failed export. | 043 runs **only in the daemon, under its single-instance lock** (`main.rs:122` before `:220`). Other openers refuse a database still holding `memory`. The retirement SQL runs only on a `verified` receipt and is applied by the `legacy_memory` step, not the unconditional runner (04 §4). |
| 2 | major | The rollback relied on old builds refusing v43, but `run_migrations` refuses only 1..41 (`database/mod.rs:126-151`). | Slice 0.5 adds a newer-schema refusal, and that build is the rollback floor. |
| 3 | major | `migrate --dry-run` came in slice 5, after the slice-3 build had already migrated the owner's store. | Moved to 3.5, and added to design §7. |
| 4 | major | Slice 0.3 exempted only some callers, so task extraction could still retire an explicit save (`search.rs:259-300` has no source filter). | Similarity supersession is off for **every** caller. |
| 5 | major | The 0.3 test could not fail on the old code, because no test embedder exists and the Jaccard fallback almost never matches (F4). | A deterministic stub `Embedder`. |
| 6 | major | Sample size: at n = 300, an equal system passes −5 pp only about 61 % of the time, and "1,440 for −2 pp" was a 50 %-power figure. | About 470 held-out questions (80 % power) plus a separate tuning split. |
| 7 | major | Export staged under the home store, then moved into project stores: `EXDEV` across volumes, partial publish on a crash, and fresh random UUIDs duplicating notes on re-run. | Stage inside each destination, record per-destination state in the receipt, use deterministic UUIDv5 ids. |
| 8 | major | Derived supersession ignored authority, so an `external` or `imported` note could hide a `user` note through a hand-edited `supersedes`. | A link counts only within the same partition and from an origin ranked at or above the target's; otherwise it is reported as a conflict. |
| 9 | major | "A note citing a forgotten note becomes a candidate" rewrote other files, breaking the one-file rule with no recovery row. | Derived at read time as `source_forgotten`; no file is rewritten. |
| 10 | major | The "existing project store" rule split one repository's facts before and after its first artifact (`artifact_write.rs:53-57` creates the store). | MD-5 uses the marker-found project root (`.openalpaca`, else `.git`), with the store created lazily. |
| 11 | major | The fixture DB is git-ignored (`.gitignore:90`, `*.db`), its absolute `scope_id`s cannot be used as-is, and it lacked the quarantine cases. | A whitelist line, placeholder roots rewritten per test, and cases for `.git`-only root, lane row, cycle and dangling link. |
| 12 | minor | Scheduled skills were said to have a persisted workspace binding; none exists (`scheduled_skills.rs:208`). | Scheduled skills are global. |
| 13 | minor | `CACHE_DIR` was deleted while the new index lives in `state/cache/`. | Kept, with its README row repointed. |
| 14 | minor | Missing manual lines, the package count (97, not 96), and "67 MB download" (it is the unpacked size). | Corrected in all documents. |
| 15 | minor | "No resurrection after snapshot recovery" had no mechanism, and "displaced bytes stay in `.history`" was unsupported. | The limit is stated: a lost snapshot quarantines, and a restored older snapshot previews what it would revive. `.history` keeps every service-written revision. |
| 16 | minor | Contracts slice 1 could not freeze. | Defined: `<partition-key>`, `confirm` and `undo`, the origin of a never-written file, the target revision for supersede, the cross-partition rule, and the `rusqlite` `vtab`/`backup` and `uuid` `v5` features. |

## Owner decisions this review raises

**Answered 2026-09-23.** MD-1, MD-2, MD-3, MD-4, MD-5, MD-7 and MD-8 as recommended. MD-6 **refuse and reset**. MD-9 **everything at once**. The owner also instructed that the old memory system (`memory`/`memory_fts`/`memory_vec float[768]` and everything built on it) be removed completely, with no compatibility code. Design and plan are at **rev 3**.

Rev 1 adopted several product decisions without surfacing them. Rev 2 lists them as **MD-1…MD-9** in [03-design.md §0](03-design.md#0-owner-decisions). Each has a recommendation, and the design is written so that the recommended answer is buildable. None is applied until the owner answers. The project convention (CLAUDE.md, plan §0) is that a default which reverses an accepted design or widens a component's job is a decision, never adopted silently.

## Pre-existing issues found in passing (not memory work)

These are recorded here so that nobody re-discovers them. None is fixed by this review.
- **iMessage runs any sender as the owner.** It was spun off as its own task.
- **Run notices go to each connector's last-used chat**, which may be a group. Telegram also receives the run's files.
- **The daily extraction caps** never reset at midnight and never bind on a $0 model.
- **`link_generate`** can mint owners with arbitrary ids.
- **`tasks/todo.md`'s "Previous — crate simplification (2026-09-21)" entry** points at `tasks/simplification-implementation.md`, which does not exist in this tree, and says its changes "remain uncommitted". It may describe work done in another checkout. The simplification wave on this branch is committed and pushed as draft PR #33.
