# OpenAlpaca Markdown memory: audit, research and replacement proposal

**Status (2026-09-28):** design [03](03-design.md) rev 7.16, plan [04](04-implementation-plan.md) rev 4.8 and the Learning v2 design and plan [09](09-learning-design.md) rev 3.51, on branch `feat/markdown-memory` (unpushed; database schema **1** since the [store revision](../store-revision/design.md) re-baselined it on 2026-09-28 (SQ-1) — baseline 43 and head 45 before; memory index schema **5** — the disposable search cache, rebuilt from the notes when its version differs). The owner answered MD-1…MD-16 (03 §0), LQ1–LQ4 (09 §10) and, on 2026-09-26, **LQ11–LQ17** (taken: 09 §10, 03 §0) and, on 2026-09-28, **LQ5–LQ10 and LQ18–LQ21** (taken: 09 §10, rev 3.51 — LQ5–LQ9 kept as shipped, LQ10 the narrow refusal, and report 10 §9's items — built by the report-9 slice `20c6d9df..8c298f60`). **Implemented:** slices 0–3 of plan 04 (the old SQL/vector engine removed completely; Markdown notes, control snapshots and a disposable FTS5 index behind `MemoryService`), Learning v2's slices L1–L7, L9 and L10a–L10c plus its residue slice, the L8 fix and scale slices, the **L8 product slice** (LQ11, LQ13, LQ14, LQ16), **LQ17's index layout revision** (09 §6.5, index schema 5) and the close-out of both (09 rev 3.41, 03 rev 7.12) with its repair rounds 1 (09 rev 3.42, 03 rev 7.13), 2 (09 rev 3.43, 03 rev 7.14) and 3 (09 rev 3.44, 03 rev 7.15); 09's status line has every range. The lead accepted LQ17's one lost proxy row (tq-0032, 09 §6.5.5). **Finished:** Learning v2's evaluation, L8 — its first runs and its re-measurement on the final build `82180b84` — reported in [10](10-learning-evaluation.md) (rev 2, final, its §4.8 cross-over replays included; rev 2 records how each of its §9 items closed). **In progress:** slice 4 — [08](08-evaluation-report.md) is its draft evaluation report, draft 6 (D measured, the gate passed; the post-fix D′ measured by L8 on another model build, against B-mlx; E′ not run; every figure it measured is pre-LQ17). `CLAUDE.md`'s *Current Work* has what remains.

**Originally (rev 1–3, 2026-09-23):** the source inspected was `7561f152712c01912436ad04a6fd57974be2bcab` (rev 1), and the review re-verified everything at `0d490847`, on branch `chore/deps-upgrade-and-simplify` with database baseline **42**. That deliverable changed documentation only, and no live memory store was read, migrated or deleted. The sections below from *Recommendation* on are that deliverable's text.

## Recommendation

Retire the current persistent SQL/vector memory engine. Replace it with three things:
- **canonical Markdown notes**
- **one small control snapshot per partition**, holding revisions, origin and tombstones
- **a rebuildable lexical search index**, owned by a native Rust `MemoryService`

SQLite FTS5 becomes a disposable index only; the rest of the app's database stays. Remove the embedding model and its ONNX/tokenizer dependencies, vector storage, the embedding indexer, similarity-based replacement, automatic importance pruning, the dead KB importer, and the duplicate retrieval paths.

No model is needed to create, read, search, edit, forget or rebuild memory. Optional extraction uses the already-configured conversational model outside the retrieval path.

**What this buys, measured and inferred:**
- **Dependencies (measured).** 97 of the daemon's 465 dependency packages go, along with the ONNX Runtime library fetched at build time (about 67 MB unpacked) and the ~1 GB model download that every install triggers on first boot today.
- **Speed and accuracy** must be measured. Today's automatic per-turn recall barely works, and the tool path is the real baseline.
- **Lifecycle** is where the largest correctness gains are: scope enforcement, no silent pruning, no similarity replacement, explicit corrections and reliable forgetting.

## Documents

| Read | Contents |
|---|---|
| [1. Current-code audit](01-code-audit.md) | End-to-end storage/retrieval/extraction map, findings F1–F8, dependency analysis, retained boundaries (rev 1; corrections in 05/06a) |
| [2a. Modern framework research](02-framework-research.md) | Mem0/OpenMemory, Letta/MemFS vs MemGPT, Graphiti/Zep, LongMemEval, LoCoMo and LongMemEval-V2 |
| [2b. Markdown/filesystem research](02-filesystem-research.md) | Claude Code, the Anthropic memory tool, OpenClaw, QMD, A-MEM, filesystem and instruction-growth studies, lexical limits |
| [3. Proposed architecture](03-design.md) **rev 7.16** | §0 owner decisions MD-1…MD-16 and the L8 decisions LQ11–LQ17; §7a the frozen contracts, Learning v2 folded in at rev 7; layout; owner keys; scope and partition rules; note format and authority; read path; single-file mutations with derived supersession; forget; tools/API/CLI/GUI; modules |
| [4. Implementation and evaluation plan](04-implementation-plan.md) **rev 4.8** | Slices 0–4 (a stop-data-loss slice first), tests-first commit table with the GUI in the cutover, refuse-and-reset switch-over, complete removal map with a completeness search, evaluation with a stated sample size, sequencing and risks |
| [5. Review](05-review.md) | What the independent verification found, and what changed from rev 1 to rev 2 |
| [6a](06a-evidence-audit-verification.md) · [6b](06b-evidence-embedding-inventory.md) · [6c](06c-evidence-identity-scope-learning.md) · [6d](06d-evidence-vec0-spike.md) | Evidence: 53 audit claims re-verified · every embedding consumer · owners, workspaces, audience and learning · the stub-`vec0` retirement spike |
| [7. Audit of rev 3](07-audit-gaps.md) | The 49 verified gaps G1–G49 in design and plan rev 3 that rev 4 applied, and the three owner decisions they raised (G2, G30, G38) |
| [8. Evaluation report](08-evaluation-report.md) **draft 6** | Plan slice 4: baselines A and B, system D measured against B (the gate passed), the hard invariants, gates and footprint; the post-fix D′ pointed at 10 (L8's D-index, on another model build, against B-mlx); E′ not run; every figure it measured pre-LQ17 (index schema 3) |
| [9. Learning v2](09-learning-design.md) **rev 3.51** | Design and plan for MD-10…MD-16: cadence and the learning queue, richer extraction, replacement and suggestions, in-chat confirmation, `memory_read` for every agent, caches, the learning evaluation, chat memory commands, layered tools, index-first recall and episode notes; slices L1–L10c; owner questions LQ1–LQ10, the L8 decisions LQ11–LQ17 and report 10 §9's LQ18–LQ21 (§10), with LQ17's index layout revision (§6.5, index schema 5); every implementer's deviations in the revision log |
| [10. Learning evaluation](10-learning-evaluation.md) **rev 2** | L8's report (09 §7.5, §13.4): the first runs at `50cf878d`, before LQ11–LQ17, and the re-measurement on the final build `82180b84` beside them — QA for B-mlx, D in both recall modes, L and L0 (the gate passed; L's −10 pp target not met), the learning targets against the extraction path, L-S, invariants, cost, the episode split, the diagnosis and the decisions it drove, the benches with the 30K design point; the cross-over replays of §4.8, which put the alias drop on the replayed transcript and leave the 60 % target unsettled for the build; every verdict the reader grading itself |
| [11. Continuity](11-continuity-design.md) **rev 1.6** | Track B, compaction ↔ memory: one working state per conversation on the session row, one writer for the loop's compaction, the turn end and the close, the episode note projected from it, the previous conversation carried into a new one on the lane and project (owner decisions CQ-1…CQ-10, §7); slices 1–7 on `track-b`, every implementer's rulings in the revision log |
| [12. Continuity evaluation](12-continuity-evaluation.md) **rev 1** | 11's slice 7: the continuity split as K0 and K and the episode split re-run on L-episodes, on the local test model — none of the three targets met (K 66.7 % against 80 %, +13.3 pp over K0 against +30 pp, the episode split 14 / 20 against 17), the findings for the lead; every verdict the reader grading itself |

Start with design §0 for the answered decisions and the review for what changed; use the audit, research and evidence appendices to inspect the grounds. Product documentation and repositories were checked live on the date above, and benchmark reproduction must pin exact versions. The modules, routes, schema fields and timings proposed here were future contracts when this was written; they are now built, and the code and 03 §7a are the reference.

## Most consequential findings

1. **Silent data loss today.**
   - Memories found only by the per-turn path are never marked accessed and are **pruned after about 125 days**.
   - Similarity replacement (probably far too loose for E5 scores; not measured) applies even to explicit saves.
   - A replaced row can come back as a second current version.
   - The plan's slice 0 stops these before the replacement lands.
2. **Per-turn recall barely works.** The whole user message is an implicit AND query. Natural questions and Chinese text match nothing, and what recall works comes from the model calling `memory_search`.
3. **Scope.** Lead-agent injection with no workspace reads every scope of the owner (F1). The search tool keeps a caller-supplied `workspace_id` (F2), which is low practical risk because the model is never told the parameter exists.
4. **Supersession, forget and lifecycle defects.**
   - Old rows stay searchable and get re-embedded (F3).
   - The supersession fallback searches for a literal "AND" (F4).
   - Forget deletes the best lexical match across scopes (F7).
   - The KB importer is dead code, so F6 no longer applies.
5. **The design's identity assumptions needed correcting.**
   - There is no group lane.
   - One owner identity already covers several people: every linked chat account, and on iMessage any `/ask` sender. That is a security issue, spun off as its own task.
   - `config reset` deletes the owner id.
   - No move lock and no `USER.md` editor exist.

   See [05](05-review.md).

The audit also records what already works, and the replacement keeps it: scoped uniqueness, owner filtering, the untrusted-memory wrappers, and the home/project store primitives.

## Research conclusions that influence the choice

| Option | Decision |
|---|---|
| Adopt Mem0 directly | Not the default replacement: its architecture uses model extraction and embeddings. Borrow evidence retention and multi-signal retrieval ideas. [Current Mem0](https://github.com/mem0ai/mem0). |
| Adopt a Markdown-oriented precedent | Borrow a small always-available context plus on-demand files. Letta documents this split, and OpenAlpaca can implement it locally without its remote Git/runtime model. [Letta memory](https://docs.letta.com/agent-sdk/memory). |
| Adopt QMD's full pipeline | Conflicts with the no-memory-model goal. Its BM25-only mode is relevant, but a separate runtime needs justification. [QMD](https://github.com/tobi/qmd). |
| Build a native scoped file service | The recommended fit for the existing Rust/store/context architecture: no new sidecar, and one correction/deletion/authority contract. This is the project's design inference. |

Recent filesystem-memory research finds organizational and search-cost benefits without establishing better answer correctness from organization alone. That is a reason to evaluate the complete system, not to reject readable files. [Filesystem-memory study, July 2026](https://arxiv.org/abs/2607.26637).

## Delivery boundaries and verification

The unreleased status permits removing the old engine and breaking its internal APIs, and the owner chose to use it fully:
- **The old engine goes completely:** the `memory`/`memory_fts`/`memory_vec float[768]` design and everything built on it, with no compatibility code.
- **Old databases are refused (MD-6).** A schema-42 database is refused like 1–41, and the switch-over is a factory reset, which also removes tasks, sessions, usage and the API keys in `llm.toml`. *(Since the store revision's SQ-1, 2026-09-28, the schema starts at 1, and every pre-squash database, 42 included, meets the one refusal of a version above this build's — plan §4's callout.)* The refusal works without sqlite-vec ([06d](06d-evidence-vec0-spike.md) Q1). Plan §4 has the refusal text and the owner's switch-over note. There is no permanent dual backend. Application history, tasks, credential management, compaction and explicit persona configuration stay separate from the memory replacement.

**Checks behind this deliverable:**
- **Rev 1:** 18 storage and 30 core memory tests passed offline, and an in-memory SQLite probe reproduced the F4 `AND` problem.
- **The review:**
  - re-ran both suites, with the same counts;
  - re-verified 53 audit claims (41 confirmed, 12 corrected, 0 refuted);
  - inventoried every embedding consumer;
  - mapped owners, scopes and learning;
  - ran the vec0 retirement spike on synthetic databases outside the repository.

None of these measures real retrieval quality, end-to-end model behavior, startup speed, packaged-app operation, or a real schema-42 conversion. No model was downloaded and no benchmark score was produced. Implementation acceptance tests and numeric targets are in the plan.
