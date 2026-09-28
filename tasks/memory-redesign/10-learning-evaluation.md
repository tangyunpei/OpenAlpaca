# Evaluation report — Learning v2 (L8)

**Status: DRAFT — final-build numbers pending the re-measurement on 82180b84.** Draft 1, 2026-09-27. The report of design [09](09-learning-design.md)'s L8: the learning evaluation (§7), the scale benches and their decision rule (§6.4) and the recall-mode and episode additions (§13.4). Every number below is a measurement with its conditions and the file it was read from, or a placeholder `⟨FINAL: …⟩` for a figure the re-measurement on `82180b84` supplies. The first runs, made at `50cf878d` before the owner decisions LQ11–LQ17, are reported whole; a final figure goes beside its first-run twin, never in its place. **Every QA and coverage verdict here is the reader grading itself** (§1, §8).

**Sources.** None of it is tracked; it lives in the session scratchpad's `memeval/`:
- the first runs: `l8/report-*.json`, `l8/full-*.log`, `l8/progress.txt`, the chain `l8/run-chain.sh`, `l8/run-B-mlx.sh`, `l8/run-rejudge.sh`;
- the diagnosis: `l8/diagnosis/SUMMARY.json` (five findings, each with two sceptics' corrections) and its notes (`rerun-badoutput.md`, `ls-suggestions.md`, `episodes.md`, `learning-gap.md`, `completeness.md` and their `*sceptic*` files);
- the capture run: `l8/capture/` (`report-replay-capture.json`, `full-replay-capture.log`, `unusable.json`);
- the offline re-score of the first runs under the fixed harness: `l8/fix/rescore.json`, `l8/fix/impl.md`;
- the benches: `l8/bench/*.log`, `*.time`, `machine.txt`, and the scale analysis `l8/bench/scale/analysis.md` with its sceptic review;
- the index layout's build: `l8/layout/impl.md`, its benches `l8/layout/bench-base/` (the commit before, same day) and `l8/layout/bench-after/`, and the retrieval proxy `l8/layout/baseline/` and `l8/layout/after/`;
- the re-measurement: `final/` (`run.sh`, `progress.txt`, the reports as they land).

The decisions are 09 §10 (LQ11–LQ17, with the lead's tq-0032 ruling in §6.5.5) and its revision log (revs 3.26–3.45), `tasks/api-fix-plan.md` §0 and CLAUDE.md's *L8 and its follow-up* paragraph. The reports carry the machine's hostname; ⟨FINAL: the path under `scripts/eval/memory/reports/` the redacted reports are archived to, as 08 archived A, B and D⟩.

## 1. Summary

- **What was measured.** QA accuracy on 08's 470 held-out questions for: D′ in both recall modes (D-index, the default, and D-excerpts); the old engine's B re-run on the same model (B-mlx); L, whose store is learned from a 51-session, 300-turn conversation corpus; and L0, the same feed with learning off, so only the reader's own `memory_store` calls learn. The learning measures of §7.3 on L, its three replays, a second live learning run (L-episodes), L0 and L-S. A 20-question episode split. The §6.4 benches and the retrieval proxy of §6.5.5.
- **On what.** One local model, `qwen3.8:27b-mlx` on Ollama, is the reader, the extractor, the QA judge and the coverage judge (owner answer LQ1). Every report records `judge_is_reader: true`. The model graded its own answers and its own extraction. The reader's temperature is not pinned, so each live run is one sample of the whole system. One machine: the owner's MacBook Pro (Apple M5 Max).
- **The gate holds** (first run). D-index against B-mlx: **+6.8 pp, 95 % CI [+4.0, +9.6]**. The CI's lower bound is above −5 pp, so the gate passes; it also excludes zero, so the claim *more accurate* is allowed — on this corpus, with this model as reader and judge, on this machine, and no further. Final: ⟨FINAL: D-index (82180b84) vs B-mlx, difference and CI⟩.
- **The two recall modes are comparable.** D-index against D-excerpts: +0.6 pp, CI [−1.3, +2.5]. Index mode reads a smaller median prompt (6,606 against 7,566 input tokens per turn) for a few more tool rounds (1.66 against 1.56 per turn).
- **Learning fell short of D on the first run.** L answered 81.3 %, against D-index's 96.8 %: **−15.5 pp, CI [−19.2, −11.9]**. §7.3's QA target, L within −10 pp of D′, is not met. L against L0 is +12.3 pp, CI [+7.6, +17.0], so extraction carries much of L's score. Final: ⟨FINAL: L vs D-index, difference and CI⟩.
- **Learning targets, read against the extraction path** (LQ12; first run, median over the extraction samples, §4.2): other-script aliases **70.5 %** (target ≥ 60 %, met); restatements raising a suggestion or a duplicate **1.5 of 15** (≤ 2, met); coverage **73.5 %** (≥ 80 %, not met as measured — the 400-token judge cap cut 5–8 verdicts per replay, which could carry it to 83.1 %); changed facts **8 of 15** (≥ 12, not met; 9 of the replays' 16 failed correction slots are those cut verdicts). Final: ⟨FINAL: the four medians⟩.
- **Invariants.** No run retired a user note without a confirm, and no run resurrected a forgotten note. But the forced re-run could check nothing in four of the five learning runs: its one extraction answer was unusable, and the harness counts an unchecked re-run as a violation (§4.6). Final: ⟨FINAL: forced re-run checked in n of the final learning runs⟩.
- **L-S raised 4 of the 8 suggestions** expected; all four confirmed retire the old note. Final: ⟨FINAL: L-S targets with a suggestion / 8⟩.
- **The episode split scored 8 of 20**, a harness defect: the summaries' daily cap of 20 stopped every session from the 21st on (§5). Final: ⟨FINAL: L-episodes and L-replay-episodes, correct / 20⟩.
- **Scale.** §6.4's decision rule fired: at 100K notes the index's search + verification was p95 150.3 ms and the cache 1,065.5 MiB. The owner took the layout revision (LQ17). Built, it measures 48.6 ms and 366.6 MiB. The service's recall p95 at 100K went 666 → 398 ms (the scale and product slices) → 264 ms (the layout), still above §6.4's proposed 100 ms; the owner put the design point at about 30K notes (LQ15). The 100K service bench's peak memory rose with the layout and is not explained (§7).
- **What the first run changed.** Five diagnosed findings and the benches drove a harness slice, a scale slice, a product slice (LQ11, LQ13, LQ14, LQ16), the index layout (LQ17) and a close-out (§6). The final build `82180b84` holds all of them but the close-out's third repair round, which changes only the forget path.

## 2. Setup

### 2.1 Model, machine and isolation

- **Model.** `qwen3.8:27b-mlx` on a local Ollama is reader, extractor (no `extraction_model` is set), QA judge and coverage judge (09 §7.4, LQ1). Each report's `conditions` record `judge_is_reader: true`, `judge_same_family: true`, and for the learning systems `judge_is_extractor: true`.
- **Temperatures.** The reader at the model's default (`reader_temperature: null`); the extractor at 0.0 (09 §7.2); the QA judge at 0.0 with a 400-token answer. The coverage judge used 400 tokens in the first run; the fixed harness gives it 2,048 (H5, §6.1).
- **Machine.** Apple M5 Max, 128 GiB, macOS 26.6.2 (25G83), 18 threads, rustc 1.93.0 (`l8/bench/machine.txt`, each report's `machine`).
- **Isolation.** Every run used `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in the session scratchpad; `~/.openalpaca` was never read or written.
- **The model configuration** (`memeval/config-mlx/llm.toml`). `[orchestrator] model = "qwen3.8:27b-mlx"`, `fallback_models = []`; only Ollama enabled; `request_timeout_secs = 900` and `[timeouts] llm_request_timeout_secs = 900`. **Rate limits:** `[rate_limits] default_rpm = 10000`, `default_tpm = 100000000`, which lifts the keyless Ollama slot off the router's defaults (50 RPM, 40,000 TPM) — they only add waiting on a local model. Concurrency stays at its defaults (2 per key, 4 global). The file's `[embeddings]` block serves B alone (the old engine's E5).
- **The harness's conditions** (each report's `conditions`). Reader rounds at most 15, the main loop's 8 with at most 4 tools a round, a $1 cost cap per turn; recall 8 KiB in all and 2 KiB pinned, at most 8 items. A learning system lifts the extraction's daily caps (`extract_max_calls_per_day = 0`, `extract_max_daily_cost_usd` 1,000,000). The queue runs in `Manual` mode, flushed after every 10th user turn of a session (`turns`) and at its end (`switch`); a replay flushes each session whole (`replay`). The first run left `[orchestrator.learning] episode_max_per_day` at its default, 20 (§5); the fixed harness lifts it to 1,000 (H1).

### 2.2 The corpus and the questions

The data files are byte-identical at `50cf878d` and `82180b84` (SHA-256 checked for all six).

| File | Content | SHA-256 (first 16) |
|---|---|---|
| `conversations-learning.json` | after §7.1's second-session review (09 rev 3.21): **51 sessions, 300 user turns** — 163 noise (54 %), 61 explicit, 33 indirect, 15 restate, 11 rejection, 10 change, 7 correction; sessions 32 en, 14 zh, 5 mixed; 26 with no project, 18 Lantern, 7 Moss; 13 suggestion sessions | `7ebc4da1702b1cfe` |
| `notes-gold.json` | 83 gold notes (08 §1) | `575a887b9aea2a50` |
| `questions-heldout.json` | 470 questions, 8 categories; 303 en, 125 zh, 42 mixed | `b6249847a1663e6c` |
| `questions-episodes.json` | 20 questions; 13 en, 5 zh, 2 mixed | `6ae072496587ce2a` |
| `judge-prompt.md` | the frozen QA judge prompt | `df1cc6fa6adb60b2` |
| `coverage-judge-prompt.md` | the frozen coverage judge prompt | `7a0557ce8197639e` |

The 15 changed facts of §7.3 are the corpus's 8 gold change pairs (4 `supersedes` pairs, 4 same-key pairs) and its 7 in-conversation corrections.

### 2.3 The systems and the builds

| Id | What | Build | Run (UTC) |
|---|---|---|---|
| **B-mlx** | 08's B — the old engine: hybrid FTS + E5, `memory_search` offered, the per-turn path live — on this model | `d08317fb` (`live-eval-e5`) | 2026-09-25 16:17–17:45 |
| **D-rejudged** | 08's archived D report (470 answers of `qwen3.8:27b-mtp-q8_0`) graded again by this model; no reader call | harness at `fa09dc0c`; the report's build is `d3a3e712` | 17:46–17:57 |
| **D-index** | the new service, `recall_mode = "index"` (the default): 09 §7.5's **D′**, the seeded run 08 §3.2 left pending | `50cf878d` | 18:04–19:44 |
| **L** | the corpus fed through the gateway, learning on, conversation summaries written; then the questions with learning off; then the forced re-run | `50cf878d` | 19:44 – 09-26 00:48 |
| **L-replay-1…3** | L's kept transcript learned again: extraction only, no reader call, no question | `50cf878d` | 00:48–01:54 |
| **L-S** | the 8 change pairs' old notes seeded as the user's, then the 13 suggestion sessions (78 turns); no question | `50cf878d` | 01:54–02:36 |
| **D-excerpts** | D in `recall_mode = "excerpts"` | `50cf878d` | 02:36–04:19 |
| **L0** | L's feed, never flushed: only the reader's own writes learn | `50cf878d` | 04:19–09:03 |
| **L-episodes** | a second live L run, asked the 20 episode questions | `50cf878d` | 09:03–12:06 |

All from `l8/progress.txt` and each report's `build`. The first runs came from a worktree pinned at `50cf878d`, the last commit of L8-prep (09 rev 3.20), after the corpus review (`7b240b27`). A two-session smoke of L ran first and is not reported. `D-rejudged`'s report names build `d3a3e712`, where 08 labels the same run `8815b4cd`; the commits between them touch no evaluated path.

**The re-measurement** runs the chain `final/run.sh` from a worktree at `82180b84`: smoke-L, D-index, D-excerpts, L, L-replay-1…3, L-replay-episodes (a replay of L that writes summaries and asks the episode split, H9), L-S, L0, L-episodes. B-mlx is not re-run: its code is the old engine's and unchanged, and it was graded by the same judge. `82180b84` holds, beyond `50cf878d`, the harness slice (09 revs 3.26–3.29), the scale slice (3.30–3.32), the product slice (3.33–3.35), the index layout (3.36–3.40) and the close-out through its repair round 2 (3.41–3.43). The branch's head differs from it in code only by the close-out's repair round 3 (`3f3a2307`, `2b49639e`): the forget path and its tests, and two doc comments.

## 3. QA results

### 3.1 The first run

| System | Correct | Accuracy | `memory_search` calls | Input tokens per turn, mean / median | Rounds per turn | Tool calls per turn | Reader p50 / p95 | Wall |
|---|---|---|---|---|---|---|---|---|
| B-mlx | 423 / 470 | **90.0 %** | 465 | 4,932 (the report's total ÷ 470) | — | — | 8.2 s / 18.2 s | 88 min |
| D-index | 455 / 470 | **96.8 %** | 201 | 11,124 / 6,606 | 1.66 | 0.79 | 8.6 s / 26.8 s | 99 min |
| D-excerpts | 452 / 470 | **96.2 %** | 203 | 11,025 / 7,566 | 1.56 | 0.65 | 9.5 s / 25.3 s | 103 min |
| L | 382 / 470 | **81.3 %** | 353 | 14,862 / 14,182 | 2.00 | 1.22 | 12.6 s / 40.1 s | 305 min, the feed 154 |
| L0 | 324 / 470 | **68.9 %** | 314 | 13,817 / 13,790 | 1.91 | 1.12 | 13.1 s / 35.2 s | 285 min, the feed 139 |

No run had a judge error, an unparsed verdict or a turn error. B's harness predates the per-turn fields, so its rounds and tool calls are not recorded; its per-turn recall delivered notes in 1 row of 470, as in 08. For D, the rows in which every relied-on note was delivered, listed or delivered, and read or excerpted are 373, 380 and 367 (D-index) and 376, 376 and 367 (D-excerpts). For L and L0 these counts are 0 by construction: a relied-on id names a gold note, which a learned store does not hold.

**Per category** (correct / n and accuracy):

| Category | n | B-mlx | D-index | D-excerpts | L | L0 |
|---|---|---|---|---|---|---|
| abstention | 45 | 44 (97.8) | 45 (100.0) | 44 (97.8) | 43 (95.6) | 44 (97.8) |
| changed facts / time | 55 | 49 (89.1) | 53 (96.4) | 50 (90.9) | 47 (85.5) | 36 (65.5) |
| cross-language | 70 | 65 (92.9) | 68 (97.1) | 68 (97.1) | 52 (74.3) | 43 (61.4) |
| exact recall | 80 | 75 (93.8) | 78 (97.5) | 78 (97.5) | 63 (78.8) | 58 (72.5) |
| paraphrase | 80 | 70 (87.5) | 77 (96.2) | 76 (95.0) | 70 (87.5) | 60 (75.0) |
| project procedures | 55 | 49 (89.1) | 52 (94.5) | 54 (98.2) | 40 (72.7) | 39 (70.9) |
| rejected / failed actions | 45 | 34 (75.6) | 43 (95.6) | 43 (95.6) | 35 (77.8) | 37 (82.2) |
| source conflicts | 40 | 37 (92.5) | 39 (97.5) | 39 (97.5) | 32 (80.0) | 7 (17.5) |

**Per language:** en (303) 89.1 / 97.7 / 97.4 / 81.8 / 70.6 %; zh (125) 92.0 / 93.6 / 92.8 / 80.0 / 64.0 %; mixed (42) 90.5 / 100.0 / 97.6 / 81.0 / 71.4 %, in the table's column order.

### 3.2 The paired comparisons (`compare.py`)

Candidate first, baseline second. Every pair shares the judge, so `compare.py` refused none.

| Candidate vs baseline | Difference | 95 % CI | SE | Discordant pairs (candidate only / baseline only) | Reading |
|---|---|---|---|---|---|
| **D-index vs B-mlx (the gate)** | **+6.8 pp** | **[+4.0, +9.6]** | 1.44 | 48, 10.2 % (40 / 8) | **PASS** at −5 pp; *more accurate* allowed |
| D-excerpts vs B-mlx | +6.2 pp | [+3.2, +9.1] | 1.49 | 51 (40 / 11) | PASS; *more accurate* |
| D-index vs D-excerpts | +0.6 pp | [−1.3, +2.5] | 0.97 | 21 (12 / 9) | *comparable measured quality* |
| L vs D-index | −15.5 pp | [−19.2, −11.9] | 1.85 | 87 (7 / 80) | outside §7.3's −10 pp |
| L vs L0 | +12.3 pp | [+7.6, +17.0] | 2.40 | 134 (96 / 38) | learning on is *more accurate* |
| L0 vs D-index | −27.9 pp | [−32.1, −23.7] | 2.15 | 139 (4 / 135) | — |

- **The gate** (plan §5, as 08 §3.1 states it: the paired difference's 95 % CI lower bound of D against B above −5 pp). D-index is better in every category: rejected / failed actions +20.0, paraphrase +8.8, changed facts +7.3, project procedures +5.5, source conflicts +5.0, cross-language +4.3, exact recall +3.8, abstention +2.2 pp. 08 §3.3's two regressions (changed facts −5.5 and source conflicts −10.0 pp, at `8815b4cd` on the other model build) do not recur; their fixes are ancestors of `50cf878d`.
- **Index against excerpts, with its cost.** Per category the modes differ in four: changed facts +5.5, abstention +2.2, paraphrase +1.2 and project procedures −3.6 pp; the other four are equal. Index mode's median prompt is 13 % smaller (6,606 against 7,566 input tokens), its mean 1 % larger (11,124 against 11,025), since it takes more rounds (1.66 against 1.56) and tool calls (0.79 against 0.65); reader p50 8.6 against 9.5 s, p95 26.8 against 25.3 s. The measured retrieval is equal: 367 rows with every relied-on note read or excerpted in both, the measure the runbook names as like for like.
- **L against D-index.** At `compare.py --margin=-10` it fails, with six categories below the block line: cross-language −22.9, project procedures −21.8, exact recall −18.8, rejected / failed actions −17.8, source conflicts −17.5, changed facts −10.9 pp; paraphrase −8.8, abstention −4.4. Of the 80 rows only D answered, the diagnosis's labels put about 54 on the extraction path (unusable answers, extraction misses, key reuse), 16 on retrieval vocabulary, 4 on dates and 6 on the reader or the judge; 20 of the 80 rely on a gold note stated in one of the 8 sessions L never learned, and the weekly-review change alone moves 10 questions (`diagnosis/SUMMARY.json`, *learning-gap*, both sceptics). §6.1 (d) follows it to the fixes.
- **L against L0** isolates extraction as far as two live samples can: both are fed the same corpus under the same conditions but for `extraction_disabled`, and each reader's replies are its own draw. Source conflicts +62.5 pp (32 against 7 of 40), changed facts +20.0, cross-language +12.9, paraphrase +12.5; L0 is ahead only on rejected / failed actions (−4.4) and abstention (−2.2).

### 3.3 The judge, measured against itself

- **Re-judge of 08's archived D.** The 470 answers `qwen3.8:27b-mtp-q8_0` gave, graded again by `qwen3.8:27b-mlx`: 442 / 470 (94.0 %), against 443 (94.3 %) under the original judge. The judges agree on 469 of 470 rows (raw 0.998), **Cohen's κ 0.981** (`l8/report-D-rejudged-mlx.json`, `agreement_with_original`). This is agreement between two builds of one model family. It cannot measure the bias of a model grading itself against an independent judge; this machine has no model of another family (09 §7.4), and the owner chose self-judging (LQ1).
- **B-mlx against 08's B** (the same code, `d08317fb`; the other build of the model as reader and judge; compared with `--allow-judge-mismatch`): 90.0 against 90.6 %, −0.6 pp, CI [−3.6, +2.3], with **49 discordant rows** (23 / 26). A change of model build alone flips about one row in ten while moving the total by under a point.

### 3.4 The final build

| System | Correct | Accuracy | Input tokens per turn, mean / median | Rounds per turn | Tool calls per turn | Reader p50 / p95 |
|---|---|---|---|---|---|---|
| D-index | ⟨FINAL: D-index correct / 470⟩ | ⟨FINAL: D-index accuracy⟩ | ⟨FINAL: D-index tokens mean / median⟩ | ⟨FINAL: D-index rounds⟩ | ⟨FINAL: D-index tool calls⟩ | ⟨FINAL: D-index p50 / p95⟩ |
| D-excerpts | ⟨FINAL: D-excerpts correct / 470⟩ | ⟨FINAL: D-excerpts accuracy⟩ | ⟨FINAL: D-excerpts tokens mean / median⟩ | ⟨FINAL: D-excerpts rounds⟩ | ⟨FINAL: D-excerpts tool calls⟩ | ⟨FINAL: D-excerpts p50 / p95⟩ |
| L | ⟨FINAL: L correct / 470⟩ | ⟨FINAL: L accuracy⟩ | ⟨FINAL: L tokens mean / median⟩ | ⟨FINAL: L rounds⟩ | ⟨FINAL: L tool calls⟩ | ⟨FINAL: L p50 / p95⟩ |
| L0 | ⟨FINAL: L0 correct / 470⟩ | ⟨FINAL: L0 accuracy⟩ | ⟨FINAL: L0 tokens mean / median⟩ | ⟨FINAL: L0 rounds⟩ | ⟨FINAL: L0 tool calls⟩ | ⟨FINAL: L0 p50 / p95⟩ |

Per category and per language: ⟨FINAL: the four reports' per-category and per-language rows, in §3.1's shape⟩.

| Candidate vs baseline | Difference | 95 % CI | Discordant pairs | Reading |
|---|---|---|---|---|
| D-index (final) vs B-mlx (the gate) | ⟨FINAL: difference⟩ | ⟨FINAL: CI⟩ | ⟨FINAL: discordant⟩ | ⟨FINAL: gate verdict and claim⟩ |
| D-index vs D-excerpts (final) | ⟨FINAL: difference⟩ | ⟨FINAL: CI⟩ | ⟨FINAL: discordant⟩ | ⟨FINAL: reading⟩ |
| L vs D-index (final) | ⟨FINAL: difference⟩ | ⟨FINAL: CI⟩ | ⟨FINAL: discordant⟩ | ⟨FINAL: against §7.3's −10 pp⟩ |
| L vs L0 (final) | ⟨FINAL: difference⟩ | ⟨FINAL: CI⟩ | ⟨FINAL: discordant⟩ | ⟨FINAL: reading⟩ |
| D-index final vs D-index first | ⟨FINAL: difference⟩ | ⟨FINAL: CI⟩ | ⟨FINAL: discordant⟩ | two builds, one model: the D side's run-to-run spread plus the build's effect |

## 4. Learning results

### 4.1 Two writers in one store (LQ12)

A live run's learned store has two writers: **extraction** (the learning flushes) and **model tool** (the reader's own `memory_store` during the feed). The first-run reports pooled them; the owner ruled that every learning metric is reported by surface and that §7.3's learning targets are read against the extraction path, QA against the whole store (LQ12). The by-surface figures below are the offline re-score (`l8/fix/rescore.json`, `fix/impl.md`), which first reproduced every archived pooled number.

| Run | Learned notes (active / superseded / candidate) | Written by extraction / model tool (`memory_changed`) | Active notes by extraction / model tool | Summaries |
|---|---|---|---|---|
| L | 137 (116 / 16 / 5) | 45 / 92 | 36 / 80 | 20 |
| L-episodes | 145 (123 / 18 / 4) | 41 / 104 | 36 / 87 | 19 |
| L0 | 99 (96 / 3 / 0) | 0 / 99 | 0 / 96 | — |
| L-replay-1 | 130 (110 / 13 / 7) | 130 / 0 | 110 / 0 | — |
| L-replay-2 | 123 (107 / 9 / 7) | 123 / 0 | 107 / 0 | — |
| L-replay-3 | 133 (115 / 12 / 6) | 133 / 0 | 115 / 0 | — |

A replay is extraction only: a fresh owner and project, no reader call (`conditions.learning.replay_is_extraction_only`). Its spread is the extraction path's variance on a fixed transcript. The live extractor lists the reader's notes among the known ones and writes fewer notes (36 active against the replays' 107–115); in 6 of the 7 live sessions whose flush wrote nothing, the reader had already stored every gold fact of the session, so the empty answer was correct deduplication (*learning-gap*, both sceptics).

### 4.2 The §7.3 targets, against the extraction path (first run)

§7.3 amended by LQ12: the median over the extraction samples — a live run's extraction figures and the replays. **A live run's coverage by surface is not the extraction path's own coverage**: the coverage judge names one note per gold, so a gold both paths learned is credited to whichever note it named (34 of the reader's 58 matches in L also had an extraction note among the hits), and L's live extraction coverage of 12 of 83 against the replays' 60–66 is that attribution and the reader's pre-emption, not variance (runbook, *The learning evaluation*). Changed facts are not split by surface in the first-run reports: a pair's outcome reads the whole store.

| Measure (target) | L live, extraction | L-replay-1 | L-replay-2 | L-replay-3 | Median | Met? | L-episodes, extraction (2nd live) | Final median |
|---|---|---|---|---|---|---|---|---|
| Other-script aliases (≥ 60 %) | 83.3 % (30 / 36) | 70.0 % (77 / 110) | 71.0 % (76 / 107) | 67.8 % (78 / 115) | **70.5 %** | yes | 52.8 % (19 / 36) | ⟨FINAL: other-script alias median⟩ |
| Any alias | 100 % | 100 % | 100 % | 100 % | 100 % | — | 100 % | ⟨FINAL: any-alias median⟩ |
| A `fact_key` | 100 % (36 / 36) | 99.1 % | 99.1 % | 99.1 % | 99.1 % | — | 100 % | ⟨FINAL: fact-key median⟩ |
| Coverage (≥ 80 %) | 12 / 83 (attribution; not a sample) | 61 / 83 (73.5 %) | 60 / 83 (72.3 %) | 66 / 83 (79.5 %) | **73.5 %** over the replays (72.9 % with L's 12) | not as measured | 12 / 83 | ⟨FINAL: coverage median⟩ |
| — unparsed verdicts, all cut at the 400-token cap | 2 (pooled) | 8 | 6 | 5 | upper bound 83.1 % | — | 2 (pooled) | ⟨FINAL: unparsed count (cap 2,048)⟩ |
| Changed facts (≥ 12 of 15), re-scored by H6 | — (pooled only) | 8 (7–8) | 9 (8–9) | 7 | **8** | no | — | ⟨FINAL: changed-facts median⟩ |
| Restatements raising a suggestion or a duplicate (≤ 2 of 15) | 0 | 2 duplicates | 1 duplicate | 2 duplicates | **1.5** | yes | 1 duplicate (and 1 automatic replacement) | ⟨FINAL: restatement median⟩ |

- **Coverage** is below target on every extraction sample as measured. Each replay's unparsed verdicts were deliberations the 400-token cap cut before the verdict line (H5, rev 3.26), so an upper bound adds them: 69, 66 and 71 of 83, median 83.1 %. The sceptics note that several of those were heading to `NONE`, so it is only a bound. The fixed harness gives the coverage judge 2,048 tokens.
- **Changed facts** on the replays are right on 6–7 of the 8 pairs (after H6) and wrong on most corrections: 1, 3 and 1 of 7, where L and L-episodes are 7 of 7 through the reader's own notes. Of the replays' 16 failed correction slots, 9 are unparsed coverage verdicts (*learning-gap*, sceptic 1). "7–8" and "8–9" are H6's offline bounds (§4.4).
- **The capture replay** (`capture/report-replay-capture.json`, at `6eb0fae8`: the fixed harness, before LQ13) is a fourth extraction sample on the same transcript, made to record unusable answers: other-script aliases 64.0 % (71 / 111), a key on 99.1 %, coverage 65 / 83 (78.3 %) with 1 unparsed verdict under the 2,048-token cap, changed facts 7, restatements 1 duplicate and 1 automatic replacement. It is not one of §7.3's samples.

### 4.3 The whole store (pooled), first run

| Measure | L | L-episodes | L0 | L-replay-1 / 2 / 3 | Final L |
|---|---|---|---|---|---|
| Coverage | 70 / 83 (84.3 %) | 69 / 83 (83.1 %) | 65 / 83 (78.3 %) | 61 / 60 / 66 | ⟨FINAL: L pooled coverage⟩ |
| — matched to an extraction / model-tool note | 12 / 58 | 12 / 57 | 0 / 65 | all extraction | ⟨FINAL: L coverage by surface⟩ |
| — `none` / unparsed / `not_found` | 11 / 2 / 0 | 11 / 2 / 1 | 15 / 2 / 1 | 12, 15, 12 / 8, 6, 5 / 2, 2, 0 | ⟨FINAL: L coverage verdict counts⟩ |
| Other-script aliases | 38.8 % (45 / 116) | 31.7 % (39 / 123) | 16.7 % (16 / 96) | 70.0 / 71.0 / 67.8 % | ⟨FINAL: L pooled other-script aliases⟩ |
| — the reader's notes alone | 18.8 % (15 / 80) | 23.0 % (20 / 87) | 16.7 % | — | ⟨FINAL: L model-tool other-script aliases⟩ |
| Any alias | 72.4 % | 69.1 % | 45.8 % | 100 % | ⟨FINAL: L pooled any alias⟩ |
| A `fact_key` | 55.2 % (64 / 116) | 55.3 % (68 / 123) | 46.9 % (45 / 96) | 99.1 % each | ⟨FINAL: L pooled fact keys⟩ |
| — the reader's notes alone | 35.0 % (28 / 80) | 36.8 % (32 / 87) | 46.9 % | — | ⟨FINAL: L model-tool fact keys⟩ |
| Key reused within a change pair (after H6) | 1 of 5 | 2 of 6 | 0 of 2 | 7 of 7, 6 of 6, 6 of 7 | ⟨FINAL: L pairs key reused⟩ |
| Changed facts (after H6; offline bound) | 11 (10–11) | 13 (12–13) | 7 | 8 (7–8), 9 (8–9), 7 | ⟨FINAL: L changed facts⟩ |
| Noise: active learned notes no coverage call chose, plus candidates | 59 of 116, + 5 | 65 of 123, + 4 | 37 of 96, + 0 | 58 of 110 + 7, 55 of 107 + 7, 56 of 115 + 6 | ⟨FINAL: L noise⟩ |
| Restatements (15 turns; after H7) | nothing written 11, the reader wrote 3, first learned 1 | nothing written 3, the reader wrote 5, extraction failed 5, automatic replacement 1, duplicate 1 | nothing written 10, the reader wrote 5 | see §4.2 | ⟨FINAL: L restatements by outcome⟩ |

The reader's notes carry few other-script aliases and few keys; 90 of its 92 writes in L cite no source (*learning-gap*, sceptic 2). The pooled alias share is therefore mostly the reader's, which is what LQ12 separated.

### 4.4 Changed facts, per pair (first run)

Outcomes as recorded; "→ H6" marks a pair the re-score moves to *new active, old superseded* when the coverage judge, asked again without the new note, names the superseded one — offline that check cannot be replayed, so it is the bound's upper end (`fix/impl.md`, *Offline re-score*, repair round 2).

| Fact | L | L-episodes | L0 | L-replay-1 | L-replay-2 | L-replay-3 |
|---|---|---|---|---|---|---|
| `user.commute` (supersedes) | old never learned | old never learned | old never learned | old never learned → H6 | old never learned → H6 | both active, unlinked |
| `user.editor` (supersedes) | old never learned | **superseded** | new missing | **superseded** | old never learned | **superseded** |
| `user.laptop` (supersedes) | new not active | old never learned → H6 | both active, unlinked | old never learned | old never learned | old never learned |
| `moss.output_dir` (supersedes) | old never learned → H6 | **superseded** | old never learned | **superseded** | **superseded** | **superseded** |
| `user.weekly_review` (same key) | new missing | new missing | both active, unlinked | **superseded** | **superseded** | **superseded** |
| `lantern.service.port` (same key) | **superseded** | **superseded** | new missing | **superseded** | **superseded** | **superseded** |
| `lantern.staging.db` (same key) | **superseded** | **superseded** | new missing | **superseded** | **superseded** | **superseded** |
| `moss.python.version` (same key) | **superseded** | **superseded** | new missing | **superseded** | **superseded** | **superseded** |
| 7 corrections, new value active | 7 | 7 | 7 | 1 | 3 | 1 |
| **Correct of 15** (recorded → re-scored) | 10 → 11 | 12 → 13 | 7 | 7 → 8 | 8 → 9 | 7 |

What the diagnosis found behind L's misses (*learning-gap*, *ls-suggestions*, their sceptics): the weekly-review notes chained under a key the reader coined (`routine.friday.weekly-review`), and cs-031's flush recorded one item dropped as a duplicate (`dropped_duplicate_key`), after which no note states the new Monday 9:30 time; `user.laptop`'s new note became a same-key loser after the extractor reused the reader's broad key `user.machine` for another fact (cs-047); `user.commute` and `moss.output_dir` are each one note matched for both golds, and H6 credits the output directory, whose note supersedes the old one, not the commute (rev 3.26); `user.editor` is partly a scoring artefact — the judge matched one superseded note for both golds, while the old value's note stays active as a same-key loser beside the reader's Helix note, which superseded nothing. LQ14 (a key only the chat model coined is not offered to the extractor) answers the first two.

Final: ⟨FINAL: the per-pair table for L, L-episodes, L0 and the three replays⟩.

### 4.5 L-S: suggestions for notes the user saved

The 8 old notes of the change pairs are seeded as the user's; four of them — editor, laptop, commute and Moss's output directory — as the user would have saved them then, present tense with no end date (`conditions.learning.seeded_as_saved_then`, LQ7). 13 sessions and 78 turns are fed. Expected (§7.2): 8 suggestions naming the right notes, 0 user notes retired.

| | First run | Final |
|---|---|---|
| Seeded notes with a suggestion | **4 of 8** (`user.weekly_review` 2, `moss.python.version`, `lantern.staging.db`, `lantern.service.port`) | ⟨FINAL: L-S targets with a suggestion / 8⟩ |
| Suggestions in all (naming another note) | 6 (1) | ⟨FINAL: L-S suggestions in all (elsewhere)⟩ |
| Confirmed and the old note superseded | 4 of 4 | ⟨FINAL: L-S confirmed and superseded⟩ |
| User notes retired without a confirm | 0 | ⟨FINAL: L-S user notes retired⟩ |
| Restate turns (6): suggestion / nothing written / the reader wrote | 1 / 2 / 3 (after H7) | ⟨FINAL: L-S restatements by outcome⟩ |

The four seeded notes with no suggestion are exactly LQ7's four present-tense seeds, and exactly the four change turns on which the reader's own `memory_store` wrote a note (cs-003 #0, cs-034 #0, cs-035 #0, cs-041 #0–1). The diagnosis's mechanism: the reader tried to supersede the saved note, G2 refused it (09 §3.1 row 5 as it then read) — proven for cs-041 and inferred for cs-003 and cs-035 — and the reader stored the new value as a second active note, which the extractor then saw as already known, so no suggestion was raised. The commute miss is different: its one change session, cs-034, stopped on an unusable extraction answer. So at most 3 of the 4 fit the pre-emption (*ls-suggestions* and *completeness*, their sceptics). LQ11 makes the reader's replacement of a saved note a suggestion itself; whether that fills the four is the final run's question, since the seeding and the pre-emption fall on the same four notes and the first run cannot separate them.

### 4.6 Invariants (hard)

| Run | User notes retired without a confirm | Forgotten notes resurrected | Forced re-run checked | Exit |
|---|---|---|---|---|
| L | 0 | 0 | yes (its item was dropped as suppressed) | 0 |
| L-episodes | 0 | 0 | **no**: its one answer unusable (362 output tokens) | 101 |
| L-replay-1 | 0 | 0 | **no**: unusable, cut at the 2,800-token ceiling | 101 |
| L-replay-2 | 0 | 0 | **no**: unusable (1,263 tokens) | 101 |
| L-replay-3 | 0 | 0 | **no**: unusable (1,117 tokens) | 101 |
| L-S | 0 | — (no re-run) | — | 0 |
| capture replay (`6eb0fae8`) | 0 | 0 | yes | 0 |
| Final | ⟨FINAL: retired, per learning run⟩ | ⟨FINAL: resurrected, per learning run⟩ | ⟨FINAL: checked, per learning run⟩ | ⟨FINAL: exit codes from `final/progress.txt`⟩ |

A re-run that checked nothing is counted a violation, never a pass (09 §7.2), so four runs failed after writing their reports (`l8/progress.txt`: `exit=101`). No violation of either invariant itself was observed. The capture replay, with the harness retrying as production does, checked it: 0 resurrected. §6.1 (a) has the cause.

### 4.7 The cost of learning (first run)

| Run | Extraction calls | Tokens in / out | Flush latency p50 / p95 | Flushes (trigger) | Stopped on an unusable answer | Summaries: calls / written | Feed |
|---|---|---|---|---|---|---|---|
| L | 55 | 235,763 / 29,199 | 15.2 s / 50.0 s | 56 (51 `switch`, 5 `turns`) | 11 (8 `switch`, 3 `turns`) | 20 / 20 | 154 min, 300 reader turns |
| L-episodes | 55 | 239,662 / 31,711 | 15.6 s / 67.4 s | 56 (51, 5) | 16 (13, 3) | 20 / 19 | 166 min |
| L-replay-1 | 52 | 209,631 / 29,976 | 17.8 s / 41.4 s | 51 `replay` | 4 | — | 16 min |
| L-replay-2 | 52 | 207,601 / 33,176 | 19.4 s / 45.9 s | 51 | 7 | — | 19 min |
| L-replay-3 | 52 | 210,230 / 29,771 | 18.0 s / 34.4 s | 51 | 5 | — | 16 min |
| L-S | 16 | 56,439 / 7,560 | 16.6 s / 40.0 s | 15 (13, 2) | 2 | — | 42 min, 78 turns |
| Final L | ⟨FINAL: L extraction calls⟩ | ⟨FINAL: L tokens in / out⟩ | ⟨FINAL: L flush p50 / p95⟩ | ⟨FINAL: L flushes by trigger⟩ | ⟨FINAL: L unusable answers, windows skipped, left unread⟩ | ⟨FINAL: L summaries calls / written⟩ | ⟨FINAL: L feed wall⟩ |

Across the six learning runs, 45 flushes stopped on an unusable answer over 282 extraction calls, and 4 of the 5 forced re-runs did; 2 of L's 11 stops and 4 of L-episodes' 16 hit the 2,800-token ceiling, the rest were refused by the parser well under it (*completeness*, sceptic 1). A stop left a session unlearned when no later flush read it: 8 of 51 in L, 13 in L-episodes.

## 5. The episode split

L-episodes asked the 20 questions of `questions-episodes.json` after a live learning run; automatic recall never delivers a summary, so the reader finds one only through `memory_search` or `memory_timeline`.

| | First run | Final |
|---|---|---|
| L-episodes (live) | **8 / 20 (40.0 %)** — en 6 / 13, zh 2 / 5, mixed 0 / 2 | ⟨FINAL: L-episodes correct / 20⟩ |
| L-replay-episodes (a replay of the final L, extraction-only store) | — | ⟨FINAL: L-replay-episodes correct / 20⟩ |
| Summaries written / capped / below 4 turns / unusable | 19 / 27 / 4 / 1 | ⟨FINAL: summaries written / capped / below the minimum / unusable⟩ |
| Reader p50 / p95; input tokens per turn, mean | 28.0 s / 82.3 s; 28,871 | ⟨FINAL: episode split latency and tokens⟩ |

**Why 8 of 20** (`diagnosis/episodes.md`, both sceptics upheld it):
- **A harness defect, not the feature.** The harness lifted the extraction's daily caps but left `episode_max_per_day` at its default of 20, and the whole corpus is fed on one UTC day. After the 20th summary call every session from cs-021 on stopped `Cap` (27 of them); 4 three-turn sessions are below the 4-turn minimum. 11 of the 20 questions ask about a session at or after cs-021. L itself had the same shape (20 written, 27 capped).
- **With a summary, 6 of 8 were right; without one, 2 of 12** — both from notes the reader had stored during the feed. Retrieval missed nothing: whenever the reader searched and a summary existed, the search or the timeline listed it and the reader read it.
- **The 12 misses:** 9 had no summary because of the cap; 1 (eq-006) because its one summary call gave an unusable answer and the harness then retried nothing; 1 (eq-014) had a summary that kept the facts already stored as notes and dropped the outcome the question asks about (L's summary of the same session drops it too — a salience question for the episode prompt); 1 (eq-017) has a gold answer that describes a conversation the live reader did not have.

The fix is H1 (`bbd94e7d`): a learning system lifts `episode_max_per_day` to 1,000 and warns when any cap binds; H9 (`1b199807`) lets a replay write summaries and ask the split. The prompt's salience, eq-017's gold and trimming the long golds (the corpus review's N9) are the owner's; none is implemented (09 rev 3.26).

## 6. What the first run found, and what changed

### 6.1 The five diagnosed findings

Each finding in `l8/diagnosis/SUMMARY.json` was checked by two sceptics working from the reports, the logs, the kept databases (read-only) and the code at `50cf878d`; none was refuted, and each correction is folded in below. Commits are those 09's revision log names.

**(a) The forced re-run's unusable answer** (*rerun-badoutput*).
- *Found.* About one extraction answer in five was unusable to a parser that took only whole or fenced JSON. An answer cut at the output ceiling was reported as "not JSON". The items budget assumed 180 output tokens an item. The harness's forced re-run made one attempt where production allows three (LR20). The sceptics sharpened it: failure is decided by the prompt, not by chance — identical prompts re-sent failed 6 of 6 times, and windows grown by one turn succeeded 3 of 3; only 7 of 46 unusable answers hit the ceiling.
- *Changed — the harness and diagnostics slice* (09 revs 3.26–3.29, `8fa9354d..c3979ba5`): **P1** (`8fa9354d`) a cut answer says it was cut; **P2** (`27aa0682`) each unusable answer is kept in memory for the report, never persisted; **P3** (`39974e27`, re-derived in `c4b21f6a`) an item is budgeted at 216 output tokens under a 2,892-token ceiling, from the line fitted to L8's 230 successful single-call flushes; **H2** (`b6aae718`, `98ba8d75`, `bfcca399`) the harness retries as production does — `switch`, `replay`, summary and forced re-run flushes at once, a `turns` window at the session's next flush — and a re-run whose window was skipped is unchecked, not passed.
- *Then the capture.* One replay of L's transcript at `6eb0fae8` recorded the unusable answers: 11 of 60 extraction calls, 9 of them "top level is not `{"items": [...]}"` and 2 "not JSON"; 3 windows skipped; the forced re-run checked (`capture/full-replay-capture.log`). 9 of the 11 were a bare array of valid items, 1 was followed by `</think>`, 1 came after prose (plan §0, LQ13).
- *Decided — LQ13:* tolerant unwrap. `<think>` blocks and prose around the answer are stripped, the one JSON value taken, a bare items array read as the items; every item still validated, a cut answer never rescued (`6120812b`; repairs `f4a1e5e6`, `04df5bce`).
- *Final:* ⟨FINAL: unusable answers / extraction calls over the final learning runs, by reason; windows skipped; forced re-runs checked⟩.

**(b) L-S's missing suggestions** (*ls-suggestions*): §4.5.
- *Decided — LQ11:* a chat model's replacement of a note the user saved is a suggestion the user confirms, through learning's path; a store under such a note's `fact_key` and an edit of its content are the same replacement; archive of a `user` note stays refused, forget is unchanged, and G2 holds (`9f1bb2f3`, `abdd9645`, `79f00f49`; repairs `edb62bbb`, `dffe7413`, `1a6a69b8`; the close-out §6.3).
- *Changed in the harness:* **H7** (`751ee824`) attributes a restatement no note cites to the reader (`model_tool_write`) or to a failed extraction.

**(c) The episode split** (*episodes*): §5. *Changed:* H1, H9.

**(d) L's gap to D** (*learning-gap*).
- *Found.* Four causes. (1) *Metric scope:* every learning metric counted the reader's own notes as learned, so a live run and the extraction-only replays measured two systems. (2) *The harness:* no retry of an unusable answer (a product defect by the sceptics' reading, finding (a)); the coverage judge's 400-token cap truncated 2–8 verdicts a run; a pair whose one note records the change was scored `old_never_learned`; restatements could not see the reader's writes. (3) *Keys:* `memory_store` accepts coarse or value-bearing keys, the extractor was told to reuse known keys, and together they replaced or hid unrelated facts. (4) *Retrieval vocabulary:* learned titles and aliases name the value, not the attribute a question asks about, and search is lexical — 16 of the 80 D-only rows.
- *Decided — LQ12* (metrics by surface; the learning targets read against extraction, QA against the whole store): **H3** (`bf094ca8`), **H4** (`c0349703`). *LQ14* (a key only the chat model coined is not offered to the extractor): `0fe524d8`, repair `2d333522`.
- *Changed in the harness:* **H5** (`01eb45bb`) the coverage judge's own 2,048-token cap; **H6** (`5bf0eda9`, judged since `cc424f1d`) a pair whose one note supersedes the old one is credited only when the judge, asked again without it, names the old note.
- *Not changed:* retrieval vocabulary. Search stays lexical by design (MD-1); aliases are the remedy, and the extractor's notes carry them (§4.2).

**(e) The completeness sweep** (*completeness*) confirmed the above and added an uncounted outage of L0's feed: 7 turns ended in an error (cs-026 #8–10, cs-027 #0–3). *Changed:* **H8** (`679f03fd`) counts and warns about fed turns that end in an error.

The offline re-score of the first runs under H3–H7 is `fix/rescore.json`; its figures are the "after H6 / H7" ones in §4. A new report compares with a first-run one only through them, never as an improvement (runbook).

### 6.2 The benches, and the scale decisions

The L8 benches (§7.1) fired §6.4's rule, and the profiled scale analysis (`bench/scale/analysis.md`, with its sceptic review) found the 100K costs almost all per-operation work over the whole partition. From it:
- **The scale slice** (09 revs 3.30–3.32, `d0d487ba..c7574587`, repairs `b1b03e0a..c4557587` and `e3df69b2..fa4bca8e`), no contract change: S1 incremental re-indexing after a restart; S2 verification without cloning the snapshot; S3 a scan-time tombstone check; S4 a cheaper index stamp and `Parsed::put`; S5 idle index maintenance; S6 a background rebuild with a file scan meanwhile; S7 the 1.5 s reply deadline on memory reads; S8 tombstones keyed by source; S9 the bench's tombstone case.
- **LQ15:** the service's design point is about 30K notes; 100K is a later target.
- **LQ16:** ruling R3's stat scan before every read stays, made cheaper: one `getattrlistbulk(2)` pass on macOS — 221–233 → 99–108 ms per 100,000-file scan, release (`0177a41f`, repair `5f36b1dd`).
- **LQ17:** the index layout revision, 09 §6.5 — trigram only over titles, aliases, tags and the bodies of scripts `unicode61` cannot split into words; word beginnings through the word table (`MatchReason` `prefix`); three contentless FTS5 tables; one copy of the text. Designed on a prototype (`181c1d86`, `af7a1414`), built as index schema 4 (`457c1122`), repaired as schema 5 (`2684af55`) and on the query side (`7076f653`), pinned (`13b7a3f9`, `c3fb1cf8`). **The lead's ruling of 2026-09-27:** the one row of the retrieval proxy's eight totals the layout loses (tq-0032: index tuning, listed or delivered, 95 → 94) is accepted, because the held-out gate set shows no net loss and every variant that recovered the row cost the held-out set at least one (§7.3).

### 6.3 The decisions, with their evidence

| Id | Decision (owner, 2026-09-26) | Evidence | Applied by |
|---|---|---|---|
| LQ11 | A chat model's replacement of a saved note is a suggestion | *ls-suggestions*, its sceptic | `9f1bb2f3`, `abdd9645`, `79f00f49`; repairs `edb62bbb`, `dffe7413`, `1a6a69b8`; the close-out `f928d25e`, `17ba8538`, `46d5008c`, `9ab4898d`, and its repair rounds `d515613f`…`c3fb1cf8`, `91fe6bef`…`b2b877d2`, `3f3a2307`, `2b49639e` |
| LQ12 | Every learning metric by surface; learning targets against extraction; QA against the whole store | *learning-gap*, its sceptics, *completeness* | `bf094ca8` (H3), `c0349703` (H4) |
| LQ13 | Tolerant unwrap of an extraction answer | *rerun-badoutput*, its sceptics; `capture/` | `6120812b`; `f4a1e5e6`, `04df5bce` |
| LQ14 | Keys only the chat model coined are not offered to the extractor | *learning-gap* | `0fe524d8`; `2d333522` |
| LQ15 | The design point is about 30K notes; 100K later | `bench/scale/analysis.md` | — (a target) |
| LQ16 | Keep R3's stat scan; make it cheaper | `bench/scale/analysis.md`, `bench/scale/sceptic/scanbench.c` | `0177a41f`; `5f36b1dd` |
| LQ17 | The index layout revision | `bench/scale/analysis.md`, `bench/scale/sceptic/` | `181c1d86`, `af7a1414`, `457c1122`, `2684af55`, `7076f653`, `13b7a3f9`, `c3fb1cf8` |

The close-out (09 revs 3.41–3.44, `f928d25e..13b7a3f9` and its three repair rounds) closed the product slice's and the layout build's last review findings: a validity-only edit of a saved note is a suggestion too; a pending candidate's forget suppresses no row the note it names, or any saved note, cites; a forget takes the named suggestions first and its result does not depend on the targets' order; a suggestion lists what else it changes (`changes`). Its changes are to the forget and edit paths and to what a suggestion shows; no first-run figure measured them.

## 7. Scale

### 7.1 The L8 benches (`4aab3afe`, 2026-09-26)

Release, one run each, one at a time on a quiet machine, each process under `/usr/bin/time -l` (`l8/bench/run.sh`, `*.log`, `*.time`). The index benches search 200 queries at limit 8 with per-file verification; the service bench seeds by writing files, as §6.4 asks.

| Index bench | Cache MiB (× body bytes) | Build s | Search + verify p50 / p95 / max ms | Peak RSS MB |
|---|---|---|---|---|
| 10K, average mix | 109.2 (8.9×) | 4.0 | 5.60 / 11.45 / 16.76 | 244 |
| 100K, average mix | 1,065.5 (8.5×) | 46.1 | 73.41 / **150.34** / 223.75 | 692 |
| 100K, learned mix | 531.8 (9.4×) | 22.7 | 19.67 / 56.58 / 85.96 | 571 |

At 100K average, `dbstat`: `fts_tri_data` 533.9 MiB (50 % of the file), `chunk` 353.7, `fts_word_data` 88.8, `note` 35.6.

| Service bench, parse cache on | 1K | 10K | 100K average | 100K learned |
|---|---|---|---|---|
| recall p50 / p95 ms | 7.8 / 9.2 | 48.4 / 59.0 | 556.0 / **666.2** | 438.7 / 484.2 |
| store (learned) p95 ms | 53.1 | 104.6 | 3,515.7 | 735.7 |
| update / forget p95 ms | 54.8 / 40.1 | 136.5 / 233.1 | 6,578.0 / 8,449.9 | 1,090.3 / 2,108.4 |
| `learning_view` (one chunk) p95 ms | 9.8 | 135.7 | 9,127.8 | 9,268.1 |
| open with no cache (the rebuild) | 0.28 s | 2.9 s | 33.0 s | 21.7 s |
| first reconciliation tick | 0.32 s | 3.6 s | 46.2 s | 27.5 s |
| parse cache holds | 1.9 MiB | 18.2 MiB | 0.0 MiB | 121.0 MiB |
| peak RSS MB | 75 | 428 | 2,381 | 2,322 |

- **§6.4's decision rule fired.** Index search + verification at 100K was p95 150.3 ms, above 100 ms, and the cache 1,065.5 MiB. The profile puts 57 % of search in the trigram leg and 42 % of FTS time in `pread`, the file being four times the 256 MiB mmap window (09 §6.5, quoting the analysis).
- **The service's own proposed targets** (§6.4: recall p95 ≤ 100 ms and learned `store` p95 ≤ 100 ms at 100K) were missed by 6.7× and 35× on the average mix, and 04 §5's 50 ms at 10K by the service's recall (59.0 ms), though not by the index's (11.45 ms).
- **Why** (`bench/scale/analysis.md`): at 100K a recall spends 44–55 % in the stat scan before every read, 27–39 % in FTS search and 6 % cloning the snapshot per hit; the first tick after a start re-indexed the unchanged partition wholesale; `learning_view` cloned the snapshot per citing row; and the parse cache, all-or-nothing at 128 MiB, never holds an average-size 100K partition.

### 7.2 After the slices

The L8 logs predate 80 commits, so the layout's build was measured against a pristine copy of the commit before it (`af7a1414`, after the scale and product slices), built and run the same day on the same machine — both under a load of 5–8 from two macOS daemons (`layout/impl.md`). "L8 → before" therefore crosses days; "before → layout" does not.

| | L8 (`4aab3afe`) | Before the layout (`af7a1414`) | The layout (`457c1122`) | Final (`82180b84`) |
|---|---|---|---|---|
| Index 10K: cache MiB / p95 ms / RSS MB | 109.2 / 11.45 / 244 | 109.2 / 10.08 / 247 | **40.2 / 8.73 / 142** | ⟨FINAL: index 10K cache / p95 / RSS⟩ |
| Index 100K average: cache MiB / p95 ms / RSS MB | 1,065.5 / 150.34 / 692 | 1,065.5 / 154.04 / 691 | **366.6 / 48.56 / 685** | ⟨FINAL: index 100K cache / p95 / RSS⟩ |
| Index 100K average: rebuild s | 46.1 | 53.9 | **15.2** | ⟨FINAL: index 100K build⟩ |
| Index 100K learned: cache MiB / p95 ms | 531.8 / 56.58 | 531.7 / 56.41 | **219.7 / 27.39** | ⟨FINAL: index 100K learned cache / p95⟩ |
| Service 10K: recall p50 / p95 ms | 48.4 / 59.0 | 28.6 / 33.7 | **27.0 / 29.8** | ⟨FINAL: service 10K recall⟩ |
| Service 10K: first tick; rebuild | 3.6 s; 2.9 s | 40 ms; 3.1 s | 40 ms; **1.1 s** | ⟨FINAL: service 10K tick and rebuild⟩ |
| Service 100K: recall p50 / p95 ms | 556.0 / 666.2 | 342.3 / 397.6 | **242.4 / 264.5** | ⟨FINAL: service 100K recall⟩ |
| Service 100K: `learning_view` p95 ms | 9,127.8 | 487.7 | **353.7** | ⟨FINAL: service 100K learning_view⟩ |
| Service 100K: store (learned) p95 ms | 3,515.7 | 3,223.0 | 3,181.5 | ⟨FINAL: service 100K store⟩ |
| Service 100K: first tick; rebuild | 46.2 s; 33.0 s | 3.2 s; 40.9 s | 3.1 s; **16.7 s** | ⟨FINAL: service 100K tick and rebuild⟩ |
| Service 100K: peak RSS MB (footprint) | 2,381 (1,240) | 1,955 (998); again 1,666 (771) | **2,157 (1,193); again 2,145 (1,197)** | ⟨FINAL: service 100K peak RSS, two runs⟩ |

Figures from `l8/bench/`, `layout/bench-base/`, `layout/bench-after/` and `layout/impl.md`. Since the scale slice (S6) the daemon opens with no index at once and rebuilds it in the background (1.7 ms, then the rebuild), so the rebuild is off the request path. The layout's two repair rounds move the index benches within their runs' noise (09 §6.5.5).

- **§6.4's decision rule is met** after the layout: index search + verification p95 at 100K is 48.6 ms (47.2 ms on the prototype), 27.4 ms on the learned mix; at the design point, 30K, 16.6 ms against the old layout's 29.4 ms (prototype, 09 §6.5.5). That holds for the bench's questions of 1–11 words. **A whole message**, which is what the per-turn recall searches (up to 512 characters and 32 terms), takes search alone median / p95 86.6 / 103.8 ms at 30K and 273.5 / 320.6 ms at 100K on the build — half the old layout's, still about three times 100 ms at 100K (09 §6.5.5, the second review's measurement).
- **The service's proposed targets are still missed at 100K:** recall p95 264.5 ms against 100 ms; a learned store 3,181.5 ms on the average mix. At 10K both are under 100 ms: recall 29.8 ms, inside 04 §5's 50 ms too, and a learned store 86.0 ms.
- **One open finding: the 100K service bench's peak memory rose with the layout** — 1,955 and 1,666 MB before, 2,157 and 2,145 MB after, two runs each — while the index benches and the 10K service bench fell. Which phase holds it is not measured (09 rev 3.38).

### 7.3 What the layout costs retrieval

The retrieval proxy is system D with no model, no judge and outbound networking denied: what the per-turn recall alone delivers (`layout/baseline/`, `layout/after/proxy-compare.txt`).

| Run | Every relied-on note delivered | Listed or delivered |
|---|---|---|
| index, held-out (470) | 254 → **257** | 293 → 293 |
| excerpts, held-out (470) | 286 → **288** | 286 → 288 |
| index, tuning (150) | 82 → 82 | 95 → **94** |
| excerpts, tuning (150) | 93 → **94** | 93 → 94 |

Seven of the eight totals rise or hold; index tuning's listed-or-delivered loses tq-0032, which the lead accepted (§6.2). The build equals the prototype row for row on all 1,240 rows, and so does `13b7a3f9` (09 rev 3.41); over seven runs the index held-out total is 257 five times and 258 twice, from ties at the excerpt cut. What the layout no longer finds — a term inside a word of a body in a script `unicode61` splits into words — is 09 §6.5.4's.

### 7.4 Realistic growth, and what remains for 100K

- **Growth** (`bench/scale/analysis.md`). The L8 runs learned 123–145 notes plus 19–20 summaries per 300 turns, 0.41–0.48 notes a turn on a fact-dense corpus; nothing prunes. Assuming everyday chat yields 0.10–0.15 notes a turn (0.45 at most): a regular user (20 turns a day) holds 2–4K notes after a year, a heavy one (50) 4–10K, a very heavy one (150) 13–32K. 100K is a multi-year or import scenario, hence LQ15's 30K.
- **No service bench ran at 30K**; the design point's service latency is interpolated, not measured. ⟨FINAL: a 30K service bench on the final build, if the lead runs one⟩.
- **What remains for 100K:**
  - recall (264.5 ms against 100 ms): O5, mapping the whole cache file (estimated −20 to −25 % of search), and `prefix=`, both open levers (09 §6.5.5, §6.5.8);
  - the write path (3.2 s against 100 ms on the average mix): incremental partition state and an append-only control log (O15, O16), `pending` out of the snapshot (O13), a frontmatter-only parse cache (O14), management reads from the index (O6, O7), a debounced `MEMORY.md` (O9) — none built; each is the owner's (09 rev 3.30);
  - the cliff at the cap: a word held whole in more than 256 eligible bodies can lose every holder to longer forms; the ranked body query would close it at +34 ms of p95 at 100K, a measured trade-off left to the lead (09 §6.5.4);
  - the header relevance's length bias, which at 100K of the learned mix drops three identity questions (hq-0001, -0005, -0039) outside the 50 hits that the old layout kept within 20 (09 §6.5.5);
  - the peak-memory rise (§7.2);
  - a 100K learned-mix service bench, not re-run since L8.

## 8. Caveats and threats to validity

- **The reader grades itself.** One model answers, extracts and judges both QA and coverage (LQ1, `judge_is_reader: true`, `judge_is_extractor: true`). A judge that shares the reader's blind spots passes answers an independent judge would fail, and the coverage judge rates notes its own extractor wrote. The re-judge's κ 0.981 measures agreement between two builds of the same family (§3.3), not independence. The gate's comparison is at least like for like: B-mlx and D-index share the reader and the judge.
- **One sample of the live system.** The reader's temperature is not pinned; each live run is one draw. QA was measured once per system. L and L-episodes are the only two live samples of the learning side, and they differ: pooled coverage 70 against 69, changed facts 11 against 13 (re-scored), the extraction path's other-script aliases 83.3 against 52.8 %. One fact can move ten questions (the weekly-review change, §3.2).
- **Temperature 0 is not variance 0.** The extractor runs at temperature 0, and the same prompt mostly fails or succeeds the same way again (6 of 6 re-sent failures failed; the runbook's 13 of 13 same-prompt pairs). Yet the three replays of one transcript diverge — coverage 60–66 of 83, 4–7 stopped flushes — and the completeness review lists non-determinism at temperature 0 among its smaller issues; once one answer differs, every later known-notes section differs.
- **Run-to-run noise on the QA side.** A change of model build alone flipped 49 of 470 rows while moving B's total by 0.6 pp (§3.3). D-index and D-excerpts, one store and two recall modes, differ on 21 rows. The retrieval proxy repeats exactly but for tied rows (§7.3). A difference of about a point between two single runs is within that noise; the gate's CI (+4.0 pp at its lower bound) is not.
- **A synthetic corpus.** A fictional user, written by a design session from the gold notes, English and Chinese, dense in facts (0.41–0.48 learned notes a turn) with 54 % noise turns. A learning run cannot backdate, so dates are stated in the text; wall-clock dating still costs L on time questions (a strict date match in the completeness review finds 58 L answers carrying the run's own dates, 9 of them wrong). D's gold notes carry curated aliases with the questions' vocabulary; learned notes do not, which costs L 16 of its 80 D-only rows (§6.1 d). The episode split's eq-017 describes a conversation the live reader did not have (§5).
- **The harness is not the daemon.** The queue runs in `Manual` mode with the triggers replayed; the idle trigger is not exercised; the retry pace differs from production's backoff (runbook). Fed turns are unattended, so the destructive `memory_update` is refused and `memory_confirm` never offered: L's kept database holds 24 `tool_approval_unavailable` rows for `memory_update`, L-episodes' 29 (*completeness*, sceptic 2) — how much that costs L0's source conflicts (7 of 40) is not measured. A replay flushes whole sessions and carries none of the reader's notes.
- **First-run reports and the fixed harness.** H3–H7 reclassify; H5 changes the coverage cap. A final figure compares with a first-run one only through the re-scored figures (§4), never as an improvement. Changed facts carry an offline bound (§4.4).
- **Two benches' conditions.** The L8 benches ran on a quiet machine, the layout's before-and-after pair under a load of 5–8 from two macOS daemons; single runs each.

## 9. Owner items still open

- **LQ5–LQ10** (09 §10), pending, none adopted: LQ5 learning drops a chunk whose project directory is gone; LQ6 a denied or timed-out card hides every later suggestion for the same note on the lane until restart; LQ7 L-S seeds four gold notes present-tense (the first run cannot separate it from the pre-emption, §4.5); LQ8 full UUIDs on the index lines; LQ9 which summaries automatic recall leaves out, and a summary's forget suppressing nothing; LQ10 a model's edit of a summary (its origin and its `fact_key`).
- **The 100K work** (§7.4): the service's recall and store targets, the options not built (O5, `prefix=`, O6, O7, O9, O13–O16), the cliff at the cap (the lead's trade-off) and the header's length bias.
- **The 100K service bench's peak-memory rise** (§7.2): unexplained.
- **Raised by the diagnosis, not an LQ, and not implemented** (09 rev 3.26): whether the episode prompt records outcomes and skips facts already stored (eq-014); eq-017's gold; trimming the long episode golds (N9).
- **For the lead:** whether prose around an extraction answer yields only its last value (09 rev 3.34, LQ13's repair round 1).
- **Unchanged from before L8:** the owner's review of the shipped defaults; slice 3's GUI round-trip on an isolated store and slice 4's packaged, network-denied verification (08).

## 10. Reproduce

From the runbook (`scripts/eval/memory/README.md`) and the chains that made these runs (`l8/run-chain.sh`, `l8/run-B-mlx.sh`, `l8/run-rejudge.sh`, `final/run.sh`, `l8/bench/run.sh`). `$M` is a scratch directory; never `~/.openalpaca`.

**The model configuration**, `$M/config-mlx/llm.toml`: the runbook's file for the test model (*Prerequisites*), plus

```toml
[providers.ollama]
request_timeout_secs = 900

[timeouts]
llm_request_timeout_secs = 900

[rate_limits]          # the keyless Ollama slot, off the 50 RPM / 40,000 TPM defaults
default_rpm = 10000
default_tpm = 100000000
```

**The QA and learning runs**, from a worktree at the build (`82180b84` for the final runs, `50cf878d` for the first):

```sh
git worktree add $M/wt $BUILD && cd $M/wt
export OPENALPACA_LIVE_EVAL=1 OPENALPACA_CONFIG_DIR=$M/config-mlx CARGO_TERM_COLOR=never
CMD=(cargo test -p openalpaca_core --features live-eval --test memory_eval -- --ignored memory_eval --nocapture)
run() { local name=$1; shift; rm -rf $M/store-$name; mkdir -p $M/store-$name
  env OPENALPACA_HOME_STORE=$M/store-$name OPENALPACA_MEMORY_EVAL_REPORT=$M/report-$name.json "$@" \
    $CMD > $M/full-$name.log 2>&1; }
run smoke-L    OPENALPACA_MEMORY_EVAL_SYSTEM=L OPENALPACA_MEMORY_EVAL_SESSIONS=2 OPENALPACA_MEMORY_EVAL_LIMIT=5
run D-index    OPENALPACA_MEMORY_EVAL_SYSTEM=D OPENALPACA_MEMORY_EVAL_RECALL_MODE=index
run D-excerpts OPENALPACA_MEMORY_EVAL_SYSTEM=D OPENALPACA_MEMORY_EVAL_RECALL_MODE=excerpts
run L          OPENALPACA_MEMORY_EVAL_SYSTEM=L
# copy L's kept scratch (conditions.learning.scratch_kept) out of the system temp directory first
SCR=$M/L-scratch
for i in 1 2 3; do run L-replay-$i OPENALPACA_MEMORY_EVAL_SYSTEM=L OPENALPACA_MEMORY_EVAL_LEARN_FROM=$SCR; done
run L-replay-episodes OPENALPACA_MEMORY_EVAL_SYSTEM=L OPENALPACA_MEMORY_EVAL_LEARN_FROM=$SCR \
  OPENALPACA_MEMORY_EVAL_QUESTIONS=$M/wt/scripts/eval/memory/questions-episodes.json
run L-S        OPENALPACA_MEMORY_EVAL_SYSTEM=L-S
run L0         OPENALPACA_MEMORY_EVAL_SYSTEM=L0
run L-episodes OPENALPACA_MEMORY_EVAL_SYSTEM=L \
  OPENALPACA_MEMORY_EVAL_QUESTIONS=$M/wt/scripts/eval/memory/questions-episodes.json
```

**B-mlx**, the old engine, from a worktree at `d08317fb`, with the exports above and the old E5 feature (it downloads the E5 model once into the scratch store):

```sh
OPENALPACA_HOME_STORE=$M/store-B-mlx OPENALPACA_MEMORY_EVAL_SYSTEM=B \
OPENALPACA_MEMORY_EVAL_REPORT=$M/report-B-mlx.json \
  cargo test -p openalpaca_core --features live-eval-e5 --test memory_eval -- --ignored memory_eval --nocapture
```

**The re-judge** of 08's archived D, no reader call and no store:

```sh
OPENALPACA_MEMORY_EVAL_REJUDGE=$PWD/scripts/eval/memory/reports/system-D-prefix-2026-09-24.json \
OPENALPACA_MEMORY_EVAL_REPORT=$M/report-D-rejudged-mlx.json \
  cargo test -p openalpaca_core --features live-eval --test memory_eval -- --ignored memory_eval --nocapture
```

**The comparisons** (candidate first):

```sh
python3 scripts/eval/memory/compare.py $M/report-D-index.json   $M/report-B-mlx.json      # the gate
python3 scripts/eval/memory/compare.py $M/report-D-index.json   $M/report-D-excerpts.json
python3 scripts/eval/memory/compare.py --margin=-10 $M/report-L.json $M/report-D-index.json
python3 scripts/eval/memory/compare.py $M/report-L.json         $M/report-L0.json
```

**The benches**, release, each binary run alone under `/usr/bin/time -l` so the compile is not counted:

```sh
cargo test --release -p openalpaca_storage --lib --no-run
cargo test --release -p openalpaca_core --lib --no-run
/usr/bin/time -l ./target/release/deps/openalpaca_storage-<hash> --ignored --exact \
  memory_index::bench::bench_search_100k_notes_with_verification --nocapture --test-threads=1
# likewise bench_search_10k_notes_with_verification, bench_search_100k_learned_notes_with_verification,
# and in openalpaca_core memory::service::bench::bench_service_{1k,10k,100k,100k_learned}_notes
```

**The retrieval proxy**: system D with `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL=none`, an `llm.toml` whose providers and CLI backends are all disabled (Ollama pointed at `127.0.0.1:1`), under `sandbox-exec -f scripts/eval/memory/no-network.sb`, in both recall modes, with the held-out and then the tuning questions (`OPENALPACA_MEMORY_EVAL_QUESTIONS`) — the runbook's *A dry run with no model*. Read `totals.rows_with_every_relied_note_delivered` and `…_listed_or_delivered`.

## Revision log

- **draft 1 (2026-09-27):** the first L8 runs at `50cf878d` (QA, learning, L-S, the episode split, invariants, cost), their diagnosis and the offline re-score, the decisions LQ11–LQ17 and the changes they drove, the benches before and after the scale slice and the layout, caveats, open items and the recipe; every final-build figure a `⟨FINAL: …⟩` placeholder.
