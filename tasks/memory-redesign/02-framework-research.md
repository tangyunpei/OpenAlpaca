# Modern memory frameworks and evaluation research

Checked against live primary sources on **2026-09-23**. This is a research input for OpenAlpaca's Markdown memory redesign, not a completed implementation or a performance measurement. Repository and vendor documentation describe moving products; paper publication dates are included to avoid conflating older experiments with current software.

## Findings that change the design decision

A canonical Markdown store with selective, lexical retrieval can satisfy the goal of removing a dedicated local embedding model and its tokenizer/model downloads. **Markdown alone does not establish better speed or accuracy.** Those outcomes depend on curation, evidence preservation, scope filtering, retrieval, context limits, and evaluation. A resident SQLite index can outperform repeated file scanning. The intended improvement is simpler dependencies and cheap, bounded local retrieval, not a property of the filename extension.

Use the existing conversational model for optional memory extraction or consolidation. Do not introduce an independent LLM, embedding service, graph database, Python sidecar, or cloud dependency into the default memory path. File reads and lexical lookup can operate without any model inference. Automatic extraction still consumes the existing model's inference budget. A lexical tokenizer is ordinary text processing; removing a learned embedding/tokenizer package does not eliminate the need to split text or budget context.

## Framework comparison

| System | What current primary sources establish | Fit for OpenAlpaca |
|---|---|---|
| Mem0 OSS / managed platform | Memory processing uses an LLM, embeddings, a vector store and a history store; the managed system has optimizations absent from OSS. | Good comparative baseline and source of retention/retrieval lessons. Direct adoption would preserve dependencies the user wants removed. |
| Former OpenMemory MCP | Archived self-hosted MCP server/dashboard; current OpenMemory repository also hosts a different session-transfer tool. | Do not choose the archived server as the new foundation. MCP can remain an adapter around an independently designed native store. |
| Current Letta Agent SDK / MemFS | Markdown memory in a Git repository, with always-in-context and on-demand tiers; background consolidation. | Closest strong contemporary precedent for the proposed representation and retrieval hierarchy. Borrow concepts rather than replace the Rust agent runtime. |
| MemGPT paper | Explicit movement between limited prompt context and external memory. | Supports tiered memory and agent-managed recall conceptually; not evidence that all external memory should be vectors or Markdown. |
| Graphiti / Zep | Temporal facts, provenance, hybrid semantic/keyword/graph retrieval; self-hosted Graphiti needs graph infrastructure and model services. | Borrow temporal validity and evidence lineage; avoid adding a graph runtime to a small desktop application's default memory. |

### Mem0: current architecture versus the 2025 paper

Current Mem0 OSS defaults are `gpt-5-mini`, `text-embedding-3-small`, local Qdrant, and SQLite history. The self-hosted server defaults to Postgres/pgvector. These can be configured, but this is not a model-free or Markdown-native system. Hosted inference eliminates local model downloads by moving work to an API, not by removing the dependency. Operational costs include extraction calls, document/query embeddings, storage/index operations, and any optional reranker. [Mem0 OSS overview](https://docs.mem0.ai/open-source/overview).

The live README labels a **new April 2026 algorithm**: single-pass ADD-only extraction, entity linking, semantic/BM25/entity fusion, and temporal ranking. It reports 92.5 on LoCoMo and 94.4 on LongMemEval, with approximately 7K retrieved tokens; it explicitly says these are **managed-platform results with proprietary optimizations**, not reproducible expectations for the OSS library. Optional enhanced NLP setup downloads a spaCy language package. Its embedded-entity design still uses embeddings. These facts make “use Mem0 to remove models” a poor fit. [Mem0 current README](https://github.com/mem0ai/mem0).

The **2025-04-28 Mem0 paper** describes an older extraction/consolidation design and its graph extension. It reports a 26% *relative* LLM-judge improvement over its tested OpenAI baseline, 91% lower p95 latency, and over 90% lower token cost than full-context processing. These are author-reported comparisons under that experiment, not independent evidence that Mem0 beats OpenAlpaca, nor evidence about Markdown. They should not be mixed with the 2026 scores or called universal percentage improvements. [Mem0 paper](https://arxiv.org/abs/2504.19413).

**Design inference:** keep source-backed evidence when state changes; create a new version or superseding fact instead of silently destroying history. Carry lexical aliases and explicit entity labels. Do not copy the policy of giving assistant statements equal factual weight: an assistant saying “I booked it” should remain a claim unless a tool result or user confirmation proves completion. Preserve role, source message, and evidence type.

### OpenMemory: naming and maintenance matter

The old OpenMemory self-hosted MCP server/dashboard is explicitly **archived and no longer actively maintained**. Its retained README shows separate LLM and embedding configurations, including OpenAI defaults and Ollama alternatives. “Local” storage therefore did not itself mean zero remote inference or zero model dependency. The maintainers direct users to the current Mem0 self-hosted server. [Archived OpenMemory server](https://github.com/mem0ai/openmemory/tree/main/openmemory-archive).

The current `mem0ai/openmemory` README describes a CLI/TUI for transferring coding sessions between Claude Code, Codex and OpenCode; memory-file/skill portability and real-time sync are listed as roadmap items. Do not confuse this with a maintained memory retrieval server. [Current OpenMemory](https://github.com/mem0ai/openmemory/blob/main/README.md).

### Letta: a contemporary Markdown precedent

The current Agent SDK documentation says memory entries become Markdown files in an agent-owned Git repository. Files under `system/` enter the system prompt every turn; other files are exposed through a tree and loaded when needed. MemFS projects that repository onto the working computer, and edits are committed/pushed. Shared repositories and background “dreaming” support reuse and consolidation. This is current product documentation, not an independent accuracy benchmark, and its remote/Git operational model need not be copied. [Letta Agent SDK memory](https://docs.letta.com/agent-sdk/memory).

The older **MemGPT** paper (2023-10-12; revised 2024-02-12) motivates hierarchical memory and controlled movement between fast context and slower external storage. It is useful architectural background, not a description of every current Letta backend. [MemGPT paper](https://arxiv.org/abs/2310.08560).

**Design inference:** expose a tiny, curated core plus a compact catalog; read supporting notes on demand. Letta's system-prompt insertion is a precedent, not a security policy to copy blindly: automatically derived memory should remain labelled reference data, and should not gain system-instruction authority. Local atomic revisions are sufficient for first release; mandatory cloud Git sync is not necessary. Background consolidation needs inference accounting and conflict detection against concurrent human edits.

### Graphiti / Zep: valuable semantics without the graph dependency

Graphiti models entities, facts with validity windows, and source episodes. Its current README describes semantic/BM25/graph retrieval and defaults to OpenAI inference and embeddings, plus an external graph backend. Neo4j/FalkorDB/Neptune are supported; Kuzu is currently deprecated. Zep is a managed product with proprietary infrastructure, while Graphiti is a framework requiring surrounding implementation. Vendor latency claims for Zep should not be assigned to a local Graphiti installation. [Graphiti current README](https://github.com/getzep/graphiti).

The **2025-01-20 Zep paper** reports DMR scores of 94.8% versus 93.4% for the tested MemGPT comparison, and improvements on LongMemEval. These are author/vendor experiments over particular configurations. They neither establish a fair comparison to 2026 Mem0 nor prove a graph is required. [Zep paper](https://arxiv.org/abs/2501.13956).

**Design inference:** retain both observation time and effective time where known; add `supersedes`, source links and a current/superseded state to Markdown facts. “I moved in June” recorded in September has two dates. Unknown dates must remain unknown. A lightweight link map can be rebuilt from Markdown; a graph database is unnecessary until measured relationship queries justify it.

## Evaluation research and implications

### LongMemEval: correctness across changed facts and missing evidence

LongMemEval's official repository describes 500 questions covering extraction, multi-session reasoning, temporal reasoning, updates and abstention. The supplied S, M and oracle datasets differ; oracle histories contain only evidence sessions. The repo announces cleaned histories in September 2025 and a newer V2. Its metadata includes gold evidence annotations such as `has_answer` and answer-session IDs. [LongMemEval official repository](https://github.com/xiaowu0162/LongMemEval).

**Evaluation design:** pin the cleaned dataset revision, strip gold annotations before memory ingestion, and keep questions/answers unavailable during offline consolidation. Test ordinary S histories first; use oracle only to isolate reader failures. Report category-level accuracy, retrieval evidence recall, supported-citation rate and abstention correctness. A system returning an old address beautifully is still wrong. Do not use the label “LongMemEval” without specifying the variant, history scale, generator, reader, judge, retrieval budget and allowed query loops.

### LoCoMo: useful complement, not a universal leaderboard

LoCoMo's project describes long conversations generated from personas and temporal event graphs, then checked by humans. Evaluation includes QA, event summarization and multimodal dialogue. QA covers single-hop, multi-hop, temporal, world-knowledge and adversarial questions. The original reported QA metric is answer F1, which is not interchangeable with later vendor LLM-judge scores. [LoCoMo project and paper links](https://snap-research.github.io/locomo/).

**Evaluation design:** use QA categories that directly test the planned memory features and include adversarial/unanswerable cases. Record the exact category subset and scoring protocol. Preserve an untouched holdout; improving a public benchmark by repeatedly tuning retrieval against its questions is not deployment evidence.

### LongMemEval-V2: more relevant to a coding agent's learned experience

The current official V2 repository describes **451 curated questions**, up to 500 trajectories per history, and web/enterprise domains. It tests state recall, dynamic changes, workflows, local gotchas and premise awareness, and scores both accuracy and latency. It includes RAG, Codex and AgentRunbook variants; its harness separates retrieval input from private gold metadata. Some benchmark methods use embeddings, which does not require an evaluated backend to do so. [LongMemEval-V2 official repository](https://github.com/xiaowu0162/LongMemEval-V2).

**Evaluation design:** after the first release's local fixtures and classic QA suite, evaluate a V2 subset and OpenAlpaca-specific tasks: repeated build fixes, changed repository commands, cancelled decisions, failed tool actions and instructions that apply only to one workspace. This is more relevant than merely remembering biographical facts. Keep benchmark dependencies isolated from the shipped application.

## Proposed comparison protocol for the implementation plan

These are proposed experimental controls, not external benchmark results:

1. Freeze representative OpenAlpaca conversation fixtures and their expected source-backed memories before changing the implementation.
2. Compare the existing system, no persistent memory, a single bounded `MEMORY.md`, and the proposed tiered Markdown plus lexical index design.
3. Use identical reader model/version, query set, prompt budget, tool-loop allowance and evaluation judge for every candidate. Measure indexing/consolidation separately from retrieval and answer generation.
4. Test cold start with network disabled, warm search p50/p95, peak RSS, disk use, package size, extraction token cost, end-to-end response time and indexed-note growth. File format alone is not the comparison variable.
5. Include paraphrases and Chinese/English queries, identifier/path exact matches, low-overlap semantic queries, renamed entities, contradictory updates, historical questions, “forget this,” and sources containing instructions addressed to the assistant.
6. Validate behavior after manual file edits, malformed frontmatter, deleted notes, stale indexes, concurrent writes and crashes between file/index updates. Canonical notes plus durable control records must rebuild all retrieval state.
7. Predeclare go/no-go criteria: no mandatory model download; no hidden network lookup on basic search; no cross-scope retrieval; no deleted-fact resurrection; no significant task-accuracy regression; a measured improvement in resource use or latency under realistic note counts.

Semantic recall without embeddings remains the largest research risk. Mitigate it with curated aliases, query normalization, scope/entity/date filters, and a bounded model-assisted follow-up search only when initial results are weak. Measure that fallback's latency and cost explicitly. If a neural reranker or embedder is later evaluated, keep it an optional experiment rather than quietly restoring a mandatory model dependency.

## Evidence boundaries

No reviewed source proves that Markdown is universally faster or more accurate than database-backed memory. The strongest directly relevant current precedent is Letta's file/Git memory architecture. Mem0 and Graphiti provide useful retention, hybrid retrieval and temporal lessons, but would not remove embeddings by default. No packages were installed, model APIs called, paid services configured, or benchmarks executed during this research pass. Pricing tiers were not used for the recommendation; costs above identify the operations that would need measurement rather than quoting volatile dollar amounts.
