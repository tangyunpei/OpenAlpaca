# Markdown and filesystem memory research for OpenAlpaca

Research snapshot: 2026-09-23. This is a primary-source research note, not an implementation audit. Documentation URLs are living sources checked on this date; repository `main` links can change. Recent arXiv results are preprints unless stated otherwise. Product authors' reported benchmark results are not independently reproduced here.

## Decision supported by the evidence

Adopt Markdown as the inspectable canonical memory representation, with a small bootstrap index, scoped topic notes, and deterministic lexical retrieval. Keep search indexes disposable. This can eliminate embedding, reranking, and query-expansion model downloads from the memory path. It does **not** establish that Markdown alone improves answer accuracy or total task latency. Agentic search adds model/tool round trips; note extraction and consolidation can still require the already-configured conversational LLM.

Treat “no tokenizer model” precisely: no learned tokenizer assets or inference model should be needed to store, chunk, index, or retrieve memories. Ordinary Unicode splitting, byte/character budgets, BM25 term analysis, and character n-grams are algorithms, not downloaded ML models. The conversation model itself still consumes tokens.

## Twelve source groups and what they actually establish

### 1. Claude Code auto memory: literal Markdown in a shipping workflow

Current official docs describe a repository-scoped local memory directory, a `MEMORY.md` index, and individual topic `.md` files. Only the first 200 lines or 25 KB of the index, whichever comes first, load at conversation start; topic files load on demand. Notes distinguish user, feedback, project, and reference content, and the docs advise against saving facts readily derivable from code. Frontmatter can receive an automatically maintained modification timestamp. This is operational precedent for a compact index and selective file reading, not a comparative retrieval benchmark. [Claude Code memory documentation](https://code.claude.com/docs/en/memory)

**OpenAlpaca lesson:** make startup memory small and explicit; show exactly which notes entered context. Keep user preferences, scoped project knowledge, and transient task state distinct. Prefer timestamps and references to frequently stale copies of code facts.

### 2. Anthropic memory tool: client-owned filesystem interface

Anthropic exposes `view`, `create`, `str_replace`, `insert`, `delete`, and `rename` operations against an application-owned memory namespace. Storage can be a real filesystem or another backend; the tool does not mandate Markdown. File sizes and returned content need limits, and every operation must confine resolved paths to the memory root. The tool pairs with compaction: durable notes and active-context reduction solve different problems. [Anthropic memory tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool)

**OpenAlpaca lesson:** implement a provider-neutral Rust tool contract, then optionally adapt Anthropic's tool surface. Do not make memory dependent on one provider's special API. Enforce scoping and path confinement in code, not in notes.

### 3. OpenClaw's canonical Markdown layout

Current docs separate `USER.md`, durable `MEMORY.md`, dated working notes, and optional `DREAMS.md` review summaries. The compact persistent layer is not a raw transcript archive. Oversized bootstrap content is truncated in context while the disk file survives. The docs explicitly recognize changed preferences, supersession, source authority, expiry, and action boundaries. [OpenClaw memory overview](https://docs.openclaw.ai/concepts/memory)

**OpenAlpaca lesson:** allow compact curated knowledge and detailed evidence to evolve independently. A durable preference update must supersede an old active preference, rather than append a contradiction. Memory may preserve approval context but must never be the application's permission-enforcement mechanism.

### 4. OpenClaw builtin Memory, including the QMD retirement

OpenClaw's v2026.8.1 release and current engine docs say its QMD integration has been removed; builtin Memory now owns the core path. Canonical Markdown remains intact while indexes are rebuilt. Builtin supports FTS5/BM25 and optional embeddings, with keyword-only operation when no embedding provider exists. It has deterministic ranking, bounded lexical expansion, CJK trigram support, and debounced incremental indexing. Provenance trust classes live outside editable prose. [Builtin engine](https://docs.openclaw.ai/concepts/memory-builtin), [v2026.8.1 memory release notes](https://docs.openclaw.ai/releases/2026.8.1/memory)

**OpenAlpaca lesson:** Markdown and SQLite are compatible when SQLite is a rebuildable index. However, an index is not disposable if it also contains the only trust labels or audit records; keep those in a separate durable control ledger. Do not recommend the retired OpenClaw QMD adapter as a current integration.

### 5. QMD: Markdown search does not imply model-free search

QMD itself remains a standalone local document search project. Its `search` mode is BM25-only and needs no LLM. `vsearch` requires embeddings; `query` adds hybrid retrieval, query expansion, and reranking. Current documented defaults include approximately 300 MB of embedding weights, 640 MB of reranker weights, and 1.1 GB of query-expansion weights. “All local” therefore does not mean “no models.” [QMD repository and README](https://github.com/tobi/qmd)

**OpenAlpaca lesson:** borrow collection scoping, source paths, snippets, and explainable retrieval. Its full pipeline conflicts with a zero-memory-model dependency target. Embedding QMD's separate Node/Bun runtime solely for BM25 would need a concrete advantage over native Rust/SQLite.

### 6. Letta filesystem benchmark: useful evidence, limited comparison

Letta's 2025-08-12 vendor research post reports 74.0% LoCoMo accuracy with GPT-4o-mini using conversation files and agent-directed search. The post contrasts its result with Mem0's reported number, but this is not proof that files intrinsically beat Mem0: model behavior, prompts, tool-call budgets, and retrieval loops matter. The report itself stresses that benchmark comparisons must isolate the agent architecture, tools, and underlying model. Its filesystem interface is not necessarily a plain local `.md` store. [Letta benchmark post](https://www.letta.com/blog/benchmarking-ai-agent-memory/)

**OpenAlpaca lesson:** compare complete systems under matched models and budgets. Include an agentic file-search baseline as well as one-shot lexical retrieval; do not import vendor accuracy claims into product acceptance targets.

### 7. Letta Code MemFS: current file projection and versioned context

The current official Letta Code prompt describes projecting memory blocks and external memory to a local MemFS and operating through normal file and Git tools. It separates retained conversation experience from agent-managed memory, and distinguishes in-context blocks from external notes reachable through indexes. This is direct implementation evidence for Markdown-facing memory, but a projection alone does not establish that every server-side canonical state or dependency is replaced by files. [Letta Code MemFS prompt](https://github.com/letta-ai/letta-code/blob/main/src/agent/prompts/letta_local_memfs.md)

**OpenAlpaca lesson:** expose human-readable diffs and revision history; separate replayable source conversations from distilled notes. Do not require users to install Git or infer that a Markdown projection removes backing services.

### 8. Actual filesystem-memory study, July 2026

*Filesystem-Based Memory for LLM Agents: Organization, Evolution, and Sustainability*, submitted 2026-07-29, explicitly studies directories of Markdown notes. It varies store shape, search tools, management/search models, and growth across conversational and embodied-task benchmarks. Its reported benefit of organization is reduced retrieval cost, roughly halved on large material; no studied agent converts organization itself into universally better answer quality. Most management agents lose organizational discipline as stores grow. Dated preference updates are an identified weakness. These are preprint findings about measured costs and task outcomes, not a guaranteed wall-clock speedup on OpenAlpaca. [Paper](https://arxiv.org/abs/2607.26637), [full text](https://arxiv.org/html/2607.26637v1)

**OpenAlpaca lesson:** use shallow organization and descriptive metadata; test source retention, preference changes, search effort, and answer quality as the corpus grows. Do not fund elaborate autonomous folder restructuring before proving it helps this application.

### 9. Memory growth and deletion rationale, August 2026

*Why Does CLAUDE.md Keep Growing? Catastrophic Remembering in Agentic Coding*, submitted 2026-08-11, studies instruction-file evolution and controlled instruction-maintenance experiments. It reports accumulation of instructions and suggests retaining rationale can make future pruning safer. This concerns agentic instruction documents, not a direct comparison of conversational memory retrieval algorithms; the findings are a recent preprint. [Paper](https://arxiv.org/abs/2608.11095), [full text](https://arxiv.org/html/2608.11095v1)

**OpenAlpaca lesson:** keep the reason, source, scope, and invalidation condition for a rule or preference. A small maintained index needs explicit deletion and supersession policy; “append forever” is not maintenance.

### 10. A-MEM: useful note design, not model-free Markdown evidence

The A-MEM paper (initial submission 2025-02-17) proposes Zettelkasten-inspired notes, contextual attributes, links, and memory evolution. Its official implementation uses ChromaDB, embeddings (the example selects `all-MiniLM-L6-v2`), and an LLM backend. Thus it supports structured note and link ideas, but cannot substantiate that Markdown-only lexical retrieval is faster or more accurate. The repository also distinguishes the reusable memory package from the experimental reproduction code. [A-MEM paper](https://arxiv.org/abs/2502.12110), [official implementation](https://github.com/agiresearch/A-mem)

**OpenAlpaca lesson:** adopt meaningful descriptions, aliases, explicit links, and provenance; avoid automatically rewriting many related notes on every insertion until consistency and cost are measured.

### 11. Broader agent-memory survey

*Memory in the Age of AI Agents*, initially submitted 2025-12-15, separates memory forms, functions, and dynamics: factual, experiential, and working memory differ; formation, evolution, and retrieval are separate concerns. It surveys benchmarks and frameworks rather than validating Markdown storage as a universal winner. [Survey](https://arxiv.org/abs/2512.13564)

**OpenAlpaca lesson:** specify exactly which memory subsystem is being retired. Persistent facts, current conversation context, transcript history, skills, and task checkpoints need different retention and retrieval rules even if several use Markdown.

### 12. SQLite FTS5: model-free lexical implementation details

SQLite's official FTS5 docs define deterministic Unicode, ASCII, stemming, and trigram tokenizers and BM25 ranking. Unicode61 splits on character classes; it is not a trained tokenizer. Trigram search supports substrings, but full-text queries shorter than three Unicode characters do not match. Contentless/external-content indexes have consistency constraints. [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html)

**OpenAlpaca lesson:** test CJK and short identifiers explicitly. Use English/identifier lexical retrieval plus a CJK/substring path; add a bounded fallback for one- and two-character queries. Do not describe a trigram-only index as complete Chinese recall. Rebuild derived search state from canonical files, with content hashes to detect stale results.

## Concrete architecture implications (proposed, not claims about current OpenAlpaca)

1. **Canonical records:** one modest note per stable topic/decision with frontmatter fields such as stable ID, title, scope, kind, observed/updated timestamps, aliases, source conversation/turn IDs, and supersession. Do not treat frontmatter supplied by an agent or arbitrary external editor as authority for cross-scope access or trust elevation.
2. **Three content layers:** a small curated index/pinned preferences; topic notes retrieved by relevance; optional evidence/journal records read only when needed. Keep raw transcripts in the existing transcript subsystem rather than duplicating everything into memory.
3. **No model on the deterministic retrieval path:** lexical candidate generation, metadata filtering, exact identifier/path matching, aliases, section-aware byte-limited chunks, and deterministic scoring. Let the existing conversation model reformulate queries only as a bounded fallback, and count those calls separately.
4. **Provider-neutral tools:** `memory_search`, `memory_read`, `memory_write`/patch, `memory_forget`, and `memory_list`. Return stable source IDs, path/section/line locations, revision/content hash, and why a candidate matched. A search result should be evidence, not a privileged instruction message.
5. **Persistence:** lock by scoped store, compare expected revision, write a temporary sibling, sync as needed, atomically rename, then update a disposable index. Detect external edits by content hash; tolerate malformed files without silently deleting them. Readers must not receive stale deleted/superseded content just because an async index update is pending.
6. **Trust/control separation:** filesystem notes contain knowledge. Application permissions, scope ownership, provenance attestation, deletion tombstones, and audit events belong in a durable application-controlled record. If all durable state must be file-based, define a separate protected manifest/event log rather than hide essential state in the disposable search DB.
7. **Explicit forget semantics:** remove the canonical memory and every retrieval projection; define whether historical revisions and transcript evidence remain. Tombstones must prevent later consolidation from resurrecting a deliberately forgotten fact.
8. **Bounded maintenance:** prefer updates driven by explicit corrections or completed tasks. Consolidate outside response latency, with revision checks, preserved evidence, and a change log. An ordinary note write should not trigger global model-based reorganization.

## Evaluation needed before claiming “faster and more accurate”

Use the same conversation model, prompts, corpus, and context budget to compare current OpenAlpaca memory, Markdown plus deterministic lexical search, and bounded agentic file retrieval. Track cold/warm retrieval p50/p95, total answer latency, model round trips, indexing duration, on-disk size, RSS, and model downloads. Separate search latency from end-to-end answer time.

Quality cases should include exact names/paths, paraphrases, bilingual English/Chinese queries, two-character Chinese terms, temporal preference reversals, conflicting notes, source citation correctness, deleted facts, misleading third-party text, cross-project isolation, and long-term growth. Measure answer correctness, supportedness, abstention on absent evidence, and stale/forbidden-memory recall. Include constrained context and failed/missing search indexes. Public LoCoMo/LongMemEval-style tests can supplement an application-specific corpus, but cannot replace it.

Suggested gates are project choices, not published guarantees: zero memory-specific model downloads; zero network calls for deterministic search; scope/deletion/supersession correctness as hard invariants; no loss of answer quality on agreed tasks; retrieval and end-to-end latency improvements measured separately.
