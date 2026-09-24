# Evaluation report — the Markdown memory replacement

**Status: DRAFT (2026-09-24), systems D and E still running.** Plan slice 4 ([04-implementation-plan.md](04-implementation-plan.md) §2, §5). Every number below is a measurement with its conditions, or is marked *pending*. No claim in this report exceeds the measurements.

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

*Pending — D and E are running (`memeval/run-DE.sh`; each run is about 2–2.5 h on this machine).* The 3-question smoke of D scored 3/3 with per-turn recall hits in 2 rows and `memory_search` called in 2 (the harness logs `recalled=` and `relied=` per row).

| System | Correct | Accuracy | Per-turn recall rows | `memory_search` calls | Reader p50 / p95 | Wall |
|---|---|---|---|---|---|---|
| D | pending | | | | | |
| E | pending | | | | | |

Comparison D vs B (`compare.py`): *pending*.

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
| Build and footprint | **Measured (3.6):** 97 of 465 daemon dependency packages gone; release `openalpacad` 75,514,144 → 54,556,208 bytes (−21.0 MB), release `openalpaca` 32,624,656 → 14,311,120 bytes (−18.3 MB); no build-time ONNX Runtime fetch; no first-boot model download. Bundle scan: *pending (building)*. |
| End to end | Reader latency per system is in §3; token counts are in the archived reports. |

## 5. Gates at the report's head

Run by the session lead at `d3a3e712` (slice 3 complete): `cargo test --workspace --exclude openalpaca_gui` 3,767 passed / 0 failed / 7 ignored; `cargo clippy --workspace --exclude openalpaca_gui --all-targets` exit 0 (93 pre-existing warning lines, none on changed lines per each scoped review); `python3 scripts/gen_api_docs.py --check` 0; `cargo check -p openalpaca_gui` 0; `cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run` 0; `bun run check` / `test` (96 files, 1,149) / `format:check` / `build` all 0.

## 6. Residue and follow-ups

- The CLI `memory edit` truncated-note guard (3.9 review finding 2, CLI half): *in progress*.
- `GET /v1/memories/recently-used` has no GUI reader (design §7c R8; DESIGN_SPEC §5.7).
- Subagent deliveries never reach the recently-used ring (plan §7a S3-R3).
- 100K-note stress and the startup timing at scale (above).
- A live GUI click-through on an isolated store is the owner's check; the GUI's unit gates and the Vite build passed.

## Revision log

- **draft 1 (2026-09-24):** baselines, invariants, gates, footprint; D/E pending.
