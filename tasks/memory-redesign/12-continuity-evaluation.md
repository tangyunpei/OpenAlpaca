# 12 — Continuity evaluation (design 11 §10, slice 7)

**Status: rev 1 (2026-09-29) — the report of design [11](11-continuity-design.md)'s slice 7: the continuity split (§10.1) run as K0 and K, the episode split re-run on L-episodes (§10.3, CQ-10's gate), and the summariser's calls by label (§10.4); §10.2's compaction invariant runs offline in the default suite (slice 3) and is not repeated here.** One run of each system, on the local test model, at `78d5c013` / `63a9c915` on the branch `track-b` (unpushed). Every number below is a measurement with its conditions and the file it was read from. **Every verdict here is the reader grading itself** (§2.1, §8). **None of the three targets is met: K answered 66.7 % of the 60 (target 80 %), 13.3 pp above K0 (target +30 pp), and the episode split answered 14 of 20 (CQ-10's gate is 17).** Nothing in the design is revised here — not the projection either: the lead decides (§7).

**Sources.** The archived reports, redacted as the L8 archive is (hostname `owner-macbook-pro`; paths `<scratch>`, `<repo>`, `<tmp>`, `<home>`), are in `scripts/eval/memory/reports/continuity/`: `K.json`, `K0.json`, `L-episodes.json`, `compare-K-vs-K0.txt`, and `K-stopped-fed-inheriting.json` — the first K run, stopped after 9 of 24 sets for the harness defect of §6.1, kept as its evidence and not reported as K. The rest lives in the session scratchpad's `program/track-b/eval/`: `run.sh` and `run-K.sh`, `logs/*.log` (every turn, flush and close), `reports/` (the unredacted reports and the one-set smoke), `bin/` (the binaries with their SHA-256), `figures.py` (every figure below), `redact.py`.

## 1. Summary

| Target (design 11 §10) | Measured | Verdict |
|---|---|---|
| **K ≥ 80 %** over the 60 answers | **40 / 60 = 66.7 %** — doing 17/20, decided 19/20, **next 4/20**; the 4 abstentions 4/4 besides | **not met** |
| **K ≥ K0 + 30 pp** | K0 32 / 60 = 53.3 %: **+13.3 pp**, paired 95 % CI [−0.1, +26.8] (13 answers only K got right, 5 only K0) | **not met** |
| **Episode split ≥ 17 / 20** on L-episodes (CQ-10) | **14 / 20 (70 %)** — en 10/13, zh 3/5, mixed 1/2; 47 summaries projected, every close ran | **not met** |
| `carry_over_delivered`, `state_fresh` | K: 64 / 64 rows carried the fed conversation's block, on every turn 1–3; 24 / 24 states fresh. K0: 0 / 64 carried (as it must); 23 / 24 fresh | as designed |
| `summariser_calls` (§10.4) | one label only, `orchestrator_summary`: K 24 calls for 24 sets (one close each); K0 26 (one close's three unusable answers); L-episodes 52 for 51 sessions (one unusable answer retried) | as designed |

**What the numbers say.**
- **The carry-over does what it is for on `doing`:** K knew which conversation it continued and what it was about in 17 of 20 sets, K0 in 11 — **+30 pp**, CI [+5.6, +54.4]. K0's reader had only memory: every note of every set was written on one day, so "the last conversation" was the newest-looking notes, and in 7 of its 9 `doing` misses it described another set's conversation (§3.3).
- **`next` is where both fail, 4 of 20 each, and the gold is part of why.** Of K's 16 `next` misses, 15 report the carried state's `next` and `open` faithfully — the assistant's last offer or clarifying question (11; "wait for the user to decide whether to add a guard script" is the shape of 14 of the 24 states' `next`) or an approval or a workflow the unattended eval turn could not give (4) — not the step the user was taking, which is what the golds (written from the user's turns alone) name; the 16th said "nothing was left pending", as its gold does, and failed on a wrong detail. Six of the 20 golds say nothing was pending; in four of those the live conversation had left something pending in K (§3.2).
- **Without `next`, K answered 36 / 40 (90 %), K0 28 / 40 (70 %)**, +20 pp, CI [+4.2, +35.8].
- **The carry-over is cheaper, not dearer:** K's first question turn read 11,117 input tokens on average (median 10,066) with 1.5 model calls; K0's read 25,075 (median 26,520) with 3.1 — K0 used a memory tool on 21 of its 24 first turns to find the conversation, K on 12 (§4).
- **The episode split fell from L8's 17 to 14** (§5): two of its six misses are the reader not opening a summary that held the answer, two are the projection — a state records what the conversation settled and left for the assistant, not the questions the user asked — one is the judge, one the summary against a gold the live conversation did not match.
- **Two design findings for the lead** (§6): a new conversation given the previous one's block folds it into its own working state at its close, and the next conversation inherits it again (the stopped run, and L-episodes' own summaries, §6.1); and the writer records the assistant's side of the last exchange as the conversation's `next` (§6.2).

## 2. Setup

### 2.1 Model, machine and isolation

- **Model.** `qwen3.8:27b-mlx` (Ollama 0.34.4; family `qwen3_5`, 27.8B, `nvfp4`, 262,144-token context) is the reader, the working-state writer (no `summary_model` is set, so the close's refresh runs on the router's default), the extractor (L-episodes) and the judge — **every verdict here is the reader grading itself** (`judge_is_reader: true` in every report, LQ1). The reader runs at the model's default temperature (`reader_temperature: null`); the writer and the judge at 0.0, the judge within 400 output tokens.
- **Configuration.** `eval/config/llm.toml`, a byte copy of L8's `memeval/config-mlx/llm.toml` (SHA-256 `d7cf4d8021f45b52…`): only Ollama enabled, `fallback_models = []`, a 900 s request timeout, the router's rate limits lifted for the keyless slot.
- **Machine.** Apple M5 Max, 128 GiB, 18 threads, macOS 26.6.2 (25G83) — L8's.
- **Builds.** K0 and L-episodes ran the harness commit `78d5c013`, K the fix `63a9c915` (§6.1; for K0 the two are the same run — with `carry_over_turns = 0` no conversation is given a block, whatever it inherits). Each was built by the repository's pinned toolchain, rustc 1.93.0 (`254b5960`), as a debug test binary (`cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run`) copied to `eval/bin/` (`memory_eval` SHA-256 `cf02a2f5aa4a19ff…`, `memory_eval-K` `de6643d86403ca7b…`) and run directly, so that nothing built in the worktree meanwhile could replace it; each report's `build.git_head` names its commit. Every report's `machine.rustc` says 1.98.0: the harness asks `rustc --version` in the run's working directory, which is outside the repository and so outside `rust-toolchain.toml`; the binaries are 1.93.0's.
- **Isolation.** Every run's working directory, `OPENALPACA_HOME_STORE` (`eval/store-<run>`), `OPENALPACA_CONFIG_DIR` (`eval/config`) and `TMPDIR` (`eval/tmp`) lie in the session scratchpad's `program/track-b/eval/`, outside the repository and the worktree; `~/.openalpaca` was never read or written. No daemon and no CLI ran.
- **Concurrency.** The three ran **at the same time** against the one Ollama — K0 and L-episodes from 12:48 UTC, K (restarted) from 14:19 — so each waited on the others' calls: a turn answered in 9–30 s alone (the smoke) took 40–190 s here. **No latency here is comparable with any other report**; accuracy, tokens and calls are not affected.

| Run | Build | UTC (2026-09-29) | Report |
|---|---|---|---|
| smoke-K (one set; not reported) | `78d5c013` | 12:45–12:47 | `eval/reports/report-smoke-K.json` |
| K, first run (stopped at 9 sets, §6.1) | `78d5c013` | 12:48–14:17 | `K-stopped-fed-inheriting.json` |
| **K0** | `78d5c013` | 12:48–17:29 | `K0.json` |
| **K** | `63a9c915` | 14:19–18:25 | `K.json` |
| **L-episodes** | `78d5c013` | 12:48–19:03 | `L-episodes.json` |

### 2.2 The continuity split

`scripts/eval/memory/questions-continuity.json` (SHA-256 `1bc5843502d93738…`): **24 sets** over the 24 conversations of the learning corpus with at least 6 user turns — every such conversation. The other data files are L8's, byte for byte, but for `questions-episodes.json` (§5).

- **20 sets of three questions** — `doing`, `decided`, `next`, asked in that order as the next conversation's turns 1, 2 and 3 — in the conversation's language (15 en, 4 zh, 1 mixed: 45, 12 and 3 of the 60 answers). `doing` always asks what we were working on with no topic named; `decided` and `next` name a topic where the conversation held more than one thread.
- **4 sets of one question** about work the user turned down before the split (cs-024's dark-mode toggle, cs-030's retry around a flaky test, cs-033's hand-written template engine, cs-044's Kafka; 3 en, 1 zh), `expect_abstain`: the gold says nothing is pending on it, and the question is the next conversation's first turn — as a fourth turn of a set it would fall past `carry_over_turns` and never be given the block.
- **Splits** leave at least 3 turns on each side (3 to 9 fed turns, 105 in all; 12 sets feed 4 or more, so their close projects a summary). Every gold is written from the corpus's user turns before the split; the corpus holds no assistant turn, so no gold depends on what the live reader answered — and none accounts for it either (§3.2).
- **Six `next` golds say nothing was pending** (kq-002, -003, -006, -007, -008, -009): five conversations that only told the assistant about the user before their split, and one whose problem was solved. They are answers, not abstentions: "nothing was left open" is correct, "I don't know" is not.
- **Validated in default CI** (`memory_eval_corpus.rs`, `the_continuity_split_is_well_formed`): the counts, kinds and order, the sides, the language, a rejection before an abstention set's split, no id another split uses, no question or gold sharing an 8-word or 12-CJK-character shingle with a turn after its split.
- **Written, then smoke-run once.** The one-set run of K before the full runs showed kq-001's `decided` gold, "nothing had been decided yet", failing against a state whose `decisions` held the assistant's recommendations for the question the user had asked; for a conversation that decided nothing, the question now asks what it established (which team owns Lantern). No other question changed after a model had seen it.

### 2.3 The systems

| Id | What |
|---|---|
| **K** | For each set, in file (story) order, on one owner's one lane: a conversation opened as "Start fresh" opens one (`inherit: false`; on the scratch project when the corpus session has a project, on none otherwise), its turns `[0, split)` fed through the gateway and answered by the reader, closed by the learning queue's `flush_episodes` (the working state's refresh on the writer, then the summary projected from it at 4 user turns or more), then the next conversation opened on the same lane and project as "New chat" opens one, and the set's questions asked as its first turns and judged. Memory as D runs it (recall, the read tools, the main loop's writers), nothing seeded; `carry_over_turns` = 3 as shipped. No learning flush runs; learning is enabled for the close, and the summariser's ceiling lifted. |
| **K0** | K with `carry_over_turns = 0`: no conversation is given the block. |
| **L-episodes** | L (design 09 §7.2) — the 51 corpus sessions fed with learning on, flushed after every 10th user turn and at each end, each end closed — then the 20 questions of `questions-episodes.json`, learning off. |

K0 is **memory without the carry-over**, not nothing: its reader can find a closed conversation through the summary its close projected (12 sets) or through its own `memory_store` notes of the fed turns (its fed turns stored notes in 22 of 24 sets, K's in 23), and it used a memory tool on 45 of its 64 turns. A no-memory floor would be lower.

## 3. Continuity results

### 3.1 By kind

| | K | K0 | K − K0 (paired, 95 % CI) |
|---|---|---|---|
| **the 60 answers** | **40 (66.7 %)** | **32 (53.3 %)** | **+13.3 pp [−0.1, +26.8]** |
| doing (20) | 17 (85 %) | 11 (55 %) | +30.0 pp [+5.6, +54.4] |
| decided (20) | 19 (95 %) | 17 (85 %) | +10.0 pp [−9.1, +29.1] |
| next (20) | 4 (20 %) | 4 (20 %) | +0.0 pp [−24.0, +24.0] |
| doing + decided (40) | 36 (90 %) | 28 (70 %) | +20.0 pp [+4.2, +35.8] |
| the 54 without an absence gold | 39 (72.2 %) | 30 (55.6 %) | — |
| abstentions (4, apart) | 4 | 4 | — |
| by language, the 60: en / zh / mixed | 26/45, 11/12, 3/3 | 24/45, 7/12, 1/3 | — |

`compare.py` over all 64 paired rows (the abstentions are concordant): K 68.8 %, K0 56.2 %, +12.5 pp, CI [−0.1, +25.1], 18 discordant pairs (13 K only, 5 K0 only) — `compare-K-vs-K0.txt`. Its gate line is the held-out set's non-inferiority margin and does not apply here. No row was unparsed, no judge call failed, no turn errored, in either report.

### 3.2 `next`

K answered `next` from the carried state, and the state's `next` is not the gold's:

- **The assistant's offer as the next step — 11 rows** (kq-002, -004, -005, -006, -007, -011, -012, -015, -016, -020, -023). The fed conversation's last reply ended with an offer or a clarifying question — a ten-line before/after snippet, a guard script in CI, "which city and budget?", "Postgres, MySQL or SQLite?", "cron or launchd?" — and the writer recorded it as `next` ("Wait for user to confirm if they want the before/after snippet…") and in `open`. The reader reported exactly that; the gold names the user's own step (apply the fix, write the backfill, write the migration). Three of these rows have an absence gold (kq-002, -006, -007), which the offer contradicts as well.
- **What the eval's turns could not do — 4 rows** (kq-001, -009, -021, -024). Every fed turn is `unattended`, so the reader's `memory_update`s were refused for want of an approval, and `start_workflow` is capped at 0; the state then carried "memory note … needs update … pending approval in GUI or interactive session" or "workflow slot limit reached", and the reader offered those as the next step. In a daemon with a person at the keyboard these would not arise; they are the harness's conditions reaching the state, and they cost K four `next` answers.
- **The judge — 1 row** (kq-003): "Short answer: no — nothing was left pending", the gold's fact, failed for listing "project context (Lantern)" among what the conversation captured, which it had not.
- **Right — 4 rows** (kq-008, -010, -013, -018). kq-018's state also carried "the full Loki query … was cut off", which it was not — §6.3.

K0's 4 of 20 are the same size for another reason: it seldom found the conversation at all (§3.3), and when it did it found the notes, not the next step.

### 3.3 `doing` and `decided`

- **K0's misses are the wrong conversation.** In 7 of its 9 `doing` misses K0's reader described another set's conversation — the Lantern background, the profile set-up, the retry policy — picked from the day's notes (kq-005, -006, -007, -013, -016, -018, -024); in kq-001 it found Lantern and not the question; in kq-021 it named the right thread among others and the judge failed the extra one. Its `decided` misses: kq-016 and kq-023, "no record of that" (its searches found no note of it); kq-024, another set's column.
- **K's `doing` misses are the judge reading a wider answer as a wrong one** (kq-012, -016, -021): each named the gold's work among the conversation's other threads — the PR's conventions and release checklist, the YubiKey and zoxide before the backup script, ADR-005 before the pre-push hook — and the judge failed each for "facts not in the gold answer", which rule 1 of the frozen prompt says do not hurt. Its one `decided` miss, kq-002, named the decision and then called the question "not decided yet — that was the open item" (the guard script of §3.2).
- **Carry-over and its turns.** K's block reached turns 1, 2 and 3 of every three-question set and turn 1 of every abstention set (24 / 20 / 20); K0's never. On its first turns K's reader opened the fed conversation's summary with `memory_read` in 9 of the 12 sets that had one — the block names the note and says to read it — and in 2 sets with none (3 fed turns) read other notes, the block's heading pointing at a timeline entry no close would write (rev 1.4 item 11 (a)); it used a memory tool on 17 of its 64 turns in all (K0: 45).

### 3.4 Abstentions

4 of 4 in both, and in both for the same reason: the fed turn's reader had stored the rejection as a note, and the per-turn recall delivered it on the question's own words ("dark-mode toggle", "retry", "Kafka") in 7 of the 8 answers (K0's kq-019 found it with a search), so K0 answered from the note and K from the note and the state (whose `decisions` held it). The four do not tell the systems apart; they show the carried state did not revive dropped work.

## 4. Cost and calls

| | K | K0 |
|---|---|---|
| first question turn: input tokens, mean / median | **11,117 / 10,066** | 25,075 / 26,520 |
| first question turn: model calls, mean | 1.54 | 3.08 |
| first question turn: a memory tool used | 12 of 24 | 21 of 24 |
| every question turn: input tokens mean; model calls mean; tool calls mean | 9,591; 1.33; 0.42 | 19,224; 2.42; 2.08 |
| closes: summariser calls (by label, §10.4) | 24 (`orchestrator_summary`) | 26 (`orchestrator_summary`: kq-007's three) |
| closes: input / output tokens | 41,768 / 7,819 | 43,563 / 6,956 |
| summaries projected | 12 (the 12 sets of 4 or more fed turns) | 11 (kq-007's refresh never landed) |
| working state as stored (JSON): mean / max characters | 900 / 1,406 | 884 / 1,613 |

The block is at most 4,000 characters (the states here were 900 characters of JSON on average, before the block's heading and the last exchange); without it K0's reader spent more than twice K's input tokens searching and reading memory for the conversation it was asked about. Every summariser call in both runs was a close's refresh — no turn-end lag refresh and no compaction fired (every conversation stayed well inside the 40-row window and the model's window) — so §9's `T1` for these conversations is `Q′` alone: one call a set. K0's kq-007 (cs-010) is the one refresh that never landed: the writer answered `null` three times at temperature 0 (`unusable_answers`: "the answer is not one JSON object", 4 characters, finish `stop`), the queue gave the refresh up, and the conversation kept no state and got no summary — `state_fresh` false for that set.

## 5. The episode split

L-episodes fed the 51 corpus sessions with learning on and closed each (the working state's refresh, then the projection), then asked the 20 questions of `questions-episodes.json` with learning off; automatic recall never delivers a summary, so the reader finds one only through `memory_search` or `memory_timeline`. This build's `questions-episodes.json` is the corrected one (SHA-256 `1c2545f5d6b199f7…`, eq-017's gold since `c6db079f`); L8's runs used `6ae072496587ce2a…`.

| | L8, final (`82180b84`, 10 §5) | This build (`78d5c013`) |
|---|---|---|
| **L-episodes** | 17 / 20 (85.0 %) — en 12/13, zh 4/5, mixed 1/2 | **14 / 20 (70.0 %)** — en 10/13, zh 3/5, mixed 1/2 |
| summaries written / sessions below 4 user turns | 47 / 4 | 47 / 4 (cs-034, cs-037, cs-039, cs-047) |
| summary calls; input / output tokens | 47 episode calls (the episode prompt); 140,324 / 9,929 | 52 close refreshes (`orchestrator_summary`, one per session with a user turn past the state — the short ones included — and cs-041's one unusable answer retried); 120,437 / 16,841 |
| reader input tokens per question turn, mean | 29,188 | 26,760 |

No cap bound and no warning was written; every close ran; the invariants hold (no user note retired, nothing resurrected, the forced re-run checked); coverage 76 of 83 (the reader's own notes matched 57, extraction 19).

**CQ-10's gate is not met: 14 < 17.** The rendering of the summary is not revised in this slice (design 11 CQ-10: "else the rendering is revised before release" — the lead's). The six misses, one line each:
- *eq-002* (cs-003, **the projection**): the reader read the conversation's summary; its *Discussed* is the state's goal and `done` — the machine profile updated, the incident and the decision saved as notes — and its *Left open* the assistant's pending edits; the two questions the user asked on the side (which toolchain a directory uses, what SIGILL means) are in neither, and the answer, true to the summary, left them out. L8's final run, whose summary came from the episode prompt, answered it.
- *eq-006* (cs-016, **retrieval**): the right summary ("Cloud budget and dinner recipe": the shellfish-free recipe, rice noodles, fridge life) existed; the reader's search did not list it, and it answered from notes of another conversation's team dinner. Every summary is titled "Conversation on 2026-09-29" — the whole corpus is fed on one wall-clock day — so a date cannot tell them apart.
- *eq-009* (cs-026, **retrieval**): the summary held the batched backfill; the reader read three notes instead, answered the enum and the offline mode right, and called the backfill "left open".
- *eq-014* (cs-018, **the projection**): the reader read the summary, whose *Left open* says four answers of the first round "were not given" and the second round "not answered" — the assistant's view of a quiz the user had played through — and answered that the practice had no result. L8's first and final runs missed it too, for salience.
- *eq-017* (cs-019, **the summary against the gold**): the summary says the user settled on an entry-level cooker of 150–250 yuan, Supor or Midea, 4 L — what the live conversation decided — and the gold (corrected in `c6db079f`) says the user said "buy the first one" of two models without naming it; the answer followed the summary.
- *eq-019* (cs-044, **the judge**): the first search listed the conversation's summary second; the reader did not open it and answered from the listing and the recall — the outbox table and deduplication on the consumer side, the gold's two points, with more — and the judge failed it for details not in the gold.

Beside them, the carry-over reached L's summaries (§6.1): cs-010's (the editor set-up, the next conversation with no project after cs-008's birthday planning) is titled "Tomas birthday plan and vim config".

## 6. Findings for the lead

### 6.1 The carry-over folds into the next conversation's state (the stopped K run)

The first K run fed each set's conversation as "New chat" opens one — inheriting. On one lane, a set's fed conversation is then a new conversation on a lane and project an earlier set's asked conversation had just left, and — carry-over on — its first three turns were given that conversation's block. The reader took it up: kq-007's fed conversation (cs-010, the editor set-up) answered about the Tomas trip of kq-005's asked conversation, its close's writer folded the trip into the state ("Editor preferences and Tomas trip planning"; `open`: "Tomas trip budget, timeline, and preferences are pending"), and kq-008's fed conversation (cs-011, habits and formats) inherited it from kq-007's in turn ("…并推进 Tomas 行程规划"). kq-007's `next` question was then answered with the trip. The run was stopped at 9 sets and the harness fixed (`63a9c915`: a fed conversation now starts fresh, which the offline test pins); the stopped run is archived as the evidence.

This is the product's behaviour, not the harness's: a person who starts a new chat on another subject is given the previous conversation for three turns (CQ-5), and if the model talks about it, the new conversation's close writes it into the new state — which the conversation after that is given in turn. L-episodes, whose feed opens each corpus session as "New chat" does, shows it in a summary: cs-010's (the editor set-up, opened after cs-008's birthday planning, both with no project) is titled "Tomas birthday plan and vim config" (§5). Design 11 §12 names "residual over-continuation is the model's"; this shows the writer carrying it forward, conversation to conversation. Whether the writer should leave out what reached a conversation only through its block, or the carry-over should stop when the user's first turn is on another subject, is the lead's (no change made).

### 6.2 The writer records the assistant's side as `next`

In 14 of K's 24 states `next` is a wait on the user — "Wait for user to confirm…", "Await user decision on…", "User to choose…" — the offer or question the assistant's last reply ended on, and the open list is those offers. That is a faithful record of the conversation's last exchange, and the reader carries it faithfully (§3.2); but it is not "the next step" of the work as a person would state it, and it crowds the user's own step out. Whether the prompt's `next` should name the work's next step rather than the assistant's pending offer — and whether a refused approval belongs in the state — is the lead's (the prompt is design 11 §3.1's, unchanged here).

### 6.3 A line the writer reads cut looks unfinished

The writer's lines are each cut to `msg_trunc_chars` (1,500) with no mark (§3.1; rev 1.5 item 4 (c)), so a long answer ends mid-sentence in what it reads. The states then say so: the smoke's kq-001 ("Assistant response on axum config sharing was truncated mid-sentence"), K's kq-018 ("the full Loki query … was cut off"; the fed answer was 2,292 characters, complete) and K0's kq-016 ("Launchd plist … output was truncated"), and K's reader offered to finish the query that was never cut. A mark at the cut (`…`) would tell the writer the line is shortened, not the answer. Not changed here.

### 6.4 The judge and the golds

The frozen judge prompt was written for questions about stored notes. Here it failed three of K's `doing` answers and one `next` answer for naming more of the conversation than the gold does (§3.2, §3.3) — rule 1 says extra correct detail does not hurt, and the reader grading itself did not apply it. The golds, for their part, are the user's side only (§2.2): a question about what was left pending is answered by what the conversation left, and that includes the assistant's side. A second judge, and golds re-written against the transcript K was fed, would both move `next`; neither was done, and the targets are read as measured.

## 7. What is not decided here

- **K's 80 % and the +30 pp are not met** on this run. The design's text and the carry-over are unchanged; §6.1–§6.3 are the candidates the runs point at, and the question set's `next` golds (§3.2, §6.4) are the other.
- **The episode split is below CQ-10's gate**, 14 < 17 (§5). CQ-10 says the rendering is then revised before release; the projection is not changed in this slice — two of the six misses are its (eq-002, eq-014), two retrieval, one the judge, one the gold.
- Design 11 is not revised in this slice: its revision log records the rulings (rev 1.6), and the lead decides.

## 8. Caveats and threats to validity

- **The reader grades itself** (LQ1): every verdict is `qwen3.8:27b-mlx` judging its own answers with the frozen prompt. No second judge was run.
- **One run each.** The reader answers at the model's default temperature, so each fed conversation — and the state, the summary and the notes it leaves — differs from run to run; K and K0 were fed separately and did not answer from the same transcript. A difference between them is the carry-over plus that variance, and the CIs above are over the 60 questions only, not over runs.
- **The golds are the implementer's**, written from the corpus's user turns before each split; no second reader checked them.
- **The harness's conditions reach the state**: unattended turns (no approval can be given) and a workflow cap of 0, as in every run of this harness (§3.2).
- **K and K0 ran at different commits** (§2.1), the same code for K0's purposes.
- **Concurrent runs** (§2.1): no latency is comparable.
- **The question set was smoke-tested once** on the model before the full runs (§2.2): one question changed after the model had seen it.

## 9. Reproduce

From a checkout of `63a9c915`, with a local Ollama serving `qwen3.8:27b-mlx` and a scratch directory outside the repository:

```sh
cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run   # prints the binary
cp target/debug/deps/memory_eval-<hash> <scratch>/bin/memory_eval
# <scratch>/config/llm.toml: the file of scripts/eval/memory/README.md, *Prerequisites*
cd <scratch>
for run in "K0 K0" "K K" "L-episodes L <repo>/scripts/eval/memory/questions-episodes.json"; do
  set -- $run
  OPENALPACA_LIVE_EVAL=1 OPENALPACA_MEMORY_EVAL_SYSTEM=$2 \
  OPENALPACA_MEMORY_EVAL_REPORT=<scratch>/reports/report-$1.json \
  OPENALPACA_HOME_STORE=<scratch>/store-$1 OPENALPACA_CONFIG_DIR=<scratch>/config \
  TMPDIR=<scratch>/tmp/ ${3:+OPENALPACA_MEMORY_EVAL_QUESTIONS=$3} \
    <scratch>/bin/memory_eval --ignored memory_eval --nocapture
done
python3 scripts/eval/memory/compare.py <scratch>/reports/report-K.json <scratch>/reports/report-K0.json
```

The session's own scripts are `eval/run.sh` and `eval/run-K.sh`; the three ran at once (§2.1). The redaction is `eval/redact.py` (the L8 archive's, with this worktree's paths).

## Revision log

- rev 1 (2026-09-29): K0, K and L-episodes run and reported; the first K run stopped and set aside (§6.1).
