# 12 — Continuity evaluation (design 11 §10, slice 7)

**Status: rev 2.1 (2026-10-02): a note at the top of §5 — after these figures the owner took CQ-11 (2026-09-30), which restores the episode note's own summariser call ([11](11-continuity-design.md) rev 1.9, §3.4); no new measurement. Rev 2 (2026-09-29; the re-runs ran 2026-09-30 01:15–05:51 UTC) — the report of design [11](11-continuity-design.md)'s slice 7: the continuity split (§10.1) run as K0 and K, the episode split re-run on L-episodes (§10.3, CQ-10's gate), and the summariser's calls by label (§10.4); §10.2's compaction invariant runs offline in the default suite (slice 3) and is not repeated here. Rev 1 ran all three at `78d5c013` / `63a9c915` on the branch `track-b`; rev 2 re-ran K and L-episodes after the lead's fix round F1–F6 (`0cc26c11..db256d2b`, design 11 rev 1.7), at `63490c5c` on `feat/markdown-memory` (unpushed); K0 was not re-run (the lead's ruling) and K is paired against rev 1's K0.** One run of each system, on the local test model. Every number below is a measurement with its conditions and the file it was read from. **Every verdict here is the reader grading itself** (§2.1, §8). **None of the three targets is met in rev 2: K answered 63.3 % of the 60 (target 80 %; rev 1 66.7 %), 10.0 pp above K0 (target +30 pp; rev 1 +13.3 pp), and the episode split answered 11 of 20 (CQ-10's gate is 17; rev 1 14).** Nothing in the design or the code is revised here: the lead decides (§7).

**Sources.** The archived reports, redacted as the L8 archive is (hostname `owner-macbook-pro`; paths `<scratch>`, `<repo>`, `<tmp>`, `<home>`), are in `scripts/eval/memory/reports/continuity/`. Rev 2: `K-rev2.json`, `L-episodes-rev2.json`, `compare-K-rev2-vs-K0.txt`. Rev 1: `K.json`, `K0.json`, `L-episodes.json`, `compare-K-vs-K0.txt`, and `K-stopped-fed-inheriting.json` — the first K run, stopped after 9 of 24 sets for the harness defect of §6.1, kept as its evidence and not reported as K. The rest lives in the session scratchpad: rev 1's in `program/track-b/eval/` (`run.sh` and `run-K.sh`, `logs/*.log`, the unredacted reports, `bin/`, `figures.py`, `redact.py`); rev 2's in `program/track-b/eval2/` (`run.sh`, `logs/K.log` and `logs/L-episodes.log` with their `.exit` files, `reports/` unredacted, `bin/` with the binary's SHA-256 and commit, `figures2.py` for every K figure below and the paired CIs, `episodes_misses.py` and `chain.py` for §5 and §6.1, `redact.py`, rev 1's).

## 1. Summary

| Target (design 11 §10) | Rev 1 | **Rev 2 (measured)** | Verdict |
|---|---|---|---|
| **K ≥ 80 %** over the 60 answers | 40 / 60 = 66.7 % — doing 17/20, decided 19/20, next 4/20; abstentions 4/4 | **38 / 60 = 63.3 %** — doing 16/20, decided **20/20**, **next 2/20**; the 4 abstentions 4/4 besides | **not met** |
| **K ≥ K0 + 30 pp** | K0 32 / 60 = 53.3 %: +13.3 pp, paired 95 % CI [−0.1, +26.8] | against the same K0 (rev 1's, not re-run): **+10.0 pp, paired 95 % CI [−2.8, +22.8]** (11 answers only K got right, 5 only K0) | **not met** |
| **Episode split ≥ 17 / 20** on L-episodes (CQ-10) | 14 / 20 (70 %) | **11 / 20 (55 %)** — en 8/13, zh 2/5, mixed 1/2; 47 summaries projected, every close ran | **not met** |
| `carry_over_delivered`, `state_fresh` | K: 64 / 64 carried, 24 / 24 fresh | K: 64 / 64 rows carried the fed conversation's block, on every turn 1–3; 24 / 24 states fresh | as designed |
| `summariser_calls` (§10.4) | one label, `orchestrator_summary`: K 24, L-episodes 52 | one label only, `orchestrator_summary`: K 24 calls for 24 sets (one close each, no unusable answer); L-episodes 51 for 51 sessions (no unusable answer) | as designed |

**What the numbers say.**
- **The fix round did not move K.** Paired against rev 1's K on the same 60 questions, rev 2 is −3.3 pp, CI [−11.3, +4.6] (2 answers only rev 2 got right, 4 only rev 1) — inside one run's variance (§8). `doing` + `decided` is 36 / 40 in both runs (rev 2: 90 %, K0 70 %, +20.0 pp, CI [+4.2, +35.8]); **`next` fell from 4 to 2 of 20**.
- **F1 moved the assistant's offers out of `next` and into `open`, and rule 7 did not take** (§3.2): a wait on the user is `next` in 1 of the 24 states (rev 1: 10, or 14 counting the waits on an approval), `next` is empty in 10 (rev 1: 4) — and the daemon's conditions (a refused approval, the workflow cap, the Memory view's unconfirmed suggestions) are in `next` in 8 states and in `next` or `open` in 11 (rev 1: 6 and 7). Asked for the next step, the reader answers what `next` or `open` holds: of the 18 `next` misses, **12 are the writer's** (8 a daemon condition, 3 the assistant's questions worded as `next`, 1 the user's own stated step left out), 5 the golds' (the six absence golds all fail, and several golds name a question the live assistant had answered), 1 the judge's.
- **The episode split fell from 14 to 11** (§5), and **the chain of §6.1 is the largest cause**: the writer, now handed the previous conversation's state labelled as carried (F2), copied it into the new conversation's state — **12 of the 47 summaries share items verbatim with the summary of the conversation before them on the same project (rev 1: 0)**, five chains run three conversations deep, and in 5 of the 9 misses the conversation's own work had been crowded out of its summary by the carried one's. Of the 9: writer 6 (5 the chain), gold 2, judge 1.
- **F3's `asked` answered eq-002** (the side questions of the SIGILL conversation, rev 1's projection miss) and is in 20 of K's 24 states; with it, **39 of the 47 summaries reach the note's 1,024-byte cut** (rev 1: 10), 14 lose their *Decided* line and 20 their *Left open* (rev 1: 0 and 4) — §6.5.
- **The carry-over stays cheaper than searching**: K's first question turn read 11,734 input tokens on average (median 10,064) with 1.58 model calls (rev 1: 11,117 / 10,066 / 1.54); K0's read 25,075 (median 26,520) with 3.1 (§4).

## 2. Setup

### 2.1 Model, machine and isolation

- **Model.** `qwen3.8:27b-mlx` (Ollama 0.34.4; family `qwen3_5`, 27.8B, `nvfp4`, 262,144-token context) is the reader, the working-state writer (no `summary_model` is set, so the close's refresh runs on the router's default), the extractor (L-episodes) and the judge — **every verdict here is the reader grading itself** (`judge_is_reader: true` in every report, LQ1). The reader runs at the model's default temperature (`reader_temperature: null`); the writer and the judge at 0.0, the judge within 400 output tokens.
- **Configuration.** `eval/config/llm.toml` (rev 1) and `eval2/config/llm.toml` (rev 2), each a byte copy of L8's `memeval/config-mlx/llm.toml` (SHA-256 `d7cf4d8021f45b52…`, recorded in every report): only Ollama enabled, `fallback_models = []`, a 900 s request timeout, the router's rate limits lifted for the keyless slot.
- **Machine.** Apple M5 Max, 128 GiB, 18 threads, macOS 26.6.2 (25G83) — L8's.
- **Builds.** Rev 1: K0 and L-episodes ran the harness commit `78d5c013`, K the fix `63a9c915` (§6.1; for K0 the two are the same run — with `carry_over_turns = 0` no conversation is given a block, whatever it inherits). Rev 2: K and L-episodes ran `63490c5c`, the fix round's head (its last two commits are the records — two Rust doc comments among them — so the code is the F6 commit `db256d2b`'s). Each was built by the repository's pinned toolchain, rustc 1.93.0 (`254b5960`), as a debug test binary (`cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run`) copied out of the tree and run directly, so that nothing built meanwhile could replace it — rev 1's to `eval/bin/` (`memory_eval` SHA-256 `cf02a2f5aa4a19ff…`, `memory_eval-K` `de6643d86403ca7b…`), rev 2's to `eval2/bin/memory_eval` (`bb3e811c785eb7c9…`; `built-at-commit.txt` names `63490c5c`). Every report's `build.git_head` names its commit, and every report's `machine.rustc` says 1.98.0: the harness asks `rustc --version` in the run's working directory, which is outside the repository and so outside `rust-toolchain.toml`; the binaries are 1.93.0's.
- **Isolation.** Every run's working directory, `OPENALPACA_HOME_STORE` (`eval/store-<run>`, `eval2/store-<run>`), `OPENALPACA_CONFIG_DIR` (`eval/config`, `eval2/config`) and `TMPDIR` (`eval/tmp`, `eval2/tmp`) lie in the session scratchpad's `program/track-b/`, outside the repository; `~/.openalpaca` was never read or written. No daemon and no CLI ran.
- **Concurrency.** Rev 1's three ran **at the same time** against the one Ollama — K0 and L-episodes from 12:48 UTC, K (restarted) from 14:19; rev 2's two from 01:15 UTC, K to 04:11 and L-episodes alone after it. Each waited on the others' calls, so **no latency here is comparable with any other report**; accuracy, tokens and calls are not affected.

| Run | Build | UTC | Report |
|---|---|---|---|
| smoke-K (one set; not reported) | `78d5c013` | 2026-09-29 12:45–12:47 | `eval/reports/report-smoke-K.json` |
| K, first run (stopped at 9 sets, §6.1) | `78d5c013` | 2026-09-29 12:48–14:17 | `K-stopped-fed-inheriting.json` |
| **K0** (rev 1; not re-run) | `78d5c013` | 2026-09-29 12:48–17:29 | `K0.json` |
| K (rev 1) | `63a9c915` | 2026-09-29 14:19–18:25 | `K.json` |
| L-episodes (rev 1) | `78d5c013` | 2026-09-29 12:48–19:03 | `L-episodes.json` |
| **K (rev 2)** | `63490c5c` | 2026-09-30 01:15–04:11 | `K-rev2.json` |
| **L-episodes (rev 2)** | `63490c5c` | 2026-09-30 01:15–05:51 | `L-episodes-rev2.json` |

### 2.2 The continuity split

`scripts/eval/memory/questions-continuity.json` (SHA-256 `1bc5843502d93738…`, the same file in both revisions): **24 sets** over the 24 conversations of the learning corpus with at least 6 user turns — every such conversation. The other data files are L8's, byte for byte, but for `questions-episodes.json` (§5).

- **20 sets of three questions** — `doing`, `decided`, `next`, asked in that order as the next conversation's turns 1, 2 and 3 — in the conversation's language (15 en, 4 zh, 1 mixed: 45, 12 and 3 of the 60 answers). `doing` always asks what we were working on with no topic named; `decided` and `next` name a topic where the conversation held more than one thread.
- **4 sets of one question** about work the user turned down before the split (cs-024's dark-mode toggle, cs-030's retry around a flaky test, cs-033's hand-written template engine, cs-044's Kafka; 3 en, 1 zh), `expect_abstain`: the gold says nothing is pending on it, and the question is the next conversation's first turn — as a fourth turn of a set it would fall past `carry_over_turns` and never be given the block.
- **Splits** leave at least 3 turns on each side (3 to 9 fed turns, 105 in all; 12 sets feed 4 or more, so their close projects a summary). Every gold is written from the corpus's user turns before the split; the corpus holds no assistant turn, so no gold depends on what the live reader answered — and none accounts for it either (§3.2).
- **Six `next` golds say nothing was pending** (kq-002, -003, -006, -007, -008, -009): five conversations that only told the assistant about the user before their split, and one whose problem was solved. They are answers, not abstentions: "nothing was left open" is correct, "I don't know" is not.
- **Validated in default CI** (`memory_eval_corpus.rs`, `the_continuity_split_is_well_formed`): the counts, kinds and order, the sides, the language, a rejection before an abstention set's split, no id another split uses, no question or gold sharing an 8-word or 12-CJK-character shingle with a turn after its split.
- **Written, then smoke-run once.** The one-set run of K before rev 1's full runs showed kq-001's `decided` gold, "nothing had been decided yet", failing against a state whose `decisions` held the assistant's recommendations for the question the user had asked; for a conversation that decided nothing, the question now asks what it established (which team owns Lantern). No other question changed after a model had seen it, and none changed for rev 2.

### 2.3 The systems

| Id | What |
|---|---|
| **K** | For each set, in file (story) order, on one owner's one lane: a conversation opened as "Start fresh" opens one (`inherit: false`; on the scratch project when the corpus session has a project, on none otherwise), its turns `[0, split)` fed through the gateway and answered by the reader, closed by the learning queue's `flush_episodes` (the working state's refresh on the writer, then the summary projected from it at 4 user turns or more), then the next conversation opened on the same lane and project as "New chat" opens one, and the set's questions asked as its first turns and judged. Memory as D runs it (recall, the read tools, the main loop's writers), nothing seeded; `carry_over_turns` = 3 as shipped. No learning flush runs; learning is enabled for the close, and the summariser's ceiling lifted. |
| **K0** | K with `carry_over_turns = 0`: no conversation is given the block. |
| **L-episodes** | L (design 09 §7.2) — the 51 corpus sessions fed with learning on, each opened as "New chat" opens one (inheriting, so its first three turns are given the previous conversation's block and, since F2, its close's writer reads that conversation's state as carried), flushed after every 10th user turn and at each end, each end closed — then the 20 questions of `questions-episodes.json`, learning off. |

K0 is **memory without the carry-over**, not nothing: its reader can find a closed conversation through the summary its close projected (12 sets) or through its own `memory_store` notes of the fed turns (its fed turns stored notes in 22 of 24 sets, rev 1's K in 23, rev 2's K in 23), and it used a memory tool on 45 of its 64 turns. A no-memory floor would be lower. K does not exercise F2: its fed conversations start fresh, so no close in K reads anything carried (§6.1).

## 3. Continuity results

### 3.1 By kind

| | K rev 2 | K rev 1 | K0 (rev 1) | K rev 2 − K0 (paired, 95 % CI) | K rev 2 − K rev 1 (paired) |
|---|---|---|---|---|---|
| **the 60 answers** | **38 (63.3 %)** | 40 (66.7 %) | 32 (53.3 %) | **+10.0 pp [−2.8, +22.8]** | −3.3 pp [−11.3, +4.6] |
| doing (20) | 16 (80 %) | 17 (85 %) | 11 (55 %) | +25.0 pp [−2.3, +52.3] | −5.0 pp [−14.6, +4.6] |
| decided (20) | 20 (100 %) | 19 (95 %) | 17 (85 %) | +15.0 pp [−0.6, +30.6] | +5.0 pp [−4.6, +14.6] |
| next (20) | 2 (10 %) | 4 (20 %) | 4 (20 %) | −10.0 pp [−29.1, +9.1] | −10.0 pp [−29.1, +9.1] |
| doing + decided (40) | 36 (90 %) | 36 (90 %) | 28 (70 %) | +20.0 pp [+4.2, +35.8] | +0.0 pp [−6.9, +6.9] |
| the 54 without an absence gold | 38 (70.4 %) | 39 (72.2 %) | 30 (55.6 %) | +14.8 pp [+1.8, +27.8] | — |
| the 6 absence golds | 0 | 1 | 2 | — | — |
| abstentions (4, apart) | 4 | 4 | 4 | — | — |
| by language, the 60: en / zh / mixed | 27/45, 9/12, 2/3 | 26/45, 11/12, 3/3 | 24/45, 7/12, 1/3 | — | — |

The CIs are `compare.py`'s paired interval (SE = √(b + c − (b − c)²/n) / n over the discordant pairs), over the kind's rows (`figures2.py`, which reproduces rev 1's intervals from rev 1's reports). `compare.py` over all 64 paired rows (the abstentions are concordant): K rev 2 65.6 %, K0 56.2 %, +9.4 pp, CI [−2.7, +21.4], 16 discordant pairs (11 K only, 5 K0 only) — `compare-K-rev2-vs-K0.txt`; its gate line is the held-out set's non-inferiority margin and does not apply here. K only: kq-001, -005, -006, -007, -013, -018, -024 `doing`; kq-016, -023, -024 `decided`; kq-023 `next`. K0 only: kq-011, -012 `doing`; kq-003, -007, -021 `next`. Against rev 1's K, rev 2 alone answered kq-002 `decided` and kq-023 `next`, rev 1 alone kq-011 `doing` and kq-008, -010, -018 `next`. No row was unparsed, no judge call failed, no turn errored, in either revision.

### 3.2 `next`

The fix round's F1 changed what the state says `next` is (the work's step as the user would state it; an offer or a clarifying question of the assistant's goes in `open`) and added rule 7 (a refused tool call, an approval not given, a limit the assistant met: leave it out). The 24 states after the close:

| `next` in the state after the close (each state counted once, in the first row that fits) | rev 1 | rev 2 |
|---|---|---|
| the daemon's condition — a refused approval, the workflow cap — as the step, alone or beside a question | 6 (kq-001, -009, -014, -017, -021, -024) | **8** (kq-001, -005, -006, -009, -010, -014, -020, -024) |
| a wait on the user — the assistant's offer or question as the step ("Wait for user to confirm…", "Await user decision…", "User to choose…") | 10 | 1 (kq-013) |
| the assistant's clarifying questions, worded as the user's step ("Provide…", "Confirm…", "Get … from user") | 3 (kq-005, -015, -020) | 3 (kq-012, -015, -018) |
| the work's step | 1 (kq-018: "complete the Loki query", from the cut of §6.3) | 2 (kq-017, -021) |
| empty | 4 | **10** |

(Rev 1's §6.2 counted 14 waits: its 10 here and 4 of the daemon row — kq-014, -017, -021, -024 — worded as waits.)

The offers moved from `next` to `open`, as F1 says. Rule 7 was not followed: a daemon condition is in `next` or `open` in **11** of rev 2's states (kq-001, -005, -006, -007, -009, -010, -014, -015, -017, -020, -024 — "pending approval in GUI or interactive session", "workflow slot limit", "User has not confirmed the extracted suggestions in the app's Memory view"), against 7 in rev 1's. And the reader, asked what was next or whether anything was left, answers from `open` when `next` is empty. `next` answered **2 of 20** (kq-013, -023; rev 1 4). The 18 misses, one line each:

- *kq-001* (cs-001, **writer**): `next` and `open` are the refused note update ("Retry Lantern note update from GUI or interactive openalpaca chat session…"); the reader answered it. The gold's question (sharing a config struct between axum handlers) is in the state's `asked`, answered.
- *kq-002* (cs-003, **gold**): `next` is empty and `open` the assistant's offer to record the fix commands, which the user did not take up — where F1 puts it; the reader called it open. The gold (from the user's turns alone) says nothing was open.
- *kq-003* (cs-005, **gold**): the same shape — `open` holds the assistant's offer to dig into Lantern; the absence gold says nothing was left. (K0 answered it.)
- *kq-004* (cs-006, **writer**): the user's last fed turn was "OK, I'll scope the guard in a block instead" — the step the gold names; the writer filed it under `decisions`, left `next` empty and put the assistant's two offers in `open`, and the reader offered those.
- *kq-005* (cs-008, **writer**): `next` is "Provide budget and in-person/shipped details, then retry planning workflow when slot frees up" — the assistant's questions and the workflow cap; the reader answered it, the gold is ideas for the birthday the user had started planning.
- *kq-006* (cs-009, **writer**): `next` is to confirm in the desktop GUI so the project profile is saved — the refused write; the absence gold says nothing was started.
- *kq-007* (cs-010, **writer**): `next` is empty and `open` is "User has not confirmed the extracted suggestions in the app's Memory view" — the daemon's state of four notes, not the conversation's work; the absence gold says nothing was pending.
- *kq-008* (cs-011, **gold**): `open` holds the assistant's two offers (merge the drink notes, tidy the other preferences); the absence gold says nothing was started. (Rev 1's K answered it.)
- *kq-009* (cs-012, **writer**): `next` is "Re-issue pending preferences in GUI or interactive chat to approve and save"; the absence gold says the user had only been listing conventions.
- *kq-010* (cs-013, **writer**): `next` is to approve the Moss note's update in the GUI; the gold's step (how markdown-it-py plugins register, wiring the footnote plugin in) is in `asked`, answered. (Rev 1's K answered it.)
- *kq-011* (cs-014, **gold**): the gold's step — a pytest fixture for a temporary content directory — is the user's last question before the split, which the live assistant answered (`tmp_path`; the state's `asked` and `done`); `next` is empty and the reader offered the assistant's three offers from `open`.
- *kq-012* (cs-015, **writer**): `next` is the assistant's clarifying questions ("Confirm the meaning of second Wednesday and fill in PR description details"); the user's last fed turn opened the release checklist — the gold — which the state does not record as work at all.
- *kq-015* (cs-026, **writer**): `next` is the assistant's questions ("Provide real region values and database dialect to finalize migration SQL"); the gold's backfill is in `asked` ("How to backfill existing rows in the same migration: Option A…"), and the reader made it a detail of finalising the SQL.
- *kq-016* (cs-027, **gold**): the gold's step, the nightly backup script, was written by the live assistant in the fed turns (`done`); the reader offered to finish the cron scheduling, which the state's `open` says "were truncated in the assistant's response" — they were not (§6.3).
- *kq-018* (cs-032, **writer**): `next` is the assistant's questions about the key-rotation checklist; the gold's Loki query by `request_id` is in `asked`, answered. (Rev 1's K answered it — from rev 1's cut claim, §6.3.)
- *kq-020* (cs-035, **writer**): `next` is "Retry the workflow to write the README sections or apply the inline drafts manually" — the workflow cap; the reader answered with the three drafted sections, the gold is the Contributing note.
- *kq-021* (cs-038, **judge**): `next` is the work's step ("Save the provided pre-push hook to .githooks/pre-push…"), and the reader put it first of three; the judge failed it for the other two, which rule 1 of the frozen prompt says do not hurt.
- *kq-024* (cs-050, **writer**): `next` is "Provide repository, branch location, and PR details, or enable workflow capacity to proceed", which the reader quoted; the gold is writing the fee column's migration.

**By cause: writer 12, gold 5, judge 1.** Of the writer's 12, a daemon condition (rule 7 not followed) is the answer in 8 (kq-001, -005, -006, -007, -009, -010, -020, -024), the assistant's questions worded as `next` in 3 (kq-012, -015, -018), and the user's own stated step left out of `next` in 1 (kq-004). Five of the golds' steps (kq-001, -010, -011, -015, -018) are the user's last question before the split, which the live assistant answered and F3 now keeps under `asked`; the reader, asked for the next step, did not name them. The six absence golds all failed in rev 2 (rev 1's K answered 1, K0 2).

K0's 4 of 20 are the same size for another reason: it seldom found the conversation at all (§3.3), and when it did it found the notes, not the next step.

### 3.3 `doing` and `decided`

- **K0's misses are the wrong conversation** (rev 1, unchanged). In 7 of its 9 `doing` misses K0's reader described another set's conversation — the Lantern background, the profile set-up, the retry policy — picked from the day's notes (kq-005, -006, -007, -013, -016, -018, -024); in kq-001 it found Lantern and not the question; in kq-021 it named the right thread among others and the judge failed the extra one. Its `decided` misses: kq-016 and kq-023, "no record of that" (its searches found no note of it); kq-024, another set's column.
- **K rev 2's `doing` misses are the judge's, 4** — three of them rev 1's as well:
  - *kq-011* (cs-014, **judge**): the answer is the conversation's pytest set-up — the dev dependency, one test file, `capture`, `tmp_path` — under the state's topic "uv, pytest, and ruff workflow"; the judge read "learning uv, pytest and ruff workflows" as another fact than "setting up pytest for Moss's tests" (the state's goal does not name Moss). Rev 1's K answered it.
  - *kq-012* (cs-015, **judge**): the answer lists the drafted PR description among the conversation's threads, as done and as open placeholders; the judge failed it for the other threads.
  - *kq-016* (cs-027, **judge**): the nightly backup script is the third of three threads; failed for the other two.
  - *kq-021* (cs-038, **judge**): the pre-push hook is the first open thread; failed for the other threads and for not naming `just run` among the hook's steps (the state's `decisions` names the four CI gates; `asked` has the smoke test's hook).
- **`decided` is 20 of 20** (rev 1 19; kq-002, rev 1's one miss, answered).
- **Carry-over and its turns.** K's block reached turns 1, 2 and 3 of every three-question set and turn 1 of every abstention set (24 / 20 / 20); K0's never. On its first turns K's reader opened the fed conversation's summary with `memory_read` in 9 of the 12 sets that had one (rev 1: 9); in the 12 sets without one (3 fed turns), it called `memory_timeline` in 1 (kq-024; rev 1: 2, kq-001 and -010) — the heading's timeline hint is conditional since F6 — read a note the fed turns stored in 1 (kq-001) and searched in 1 (kq-013). It used a memory tool on 16 of its 64 turns in all (rev 1: 17; K0: 45).

### 3.4 Abstentions

4 of 4 in every run, and for the same reason: the fed turn's reader had stored the rejection as a note, and the per-turn recall delivered it on the question's own words ("dark-mode toggle", "retry", "Kafka"); in rev 2 all four answered with no tool call, from the recalled note and the state (whose `decisions` held the rejection in each). The four do not tell the systems apart; they show the carried state did not revive dropped work.

## 4. Cost and calls

| | K rev 2 | K rev 1 | K0 (rev 1) |
|---|---|---|---|
| first question turn: input tokens, mean / median | **11,734 / 10,064** | 11,117 / 10,066 | 25,075 / 26,520 |
| first question turn: model calls, mean | 1.58 | 1.54 | 3.08 |
| first question turn: a memory tool used | 12 of 24 | 12 of 24 | 21 of 24 |
| every question turn: input tokens mean; model calls mean; tool calls mean | 9,609; 1.31; 0.41 | 9,591; 1.33; 0.42 | 19,224; 2.42; 2.08 |
| closes: summariser calls (by label, §10.4) | 24 (`orchestrator_summary`) | 24 (`orchestrator_summary`) | 26 (`orchestrator_summary`: kq-007's three) |
| closes: input / output tokens | 45,201 / 10,045 | 41,768 / 7,819 | 43,563 / 6,956 |
| summaries projected | 12 (the 12 sets of 4 or more fed turns) | 12 | 11 (kq-007's refresh never landed) |
| working state as stored (JSON): mean / max characters | 1,059 / 2,003 | 900 / 1,406 | 884 / 1,613 |
| states with an `asked` item | 20 of 24 | — (no such field) | — |

The block is at most 4,000 characters; without it K0's reader spent more than twice K's input tokens searching and reading memory for the conversation it was asked about. Every summariser call in both K runs was a close's refresh — no turn-end lag refresh and no compaction fired (every conversation stayed well inside the 40-row window and the model's window) — so §9's `T1` for these conversations is `Q′` alone: one call a set. Rev 2's states are 18 % longer than rev 1's (the `asked` list); no close's answer was unusable. (Rev 1: K0's kq-007 (cs-010) is the one refresh that never landed — the writer answered `null` three times at temperature 0 — and the conversation kept no state and got no summary, `state_fresh` false for that set.)

## 5. The episode split

*(Rev 2.1, 2026-10-02: after these figures the owner took **CQ-11** (2026-09-30; [11](11-continuity-design.md) rev 1.9, §3.4), which reverses CQ-2's projection half: the episode note is again written by its own summariser call over the conversation's rows at the close — the prompt, the parsers and the note of the build L8 measured at 17 of 20 (10 §5, the first column below), now on `summary_model` under the label `orchestrator_episode` and `summary_max_daily_cost_usd` (before slice 4: the extraction model and extraction's daily gates). The projection this section measures — 14, then 11 of 20 — is no longer the product's path. No run has measured the restored call on this build; the figures below stand as measured, of the projection.)*

L-episodes fed the 51 corpus sessions with learning on and closed each (the working state's refresh, then the projection), then asked the 20 questions of `questions-episodes.json` with learning off; automatic recall never delivers a summary, so the reader finds one only through `memory_search` or `memory_timeline`. Both revisions used the corrected `questions-episodes.json` (SHA-256 `1c2545f5d6b199f7…`, eq-017's gold since `c6db079f`); L8's runs used `6ae072496587ce2a…`.

| | L8, final (`82180b84`, 10 §5) | Rev 1 (`78d5c013`) | **Rev 2 (`63490c5c`)** |
|---|---|---|---|
| **L-episodes** | 17 / 20 (85.0 %) — en 12/13, zh 4/5, mixed 1/2 | 14 / 20 (70.0 %) — en 10/13, zh 3/5, mixed 1/2 | **11 / 20 (55.0 %)** — en 8/13, zh 2/5, mixed 1/2 |
| summaries written / sessions below 4 user turns | 47 / 4 | 47 / 4 | 47 / 4 (cs-034, cs-037, cs-039, cs-047) |
| summary calls; input / output tokens | 47 episode calls (the episode prompt); 140,324 / 9,929 | 52 close refreshes (`orchestrator_summary`; cs-041's one unusable answer retried); 120,437 / 16,841 | 51 close refreshes (`orchestrator_summary`, one per session; no unusable answer); 149,746 / 27,108 |
| summaries cut at the note's 1,024 bytes; without a *Decided* / *Left open* line | — | 10; 0 / 4 | **39**; 14 / 20 (§6.5) |
| summaries sharing an item verbatim with the previous conversation's (§6.1) | — | 0 | **12** |
| reader input tokens per question turn, mean; model calls; tool calls | 29,188 | 26,760; 2.7; 2.35 | 37,884; 3.15; 3.1 |

Paired against rev 1 on the same 20: −15 pp, CI [−43.7, +13.7]; rev 2 alone answered eq-002, -009 and -019, rev 1 alone eq-004, -005, -011, -012, -016 and -020. No cap bound; every close ran; the invariants hold (no user note retired, nothing resurrected, the forced re-run checked); coverage 75 of 83 (the reader's own notes matched 56, extraction 19). One warning, the extractor's and not the summaries': cs-049's learning window was skipped after three unusable answers, so its turns were never learned (the learning phase had four unusable answers: 3 cut at `max_tokens`, 1 not an `items` object).

**CQ-10's gate is not met: 11 < 17.** The nine misses, one line each:
- *eq-004* (cs-012, **writer — the chain**): the reader read the conversation's summary, "Global preferences and Hangzhou lunch" — cs-011's topic; 8 of its 14 items are cs-011's summary's, verbatim, and come first — and the 1,024-byte cut ends it at the conversation's own repo-URL rule, before the bookmarks app and the schema the user asked for; the answer, true to it, had no app and no sketch. Rev 1's summary, "Global preferences and bookmarks schema", answered it.
- *eq-005* (cs-014, **writer — the chain**): the conversation's summary, "Moss 项目 Python 测试与 uv 配置", holds nothing of pytest: all 10 of its items are cs-013's summary's (cs-009's project details and cs-013's footnote answers). The reader did not open it (it read two notes: the test command and uv) and answered `uv run pytest -q` only.
- *eq-006* (cs-016, **writer — the chain**): the reader read the summary, "Global preferences, Hangzhou lunch, cloud budget, dinner rec" — 11 of its 13 items are cs-012's summary's — whose text names the shellfish-free dinner and is cut before the rice noodles and the fridge; the answer named the recipe and said the notes held no more. (Rev 1 missed it too, for retrieval.)
- *eq-011* (cs-030, **writer — the chain**): the conversation's summary is titled "Lantern diesel to sqlx migration" and all 11 of its items are cs-028's summary's, 8 of which are cs-026's; the port move and the alert are not in it. The reader found the alert's thresholds in a note, not the evaluation interval the user asked about, and said it could not join the two.
- *eq-012* (cs-035, **gold**): the summary says "Licence badge not included due to missing licence name and static/dynamic decision" — the live assistant's badge waited on the licence's name, and the user closed the README without giving one — and the answer followed it; the gold, from the user's turns ("And a badge for the licence at the top?"), says the badge was added. Rev 1's reader answered it.
- *eq-014* (cs-018, **writer**): the summary records the quiz as the assistant's rounds — round 1's third question explained, "第 2 轮 5 题尚未作答，留待后续完成" (round 2 unanswered, left for later) — not that the user got the third wrong and ended the practice ("今天就到这"); the reader answered that there was no result. `asked` holds the third question's explanation. (Rev 1 read it as the projection's; L8's first and final runs missed it too.)
- *eq-016* (cs-013, **writer — the chain**): the reader read the conversation's summary, titled "Moss 静态站点生成器项目介绍" — cs-009's topic; 7 of its 10 items are cs-009's summary's — and the byte cut ends it at "解释脚…", past the plugin registration and the HTML structure and before the back-link question and "I'll try it myself"; the answer gave the two it could read and said the summary was cut. Rev 1's reader answered it.
- *eq-017* (cs-019, **gold**, as in rev 1): the summary says the user confirmed the Xiaomi Mijia C1 3 L — what the live conversation decided — and the gold says the user said "buy the first one" without naming it; the answer followed the summary.
- *eq-020* (cs-048, **judge**): the answer names both of the gold's points (what to prepare the day before, how to prioritise too many alerts) with the rota's other facts, and the judge failed it for details not in the gold. Rev 1's reader answered it.

**By cause: writer 6 (5 of them the chain of §6.1), gold 2, judge 1; retrieval and the projection none.** Rev 2 alone answered eq-002 (the side questions of the SIGILL conversation, rev 1's projection miss — now in the summary's *Discussed* through `asked`, F3), eq-009 (rev 1's retrieval miss) and eq-019 (rev 1's judge miss).

Every summary in rev 2 is titled "Conversation on 2026-09-30" (§8).

## 6. Findings for the lead

### 6.1 The carry-over folds into the next conversation's state — **still open, and larger in rev 2**

Rev 1 found it in the stopped K run: a conversation fed as "New chat" opens one was given the previous conversation's block, the reader took it up, the close's writer folded it into the new state, and the conversation after that inherited it again (kq-005 → kq-007 → kq-008, archived as `K-stopped-fed-inheriting.json`); and in one L-episodes summary, cs-010's "Tomas birthday plan and vim config". The harness was fixed (`63a9c915`: a fed conversation starts fresh). F2 then handed the writer, at the close and the lag refresh of an inheriting conversation, the previous conversation's state under the heading `[carried from the previous conversation — not this conversation's work unless the user continued it]`, with rule 8 of the prompt to leave out what reached the conversation only through it.

Rev 2 measures the opposite of what F2 was for. K does not exercise it (its fed conversations start fresh, §2.3). L-episodes does, on every session but the first on its project (the harness puts every corpus session with a workspace on its one scratch project and the rest on none): **12 of its 47 summaries share at least one item verbatim with the summary of the conversation before them on the same project — the one each inherited; rev 1's shared none** (`chain.py` over both reports; rev 1's one fold, cs-010's title, came through the assistant's replies, not as copied items). In 4 of the 12 the carried items are 80 % or more of the summary (cs-006 8 of 10, cs-014 10 of 10, cs-016 11 of 13, cs-030 11 of 11), in 3 more half or more (cs-012 8 of 14, cs-013 7 of 10, cs-028 8 of 11), and five chains run three conversations deep: cs-001 → cs-003 → cs-006 and cs-026 → cs-028 → cs-030 (Lantern), cs-009 → cs-013 → cs-014 (Moss), cs-011 → cs-012 → cs-016, cs-021 → cs-023 → cs-025. The carried items come first in `done`, so the note's 1,024-byte cut (§6.5) takes the conversation's own work: 5 of the episode split's 9 misses (§5). As built, the carried state reaches the writer as the first line of `<new_lines>`, ahead of the conversation's own lines (`working_state.rs:156-168`), where rule 1 tells the writer to "start from the previous state and add what the new lines say" and rule 8 to leave the carried section out; on this model the carried items were kept in 12 of the 47 summaries. A production conversation opened as "New chat" on the same lane and project is in L-episodes' position. The lead's (no change made).

### 6.2 The writer records the assistant's side and the daemon's conditions as `next` — **half closed, still open**

Rev 1: in 14 of K's 24 states `next` was a wait on the user, and four `next` answers were the harness's refused approvals and workflow cap. F1 (the field descriptions) and rule 7 (leave the daemon's conditions out) answered it. Rev 2 (§3.2): the waits left `next` (1 state against 10–14) for `open`, where F1 puts the assistant's offers — and where the reader, asked what was left, finds them; rule 7 did not take: a refused approval, the workflow cap or the Memory view's unconfirmed suggestions are `next` in 8 states and `next` or `open` in 11 (rev 1: 6 and 7), and they are the answer in 8 of the 18 `next` misses. The work's step reached `next` in 2 states (kq-017, -021; rev 1: 1, kq-018's, from the cut of §6.3). `next` answered 2 of 20 (rev 1: 4).

### 6.3 A line the writer reads cut looks unfinished — **still open**

F4 marks a cut line with ` …` and the prompt says such a line "was shortened here; the message itself was not cut off". Rev 2's kq-016 state still says the cron scheduling details "were truncated in the assistant's response": its third fed answer was 2,887 characters, so the writer read it cut at 1,500 and marked; the reader then offered to finish them (§3.2). One state of 24 names a cut in each revision (rev 1: kq-018, "the full Loki query … was cut off").

### 6.4 The judge and the golds — **still open**

The frozen judge prompt (written for questions about stored notes) failed 5 of K rev 2's answers — the four `doing` misses and kq-021's `next` — and eq-020 in the episode split for naming more of the conversation than the gold does; rule 1 says extra correct detail does not hurt, and the reader grading itself did not apply it. The golds, for their part, are the user's side only (§2.2): 5 of K's `next` misses and 2 of the episode split's are the gold against what the live conversation did — an offer F1 keeps in `open` against an absence gold (kq-002, -003, -008), a question the live assistant had answered named as the next step (kq-011) or a step it had done (kq-016), a badge it did not add (eq-012), a model the user never named (eq-017). No second judge was run and no gold was re-written; the targets are read as measured.

### 6.5 The summary's byte cut, now that it carries `asked` — **new**

F3 added the user's answered questions to the summary's *Discussed* line (goal + `done` + `asked`), which now holds 8.5 items on average against rev 1's 5.5. The note's text is cut at 1,024 bytes (`EPISODE_MAX_KNOWLEDGE_BYTES`, `memory/learning/mod.rs:129`; *Discussed* first, then *Decided*, then *Left open*): **39 of rev 2's 47 summaries reach the cut (rev 1: 10), 14 have no *Decided* line left and 20 no *Left open* (rev 1: 0 and 4)** — 27 of the 35 summaries outside the chain are cut too. No miss of rev 2's is the cut alone (eq-016's is the chain's items filling the room); eq-002 is the one the added `asked` answered.

### 6.6 The fix round's other rows

- **F5** (the writer never reads a row a forget suppressed): neither run forgets anything; not measured here.
- **F6** (the conditional timeline hint): first-turn `memory_timeline` calls in the 12 sets without a summary, 2 → 1 (§3.3).
- **F7** (the state writer shares extraction's cut read): records only; rev 2 had no unusable close answer (K 24 calls for 24 closes, L-episodes 51 for 51), so it shows nothing either way.

## 7. What is not decided here

- **K's 80 % and the +30 pp are not met** in either revision: rev 2 63.3 % and +10.0 pp (CI [−2.8, +22.8]), within one run's variance of rev 1's 66.7 % and +13.3 pp. `doing` + `decided` stand at 90 %; `next` is 2 of 20, 12 of its 18 misses the writer's (§3.2, §6.2) and 5 the golds' (§6.4).
- **The episode split is below CQ-10's gate** in both: rev 2 11 < 17 (rev 1 14). CQ-10 says the rendering is then revised before release; five of rev 2's nine misses are the chain (§6.1), which rev 2's summaries show more of than rev 1's.
- Design 11 and the code are not revised in this round: the lead decides after rev 2.

## 8. Caveats and threats to validity

- **The reader grades itself** (LQ1): every verdict is `qwen3.8:27b-mlx` judging its own answers with the frozen prompt. No second judge was run.
- **One run each.** The reader answers at the model's default temperature, so each fed conversation — and the state, the summary and the notes it leaves — differs from run to run; K and K0 were fed separately and did not answer from the same transcript, and neither did K rev 1 and K rev 2 (their difference, −3.3 pp, CI [−11.3, +4.6], is a measure of that variance plus the fix round). A difference between K and K0 is the carry-over plus that variance, and the CIs above are over the 60 questions only, not over runs.
- **K0 was not re-run for rev 2** (the lead's ruling: nothing in the fix round changes what K0 sees). K0's reader has no block, but it can open the summaries its closes projected (11 sets), and those were projected from rev 1's writer — before F1, F3 and F4; rev 2's K is paired against that K0 across two builds and two feeds.
- **The golds are the implementer's**, written from the corpus's user turns before each split; no second reader checked them.
- **The harness's conditions reach the state**: unattended turns (no approval can be given) and a workflow cap of 0, as in every run of this harness (§3.2, §6.2).
- **The builds differ**: rev 1's K and K0 ran at two commits (§2.1), the same code for K0's purposes; rev 2 at a third.
- **Concurrent runs** (§2.1): no latency is comparable.
- **The question set was smoke-tested once** on the model before rev 1's full runs (§2.2): one question changed after the model had seen it; none since.
- **Every summary carries one date** (the fix round's F8, recorded and not fixed): a summary is titled "Conversation on <the day it was closed>", and the whole corpus is fed within one run, so every summary of a run bears one date — "Conversation on 2026-09-29" in rev 1, "Conversation on 2026-09-30" (UTC) in all 47 of rev 2's L-episodes — and a date cannot tell two summaries apart, in a search's listing or in `memory_timeline` (rev 1's eq-006 was lost that way). Production conversations fall on different days; the eval's retrieval of a summary by its date is not the product's.

## 9. Reproduce

Rev 2, from a checkout of `63490c5c`, with a local Ollama serving `qwen3.8:27b-mlx` and a scratch directory outside the repository (rev 1's commands, at `63a9c915`, are rev 1's §9 — the same with `K0` and `K` run from their own binaries):

```sh
cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run   # prints the binary
cp target/debug/deps/memory_eval-<hash> <scratch>/bin/memory_eval
# <scratch>/config/llm.toml: the file of scripts/eval/memory/README.md, *Prerequisites*
cd <scratch>
for run in "K K" "L-episodes L <repo>/scripts/eval/memory/questions-episodes.json"; do
  set -- $run
  OPENALPACA_LIVE_EVAL=1 OPENALPACA_MEMORY_EVAL_SYSTEM=$2 \
  OPENALPACA_MEMORY_EVAL_REPORT=<scratch>/reports/report-$1.json \
  OPENALPACA_HOME_STORE=<scratch>/store-$1 OPENALPACA_CONFIG_DIR=<scratch>/config \
  TMPDIR=<scratch>/tmp/ ${3:+OPENALPACA_MEMORY_EVAL_QUESTIONS=$3} \
    <scratch>/bin/memory_eval --ignored memory_eval --nocapture
done
python3 scripts/eval/memory/compare.py scripts/eval/memory/reports/continuity/K-rev2.json \
  scripts/eval/memory/reports/continuity/K0.json
```

The session's own script is `eval2/run.sh` (rev 1's `eval/run.sh` with `EVAL` = `eval2`); the two ran at once (§2.1). The redaction is `eval2/redact.py` (rev 1's, the L8 archive's); the figures are `eval2/figures2.py` (K, the paired CIs against K0 and rev 1's K), `eval2/episodes_misses.py` (§5's misses with their summaries) and `eval2/chain.py` (§6.1's shared items).

## Revision log

- rev 1 (2026-09-29): K0, K and L-episodes run and reported; the first K run stopped and set aside (§6.1).
- rev 2 (2026-09-29): K and the episode split re-run after the fix round F1–F6 (design 11 rev 1.7), at `63490c5c`; K0 not re-run, K paired against rev 1's K0. §1, §2.1's builds and runs, §3 (the by-kind table with rev 1 and the pairs against K0 and rev 1's K; §3.2 and §3.3 re-read on rev 2's answers, one line per miss with its cause), §4 and §5 (re-read, one line per miss) re-written with rev 2's figures; §6.1–§6.4 marked (§6.1 still open and larger, §6.2 half closed, §6.3 and §6.4 still open), §6.5 (the summary's byte cut with `asked`) and §6.6 (F5–F7) new; §7 and §8 (K0 not re-run; the one date of every summary, F8) and §9 for rev 2. Verdicts as measured: none of the three targets met (K 63.3 %, +10.0 pp over K0, the episode split 11 of 20). Nothing in the design or the code revised.
- rev 2.1 (2026-10-02): a note at the top of §5 and in the status line — owner decision CQ-11 (2026-09-30; 11 rev 1.9) restored the episode note's own summariser call after these figures, so the projection §5 measures is no longer the product's path. No new measurement, no figure changed.
