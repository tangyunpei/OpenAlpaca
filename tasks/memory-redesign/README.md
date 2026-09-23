# OpenAlpaca Markdown memory: audit, research and replacement proposal

**Date:** 2026-09-23. **Status: design and plan at rev 3.** The owner answered MD-1…MD-9 and instructed that the old memory system be removed completely. The written design awaits the owner's review, and nothing is implemented.

**Source inspected:** `7561f152712c01912436ad04a6fd57974be2bcab` (rev 1). The review re-verified everything at `0d490847`; only `tasks/lessons.md` differs. Branch `chore/deps-upgrade-and-simplify`, database baseline **42**. This deliverable changes documentation only. No live memory store was read, migrated or deleted.

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
| [3. Proposed architecture](03-design.md) **rev 3** | §0 owner decisions MD-1…MD-9; layout; owner keys; scope and partition rules; note format and authority; read path; single-file mutations with derived supersession; forget; tools/API/CLI/GUI; modules |
| [4. Implementation and evaluation plan](04-implementation-plan.md) **rev 3** | Slices 0–4 (a stop-data-loss slice first), tests-first commit table with the GUI in the cutover, refuse-and-reset switch-over, complete removal map with a completeness search, evaluation with a stated sample size, sequencing and risks |
| [5. Review](05-review.md) | What the independent verification found, and what changed from rev 1 to rev 2 |
| [6a](06a-evidence-audit-verification.md) · [6b](06b-evidence-embedding-inventory.md) · [6c](06c-evidence-identity-scope-learning.md) · [6d](06d-evidence-vec0-spike.md) | Evidence: 53 audit claims re-verified · every embedding consumer · owners, workspaces, audience and learning · the stub-`vec0` retirement spike |

Start with design §0 for the answered decisions and the review for what changed; use the audit, research and evidence appendices to inspect the grounds. Product documentation and repositories were checked live on the date above, and benchmark reproduction must pin exact versions. Proposed modules, routes, schema fields and timings are future contracts, not current app features.

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
- **Old databases are refused (MD-6).** A schema-42 database is refused like 1–41, and the switch-over is a factory reset, which also removes tasks, sessions, usage and the API keys in `llm.toml`. The refusal works without sqlite-vec ([06d](06d-evidence-vec0-spike.md) Q1). Plan §4 has the refusal text and the owner's switch-over note. There is no permanent dual backend. Application history, tasks, credential management, compaction and explicit persona configuration stay separate from the memory replacement.

**Checks behind this deliverable:**
- **Rev 1:** 18 storage and 30 core memory tests passed offline, and an in-memory SQLite probe reproduced the F4 `AND` problem.
- **The review:**
  - re-ran both suites, with the same counts;
  - re-verified 53 audit claims (41 confirmed, 12 corrected, 0 refuted);
  - inventoried every embedding consumer;
  - mapped owners, scopes and learning;
  - ran the vec0 retirement spike on synthetic databases outside the repository.

None of these measures real retrieval quality, end-to-end model behavior, startup speed, packaged-app operation, or a real schema-42 conversion. No model was downloaded and no benchmark score was produced. Implementation acceptance tests and numeric targets are in the plan.
