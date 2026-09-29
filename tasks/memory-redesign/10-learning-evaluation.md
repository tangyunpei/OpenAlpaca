# Evaluation report — Learning v2 (L8)

**Status: rev 2 (2026-09-29) — final.** Every final-build figure is in, the benches and the cross-over replays of §4.8 included; rev 2 records how each of §9's items closed, and changes no figure. Draft 1, 2026-09-27; drafts 2 and 3, 2026-09-28; rev 1 and rev 2, 2026-09-29. The report of design [09](09-learning-design.md)'s L8: the learning evaluation (§7), the scale benches and their decision rule (§6.4) and the recall-mode and episode additions (§13.4). Every number below is a measurement with its conditions and the file it was read from. The first runs, made at `50cf878d` before the owner decisions LQ11–LQ17, are reported whole; a final figure goes beside its first-run twin, never in its place. **Every QA and coverage verdict here is the reader grading itself** (§1, §8).

**Sources.** Except the archived reports named below, none of it is tracked; it lives in the session scratchpad's `memeval/`:
- the first runs: `l8/report-*.json`, `l8/full-*.log`, `l8/progress.txt`, the chain `l8/run-chain.sh`, `l8/run-B-mlx.sh`, `l8/run-rejudge.sh`;
- the diagnosis: `l8/diagnosis/SUMMARY.json` (five findings, each with two sceptics' corrections) and its notes (`rerun-badoutput.md`, `ls-suggestions.md`, `episodes.md`, `learning-gap.md`, `completeness.md` and their `*sceptic*` files);
- the capture run: `l8/capture/` (`report-replay-capture.json`, `full-replay-capture.log`, `unusable.json`);
- the offline re-score of the first runs under the fixed harness: `l8/fix/rescore.json`, `l8/fix/impl.md`;
- the benches: `l8/bench/*.log`, `*.time`, `machine.txt`, and the scale analysis `l8/bench/scale/analysis.md` with its sceptic review;
- the index layout's build: `l8/layout/impl.md`, its benches `l8/layout/bench-base/` (the commit before, same day) and `l8/layout/bench-after/`, and the retrieval proxy `l8/layout/baseline/` and `l8/layout/after/`;
- the re-measurement: `final/` (`run.sh`, `progress.txt`, `report-*.json`, `full-*.log`, the final L's kept transcript `L-scratch/`);
- the final figures, each area with a sceptic's re-derivation beside it: `final/figures/` (`qa.md`, `learn-targets.md`, `learn-store.md`, `ls-inv-cost-episodes.md`, their `verify-*.md`, and the `compare-*.txt` outputs of `compare.py`);
- the diagnosis of the final build's other-script alias share and its two sceptics: `final/alias-drop/` (`diagnosis.md`, `sceptic-1.md`, `sceptic-2.md`, and their scripts), and the cross-over replays that test it: `crossover/` (`run.sh`, `progress.txt`, `report-*.json`, `full-*.log`) with their analysis and its sceptic, `crossover/analysis/` (`analysis.md`, `sceptic.md`, `sceptic/`, and the scripts); the six cross-over reports are archived, redacted, under `scripts/eval/memory/reports/l8-crossover/`;
- the final benches and the test gates before them: `final-gates/` (`run.sh`, `progress.txt`, the gates' logs, `bench/*.log`, `*.time`).

The decisions are 09 §10 (LQ11–LQ17, with the lead's tq-0032 ruling in §6.5.5) and its revision log (revs 3.26–3.46, rev 3.46 holding the lead's ruling on which samples each learning median takes), `tasks/api-fix-plan.md` §0 and CLAUDE.md's *L8 and its follow-up* paragraph. The reports carry the machine's hostname and scratch paths; the redacted reports are archived, as 08 archived A, B and D, under `scripts/eval/memory/reports/l8-first/<run>.json` (B-mlx, D-rejudged-mlx, D-index, D-excerpts, L, L-S, L-episodes, L-replay-1…3, L0) and `scripts/eval/memory/reports/l8-final/<run>.json` (D-index, D-excerpts, L, L-replay-1…3, L-replay-episodes, L-S, L0, L-episodes), with the final `compare.py` outputs beside them as `l8-final/compare-*.txt`, hostname and paths redacted as 08's were. The `compare-*.txt` files were produced from the archived copies themselves, from the repository root, and match the scratchpad's outputs figure for figure.

## 1. Summary

- **What was measured.** QA accuracy on 08's 470 held-out questions for: D′ in both recall modes (D-index, the default, and D-excerpts); the old engine's B re-run on the same model (B-mlx); L, whose store is learned from a 51-session, 300-turn conversation corpus; and L0, the same feed with learning off, so only the reader's own `memory_store` calls learn. The learning measures of §7.3 on L, its three replays, a second live learning run (L-episodes), L0 and L-S. A 20-question episode split. The §6.4 benches and the retrieval proxy of §6.5.5. The QA and learning runs were made twice: first at `50cf878d`, then on the final build `82180b84` under the fixed harness, where a replay of L that writes conversation summaries and asks the episode split (L-replay-episodes) joins them; B-mlx was not re-run.
- **On what.** One local model, `qwen3.8:27b-mlx` on Ollama, is the reader, the extractor, the QA judge and the coverage judge (owner answer LQ1). Every report of this harness (D, L and their final runs) records `judge_is_reader: true`; B-mlx's older harness does not record the field, and D-rejudged, whose answers came from another model, correctly records `false`. The model graded its own answers and its own extraction. The reader's temperature is not pinned, so each live run is one sample of the whole system. One machine: the owner's MacBook Pro (Apple M5 Max).
- **The gate holds** (first run). D-index against B-mlx: **+6.8 pp, 95 % CI [+4.0, +9.6]**. The CI's lower bound is above −5 pp, so the gate passes; it also excludes zero, so the claim *more accurate* is allowed — on this corpus, with this model as reader and judge, on this machine, and no further. On the final build the gate still passes: D-index against the same B-mlx report is **+6.6 pp, CI [+3.7, +9.5]** over 469 paired rows (one final verdict was unparsed, §3.4), so *more accurate* is still allowed within the same limits, though D is no longer ahead in every category (abstention −2.2 pp).
- **The two recall modes are comparable.** D-index against D-excerpts: +0.6 pp, CI [−1.3, +2.5] on the first run and +0.9 pp, CI [−0.7, +2.4] on the final build. Index mode reads a smaller median prompt (6,606 against 7,566 input tokens per turn; final 6,842 against 7,800) for a few more tool rounds (1.66 against 1.56 per turn; final 1.68 against 1.54).
- **Learning fell short of D on the first run.** L answered 81.3 %, against D-index's 96.8 %: **−15.5 pp, CI [−19.2, −11.9]**. §7.3's QA target, L within −10 pp of D′, is not met. L against L0 is +12.3 pp, CI [+7.6, +17.0], so extraction carries much of L's score. On the final build L answered 88.1 % (414 / 470) against D-index's 96.4 % (453 / 470); paired over the 469 rows both judged, the difference is **−8.3 pp, CI [−11.3, −5.3]**. The point estimate is inside −10 pp, but the CI's lower bound is not, so the target is still not met, and two categories fall below the block line (cross-language −12.9, rejected / failed actions −13.3 pp). L against L0 is +17.4 pp, CI [+13.2, +21.7]. L's own move across builds (+6.8 pp) mixes the build with a fresh learned store and is no measure of the build alone (§8).
- **Learning targets, read against the extraction path** (LQ12, with the lead's rev 3.46 ruling on which samples each median takes, §4.2). First run: other-script aliases **70.0 %** (target ≥ 60 %, met); restatements raising a suggestion or a duplicate **2 of 15** (≤ 2, met); coverage **73.5 %** (≥ 80 %, not met as measured — the 400-token judge cap cut 5–8 verdicts per replay, which could carry it to 83.1 %); changed facts **8 of 15** (≥ 12, not met; 9 of the replays' 16 failed correction slots are those cut verdicts). Final build, under the fixed harness: other-script aliases **56.1 %** (spread 48.0–62.5 %, not met); coverage **84.3 %** (81.9–86.7 %, met, with the coverage judge's cap at 2,048 tokens); changed facts **10 of 15** (9–10, not met, now judged at run time rather than bounded offline); restatements **0 of 15** (met). The harness's coverage cap and changed-facts check differ between the two, and the final replays replay a different transcript, so neither pair of medians measures the build.
- **The other-script alias share fell below target on the final build.** A diagnosis checked by two sceptics finds that, stratified by session, a flush gives an English-titled note a Chinese alias as often as before (the first sceptic rates this *likely*; counted per note, it holds only stratified by session with cs-015 left out). The drop is in which sessions yield notes, how many notes they yield, and one seven-note flush flipping. Cross-over replays then settled what they could: on the final build, the first L's transcript gives 62.9, 65.0 and 75.6 % and the final L's 50.4, 56.6 and 56.1 %, a gap carried almost wholly by three sessions that follow the transcript in all 13 runs over three builds; the build's extraction path is *likely* not the cause, and why those sessions flip is unexplained. The transcript is itself one live run's output, so whether the final build tends to write such transcripts is untested, and pass or fail at 60 % is not a property of the build these data can settle — the two live samples straddle it, 62.5 and 48.0 % (§4.8).
- **Invariants.** No run retired a user note without a confirm, and no run resurrected a forgotten note. But the forced re-run could check nothing in four of the five learning runs: its one extraction answer was unusable, and the harness counts an unchecked re-run as a violation (§4.6). On the final build the forced re-run checked in all six runs that make one, under the fixed harness's stricter meaning of *checked* (no window skipped as well as a call and no stop); no run retired a user note or resurrected a forgotten one, and every run exited 0. In both builds the retired-note invariant tests something only in L-S: no other run holds a note of origin `user`, so its 0 there holds because nothing could be retired (§4.6).
- **L-S raised 4 of the 8 suggestions** expected; all four confirmed retire the old note. On the final build it raised a suggestion for **7 of the 8**, five of them through the reader's own `memory_store` under LQ11 and two through extraction, and all seven confirmed retire the old note; the commute is the one missed (§4.5).
- **The episode split scored 8 of 20**, a harness defect: the summaries' daily cap of 20 stopped every session from the 21st on (§5). On the final build, with the cap lifted (H1) and no cap binding, L-episodes scored **17 of 20** and L-replay-episodes 14 of 20.
- **Scale.** §6.4's decision rule fired: at 100K notes the index's search + verification was p95 150.3 ms and the cache 1,065.5 MiB. The owner took the layout revision (LQ17). Built, it measures 48.6 ms and 366.6 MiB. The service's recall p95 at 100K went 666 → 398 ms (the scale and product slices) → 264 ms (the layout), still above §6.4's proposed 100 ms; the owner put the design point at about 30K notes (LQ15). On the final build, measured at 30K for the first time, recall p95 is 81.8 ms, under that 100 ms, while a learned store's is 189.6 ms; at 100K recall is 261.3 ms and a learned store 3.1 s (§7.2, §7.4). The 100K service bench's peak memory rose with the layout, the final build holds it, and it is not explained (§7).
- **What the first run changed.** Five diagnosed findings and the benches drove a harness slice, a scale slice, a product slice (LQ11, LQ13, LQ14, LQ16), the index layout (LQ17) and a close-out (§6). The final build `82180b84` holds all of them but the close-out's third repair round, which changes only the forget path.

## 2. Setup

### 2.1 Model, machine and isolation

- **Model.** `qwen3.8:27b-mlx` on a local Ollama is reader, extractor (no `extraction_model` is set), QA judge and coverage judge (09 §7.4, LQ1). The `conditions` of every report of this harness (D, L and their final runs) record `judge_is_reader: true`, `judge_same_family: true`, and for the learning systems `judge_is_extractor: true`. B-mlx's older harness records neither field, and D-rejudged records `judge_is_reader: false`, since its answers are another model's.
- **Temperatures.** The reader at the model's default (`reader_temperature: null`); the extractor at 0.0 (09 §7.2); the QA judge at 0.0 with a 400-token answer, in both builds (`conditions.judge_max_tokens`). The coverage judge used 400 tokens in the first run; the fixed harness gives it 2,048 (H5, §6.1).
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
| `questions-episodes.json` | 20 questions; 13 en, 5 zh, 2 mixed | `6ae072496587ce2a` (every run here); `1c2545f5d6b199f7` since eq-017's gold was corrected (`c6db079f`, rev 2) |
| `judge-prompt.md` | the frozen QA judge prompt | `df1cc6fa6adb60b2` |
| `coverage-judge-prompt.md` | the frozen coverage judge prompt | `7a0557ce8197639e` |

The 15 changed facts of §7.3 are the corpus's 8 gold change pairs (4 `supersedes` pairs, 4 same-key pairs) and its 7 in-conversation corrections.

### 2.3 The systems and the builds

| Id | What | Build | Run (UTC) |
|---|---|---|---|
| **B-mlx** | 08's B — the old engine: hybrid FTS + E5, `memory_search` offered, the per-turn path live — on this model | `d08317fb` (`live-eval-e5`) | 2026-09-25 16:17–17:45 |
| **D-rejudged** | 08's archived D report (470 answers of `qwen3.8:27b-mtp-q8_0`) graded again by this model; no reader call | harness at `fa09dc0c`; the report's build is `d3a3e712` | 17:46–17:57 |
| **D-index** | the new service, `recall_mode = "index"` (the default): 09 §7.5's **D′**, the seeded run 08 §3.3 left pending | `50cf878d` | 18:04–19:44 |
| **L** | the corpus fed through the gateway, learning on, conversation summaries written; then the questions with learning off; then the forced re-run | `50cf878d` | 19:44 – 09-26 00:48 |
| **L-replay-1…3** | L's kept transcript learned again: extraction only, no reader call, no question | `50cf878d` | 00:48–01:54 |
| **L-S** | the 8 change pairs' old notes seeded as the user's, then the 13 suggestion sessions (78 turns); no question | `50cf878d` | 01:54–02:36 |
| **D-excerpts** | D in `recall_mode = "excerpts"` | `50cf878d` | 02:36–04:19 |
| **L0** | L's feed, never flushed: only the reader's own writes learn | `50cf878d` | 04:19–09:03 |
| **L-episodes** | a second live L run, asked the 20 episode questions | `50cf878d` | 09:03–12:06 |

All from `l8/progress.txt` and each report's `build`. The first runs came from a worktree pinned at `50cf878d`, the last commit of L8-prep (09 rev 3.20), after the corpus review (`7b240b27`). A two-session smoke of L ran first and is not reported. `D-rejudged`'s report names build `d3a3e712`, where 08 labels the same run `8815b4cd`; the commits between them touch no evaluated path.

**The re-measurement** ran the chain `final/run.sh` from a worktree at `82180b84` on 2026-09-28 (UTC; `final/progress.txt`): smoke-L 03:51–03:56, D-index 03:56–05:28, D-excerpts 05:28–07:07, L 07:07–11:52, L-replay-1…3 11:52–13:01, L-replay-episodes (a replay of L that writes summaries and asks the episode split, H9) 13:01–13:39, L-S 13:39–14:20, L0 14:20–18:56, L-episodes 18:56–22:05. Every run exited 0. The final replays read the final L's kept transcript (`final/L-scratch`), not the first L's, so they are not paired with the first replays. B-mlx is not re-run: its code is the old engine's and unchanged, and it was graded by the same judge. `82180b84` holds, beyond `50cf878d`, the harness slice (09 revs 3.26–3.29), the scale slice (3.30–3.32), the product slice (3.33–3.35), the index layout (3.36–3.40) and the close-out through its repair round 2 (3.41–3.43). The branch's head differs from it in code only by the close-out's repair round 3 (`3f3a2307`, `2b49639e`): the forget path and its tests, and two doc comments.

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
| D-index | 453 / 470 | **96.4 %** | 11,601 / 6,842 | 1.68 | 0.79 | 7.9 s / 24.2 s |
| D-excerpts | 450 / 470 | **95.7 %** | 11,227 / 7,800 | 1.54 | 0.63 | 8.9 s / 23.1 s |
| L | 414 / 470 | **88.1 %** | 14,799 / 11,598 | 1.89 | 1.08 | 10.8 s / 36.6 s |
| L0 | 332 / 470 | **70.6 %** | 15,644 / 14,877 | 2.02 | 1.27 | 11.4 s / 35.6 s |

From `final/report-*.json` `totals` (`final/figures/qa.md`, re-derived row by row in `verify-qa.md`). `memory_search` calls: 209, 196, 287 and 375 in the table's order (first run 201, 203, 353, 314). Wall: 92, 98, 285 (the feed 153) and 276 min (the feed 135). The rows in which every relied-on note was delivered, listed or delivered, and read or excerpted: D-index 376, 378 and 372 (first run 373, 380, 367); D-excerpts 375, 375 and 369 (376, 376, 367); L and L0 0 by construction, as before. Index mode's median prompt is 12.3 % smaller than excerpt mode's and its mean 3.3 % larger (first run 13 % and 1 %). No report records a judge error, a turn error or a lagged bus event (`events_lagged`, new in the final harness). Two rows need a word:
- **hq-0118, D-index's one unparsed QA verdict** (paraphrase, zh). Its judge reply ran to the QA judge's 400-token cap, still 400 in both builds (only the coverage judge's cap rose, to 2,048), and ends mid-reasoning; the first run judged the row correct. The report counts it not correct, so the table reads 453 / 470 = 96.4 %. `compare.py` leaves an unparsed row out, so every pair with the final D-index pairs 469 rows and prints 96.6 % for it, with paraphrase at n = 79. Both figures are the same run: 96.4 % over 470, 96.6 % over the 469 judged.
- **hq-0344, one `memory_store` call in D-index** (first run 0). The store's control snapshots still hold exactly the 83 seeded notes, all of origin `user`, so the call left nothing durable and no later question saw a changed store; the report does not record the tool's result.

The QA conditions of each system are identical between its first and final report but for paths, the owner id, the seeded note ids (`seeded_ids`, for D-index and D-excerpts) and, for L and L0, `conditions.learning` (the coverage judge's cap, `episode_max_per_day`, `replay_is_extraction_only`), so the QA fields mean the same in both harnesses.

**Per category** (correct / n and accuracy), final build:

| Category | n | D-index | D-excerpts | L | L0 |
|---|---|---|---|---|---|
| abstention | 45 | 43 (95.6) | 42 (93.3) | 45 (100.0) | 45 (100.0) |
| changed facts / time | 55 | 53 (96.4) | 51 (92.7) | 49 (89.1) | 39 (70.9) |
| cross-language | 70 | 67 (95.7) | 68 (97.1) | 58 (82.9) | 44 (62.9) |
| exact recall | 80 | 79 (98.8) | 78 (97.5) | 71 (88.8) | 59 (73.8) |
| paraphrase | 80 | 75 (93.8) | 77 (96.2) | 68 (85.0) | 56 (70.0) |
| project procedures | 55 | 53 (96.4) | 53 (96.4) | 49 (89.1) | 33 (60.0) |
| rejected / failed actions | 45 | 44 (97.8) | 43 (95.6) | 38 (84.4) | 36 (80.0) |
| source conflicts | 40 | 39 (97.5) | 38 (95.0) | 36 (90.0) | 20 (50.0) |

**Per language**, final build: en (303) 97.0 / 96.0 / 90.1 / 72.6 %; zh (125) 93.6 / 93.6 / 79.2 / 67.2 %; mixed (42) 100.0 / 100.0 / 100.0 / 66.7 %, in the table's column order.

| Candidate vs baseline | Difference | 95 % CI | Discordant pairs | Reading |
|---|---|---|---|---|
| D-index (final) vs B-mlx (the gate) | **+6.6 pp** (469 rows) | **[+3.7, +9.5]**, SE 1.46 | 49, 10.4 % (40 / 9) | **PASS** at −5 pp; *more accurate* allowed |
| D-index vs D-excerpts (final) | +0.9 pp (469 rows) | [−0.7, +2.4], SE 0.80 | 14, 3.0 % (9 / 5) | *comparable measured quality* |
| L vs D-index (final) | −8.3 pp (469 rows) | [−11.3, −5.3], SE 1.53 | 55, 11.7 % (8 / 47) | **FAIL** at `--margin=-10`: the point estimate is inside −10 pp, the CI's lower bound is not; two categories blocked |
| L vs L0 (final) | +17.4 pp | [+13.2, +21.7], SE 2.19 | 120, 25.5 % (101 / 19) | learning on is *more accurate* |
| D-index final vs D-index first | −0.2 pp (469 rows) | [−1.8, +1.4], SE 0.83 | 15, 3.2 % (7 / 8) | two builds, one model: the D side's run-to-run spread plus the build's effect |
| D-excerpts final vs first | −0.4 pp | [−2.5, +1.6], SE 1.04 | 24, 5.1 % (11 / 13) | the same, in excerpt mode |
| L final vs first | +6.8 pp | [+3.1, +10.5], SE 1.88 | 80, 17.0 % (56 / 24) | two builds and two learned stores: not a measure of the build (§8) |
| L0 final vs first | +1.7 pp | [−2.8, +6.2], SE 2.29 | 116, 24.7 % (62 / 54) | two samples of one condition through a changed index (§8) |

The `compare-*.txt` files in `final/figures/` hold each output, ending with its exit code. No pair was refused and none needed `--allow-judge-mismatch`: every paired row was graded by `qwen3.8:27b-mlx` on both sides, and B-mlx's judge prompt hash equals the final reports'. The script's PASS on the final-vs-first rows, at its default −5 pp, has no meaning for those pairs.
- **The gate on the final build.** Per category: rejected / failed actions +22.2, paraphrase +7.6, changed facts +7.3, project procedures +7.3, exact recall +5.0, source conflicts +5.0, cross-language +2.9, abstention −2.2 pp (43 against 44 of 45). Unlike the first run, D is not ahead in every category; no category falls below the block line. The pair also crosses the Ollama version (§8).
- **Index against excerpts.** Per category (index − excerpts): changed facts +3.6, source conflicts +2.5, abstention +2.2, rejected / failed actions +2.2, exact recall +1.2, project procedures 0.0, paraphrase −1.3, cross-language −1.4 pp.
- **L against D-index.** Per category: rejected / failed actions −13.3 and cross-language −12.9 pp are below the block line and, by `compare.py`, need an explanation before a release; exact recall sits at −10.0 exactly and is not flagged (the test is strict); paraphrase −8.9, source conflicts −7.5, changed facts −7.3, project procedures −7.3, abstention +4.4 pp. The first run blocked six categories.
- **L against L0.** L0 leads in no category; source conflicts +40.0 pp (36 against 20 of 40), project procedures +29.1, cross-language +20.0, changed facts +18.2, exact recall +15.0, paraphrase +15.0, rejected / failed actions +4.4, abstention 0.0.
- **L0 across builds.** The total moves +1.7 pp, but two categories move far: source conflicts +32.5 pp (20 against 7 of 40) and project procedures −10.9 pp (33 against 39 of 55), which `compare.py` flags below its block line (exit 1; `compare-L0-final-vs-first.txt`). Both are one sample per build of a condition whose store is the reader's own writes, read through a changed index (§8).
- **D-index across builds.** 16 rows change verdict: the 15 discordant pairs (7 right only in the final: hq-0012, -0108, -0192, -0243, -0288, -0311, -0356; 8 only in the first: hq-0096, -0110, -0214, -0215, -0254, -0293, -0403, -0425) plus hq-0118. The two abstention rows the final lost are hq-0403 and hq-0425. None of the 470 answers is textually identical across the two builds, as an unpinned reader temperature implies.

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

**The final build** (`final/report-*.json`, native fields; `final/figures/learn-targets.md` and `learn-store.md`, both re-derived by their sceptics):

| Run (final) | Learned notes (active / superseded / candidate) | Written by extraction / model tool (`memory_changed`) | Active notes by extraction / model tool | Summaries |
|---|---|---|---|---|
| L | 161 (134 / 18 / 9) | 57 / 104 | 48 / 86 | 46 |
| L-episodes | 165 (140 / 14 / 11) | 66 / 99 | 50 / 90 | 47 |
| L0 | 98 (93 / 5 / 0) | 0 / 98 | 0 / 93 | — |
| L-replay-1 | 146 (123 / 14 / 9) | 146 / 0 | 123 / 0 | — |
| L-replay-2 | 133 (113 / 11 / 9) | 133 / 0 | 113 / 0 | — |
| L-replay-3 | 142 (123 / 10 / 9) | 142 / 0 | 123 / 0 | — |
| L-replay-episodes | 136 (113 / 15 / 8) | 136 / 0 | 113 / 0 | 47 |

The final replays replay the final L's transcript. The live extractor again writes fewer notes than a replay (48 and 50 active against 113–123). "Written by" counts `memory_changed` events, which leave out the summary writer's writes and the forced re-run. L's and L-episodes' learned notes each include 3 notes of kind `episode` and origin `extracted_user`, which are not conversation summaries and are counted as learned, as in the first run. In L all 18 superseded notes are the reader's and all 9 candidates are extraction's.

### 4.2 The §7.3 targets, against the extraction path

§7.3 amended by LQ12 and by the lead's ruling of 09 rev 3.46: coverage, changed facts and restatements take their median over the **three replays alone**; the per-note shares (other-script aliases, any alias, fact keys) take theirs over the three replays **and each live run's extraction figures** (L's and L-episodes'), since a share of the notes extraction did write is not pre-empted. **A live run's coverage by surface is not the extraction path's own coverage**: the coverage judge names one note per gold, so a gold both paths learned is credited to whichever note it named (34 of the reader's 58 matches in L also had an extraction note among the hits), and L's live extraction coverage of 12 of 83 against the replays' 60–66 is that attribution and the reader's pre-emption, not variance (runbook, *The learning evaluation*). Changed facts are not split by surface in either harness: a pair's outcome reads the whole store.

**First run** (`l8/fix/rescore.json`):

| Measure (target) | L live, extraction | L-episodes live, extraction | L-replay-1 | L-replay-2 | L-replay-3 | Median | Met? | Final median |
|---|---|---|---|---|---|---|---|---|
| Other-script aliases (≥ 60 %) | 83.3 % (30 / 36) | 52.8 % (19 / 36) | 70.0 % (77 / 110) | 71.0 % (76 / 107) | 67.8 % (78 / 115) | **70.0 %** | yes | **56.1 %** (48.0–62.5 %), not met |
| Any alias | 100 % | 100 % | 100 % | 100 % | 100 % | 100 % | — | 100 % |
| A `fact_key` | 100 % (36 / 36) | 100 % | 99.1 % | 99.1 % | 99.1 % | 99.1 % | — | 99.1 % (92.0–99.2 %) |
| Coverage (≥ 80 %) | 12 / 83 (attribution; not a sample) | 12 / 83 (attribution) | 61 / 83 (73.5 %) | 60 / 83 (72.3 %) | 66 / 83 (79.5 %) | **73.5 %** over the replays | not as measured | **84.3 %** (70 / 83; 81.9–86.7 %), met |
| — unparsed verdicts, all cut at the 400-token cap (final: 2,048) | 2 (pooled) | 2 (pooled) | 8 | 6 | 5 | upper bound 83.1 % | — | 1 / 1 / 2, all cut at 2,048; upper bound 86.7 % |
| Changed facts (≥ 12 of 15); first run re-scored by H6, final judged at run time | — (pooled only) | — | 8 (7–8) | 9 (8–9) | 7 | **8** | no | **10** (9–10), not met |
| Restatements raising a suggestion or a duplicate (≤ 2 of 15) | 0 | 1 duplicate (and 1 automatic replacement) | 2 duplicates | 1 duplicate | 2 duplicates (and 1 automatic replacement) | **2** | yes | **0** (0–0), met |

**Final build** (`final/report-*.json`, `learning.aliases_by_surface.extraction`, `fact_keys_by_surface.extraction`, `coverage`, `changed_facts_correct`, `restatements_by_surface.extraction`; `final/figures/learn-targets.md`, re-derived in `verify-learn-targets.md`). L-replay-episodes is reported beside the medians and left out of them, by the lead's reading of §7.3: §7.3 names three replays, and this one runs the summary writer too.

| Measure (target) | L live, extraction | L-episodes live, extraction | L-replay-1 | L-replay-2 | L-replay-3 | Median | Met? | L-replay-episodes (beside) |
|---|---|---|---|---|---|---|---|---|
| Other-script aliases (≥ 60 %) | 62.5 % (30 / 48) | 48.0 % (24 / 50) | 50.4 % (62 / 123) | 56.6 % (64 / 113) | 56.1 % (69 / 123) | **56.1 %** | no | 56.6 % (64 / 113) |
| Any alias | 100 % | 100 % | 100 % | 100 % | 100 % | 100 % | — | 100 % |
| A `fact_key` | 97.9 % (47 / 48) | 92.0 % (46 / 50) | 99.2 % (122 / 123) | 99.1 % (112 / 113) | 99.2 % (122 / 123) | 99.1 % | — | 100 % (113 / 113) |
| Coverage (≥ 80 %) | 22 / 83 (attribution) | 17 / 83 (attribution) | 72 / 83 (86.7 %) | 68 / 83 (81.9 %) | 70 / 83 (84.3 %) | **84.3 %** | yes | 65 / 83 (78.3 %) |
| — unparsed verdicts (cap 2,048) | 0 | 0 | 1 (cut at the cap) | 1 (cut) | 2 (cut) | upper bound 86.7 % | — | 1 (cut) |
| Changed facts (≥ 12 of 15), judged at run time | — (pooled only: 12) | — (pooled only: 11) | 10 | 9 | 10 | **10** | no | 9 |
| Restatements raising a suggestion or a duplicate (≤ 2 of 15) | 1 duplicate (and 1 inferred candidate) | 2 duplicates | 0 (and 1 automatic replacement) | 0 (and 1 inferred candidate) | 0 | **0** | yes | 0 (and 1 automatic replacement) |

With L-replay-episodes counted in, no verdict would change: other-script aliases 56.4 %, coverage 69 / 83 (83.1 %), changed facts 9.5. An *inferred candidate* is a candidate that names no note, so it is neither a suggestion nor a duplicate (L cs-038 #5, L-replay-2 cs-033 #3). A duplicate is decided on the citing note's stored status, as in the first run: both of L-episodes' duplicate notes are now effectively superseded.

- **The two sets of medians are not paired.** The final replays replay the final L's transcript, the first replays the first L's; the coverage judge's cap rose from 400 to 2,048 tokens (H5); and changed facts are now judged at run time by H6's link check (§4.4) where the first run's figures are offline bounds. A final figure is therefore read beside the first run's re-scored ones and upper bounds (coverage's 83.1 %, not its measured 73.5 %), never as an improvement. The other-script alias share, below target on the final build, is §4.8's.
- **Coverage** is below target on every extraction sample as measured on the first run. Each replay's unparsed verdicts were deliberations the 400-token cap cut before the verdict line (H5, rev 3.26), so an upper bound adds them: 69, 66 and 71 of 83, median 83.1 %. The sceptics note that several of those were heading to `NONE`, so it is only a bound. The fixed harness gives the coverage judge 2,048 tokens. On the final build the replays' coverage is 68–72 of 83, at or above target in each; every unparsed verdict still ran to the 2,048-token cap (1, 1 and 2), which bounds it at 73, 69 and 72 of 83 (median 86.7 %), and the live runs had none.
- **Changed facts** on the replays are right on 6–7 of the 8 pairs (after H6) and wrong on most corrections: 1, 3 and 1 of 7, where L and L-episodes are 7 of 7 through the reader's own notes. Of the replays' 16 failed correction slots, 9 are unparsed coverage verdicts (*learning-gap*, sceptic 1). "7–8" and "8–9" are H6's offline bounds (§4.4). On the final build the replays are right on 5–6 of the 8 pairs and on 3–5 of the 7 corrections; of the four final replays' 12 failed correction slots (L-replay-episodes included), 4 are verdicts unparsed even at 2,048 tokens and the rest `NONE`. H6's run-time check refused 4 links, commute and laptop in L-replay-1 and in L-replay-episodes, where the judge, asked again without the new note, answered `NONE` (§4.4).
- **The capture replay** (`capture/report-replay-capture.json`, at `6eb0fae8`: the fixed harness, before LQ13) is a fourth extraction sample on the same transcript, made to record unusable answers: other-script aliases 64.0 % (71 / 111), a key on 99.1 %, coverage 65 / 83 (78.3 %) with 1 unparsed verdict under the 2,048-token cap, changed facts 7, restatements 1 duplicate and 1 automatic replacement. It is not one of §7.3's samples.

### 4.3 The whole store (pooled)

The first run's five columns and the final L beside them (final: `learning.coverage`, `aliases`, `fact_keys`, `changed_facts_correct`, `noise`, `restatements_by_outcome`, native under H3–H7; `final/figures/learn-store.md`, re-derived in `verify-learn-store.md`).

| Measure | L | L-episodes | L0 | L-replay-1 / 2 / 3 | Final L (`82180b84`) |
|---|---|---|---|---|---|
| Coverage | 70 / 83 (84.3 %) | 69 / 83 (83.1 %) | 65 / 83 (78.3 %) | 61 / 60 / 66 | 77 / 83 (92.8 %), cap 2,048 |
| — matched to an extraction / model-tool note | 12 / 58 | 12 / 57 | 0 / 65 | all extraction | 22 / 55 |
| — `none` / unparsed / `not_found` | 11 / 2 / 0 | 11 / 2 / 1 | 15 / 2 / 1 | 12, 15, 12 / 8, 6, 5 / 2, 2, 0 | 5 / 0 / 1 |
| Other-script aliases | 38.8 % (45 / 116) | 31.7 % (39 / 123) | 16.7 % (16 / 96) | 70.0 / 71.0 / 67.8 % | 33.6 % (45 / 134) |
| — the reader's notes alone | 18.8 % (15 / 80) | 23.0 % (20 / 87) | 16.7 % | — | 17.4 % (15 / 86) |
| Any alias | 72.4 % | 69.1 % | 45.8 % | 100 % | 69.4 % (93 / 134) |
| A `fact_key` | 55.2 % (64 / 116) | 55.3 % (68 / 123) | 46.9 % (45 / 96) | 99.1 % each | 63.4 % (85 / 134) |
| — the reader's notes alone | 35.0 % (28 / 80) | 36.8 % (32 / 87) | 46.9 % | — | 44.2 % (38 / 86) |
| Key reused within a change pair (after H6) | 1 of 5 | 2 of 6 | 0 of 2 | 7 of 7, 6 of 6, 6 of 7 | 1 of 8 |
| Changed facts (after H6; offline bound) | 11 (10–11) | 13 (12–13) | 7 | 8 (7–8), 9 (8–9), 7 | 12 (judged at run time; not a bound) |
| Noise: active learned notes no coverage call chose, plus candidates | 59 of 116, + 5 | 65 of 123, + 4 | 37 of 96, + 0 | 58 of 110 + 7, 55 of 107 + 7, 56 of 115 + 6 | 68 of 134, + 9 |
| Restatements (15 turns; after H7) | nothing written 11, the reader wrote 3, first learned 1 | nothing written 3, the reader wrote 5, extraction failed 5, automatic replacement 1, duplicate 1 | nothing written 10, the reader wrote 5 | see §4.2 | nothing written 9, the reader wrote 2, first learned 2, duplicate 1, inferred candidate 1 |

The reader's notes carry few other-script aliases and few keys; 90 of its 92 writes in L cite no source (*learning-gap*, sceptic 2). The pooled alias share is therefore mostly the reader's, which is what LQ12 separated.

**The other final runs, pooled.** L-episodes: coverage 77 / 83 (92.8 %), matched 17 / 60 by extraction / model tool, `none` / unparsed / `not_found` 6 / 0 / 0; other-script aliases 30.0 % (42 / 140), the reader's notes alone 20.0 % (18 / 90); any alias 67.1 % (94 / 140); a key 73.6 % (103 / 140), the reader's 63.3 % (57 / 90); key reused in 3 of 6 pairs; changed facts 11; noise 74 of 140, + 11; restatements nothing written 9, the reader wrote 3, duplicate 2, first learned 1. L0: coverage 60 / 83 (72.3 %), all the reader's; 21 / 0 / 2; other-script aliases 17.2 % (16 / 93); any alias 47.3 % (44 / 93); a key 55.9 % (52 / 93); key reused 0 of 4; changed facts 8; noise 36 of 93, + 0; restatements nothing written 10, the reader wrote 5. The final replays' pooled figures are their extraction figures (§4.2), with key reuse 5 of 5, 6 of 7 and 6 of 6 and noise 59 of 123 + 9, 53 of 113 + 9 and 62 of 123 + 9 (L-replay-episodes 5 of 5; 58 of 113 + 8). The final figures come from a harness whose coverage cap is 2,048 (L's first-run 2 unparsed verdicts were the 400-token cap's), whose changed facts are judged (§4.4) and whose restatement classes are native rather than re-scored; they are read beside the first run's re-scored column, never as an improvement on it. The pooled restatement tally takes the outcome of the note citing a row, so a turn the reader wrote and extraction also cited counts under extraction (L-episodes: the reader wrote 4 by surface, 3 pooled).

### 4.4 Changed facts, per pair

**First run.** Outcomes as recorded; "→ H6" marks a pair the re-score moves to *new active, old superseded* when the coverage judge, asked again without the new note, names the superseded one — offline that check cannot be replayed, so it is the bound's upper end (`fix/impl.md`, *Offline re-score*, repair round 2).

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

**Final build** (`learning.changed_facts[]`, `changed_facts_correct`, `link_checks`; `final/figures/learn-store.md`, re-derived in `verify-learn-store.md`). The harness's raw outcomes map to the labels above: `new_active_old_superseded` is **superseded**, `both_active_unlinked`, `new_not_active`, `new_missing` and `old_never_learned` keep their names, and `new_active_old_same_key_loser` is **new active, old a same-key loser**, a correct outcome that no first-run pair took. H6 is now a judged run-time check, not an offline bound: when one note matched both golds, is active and no same-key loser, and supersedes a superseded note the old gold's search returned, the coverage judge is asked about the old gold again without the new note, and the pair is scored from its pick. "H6 ✓" marks a check that named a note and credited the pair, "H6 ✗" one the judge answered `NONE`, which leaves the pair *old never learned*. The "correct of 15" below is therefore the harness's own count, where the first run's re-scored figures are bounds.

| Fact | L | L-episodes | L0 | L-replay-1 | L-replay-2 | L-replay-3 | L-replay-episodes |
|---|---|---|---|---|---|---|---|
| `user.commute` (supersedes) | both active, unlinked | old never learned (one note for both) | old never learned (old gold unmatched) | old never learned, H6 ✗ | both active, unlinked | old never learned (one note for both, no link) | old never learned, H6 ✗ |
| `user.editor` (supersedes) | **superseded** | **superseded** | **superseded** | **superseded** | **superseded**, H6 ✓ | **superseded**, H6 ✓ | **superseded** |
| `user.laptop` (supersedes) | **superseded**, H6 ✓ | both active, unlinked | **superseded**, H6 ✓ | old never learned, H6 ✗ | **superseded**, H6 ✓ | **superseded**, H6 ✓ | old never learned, H6 ✗ |
| `moss.output_dir` (supersedes) | **new active, old a same-key loser** | **superseded** | new missing | **superseded** | **superseded**, H6 ✓ | **superseded** | **superseded** |
| `user.weekly_review` (same key) | new not active | new missing | new missing | new missing | new missing | new missing | new missing |
| `lantern.service.port` (same key) | **superseded** | **superseded** | new missing | **superseded** | **superseded** | **superseded** | **superseded** |
| `lantern.staging.db` (same key) | **superseded** | **superseded** | **superseded** | **superseded** | **superseded** | **superseded** | **superseded** |
| `moss.python.version` (same key) | **superseded** | **new active, old a same-key loser** | both active, unlinked | **superseded** | **superseded** | **superseded** | **superseded** |
| 7 corrections, new value active | 6 (`user.sleep` missing) | 6 (`lantern.release` missing) | 5 (`moss.rss`, `lantern.oncall` missing) | 5 (`lantern.release`, `moss.rss` missing) | 3 (`lantern.release`, `user.running`, `moss.rss`, `user.sleep` missing) | 4 (`user.running`, `moss.rss`, `user.sleep` missing) | 4 (`lantern.release`, `user.running`, `moss.rss` missing) |
| H6 checks: due / credited / refused | 1 / 1 / 0 | 0 / 0 / 0 | 1 / 1 / 0 | 2 / 0 / 2 | 3 / 3 / 0 | 2 / 2 / 0 | 2 / 0 / 2 |
| **Correct of 15** (judged) | **12** | **11** | **8** | **10** | **9** | **10** | **9** |

Every missing correction is `new_missing` (its coverage verdict `none` or unparsed). The refused links are the commute and the laptop in L-replay-1 and in L-replay-episodes. The same-key-loser credit is the harness's rule as it applies it: it does not check that the winner of the old note's key is the new note, and in L's `moss.output_dir` it is not (the old pick loses under `moss.tech_details`, while the new note carries `moss.build.output_path`).

L's pairs, first run against final, read through the re-scored figures (different harnesses and different learned stores, so not a measure of the build): the commute went from *old never learned* to *both active, unlinked* — the old gold's pick is now an extraction note, 资产变动：出售本田飞度 (key `user.vehicle.ownership`), beside the new one under `user.commute`, with no link between them; the editor to *superseded*, from the judge's own picks; the laptop from *new not active* to *superseded* with a credited check (the reader's MacBook note, the judge's pick for both golds, supersedes the reader's ThinkPad note, key `user.hardware`, which the check named); the output directory, *superseded* in the re-score, is now *new active, old a same-key loser*, correct under another outcome; the weekly review is still wrong (*new not active*: the new gold's pick is a superseded reader note under `weekly.review.summaryformat`); the three other same-key pairs are unchanged. Corrections 7 → 6 (`user.sleep`), and correct of 15 is 10 recorded, 11 re-scored, 12 final.

### 4.5 L-S: suggestions for notes the user saved

The 8 old notes of the change pairs are seeded as the user's; four of them — editor, laptop, commute and Moss's output directory — as the user would have saved them then, present tense with no end date (`conditions.learning.seeded_as_saved_then`, LQ7). 13 sessions and 78 turns are fed. Expected (§7.2): 8 suggestions naming the right notes, 0 user notes retired.

| | First run | Final |
|---|---|---|
| Seeded notes with a suggestion | **4 of 8** (`user.weekly_review` 2, `moss.python.version`, `lantern.staging.db`, `lantern.service.port`) | **7 of 8** (`user.laptop` 1, `lantern.service.port` 2, `user.weekly_review` 2, `moss.output_dir` 1, `user.editor` 1, `moss.python.version` 1, `lantern.staging.db` 1); none for `user.commute` |
| Suggestions in all (naming another note) | 6 (1) | 9 (0): 5 from the reader's own `memory_store`, 4 from extraction |
| Confirmed and the old note superseded | 4 of 4 | 7 of 7 |
| User notes retired without a confirm | 0 | 0 |
| Restate turns (6): suggestion / nothing written / the reader wrote | 1 / 2 / 3 (after H7) | 1 / 2 / 2, and 1 first learned (native H7) |

The four seeded notes with no suggestion are exactly LQ7's four present-tense seeds, and exactly the four change turns on which the reader's own `memory_store` wrote a note (cs-003 #0, cs-034 #0, cs-035 #0, cs-041 #0–1). The diagnosis's mechanism: the reader tried to supersede the saved note, G2 refused it (09 §3.1 row 5 as it then read) — proven for cs-041 and inferred for cs-003 and cs-035 — and the reader stored the new value as a second active note, which the extractor then saw as already known, so no suggestion was raised. The commute miss is different: its one change session, cs-034, stopped on an unusable extraction answer. So at most 3 of the 4 fit the pre-emption (*ls-suggestions* and *completeness*, their sceptics). LQ11 makes the reader's replacement of a saved note a suggestion itself, and the first run could not separate the seeding from the pre-emption, since both fall on the same four notes.

**The final run's answer: 7 of 8.** LQ7's four present-tense seeds got 3 of 4 — the editor, the laptop and the output directory — and all three suggestions came from the reader's own `memory_store`, written as a suggestion under LQ11 (surface `model_tool`); extraction raised none of the three. On cs-003 #0 the reader's note 780cd27d supersedes the saved laptop note and the extractor dropped its item as already learned. On cs-041 #0 the reader's acf2dbc4 ("Editor: Helix (hx)") supersedes the saved editor note, and the extractor answered with 5 output tokens and stored nothing. On cs-035 #0 the reader's 1f04544d supersedes the saved output-directory note; the extraction's `turns` attempt was cut at the 2,892-token ceiling, and its cut tail shows an output-directory item (`"replaces":"n2"`) being written; the `switch` flush's first attempt was cut too, its tail ending inside a Moss test-command item (a record keeps only a head and a tail, so whether that answer held an output-directory item is not recorded), and its usable retry stored 7 notes, among them a suggestion for `lantern.service.port`, none for the output directory. The commute is the miss again, by another mechanism: cs-034's extraction answer was usable this time (1 output token, nothing stored or dropped), and the reader's `memory_store` wrote 0d5bb4c9 "通勤方式与卖车记录" (kind `episode`, no key, no `supersedes`) as a second active note beside the saved 3495191c "开一辆本田飞度"; it named no saved note, so LQ11 had nothing to turn into a suggestion. Of the 9 suggestions, one (905a3540, an extraction candidate raised by the restatement cs-017 #2) proposes the saved weekly-review value itself; the confirm used the listed Monday 09:30 suggestion, the right one. The restate turn classed *first learned* (cs-038 #5) is an extraction note under a new key, `lantern.smoke_test_target`, citing the restated row. **Not recorded:** on cs-003 #0 and cs-034 #0 the reader called `memory_store` twice with one memory change, and what the other call returned — a refusal, a validation error or a deduplication — is in neither the report nor the log, and L-S's scratch database was not kept. So in the three cases where the reader named the saved note, LQ11 turned its write into the suggestion, and in the one case where it did not, a second active note was still left beside the saved one.

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
| L (final) | 0 | 0 | yes | 0 |
| L-episodes (final) | 0 | 0 | yes | 0 |
| L-replay-1 (final) | 0 | 0 | yes | 0 |
| L-replay-2 (final) | 0 | 0 | yes | 0 |
| L-replay-3 (final) | 0 | 0 | yes | 0 |
| L-replay-episodes (final) | 0 | 0 | yes | 0 |
| L-S (final) | 0 | — (no re-run) | — | 0 |
| L0 (final) | 0 | — (no re-run) | — | 0 |

A re-run that checked nothing is counted a violation, never a pass (09 §7.2), so four runs failed after writing their reports (`l8/progress.txt`: `exit=101`). No violation of either invariant itself was observed. The capture replay, with the harness retrying as production does, checked it: 0 resurrected. §6.1 (a) has the cause.

**On the final build** (`learning.invariants`, `learning.forced_rerun`; `final/progress.txt`) the re-run checked in all 6 runs that make one; L0 and L-S make none by design, and their unset reason is not a violation. *Checked* is stricter in the fixed harness: the first harness asked for an extraction call and no stop, the final one also for no window skipped after three unusable answers, and it names the reason when a re-run is unchecked (H2). Each final re-run made one attempt and one call, did not stop, had no unusable answer and read its session to the end, with 819–1,955 output tokens; it dropped as suppressed 1 item in L, 4 in L-episodes, 4, 2 and 0 in the three replays and 2 in L-replay-episodes. L-replay-3's re-run stored one active note (7ad12903), which is in neither `forced_rerun.resurrected` nor `rewritten_from_other_rows`: not a resurrection by either of the harness's tests (the note's sources are not in the report, and its file is not in the kept `final/store-L-replay-3`, whose control snapshot names it). Every run of the chain exited 0 (`CHAIN DONE 2026-09-28T22:05:22Z`). The retired-note invariant tests nothing outside L-S, in either harness: no other run holds a note of origin `user` (none is seeded, and learning writes only `extracted_*` origins), so its 0 there holds because nothing could be retired.

### 4.7 The cost of learning

| Run | Extraction calls | Tokens in / out | Flush latency p50 / p95 (final: per attempt) | Flushes (trigger) | Stopped on an unusable answer (final: unusable answers; windows skipped; left unread) | Summaries: calls / written | Feed |
|---|---|---|---|---|---|---|---|
| L | 55 | 235,763 / 29,199 | 15.2 s / 50.0 s | 56 (51 `switch`, 5 `turns`) | 11 (8 `switch`, 3 `turns`) | 20 / 20 | 154 min, 300 reader turns |
| L-episodes | 55 | 239,662 / 31,711 | 15.6 s / 67.4 s | 56 (51, 5) | 16 (13, 3) | 20 / 19 | 166 min |
| L-replay-1 | 52 | 209,631 / 29,976 | 17.8 s / 41.4 s | 51 `replay` | 4 | — | 16 min |
| L-replay-2 | 52 | 207,601 / 33,176 | 19.4 s / 45.9 s | 51 | 7 | — | 19 min |
| L-replay-3 | 52 | 210,230 / 29,771 | 18.0 s / 34.4 s | 51 | 5 | — | 16 min |
| L-S | 16 | 56,439 / 7,560 | 16.6 s / 40.0 s | 15 (13, 2) | 2 | — | 42 min, 78 turns |
| Final L | 60 | 236,369 / 33,035 | 13.5 s / 62.0 s (per attempt) | 56 (51 `switch`, 5 `turns`); 60 attempts | 5 unusable answers (2 cut at the ceiling, 3 the answer `{}`); 1 window skipped (cs-042); 0 left unread | 52 / 46 (6 unusable answers; 2 windows skipped, cs-003 and cs-016) | 153 min, 300 reader turns, 0 in error |
| Final L-episodes | 63 | 256,384 / 56,650 | 15.5 s / 66.2 s | 56 (51, 5); 63 attempts | 9 (8 cut, 1 not JSON); 2 skipped (cs-020, cs-036); 0 unread | 47 / 47 | 172 min, 0 in error |
| Final L0 | 0 (learning off) | — | — | — | — | — | 135 min, 300 reader turns, 0 in error |
| Final L-replay-1 | 54 | 219,603 / 32,704 | 16.0 s / 35.1 s | 51 `replay`; 52 attempts | 1 (not JSON); 0; 0 | — | 16 min |
| Final L-replay-2 | 55 | 225,204 / 34,852 | 15.8 s / 42.7 s | 51; 53 attempts | 2 (1 cut, 1 not JSON); 0; 0 | — | 17 min |
| Final L-replay-3 | 54 | 221,352 / 33,668 | 15.9 s / 41.9 s | 51; 52 attempts | 1 (cut); 0; 0 | — | 16 min |
| Final L-replay-episodes | 54 | 215,797 / 32,878 | 14.9 s / 41.0 s | 51; 52 attempts | 1 (cut); 0; 0 | 50 / 47 (3 unusable answers; 1 window skipped, cs-016) | 23 min |
| Final L-S | 17 | 68,565 / 12,447 | 10.9 s / 71.8 s | 15 (13, 2); 16 attempts | 2 (both cut, cs-035); 0; 0 | — | 41 min, 78 turns, 0 in error |

Across the six learning runs, 45 flushes stopped on an unusable answer over 282 extraction calls, and 4 of the 5 forced re-runs did; 2 of L's 11 stops and 4 of L-episodes' 16 hit the 2,800-token ceiling, the rest were refused by the parser well under it (*completeness*, sceptic 1). A stop left a session unlearned when no later flush read it: 8 of 51 in L, 13 in L-episodes.

**The final rows count per attempt, not like for like with the first run's stops** (`learning.cost`; `final/figures/ls-inv-cost-episodes.md`). The fixed harness retries as production does — up to three attempts, then the window is skipped — so its unusable-answer column counts every attempt's answer, with the windows skipped and those left unread when the run ended beside it, where the first run's column counts flushes stopped at their one unusable answer. Only one final flush ended stopped (L-S's cs-035 `turns`, whose window the `switch` flush then read), and 14 flushes met at least one unusable answer. Flush latency is per attempt in the final harness and per flush in the first, and a final summary's "unusable" in this table counts answers (§5's session row counts the windows skipped after three of them). L-S's p95 is the cut cs-035 `turns` attempt. **A data note on the final L:** its learning window of cs-042 was skipped after three unusable answers (each the answer `{}`), so those turns were never learned, and the summary windows of cs-003 and cs-016 were skipped the same way (`learning.warnings`); L's QA store lacks whatever they would have written. L-episodes skipped the learning windows of cs-020 and cs-036, L-replay-episodes the summary window of cs-016.

### 4.8 The other-script alias share on the final build

The share of active extraction notes with an alias in the other script fell from 67.8–71.0 % on the first replays to 50.4–56.6 % on the final ones, and its final median, 56.1 % over the five samples (48.0–62.5 %), is below §7.3's 60 %. A diagnosis from the reports and the two kept transcripts, made with no model call and checked by two sceptics whose corrections stand (`final/alias-drop/diagnosis.md`, `sceptic-1.md`, `sceptic-2.md`), finds no sign that the extractor follows its alias rule (the prompt's rule 5: an English statement gets Chinese aliases) less often: stratified by session, a flush gives an English-titled note a Chinese alias as often as before (Mantel–Haenszel risk difference, first minus final, +0.012 at flush level, and −0.02 to −0.04 with cs-015 left out, the final runs if anything slightly more compliant), which the first sceptic rates *likely*. The drop is in which sessions yield notes and how many: cs-024 and cs-033, which gave Chinese-titled notes in the first replays, go silent in the final ones (the extractor answers `{"items": []}`), cs-024 in every final replay and cs-033 in two of the three (its flush in L-replay-2 wrote two candidates), while cs-016, cs-049 and cs-050 move the other way; sessions yielding in both give different note counts, some fewer (cs-048 one active note and two candidates where it had given three to five, cs-035 six where it had given seven to ten) and others more English-titled notes that rarely carry a Chinese alias (cs-005 5 → 15, cs-008 10 → 17, over the three replays of each set); and one seven-note flush, cs-015, flips from 7 of 7 English-titled notes carrying a Chinese alias in every first replay (6 of 6 in the capture replay) to 0 of 7 in every final one. Sessions flip in both directions (cs-027, cs-032 and cs-036 up; cs-015 and cs-038 down), in a model that answers identical prompts differently at temperature 0 (§8). By the first sceptic's centred decomposition of the pooled −15.3 pp (69.6 → 54.3 % over the three replays of each set, not a change in the median): within-session changes −5.6 (cs-015 alone −5.8, the other sessions +0.2 net), changed note counts within shared sessions −4.5 (led by cs-005 −1.3, then cs-048 and cs-027 −0.9 each; restoring cs-048's first-run notes moves the share +1.2, cs-035's +0.2), the sessions gone silent −2.2 and the sessions newly yielding English notes without a Chinese alias (cs-010, cs-016, cs-050) −3.0 pp. The first runs' baseline was itself inflated: English sessions' Chinese-titled notes break the prompt's rule 8 (title and content in the user's language) yet count as hits, since a Chinese-titled note with an English alias always does. Whether the replayed transcript or the build (LQ13's unwrap, the known-notes lookup over LQ17's index, the 2,892-token budget, the prompt's date) drives these session-level changes is not separable with what exists: the capture control, on the first transcript at `6eb0fae8`, differs from the final replays in both; the proposed lever, replies citing note ids, is contradicted by sessions flipping both ways; and the counterfactual that restores cs-015 to 7 of 7 (median 61.8 %) is one-sided, since dropping cs-015 from both sets leaves the final median at 59.5 %. Before the cross-over, that the transcript is the cause was only *possible*. An Ollama upgrade (0.34.2 to 0.34.4) also falls between the first replays and the final runs (§8).

**The cross-over replays** (2026-09-28/29, `crossover/`; analysis and sceptic in `crossover/analysis/`, the sceptic's corrections standing; reports archived under `scripts/eval/memory/reports/l8-crossover/`). Six extraction-only replays: the first L's transcript on the final build `82180b84` three times (A), and the final L's transcript on `6eb0fae8`, the capture build, three times (B); all exited 0. Other-script alias share of active extraction notes, by transcript and build:

| Transcript \ build | `50cf878d` (first replays) | `6eb0fae8` (capture build) | `82180b84` (final build) |
|---|---|---|---|
| The first L's | 70.0 / 71.0 / 67.8 % (median 70.0) | 64.0 % (the capture replay, one run) | **62.9 / 65.0 / 75.6 %** (A; median 65.0) |
| The final L's | — | **55.0 / 58.7 / 67.0 %** (B; median 58.7; 21, 29 and 22 unusable answers, 6–9 windows skipped) | 50.4 / 56.6 / 56.1 % (the final replays; median 56.1) |

- **On the final build the transcript moves the share by −13.4 pp** in run means (67.8 → 54.4 %), with no overlap between the two sets of runs (exact p = 0.10, the floor for three runs against three). The user rows are identical in all 51 sessions and the prompt's date comes from the transcript's own rows, so the difference lies in the reader's replies.
- **Nearly all of it is three sessions** that follow the transcript in all 13 runs across three builds and two Ollama versions: cs-015's seven-note flush (7 of 7 English-titled notes with a Chinese alias against 0 of 7), cs-024 (Chinese-titled notes in an English session against none) and cs-048 (three to five notes against one consolidated note — granularity, not alias behaviour). Without those three the transcripts do not differ detectably (−5.0 pp, p = 0.60). Why they flip is unexplained: cs-015's replies carry no Chinese in either transcript, and the known notes before it do not explain it.
- **The build's extraction path shows no effect on the windows both builds read**: point estimates −0.3 to +2.8 pp, on the sessions no run of either cell skipped. Three runs a cell bound that only to about ±8 pp on the first transcript and ±14 pp on the final one, so this is the absence of a detected effect, not a bound of 3 pp. The build does lower the headline one way: `82180b84` reads windows the older builds lost to unusable answers (cs-005, cs-010, cs-016, cs-027 — English sessions with few Chinese aliases), about −2 pp on the first transcript and about −6 pp on the final one. B reads above the final replays only because the capture build lost those windows (cs-001 in every run).
- **What stays open.** Each transcript is the output of one live run of its build, so whether the final build's reader tends to write transcripts like the final one — replies that cite note ids, summarise several facts at once, LQ11's suggestions — is untested; the cross-over clears the build's extraction path (prompt, unwrap, lookup, index), not the build. Every step from `50cf878d` also crosses the Ollama upgrade. By claim: the gap between the replay sets is the transcript's, through three sessions — established; the build's extraction path is not the cause — likely; why those sessions flip — unexplained.
- **The 60 % target.** The final build meets it replaying the first transcript (65.0 %) and misses it replaying the final one (56.1 %); the only whole-system samples, the live runs' extraction notes, straddle it (L 62.5 %, 30 of 48; L-episodes 48.0 %, 24 of 50), each with roughly ±14 pp of binomial uncertainty. Pass or fail against 60 % is therefore not a property of the build these data can settle; it would take several live transcripts per build, not more replays of one. Part of the first transcript's pass is notes that break rule 8 (cs-024's Chinese titles in an English session, 1.4 pp), which the metric counts as hits.

## 5. The episode split

L-episodes asked the 20 questions of `questions-episodes.json` after a live learning run; automatic recall never delivers a summary, so the reader finds one only through `memory_search` or `memory_timeline`.

| | First run | Final |
|---|---|---|
| L-episodes (live) | **8 / 20 (40.0 %)** — en 6 / 13, zh 2 / 5, mixed 0 / 2 | **17 / 20 (85.0 %)** — en 12 / 13, zh 4 / 5, mixed 1 / 2 |
| L-replay-episodes (a replay of the final L, extraction-only store) | — | **14 / 20 (70.0 %)** — en 10 / 13, zh 2 / 5, mixed 2 / 2 |
| Sessions: summary written / capped / below 4 turns / unusable (final: window skipped after 3 unusable answers) | 19 / 27 / 4 / 1 | L-episodes 47 / 0 / 4 / 0; L-replay-episodes 46 / 0 / 4 / 1 (47 summaries, cs-006 wrote two; the skipped window, cs-016, had 3 unusable answers) |
| Reader p50 / p95; input tokens per turn, mean | 28.0 s / 82.3 s; 28,871 | L-episodes 28.6 s / 45.5 s; 29,188. L-replay-episodes 24.8 s / 44.6 s; 28,041 |

**Why 8 of 20** (`diagnosis/episodes.md`, both sceptics upheld it):
- **A harness defect, not the feature.** The harness lifted the extraction's daily caps but left `episode_max_per_day` at its default of 20, and the whole corpus is fed on one UTC day. After the 20th summary call every session from cs-021 on stopped `Cap` (27 of them); 4 three-turn sessions are below the 4-turn minimum. 11 of the 20 questions ask about a session at or after cs-021. L itself had the same shape (20 written, 27 capped).
- **With a summary, 6 of 8 were right; without one, 2 of 12** — both from notes the reader had stored during the feed. Retrieval missed nothing: whenever the reader searched and a summary existed, the search or the timeline listed it and the reader read it.
- **The 12 misses:** 9 had no summary because of the cap; 1 (eq-006) because its one summary call gave an unusable answer and the harness then retried nothing; 1 (eq-014) had a summary that kept the facts already stored as notes and dropped the outcome the question asks about (L's summary of the same session drops it too — a salience question for the episode prompt); 1 (eq-017) has a gold answer that describes a conversation the live reader did not have.

The fix is H1 (`bbd94e7d`): a learning system lifts `episode_max_per_day` to 1,000 and warns when any cap binds; H9 (`1b199807`) lets a replay write summaries and ask the split. The prompt's salience, eq-017's gold and trimming the long golds (the corpus review's N9) are the owner's; none is implemented (09 rev 3.26).

**The final build** (`final/report-L-episodes.json`, `final/report-L-replay-episodes.json`; `final/figures/ls-inv-cost-episodes.md` and its verification). No cap bound in any final run: no flush stopped `Cap`, no cap warning was written, and every run with learning on had `episode_max_per_day` 1,000. The 4 sessions below the minimum are the 3-turn cs-034, cs-037, cs-039 and cs-047. The first run's 8 was the cap's (H1), so the final 17 is read as the split with the defect removed, not as a gain of the feature; L-replay-episodes, 14, answers from an extraction-only store with no reader notes. The misses, one line each:
- *L-episodes, eq-001* (cs-002): the timeline listed the summary and the reader read it; the answer left out sorting by size in human-readable units and added details.
- *L-episodes, eq-014* (cs-018): two searches listed the summary and the reader read it; the answer described a different focus and missed the gold's details — the salience question the first run named.
- *L-episodes, eq-017* (cs-019): search listed the summary first and the reader read it; the answer says no model was decided, against the gold's purchase — the question whose gold the first run found describing a conversation the live reader did not have.
- *L-replay-episodes, eq-006* (cs-016): no summary, the window skipped after 3 unusable answers; the reader read two other notes and described a different conversation.
- *L-replay-episodes, eq-009* (cs-026): the summary was listed and read; the answer left out the sqlx offline-mode fix.
- *L-replay-episodes, eq-012* (cs-035): a summary existed, but the reader made no search or timeline call and answered from 3 recalled notes, leaving out the licence badge.
- *L-replay-episodes, eq-017* (cs-019): search listed the summary 8th of 10 and the reader did not read it; the answer invented model numbers.
- *L-replay-episodes, eq-019* (cs-044): a summary existed, no search or timeline call; the answer, from 2 recalled notes, missed the outbox fields and consumer idempotency.
- *L-replay-episodes, eq-020* (cs-048): the timeline listed the summary and the reader read it; the answer added details not in the gold (dates).

## 6. What the first run found, and what changed

### 6.1 The five diagnosed findings

Each finding in `l8/diagnosis/SUMMARY.json` was checked by two sceptics working from the reports, the logs, the kept databases (read-only) and the code at `50cf878d`; none was refuted, and each correction is folded in below. Commits are those 09's revision log names.

**(a) The forced re-run's unusable answer** (*rerun-badoutput*).
- *Found.* About one extraction answer in six was unusable to a parser that took only whole or fenced JSON (45 single-call flushes stopped on one, of 274, plus 4 of the 5 forced re-runs; the diagnosis first put it at one in five). An answer cut at the output ceiling was reported as "not JSON". The items budget assumed 180 output tokens an item. The harness's forced re-run made one attempt where production allows three (LR20). The sceptics sharpened it: failure is decided by the prompt, not by chance — identical prompts re-sent failed 6 of 6 times, and windows grown by one turn succeeded 3 of 3; only 7 of 46 unusable answers hit the ceiling.
- *Changed — the harness and diagnostics slice* (09 revs 3.26–3.29, `8fa9354d..c3979ba5`): **P1** (`8fa9354d`) a cut answer says it was cut; **P2** (`27aa0682`) each unusable answer is kept in memory for the report, never persisted; **P3** (`39974e27`, re-derived in `c4b21f6a`) an item is budgeted at 216 output tokens under a 2,892-token ceiling, from the line fitted to L8's 230 successful single-call flushes; **H2** (`b6aae718`, `98ba8d75`, `bfcca399`) the harness retries as production does — `switch`, `replay`, summary and forced re-run flushes at once, a `turns` window at the session's next flush — and a re-run whose window was skipped is unchecked, not passed.
- *Then the capture.* One replay of L's transcript at `6eb0fae8` recorded the unusable answers: 11 of 60 extraction calls, 9 of them "top level is not `{"items": [...]}"` and 2 "not JSON"; 3 windows skipped; the forced re-run checked (`capture/full-replay-capture.log`). 9 of the 11 were a bare array of valid items, 1 was followed by `</think>`, 1 came after prose (plan §0, LQ13).
- *Decided — LQ13:* tolerant unwrap. `<think>` blocks and prose around the answer are stripped, the one JSON value taken, a bare items array read as the items; every item still validated, a cut answer never rescued (`6120812b`; repairs `f4a1e5e6`, `04df5bce`).
- *Final:* over the seven final runs that extract (L0 makes no call), **21 unusable answers of 357 extraction calls**, counted per attempt: 15 cut at the 2,892-token ceiling, 3 "the answer is not JSON" (prose that settles on empty items and gives no JSON) and 3 "the answer's top level is not `{"items": [...]}`" (all L's cs-042, each the answer `{}`); 3 windows skipped after three unusable answers (L cs-042; L-episodes cs-020, cs-036), 0 left unread; the forced re-run checked in 6 of 6 (6 calls, no unusable answer). Over the same six kinds of run as the first, 20 of 303. Summaries apart: 9 unusable of 149 summary calls, all "not one JSON object", and 3 summary windows skipped (L cs-003, cs-016; L-replay-episodes cs-016). Not like for like with the first run's 45 stops over 282 calls, which counted flushes stopped at their one answer with no retry (§4.7). What is left is mostly the ceiling: 15 of the 21 answers were cut there, against 7 of the first run's 46.

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

| | L8 (`4aab3afe`) | Before the layout (`af7a1414`) | The layout (`457c1122`) | Final (`ad1ca07a`) |
|---|---|---|---|---|
| Index 10K: cache MiB / p95 ms / RSS MB | 109.2 / 11.45 / 244 | 109.2 / 10.08 / 247 | **40.2 / 8.73 / 142** | 40.2 / 11.67 / 142 |
| Index 100K average: cache MiB / p95 ms / RSS MB | 1,065.5 / 150.34 / 692 | 1,065.5 / 154.04 / 691 | **366.6 / 48.56 / 685** | 366.6 / 49.19 / 685 |
| Index 100K average: rebuild s | 46.1 | 53.9 | **15.2** | 15.8 |
| Index 100K learned: cache MiB / p95 ms | 531.8 / 56.58 | 531.7 / 56.41 | **219.7 / 27.39** | 219.7 / 29.05 |
| Service 10K: recall p50 / p95 ms | 48.4 / 59.0 | 28.6 / 33.7 | **27.0 / 29.8** | 30.4 / 33.7 |
| Service 10K: first tick; rebuild | 3.6 s; 2.9 s | 40 ms; 3.1 s | 40 ms; **1.1 s** | 40 ms; 1.2 s |
| Service 100K: recall p50 / p95 ms | 556.0 / 666.2 | 342.3 / 397.6 | **242.4 / 264.5** | 243.6 / 261.3; again 240.3 / 258.9 |
| Service 100K: `learning_view` p95 ms | 9,127.8 | 487.7 | **353.7** | 397.4; again 371.3 |
| Service 100K: store (learned) p95 ms | 3,515.7 | 3,223.0 | 3,181.5 | 3,144.7; again 3,229.9 |
| Service 100K: first tick; rebuild | 46.2 s; 33.0 s | 3.2 s; 40.9 s | 3.1 s; **16.7 s** | 3.1 s; 17.0 s; again 3.1 s; 16.8 s |
| Service 100K: peak RSS MB (footprint) | 2,381 (1,240) | 1,955 (998); again 1,666 (771) | **2,157 (1,193); again 2,145 (1,197)** | 2,127 (1,049); again 2,135 (1,165) |

Figures from `l8/bench/`, `layout/bench-base/`, `layout/bench-after/`, `layout/impl.md` and, for the Final column, `final-gates/bench/` (`*.log`, `*.time`, `load-before.txt`, `machine.txt`, and the load at each bench's start in `final-gates/progress.txt`). The Final column was measured at `ad1ca07a`, the branch head after the re-measurement: its code is `82180b84`'s plus the close-out's third repair round (the forget path) and the two 30K bench cases of §7.4, which change nothing a bench times but a forget. Release, one binary at a time under `/usr/bin/time -l`, after the test gates had finished, on 2026-09-28 from 23:06 to 23:35 UTC, under a load of 4.0–5.7 (one core held by a game client the owner left running, and the review agents of this report active during the first six minutes); the two 100K service runs are back to back. Since the scale slice (S6) the daemon opens with no index at once and rebuilds it in the background (1.4–1.9 ms on the final build, then the rebuild), so the rebuild is off the request path. The layout's two repair rounds move the index benches within their runs' noise (09 §6.5.5), and so does the final build: its 100K index figures are within 2 ms of the layout's, its 30K p95 (20.52 ms) is beside the repaired build's 19.4–20.3 ms, and its 10K index p95 (11.67 against 8.73 ms) matches the repaired build's 11.4–11.5 ms (09 rev 3.39); its service figures are within the two back-to-back 100K runs' own spread (85 ms apart on a learned store's p95, 26 ms on `learning_view`'s).

- **§6.4's decision rule is met** after the layout: index search + verification p95 at 100K is 48.6 ms (47.2 ms on the prototype), 27.4 ms on the learned mix; at the design point, 30K, 16.6 ms against the old layout's 29.4 ms (prototype, 09 §6.5.5). On the final build: 49.2 ms at 100K, 29.1 ms on the learned mix and **20.5 ms at 30K**, measured by a bench case of its own (§7.4). That holds for the bench's questions of 1–11 words. **A whole message**, which is what the per-turn recall searches (up to 512 characters and 32 terms), takes search alone median / p95 86.6 / 103.8 ms at 30K and 273.5 / 320.6 ms at 100K on the build — half the old layout's, still about three times 100 ms at 100K (09 §6.5.5, the second review's measurement; not re-measured on the final build).
- **The service's proposed targets are still missed at 100K:** recall p95 264.5 ms against 100 ms; a learned store 3,181.5 ms on the average mix (final build: 261.3 and 258.9 ms; 3,144.7 and 3,229.9 ms). At 10K both are under 100 ms: recall 29.8 ms, inside 04 §5's 50 ms too, and a learned store 86.0 ms (final: 33.7 and 87.2 ms). The realistic 10K case with one tombstone (scale slice S9's bench) reads recall p95 34.3 ms on the final build, the same as without one, where it was about 81 ms before S3.
- **One open finding: the 100K service bench's peak memory rose with the layout** — 1,955 and 1,666 MB before, 2,157 and 2,145 MB after, two runs each — while the index benches and the 10K service bench fell. The final build holds it: 2,127 and 2,135 MB (footprint 1,049 and 1,165 MB). Which phase holds it is not measured (09 rev 3.38).

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

- **Growth** (`bench/scale/analysis.md`). The first L8 runs learned 123–145 notes plus 19–20 summaries per 300 turns, 0.41–0.48 notes a turn on a fact-dense corpus; nothing prunes. The final build's learning runs learned 133–165 notes (0.44–0.55 a turn), and its two live runs wrote 46 and 47 summaries per 300 turns, once the harness lifted `episode_max_per_day` (§4.1, §5); the first run's 19–20 was the shipped daily cap of 20 binding on a corpus fed in one day. The yearly figures that follow rest on neither count: the analysis models summaries per day under that cap, and learned notes from an assumed everyday rate. Assuming everyday chat yields 0.10–0.15 notes a turn (0.45 at most): a regular user (20 turns a day) holds 2–4K notes after a year, a heavy one (50) 4–10K, a very heavy one (150) 13–32K. 100K is a multi-year or import scenario, hence LQ15's 30K.
- **The design point, measured.** Until the final build no bench ran the service at 30K, and its latency was interpolated. `ad1ca07a` adds a 30K case to each bench (`bench_search_30k_notes_with_verification`, `bench_service_30k_notes`; the average mix, run with the Final column of §7.2). The index: 112.7 MiB, built in 3.7 s, search + verification p50 / p95 7.54 / 20.52 ms, peak RSS 313 MB. The service, parse cache on: **recall p50 / p95 75.5 / 81.8 ms**, under §6.4's proposed 100 ms; `learning_view` p95 109.3 ms; a save from the app 182.6 ms and a learned store 189.6 ms, a note's edit 252.2 ms and a forget 356.4 ms (p95) — the write path is above 100 ms at the design point too; the first reconciliation tick 136 ms, a quiet one 44 ms; a missing cache's rebuild 4.2 s, off the request path; the parse cache holds 54.5 MiB; peak RSS 927 MB (footprint 555). With the parse cache off, recall p95 is 95.2 ms and a learned store 991.5 ms.
- **What remains for 100K:**
  - recall (264.5 ms against 100 ms): O5, mapping the whole cache file (estimated −20 to −25 % of search), and `prefix=`, both open levers (09 §6.5.5, §6.5.8);
  - the write path (3.2 s against 100 ms on the average mix): incremental partition state and an append-only control log (O15, O16), `pending` out of the snapshot (O13), a frontmatter-only parse cache (O14), management reads from the index (O6, O7), a debounced `MEMORY.md` (O9) — none built; each is the owner's (09 rev 3.30);
  - the cliff at the cap: a word held whole in more than 256 eligible bodies can lose every holder to longer forms; the ranked body query would close it at +34 ms of p95 at 100K, a measured trade-off left to the lead (09 §6.5.4);
  - the header relevance's length bias, which at 100K of the learned mix drops three identity questions (hq-0001, -0005, -0039) outside the 50 hits that the old layout kept within 20 (09 §6.5.5);
  - the peak-memory rise (§7.2), which the final build holds;
  - the learned mix, re-run on the final build for the first time since L8: recall p50 / p95 222.3 / 254.5 ms (L8: 438.7 / 484.2), a learned store 542.4 ms (735.7), `learning_view` 328.3 ms (9,268.1), the first tick 0.50 s (27.5 s) and the rebuild 13.1 s (21.7 s), peak RSS 2,011 MB (2,322) — the parse cache holds this partition (121.0 MiB), so its writes stay under a second where the average mix's take three.

## 8. Caveats and threats to validity

- **The reader grades itself.** One model answers, extracts and judges both QA and coverage (LQ1, `judge_is_reader: true`, `judge_is_extractor: true`). A judge that shares the reader's blind spots passes answers an independent judge would fail, and the coverage judge rates notes its own extractor wrote. The re-judge's κ 0.981 measures agreement between two builds of the same family (§3.3), not independence. The gate's comparison is at least like for like: B-mlx and D-index share the reader and the judge. The final gate pairs the final D-index with the first run's B-mlx, which also crosses the Ollama version (below).
- **One sample of the live system.** The reader's temperature is not pinned; each live run is one draw. QA was measured once per system. L and L-episodes are the only two live samples of the learning side, and they differ: pooled coverage 70 against 69, changed facts 11 against 13 (re-scored), the extraction path's other-script aliases 83.3 against 52.8 %. The final pair differs too: pooled coverage 77 and 77, changed facts 12 and 11, the extraction path's other-script aliases 62.5 against 48.0 %. One fact can move ten questions (the weekly-review change, §3.2).
- **Temperature 0 is not variance 0.** The extractor runs at temperature 0, and the same prompt mostly fails or succeeds the same way again (6 of 6 re-sent failures failed; the runbook's 13 of 13 same-prompt pairs). Yet the three replays of one transcript diverge — coverage 60–66 of 83, 4–7 stopped flushes — and the completeness review lists non-determinism at temperature 0 among its smaller issues; once one answer differs, every later known-notes section differs. The alias diagnosis's second sceptic shows it directly (`final/alias-drop/sceptic-2.md`): the first L-replay-1 and L-replay-2 sent the identical 2,552-token prompt for cs-001 with an empty store and answered 853 and 709 tokens, 5 notes (3 with a Chinese alias) against 4 (1); L-replay-2 and L-replay-3 were identical through their first flush, then sent the same 3,238-token prompt at cs-003 and answered 919 and 920 tokens with different notes; and only about 4–13 of 51 flushes match between any two replays (4–12 by the first sceptic's count, 5–13 by the second's). Three replays are three draws of a path that drifts, and a single session's outcome within them can flip whole (§4.8).
- **Run-to-run noise on the QA side.** A change of model build alone flipped 49 of 470 rows while moving B's total by 0.6 pp (§3.3). D-index and D-excerpts, one store and two recall modes, differ on 21 rows. The retrieval proxy repeats exactly but for tied rows (§7.3). A difference of about a point between two single runs is within that noise; the gate's CI (+4.0 pp at its lower bound) is not. Across builds the same holds (§3.4): D-index final against first is −0.2 pp with 15 discordant rows (and no answer text identical), D-excerpts −0.4 pp with 24, and L0 +1.7 pp, CI [−2.8, +6.2], with 116 discordant rows (24.7 %) — two samples of one condition whose QA path runs through the same changed index as D's, so their spread is run-to-run noise plus that build effect, not separable with one sample per build. L's +6.8 pp across builds (CI [+3.1, +10.5]) therefore mixes the build with a fresh learned-store sample of that variance, and is no measure of the build alone.
- **A synthetic corpus.** A fictional user, written by a design session from the gold notes, English and Chinese, dense in facts (0.41–0.48 learned notes a turn in the first run, 0.44–0.55 in the final) with 54 % noise turns. A learning run cannot backdate, so dates are stated in the text; wall-clock dating still costs L on time questions (a strict date match in the completeness review finds 58 L answers carrying the run's own dates, 9 of them wrong). D's gold notes carry curated aliases with the questions' vocabulary; learned notes do not, which costs L 16 of its 80 D-only rows (§6.1 d). The episode split's eq-017 describes a conversation the live reader did not have (§5).
- **The harness is not the daemon.** The queue runs in `Manual` mode with the triggers replayed; the idle trigger is not exercised; the retry pace differs from production's backoff (runbook). Fed turns are unattended, so the destructive `memory_update` is refused and `memory_confirm` never offered: L's kept database holds 24 `tool_approval_unavailable` rows for `memory_update`, L-episodes' 29 (*completeness*, sceptic 2), and the final L's and L-episodes' 22 and 24, all for `memory_update` (`event_log` of `final/L-scratch/eval.db` and of the final L-episodes' kept scratch, `conditions.learning.scratch_kept`, still in the system temp directory). How much that costs L0's source conflicts (7 of 40 in the first run, 20 of 40 in the final) is not measured. A replay flushes whole sessions and carries none of the reader's notes.
- **An Ollama upgrade between the runs.** Ollama moved from 0.34.2 to 0.34.4 on 2026-09-26 at 05:25 UTC (`final/alias-drop/diagnosis.md` §5; the model's manifest unchanged). By the chain times, the first runs up to D-excerpts ran on 0.34.2 (the diagnosis dates the first L, L-S, D-excerpts and the first replays; B-mlx, D-rejudged and D-index ran before them), the first L0 straddles the upgrade, and the first L-episodes, the capture replay and every final run ran on 0.34.4. So the first L against the first L-episodes is not a pure run-to-run pair, and every final-against-first comparison, the final gate against B-mlx among them, crosses the Ollama version as well as the build; no data here separates a version effect from run variance.
- **First-run reports and the fixed harness.** H3–H7 reclassify; H5 changes the coverage cap. A final figure compares with a first-run one only through the re-scored figures (§4), never as an improvement. Changed facts carry an offline bound on the first run (§4.4). Several measures changed meaning between the harnesses: *checked* for the forced re-run also demands no skipped window (§4.6); unusable answers are counted per attempt, not as flushes stopped, and flush latency per attempt (§4.7); a final summary's "unusable" is, in §5's session count, a window skipped after three unusable answers, not a stopped flush, and in §4.7's column the answers themselves; changed facts are judged at run time by H6's link check, which refused 4 links that the offline rule, crediting a link alone, would have counted (§4.4); the coverage judge's cap is 2,048, not 400; restatement classes are native H7, not re-scored; and the final replays replay the final L's transcript, not the first L's.
- **Three benches' conditions.** The L8 benches ran on a quiet machine, the layout's before-and-after pair under a load of 5–8 from two macOS daemons, and the final ones under 4.0–5.7, one core held by a game client and this report's review agents active in their first six minutes (§7.2); single runs each, but for the 100K service case, run twice in each of the last three columns.

## 9. Owner items, and how each closed

- **LQ5–LQ10** (09 §10), raised while building Learning v2: LQ5 learning drops a chunk whose project directory is gone; LQ6 a denied or timed-out card hides every later suggestion for the same note on the lane until restart; LQ7 L-S seeds four gold notes present-tense (the first run cannot separate it from the pre-emption; on the final build three of the four got a suggestion, all from the reader's own writes under LQ11, §4.5); LQ8 full UUIDs on the index lines; LQ9 which summaries automatic recall leaves out, and a summary's forget suppressing nothing; LQ10 a model's edit of a summary (its origin and its `fact_key`). **Taken 2026-09-28** (09 §10, rev 3.49): LQ5–LQ9 kept as shipped; LQ10 the narrow refusal — a non-user edit that sets or changes a `fact_key` on a conversation summary is `authority_conflict` (`dc9cce84`, `8c298f60`).
- **The 100K work** (§7.4): the service's recall and store targets, the options not built (O5, `prefix=`, O6, O7, O9, O13–O16), the cliff at the cap (the lead's trade-off) and the header's length bias. **Residue, recorded 2026-09-28:** no write-path target at 30K is set; the options not built stay unbuilt, and the cliff and the header's length bias stay recorded as measured.
- **The 100K service bench's peak-memory rise** (§7.2): unexplained. **Residue, recorded 2026-09-28:** dropped as not needed for distribution; it stays an open observation, not measured by phase.
- **The other-script alias target on the final build** (56.1 % against 60 %, §4.8): the cross-over replays put the gap between the replay sets on the transcript, through three sessions, and the build's extraction path *likely* not the cause; pass or fail at 60 % is not a property of the build these data settle (the live samples straddle it). Open: whether the final build's reader tends to write such transcripts (several live transcripts per build would say), and why cs-015, cs-024 and cs-048 flip with the transcript. Beside it, a reading question for the lead: the share credits a Chinese-titled note from an English session, which breaks the prompt's rule 8, and a per-note rule-5 reading moves with composition unless it is stratified by session (sceptic 2, correction 4). **Taken 2026-09-28 (LQ18):** reading A — a hit needs the title in its session's language (rule 8) and an alias in the other script (rule 5); the share stays over the learned active notes, by surface, not stratified; 60 % stays a proposed target, not a gate (`db2b13c0`). The archived reports are not re-scored: every alias figure above was read under the old title-script rule. Why the three sessions flip stays unanswered.
- **Raised by the final L-S, not an LQ at first:** a reader `memory_store` that names no saved note leaves a second active note beside it, and LQ11 has nothing to turn into a suggestion (the commute, cs-034, §4.5). **Taken 2026-09-28 (LQ19):** F-A — the extractor's known notes label a note only the chat model wrote `origin=assistant`, which does not make a fact known, so the item that replaces the saved note becomes its suggestion — and F-C, a `<memory_rules>` line telling the reader to name the note a changed fact replaces (`f4d8efbc`). No live run has measured either. Rule 9 treats the assistant's own note as known except when a saved note states an older value (narrowed after review, `15df7001`), so no general duplicate is expected; unmeasured live.
- **Raised by the diagnosis, not an LQ** (09 rev 3.26): whether the episode prompt records outcomes and skips facts already stored (eq-014); eq-017's gold; trimming the long episode golds (N9). **eq-017's gold is corrected** (`c6db079f`): it now says only what cs-019's own turns hold — the user asked which of two models saved more power and said to buy the first, naming none; §2.2 has the file's new digest. The episode prompt's salience (eq-014) and N9 are not decided in this batch.
- **The output ceiling, for the lead.** 15 of the final build's 21 unusable extraction answers were cut at the 2,892-token ceiling (8 of L-episodes' 9), against 7 of the first run's 46 (§6.1 a); three windows were skipped after three unusable answers. Whether P3's items budget, 216 output tokens an item from a line fitted to the first run's successful flushes, is too tight for the answers this model writes now is not measured. **Taken 2026-09-28 (LQ20):** (e) the retry of a cut answer asks for half the items and the JSON first; (f) a cut answer that ends on one complete value of the answer's shape after its reasoning is read — L-episodes' cs-010 answer above is one (`20c6d9df`). The ceiling stays 2,892 and no cap changes; whether (e) turns the deliberating cuts into answers is not measured.
- **For the lead:** whether prose around an extraction answer yields only its last value (09 rev 3.34, LQ13's repair round 1). Not decided in this batch; the reading is unchanged.
- **Unchanged from before L8:** the owner's review of the shipped defaults; slice 3's GUI round-trip on an isolated store and slice 4's packaged, network-denied verification (08). **LQ21** raises `[orchestrator.learning] episode_max_per_day`'s default from 20 to 100 (`84c46fd9`); no other default changed in this batch. The GUI round-trip and the packaged verification stay slice 4's (08), not done here.

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

**The QA and learning runs**, from a worktree at the build (`82180b84` for the final runs, `50cf878d` for the first), each build's reports, logs and stores in their own directory `$OUT` (`$M/final` for the final build, `$M/l8` for the first, as `final/run.sh` and `l8/run-chain.sh` kept them):

```sh
OUT=$M/final   # $M/l8 for the first build
git worktree add $M/wt $BUILD && cd $M/wt
export OPENALPACA_LIVE_EVAL=1 OPENALPACA_CONFIG_DIR=$M/config-mlx CARGO_TERM_COLOR=never
CMD=(cargo test -p openalpaca_core --features live-eval --test memory_eval -- --ignored memory_eval --nocapture)
run() { local name=$1; shift; rm -rf $OUT/store-$name; mkdir -p $OUT/store-$name
  env OPENALPACA_HOME_STORE=$OUT/store-$name OPENALPACA_MEMORY_EVAL_REPORT=$OUT/report-$name.json "$@" \
    $CMD > $OUT/full-$name.log 2>&1; }
run smoke-L    OPENALPACA_MEMORY_EVAL_SYSTEM=L OPENALPACA_MEMORY_EVAL_SESSIONS=2 OPENALPACA_MEMORY_EVAL_LIMIT=5
run D-index    OPENALPACA_MEMORY_EVAL_SYSTEM=D OPENALPACA_MEMORY_EVAL_RECALL_MODE=index
run D-excerpts OPENALPACA_MEMORY_EVAL_SYSTEM=D OPENALPACA_MEMORY_EVAL_RECALL_MODE=excerpts
run L          OPENALPACA_MEMORY_EVAL_SYSTEM=L
# copy L's kept scratch (conditions.learning.scratch_kept) out of the system temp directory first
SCR=$OUT/L-scratch
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
OPENALPACA_HOME_STORE=$M/l8/store-B-mlx OPENALPACA_MEMORY_EVAL_SYSTEM=B \
OPENALPACA_MEMORY_EVAL_REPORT=$M/l8/report-B-mlx.json \
  cargo test -p openalpaca_core --features live-eval-e5 --test memory_eval -- --ignored memory_eval --nocapture
```

**The re-judge** of 08's archived D, no reader call and no store:

```sh
OPENALPACA_MEMORY_EVAL_REJUDGE=$PWD/scripts/eval/memory/reports/system-D-prefix-2026-09-24.json \
OPENALPACA_MEMORY_EVAL_REPORT=$M/l8/report-D-rejudged-mlx.json \
  cargo test -p openalpaca_core --features live-eval --test memory_eval -- --ignored memory_eval --nocapture
```

**The comparisons** (candidate first; `$OUT` as above):

```sh
python3 scripts/eval/memory/compare.py $OUT/report-D-index.json $M/l8/report-B-mlx.json   # the gate
python3 scripts/eval/memory/compare.py $OUT/report-D-index.json $OUT/report-D-excerpts.json
python3 scripts/eval/memory/compare.py --margin=-10 $OUT/report-L.json $OUT/report-D-index.json
python3 scripts/eval/memory/compare.py $OUT/report-L.json       $OUT/report-L0.json
# the final build against the first, per system (candidate: the final report)
python3 scripts/eval/memory/compare.py $M/final/report-D-index.json $M/l8/report-D-index.json   # likewise D-excerpts, L, L0
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
- **draft 2 (2026-09-28):** the final-build figures filled from the re-measurement on `82180b84` (`final/`, every run exit 0), each read from `final/figures/` where a sceptic re-derived it, with the verifiers' corrections applied (L-replay-episodes' summaries counted per session, 46 / 0 / 4 / 1; only the `turns` attempt's cut tail on cs-035 shows the output-directory item): §1's final clauses; §2.3's final run times; §3.4's table, the per-category and per-language rows, the comparisons (with D-excerpts, L and L0 final against first added) and hq-0118's unparsed verdict at the 400-token QA judge cap (why 96.4 and 96.6 % differ); §4.1's final table; §4.2's final medians and a table of the final samples, L-replay-episodes beside them and out of the medians; §4.3's Final L column and the other final runs; §4.4's final per-pair table, H6 now judged at run time; §4.5's final column and the answer to its open question; §4.6's final rows; §4.7's final rows, counted per attempt, and the final L's skipped windows; §5's final column and misses; §6.1 (a)'s final line; the archive paths under `scripts/eval/memory/reports/l8-first/` and `l8-final/`. Added: §4.8, the final build's other-script alias share, its diagnosis and two sceptics; §8's final-against-first noise, the Ollama upgrade between the runs and the measures whose meaning changed between the harnesses; §9's alias item and the L-S commute. **Corrected first-run figures** (against 09 rev 3.46's ruling on which samples each median takes): §1's and §4.2's restatement median is 2 of 15, the median over the replays (2, 1, 2), not 1.5, which mixed in L's live 0 — still met; the other-script alias median is 70.0 %, not 70.5 %, now counting L-episodes' live extraction (52.8 %) — still met; L-replay-3's restatement cell reads "2 duplicates (and 1 automatic replacement)"; the first-run coverage cell drops its "(72.9 % with L's 12)" reading, since coverage's median is over the replays alone. **After two reviewers' findings** (fidelity and consistency, each checked against the figures, the verifiers, the alias sceptics or the raw reports): §4.8's centred decomposition is the first sceptic's, over the pooled −15.3 pp, with its within-session term (−5.6) and what leads the composition term, and cs-033 is silent in two of three final replays; shared sessions yield more English-titled notes as well as fewer; §1's alias bullet puts *likely* and *possible* on the right claims and names changed note counts; §1 and §2.1 say which reports record `judge_is_reader`; §1 carries L-S's targets by surface and the retired-note caveat; §3.4 names `seeded_ids` and L0's category moves across builds; §4.5's LQ11 sentence is limited to its three cases; §4.6 says what the report shows of 7ad12903; §4.7 and §8 say which table counts summary answers and which windows; §4.8 cites the second sceptic's call for more than three replays a cell; §7.4 and §8 give the final build's learning rate and summary counts, and §7.4 that its yearly figures rest on neither run's summary count; §8 gives both sceptics' flush-match counts, the final L0's source conflicts and the final L's and L-episodes' `memory_update` refusals, and marks the 0.34.2 dating of B-mlx, D-rejudged and D-index as inferred from the chain times; the Sources paragraph says the archive is still to be made; §10's recipe keeps each build's reports in its own directory. The bench column of §7.2 and a 30K service bench (§7.4) remain placeholders for the lead.
- **draft 3 (2026-09-28):** the benches on the final build, measured at the branch head `ad1ca07a` after the test gates (§7.2's Final column, its conditions and load; the 10K case with one tombstone); the design point measured for the first time, with a 30K case added to each bench (`ad1ca07a`, §7.4); the 100K learned mix re-run for the first time since L8 (§7.4); §1's scale bullet; the archive made (`scripts/eval/memory/reports/l8-first/`, `l8-final/`, the `compare-*.txt` outputs regenerated from the archived copies and matching the scratchpad's); the Sources list names `final-gates/` and `crossover/`. The cross-over replays §4.8 names are running; their result is the next revision.
- **rev 1 (2026-09-29), final:** the cross-over replays of §4.8 — the first L's transcript on `82180b84` and the final L's on `6eb0fae8`, three runs each, all exit 0 — with their analysis and its sceptic (`crossover/analysis/`): the gap between the replay sets is the transcript's, through three sessions (established), the build's extraction path is likely not the cause, and pass or fail at 60 % is not a property of the build these data settle; §4.8's table and findings, §1's alias bullet and §9's item rewritten from it, the Sources list names `crossover/analysis/` and the archive `scripts/eval/memory/reports/l8-crossover/`; the status is final.
- **rev 2 (2026-09-29):** §9's items closed or recorded, as the owner took them on 2026-09-28 (09 §10, rev 3.49): LQ5–LQ10, LQ18 (the alias share's reading A), LQ19 (the commute), LQ20 (the output ceiling) and LQ21 (100 summaries a day), each with its commit; the 100K work and the peak-memory rise recorded as residue; eq-017's gold corrected and §2.2 given the episode file's new digest beside the one every run used. No figure changes, and no archived report is re-scored.
