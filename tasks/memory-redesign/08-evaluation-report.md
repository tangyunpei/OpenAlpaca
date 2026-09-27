# Evaluation report — the Markdown memory replacement

**Status: DRAFT (2026-09-24): D measured and the gate passed; post-fix D′/E′ pending.** Plan slice 4 ([04-implementation-plan.md](04-implementation-plan.md) §2, §5). Every number below is a measurement with its conditions, or is marked *pending*. No claim in this report exceeds the measurements. Every figure here is **pre-LQ17**: it was measured on memory index schema 3, before the index layout revision of [09](09-learning-design.md) §6.5 (index schema 4) was built; that revision's own retrieval measurements are in 09 §6.5.5 and its build's record.

## 1. What was measured, and how

- **Corpus and questions:** `scripts/eval/memory/notes-gold.json` (83 gold notes, English and Chinese, one fictional user and two fictional projects) and `questions-heldout.json` (470 questions in eight categories). The tuning split (150) was never loaded by any run reported here.
- **Harness:** `crates/openalpaca_core/tests/memory_eval.rs` — an `#[ignore]`d, env-gated integration test that drives a real `Orchestrator`, seeds the notes through the memory store surface, asks each question through the chat front door and judges the answer against the gold answer with the frozen `judge-prompt.md` (binary correct/incorrect, temperature 0). Systems A and B ran on the old engine (rev at `d08317fb`, from a worktree frozen before slice 3 changed a line of it); D and E run on the new engine at the report's git head.
- **Models:** reader and judge are the same local Ollama model, `qwen3.8:27b-mtp-q8_0`, on the owner's MacBook Pro (Apple M5 Max, macOS 26.6.2, rustc 1.93.0). The owner authorized this on 2026-09-23. B additionally used the local E5 embedder (`intfloat/multilingual-e5-base`, 768 dims), downloaded once into the scratch store under explicit authorization.
- **Isolation:** every run used `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in a session scratch directory; `~/.openalpaca` was never read or written. Tests under plain `cargo test` never ran any of this.
- **Archived reports:** `scripts/eval/memory/reports/` (paths and the hostname redacted).

## 2. The systems

| Id | System | Engine |
|---|---|---|
| **A** | The per-turn FTS path only (`prompt_ctx::MemorySource`, whole-message implicit-AND query) | old |
| **B** | The `memory_search` tool path, hybrid FTS + E5 vectors, with the per-turn path also live | old |
| **D** | The new service: per-turn recall through `MemoryService::recall` over `memory_index`, `memory_search` offered | new |
| **E** | D plus at most two model-directed follow-up searches | new |

## 3. Results

### 3.1 The old engine (baselines)

| System | Correct | Accuracy | Per-turn recall rows | `memory_search` calls | Reader p50 / p95 | Wall |
|---|---|---|---|---|---|---|
| A | 61 / 470 | **13.0 %** | 1 | 0 | 17.2 s / 43.3 s | 185 min |
| B | 426 / 470 | **90.6 %** | 1 | 492 | 12.6 s / 33.2 s | 142 min |

Per category, A vs B: abstention 93.3 % vs 95.6 %; changed facts/time 0 % vs 92.7 %; cross-language 0 % vs 90.0 %; exact recall 2.5 % vs 92.5 %; paraphrase 5.0 % vs 88.8 %; project procedures 1.8 % vs 96.4 %; rejected/failed actions 26.7 % vs 77.8 %; source conflicts 0 % vs 90.0 %. A's 61 correct answers are almost all abstentions (42) — the only category where "I don't know" is right — which is the measured form of the audit's finding that the old per-turn recall barely worked (one row in 470 with a hit).

**B is the comparator.** Plan §5's gate: the paired-difference 95 % CI lower bound of D against B must be above −5 pp.

### 3.2 The new engine

**D at `8815b4cd` (the build as implemented, before any change prompted by this evaluation) — the gate-deciding run.**

| System | Correct | Accuracy | Per-turn recall rows | `memory_search` calls | Reader p50 / p95 | Wall |
|---|---|---|---|---|---|---|
| D (`8815b4cd`) | 443 / 470 | **94.3 %** | 393 | 185 | 11.5 s / 32.7 s | 136 min |

**D vs B** (`compare.py`, archived as `scripts/eval/memory/reports/compare-D-prefix-vs-B-2026-09-24.txt`): paired difference **+3.6 pp, 95 % CI [+0.5, +6.7]**, SE 1.60 pp; 57 discordant pairs (12.1 %): D-only correct 37, B-only 20. **Gate: CI lower bound +0.5 pp > −5 pp → PASS.** Because the CI excludes zero, the plan's rule allows the claim *more accurate* — on this corpus, this reader/judge model and this machine, and no further.

Per category, D vs B: abstention 97.8 % vs 95.6 % (+2.2); changed facts/time 87.3 % vs 92.7 % (**−5.5**); cross-language 92.9 % vs 90.0 % (+2.9); exact recall 98.8 % vs 92.5 % (+6.2); paraphrase 97.5 % vs 88.8 % (+8.8); project procedures 98.2 % vs 96.4 % (+1.8); rejected/failed actions 95.6 % vs 77.8 % (+17.8); source conflicts 80.0 % vs 90.0 % (**−10.0**). No category is *worse than* −10 pp, so none blocks under plan §5; the two regressions are explained in §3.3.

D also changes *how* memory reaches the answer: the per-turn recall delivered notes in 393 of 470 rows (A: 1 row, B: 1 row), and the model called `memory_search` 185 times against B's 492 — fewer tool rounds, lower reader p50 (11.5 s vs 12.6 s), more input tokens per turn (the recalled notes ride in the prompt).

### 3.3 The two regressions, diagnosed

Every miss in the two regressed categories was read against the gold notes and the notes D actually delivered (`memory_used` per row):

1. **Source conflicts (8 misses, one question family).** Two gold notes share `fact_key: lantern.staging.db`: *lantern_staging on pg-staging-1* (observed March) and *lantern_stg on pg-staging-2* (observed June), no `supersedes` link. Design §3 rule 3 then resolved such a pair by authority, then **newest `updated_at`**; the loser is hidden from automatic recall. The harness seeded notes in file order, which wrote the March note (gold index 60) first and the June note (61) right after it. Under that rule a note written later can lose only by a tie, and the stamps are to the second, so the two were almost certainly stamped in the same second and the tie fell to the lower of the two ids the service had minted at random — a coin flip, which March won. D delivered only the stale note and answered from it every time. Two defects: the harness's seeding let two conflicting notes share a second and did not follow the order a user would have told the assistant in (fidelity), and the rule itself is fragile — **editing the older note, even a typo, flips the winner** (a real product defect, independent of the eval).
2. **Changed facts/time (7 misses).** (a) A planned trip note with a validity window of 5–19 October 2026 was **never returned, even to a search for "Stockholm"**: current search filters `valid_from <= now`, hiding a window that has not started yet, while design §4 only excludes a window that has *ended* — a conformance bug. (b) A note about a car sold in April has an *ended* window, so it is hidden unless the model searches with `historical: true`; that is the design working as written, but the tool's description never tells the model when to use the flag.

**Fixes** (Opus implements, Fable reviews; landed in `f2dbe1f9`, `4efda6b8`, `be4beb8c`, `80a673f8` and `50168294`, the post-fix runs pending): the validity filter excludes only ended windows on the current path; the same-`fact_key` tie-break uses observation or first-write time so an edit cannot flip it (design §3 rule 3, §7c R9); the harness seeds in chronological order and writes a same-`fact_key` note in a later second than its pair; the `historical` flag's description says what it includes and when to use it. Because these were prompted by held-out misses, the post-fix runs below are **optimistic** and are reported beside, never in place of, the gate-deciding D above.

| System | Correct | Accuracy | Notes |
|---|---|---|---|
| D′ (post-fix) | pending | | same held-out set; optimistic |
| E′ (post-fix) | pending | | D′ + at most two follow-up searches |

A partial pre-fix E (13 rows) was stopped when the fixes started and is not reported.

## 4. Hard invariants (plan §5 "Targets")

| Dimension | Result |
|---|---|
| Model and network independence | **Met.** Fresh scratch store, daemon under `sandbox-exec -f scripts/eval/memory/no-network.sb` (outbound denied): up in 3 s, no network, download or error line in its log; CLI round-trip add → list → search → show → forget → `daemon stop` all succeeded; the six `cargo tree -p openalpacad -i <crate>` checks (`fastembed`, `ort`, `ort-sys`, `tokenizers`, `hf-hub`, `sqlite-vec`) are empty; the completeness search finds only comments, tests, fixtures and archived README texts. |
| Scope and authority | **Met by tests** (slice 2 + 3): the scope matrix, spoofed identity fields refused, a `ModelTool` write never mints `user`, `confirm` refused from the tool, store-owned paths refused by the file tools before and after resolution, the resolver's absent-parent hole closed (3.12). |
| Forget and corrections | **Met by tests:** tombstone-first forget, no revival after cache deletion or pending-write recovery, a forgotten or archived replacement does not revive its predecessor, a lower-authority link is reported not honored; the offline round-trip's forget left one tombstone and nothing else. |
| Persistence | **Met by tests:** revision conflicts surfaced (409 / 428 / 400 route tests), the schema-42 fixture refused byte-for-byte unchanged without sqlite-vec. |
| Source resolution | Recall returns the revision and span actually read (verified per file, `Freshness`); the `memory_used` field on `done` names the notes injected. Factual support is what §3.2 measures. |
| Warm retrieval | **Met.** Slice-2 bench, release build, 10K notes (1,280 B average, 20 % CJK, 4 % large): search + per-file verification **p50 8.4–13.7 ms, p95 16.7–27.4 ms**, max 42.7 ms (two runs; reviewer's and implementer's), against a 50 ms p95 target. |
| Scale stress (100K) | *Not run in this wave* — recorded as follow-up; the 10K cache was 107.7 MiB (≈ 8× the corpus, the trigram index) and a full rebuild took 8.1 s in release. |
| Startup | The reconciliation stat scan runs at service start; not separately timed beyond the 3 s daemon start above. *Follow-up: time it at 10K notes.* |
| Build and footprint | **Measured (3.6):** 97 of 465 daemon dependency packages gone; release `openalpacad` 75,514,144 → 54,556,208 bytes (−21.0 MB), release `openalpaca` 32,624,656 → 14,311,120 bytes (−18.3 MB); no build-time ONNX Runtime fetch; no first-boot model download. **Bundle (`scripts/release/package-macos.sh` at `95a26295`):** archive 52,712,993 bytes; `openalpacad` 54,894,912 and `openalpaca` 15,287,424 bytes after 3.8–3.9 added the memory routes and CLI; the `.app` holds no `.onnx`, tokenizer or ORT file; the only strings in the two binaries matching `onnxruntime|fastembed|sqlite-vec|vec0` are the superseded home-README texts embedded for the refresh tripwire (`store/superseded_readmes/home-5.txt`, `home-6.txt`) and the baseline SQL's own comment "needs no sqlite-vec" — text, not code. |
| End to end | Reader latency per system is in §3; token counts are in the archived reports. |

## 5. Gates at the report's head

Run by the session lead at `8815b4cd` (slice 3 complete, the edit guard and the follow-up docs in): `cargo test --workspace --exclude openalpaca_gui` 3,769 passed / 0 failed / 7 ignored; clippy, `gen_api_docs.py --check`, `cargo check -p openalpaca_gui` and the four `bun run` gates all exit 0. Earlier, at `d3a3e712`: `cargo test --workspace --exclude openalpaca_gui` 3,767 passed / 0 failed / 7 ignored; `cargo clippy --workspace --exclude openalpaca_gui --all-targets` exit 0 (93 pre-existing warning lines, none on changed lines per each scoped review); `python3 scripts/gen_api_docs.py --check` 0; `cargo check -p openalpaca_gui` 0; `cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run` 0; `bun run check` / `test` (96 files, 1,149) / `format:check` / `build` all 0.

## 6. Residue and follow-ups

- The CLI `memory edit` truncated-note guard (3.9 review finding 2, CLI half): **done** (`5aa4c1a5`, `d48e427a`) — the CLI refuses before the editor opens, and a route test pins that a note over 32 KiB never loads, so no cut body can reach the route.
- `GET /v1/memories/recently-used` has no GUI reader (design §7c R8; DESIGN_SPEC §5.7).
- Subagent deliveries never reach the recently-used ring (plan §7a S3-R3).
- A note whose validity window has not begun is recalled automatically since `f2dbe1f9`, but the recall header (`RecalledNote::render`, `memory/service/types.rs`) carries no `valid_from`/`valid_to`, so the model has only the dates in the body to tell a planned fact from a current one. For slice 4 to consider; nothing is changed yet.
- 100K-note stress and the startup timing at scale (above).
- A live GUI click-through on an isolated store is the owner's check; the GUI's unit gates and the Vite build passed.

## Revision log

- **draft 1 (2026-09-24):** baselines, invariants, gates, footprint; D/E pending.
- **draft 2 (2026-09-24):** §3.3's source-conflict diagnosis corrected: file order wrote the March note *before* the June one, not after it, so March most likely won a same-second `updated_at` tie on the lower random id; the fixes paragraph names its commits and the same-second seeding fix.
- **draft 3 (2026-09-24):** §6 records that the recall header carries no validity window.
- **draft 4 (2026-09-27):** the status line says every figure is pre-LQ17 (memory index schema 3); the layout revision, 09 §6.5, is built as index schema 4.
