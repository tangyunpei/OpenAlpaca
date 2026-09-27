# Memory Eval Harness (memory redesign, plan slice 1 / §5; rewritten for the new engine in 3.11; the learning evaluation, design 09 §7)

A **developer benchmark**, not a test: it seeds a fixed set of gold memory
notes, asks a fixed set of questions through the chat front door, has a model
judge every answer against a gold answer, and writes one JSON report per
system under test. Two reports are compared pairwise to answer the plan's
question — is the new lexical memory service at least as accurate as the old
hybrid engine (−5 pp non-inferiority gate against **B**, or −10 pp against
**A** when B is unavailable)? The old engine is gone; its two baselines were
measured before it was deleted and are archived in `reports/`. Nothing here
runs under plain `cargo test` but the corpus's validation test.

The **learning evaluation** (design 09 §7, `tasks/memory-redesign/09-learning-design.md`)
asks the other question: given only conversations, does learning build a store
the reader can answer from? Systems `L`, `L0` and `L-S` feed a corpus of
conversations through the gateway before anything is asked; see
[The learning evaluation](#the-learning-evaluation) below.

## Layout

| File | Purpose |
|---|---|
| `notes-gold.json` | 83 gold notes about a fictional user (Lin Wei, 林薇) and two fictional projects (**Lantern**, a Rust ledger service; **Moss**, a Python static-site generator), in the shape of design §4's frontmatter (`id`, `title`, `kind`, `status`-less, `fact_key`, `aliases`, `tags`, `valid_from`/`valid_to`, `supersedes`, `created_at`) plus two eval-only fields: `project` and `language`. English and Chinese. Four **changed-fact** pairs carry `supersedes` + `valid_to` (editor, laptop, car→metro, Moss output dir) and four **conflict** pairs share a `fact_key` with no link (staging DB, Moss Python version, Lantern port, weekly-review time). |
| `questions-heldout.json` | **470** questions, the gate set: exact_recall 80, paraphrase 80, cross_language 70, changed_facts_time 55, project_procedures 55, rejected_failed_actions 45, abstention 45, source_conflicts 40 (en 303 / zh 125 / mixed 42). Each row: `id`, `category`, `language`, `question`, `gold_answer`, `relies_on` (gold note ids), `expect_abstain`. |
| `questions-tuning.json` | **150** more, same schema, for iterating on prompts, aliases or ranking. **The gate run never loads it** (it is loaded only through `OPENALPACA_MEMORY_EVAL_QUESTIONS`). |
| `judge-prompt.md` | The frozen binary judge protocol (CORRECT / INCORRECT). Its SHA-256 is recorded in every report; change it and every report before the change is a different experiment. |
| `conversations-learning.json` | The learning corpus (design 09 §7.1): **51 sessions, 300 user turns** in story order from October 2025 to September 2026 (32 English, 14 Chinese, 5 mixed — the questions' proportions; 26 with no project, 18 Lantern, 7 Moss). Written from `notes-gold.json` only, never from the held-out questions. Every gold note is stated, 23 only in passing; the 8 change pairs old-then-new (laptop, editor and weekly review changing in a Lantern session after a session with no project); 7 in-turn corrections; 15 restatements (6 of the pairs' old notes); 11 rejections; 54 % noise. A dated fact states its date in the text, never only as "today" or "this month" — a learning run stamps the wall clock — though four give no year (two a month and day, two only a month) and assume a run in 2026; and no turn states a fact outside the gold set that a held-out abstention question asks about (09 rev 3.21). `suggestion_sessions` lists the 13 sessions L-S feeds. Each turn: `user`, `states` (gold ids), `mode` (`explicit`, `indirect`, `change`, `correction`, `restate`, `rejection`, `noise`). |
| `questions-episodes.json` | The **episode split** (design 09 §13.4): **20** questions about what a corpus conversation discussed, decided or left open — one per conversation, each with at least 4 turns, in its language (en 13, zh 5, mixed 2), each drawing on a noise or rejection turn that no gold note states — answered from the conversation summaries an L run writes. The held-out schema (`category` `episodes`, `relies_on` empty) plus `session` and `turns` (0-based) naming what the gold answer comes from. |
| `coverage-judge-prompt.md` | The frozen coverage protocol (design 09 §7.3): given a gold note and up to five notes a search returned, `MATCH <n>` or `NONE`. Its SHA-256 is recorded in every learning report beside `judge_prompt_sha256`. |
| `compare.py` | The paired comparison: accuracy per system, the paired difference with its 95 % CI, the discordant-pair rate, per-category point differences, the gate verdict. It prints both reports' judge models first — with each report's `recall_mode` beside its judge when the report records one — and **refuses two reports graded by different judges** unless `--allow-judge-mismatch`; a recall-mode mismatch is never refused. `--self-test` checks the refusal and the printout. |
| `reports/` | The archived baselines of the deleted engine, measured on 2026-09-24 on a local Ollama as reader and judge: `baseline-A-2026-09-24.json` (13.0 %), `baseline-B-2026-09-24.json` (90.6 %), and their comparison. **B is the comparator** for D and E. |
| `crates/openalpaca_core/tests/memory_eval.rs` | The runner: a data-validation test and the harness's own tests — the judge guard, the coverage verdict, the learning measures and invariants, a re-judge of a fixture-sized report, a replay of a fixture-sized scratch database, the gateway feed, forced re-run and L-S confirm on two short sessions, and the recall-mode switch with the listed-versus-delivered counts and the read recorder, all on an offline stub model (run under `--features live-eval`, no network) — and the `#[ignore]`d live eval. |
| `crates/openalpaca_core/tests/memory_eval_corpus.rs` | The corpus's validation, in **default CI** with no model and no feature: coverage, the change pairs' order, the counts, the language shares within 5 points, the size, and the leakage check — no held-out question shares an 8-word shingle, or a 12-character Han/kana/Hangul shingle, with any turn. It validates the episode split too, and that the check catches each planted fault. |
| `crates/openalpaca_core/tests/common/eval.rs` | The data files' types, shared by both test files. |

## How it works

`memory_eval` boots a real `Orchestrator` (real `LlmRouter` from the eval's
own `llm.toml`, a scratch SQLite database in a tempdir, a scratch project
directory with a `.git` marker as the turn's workspace) and, for D and E, the
daemon's own memory service (`MemoryService::open_default`, its index cache
under the scratch store) with the read tools registered the way the daemon
registers them — each behind a recorder that answers exactly as the tool does
and notes which notes `memory_read` returned with a body and what each
`memory_search`/`memory_timeline` call returned at which `detail`. It creates one owner (a `global_user` row — the service
answers for nobody else), stores every gold note through
`MemoryService::store` on the **user-directed surface** (`UserSurface`: origin
`user`, status `active` — the call `POST /v1/memories` and
`openalpaca memory add` make), then asks each question with
`Orchestrator::handle_message` — exactly what the daemon's `POST /v1/chat`
handler calls — on a fresh lane per question, as that owner. After each turn
it takes the turn's `memory_used` (what the daemon's bridge puts on
`done.memory_used`). The answer text is handed to the judge model with the
frozen prompt at temperature 0, and the last line of the judge's reply is the
verdict.

Gold notes go in **natively**: `kind`, `scope`, `aliases`, `tags`,
`fact_key`, `valid_from`/`valid_to`, and each `supersedes` link as the
service's `{id, expected_revision}` of the note it replaces. They are stored
in the order a user would have told them: oldest `created_at` first, ties in
file order, and a replacing note only after the note it replaces. The gold
`created_at` is not an input — the service stamps its own `created_at` and
`updated_at` at write time — so the seed order is what those stamps say,
and they decide a same-`fact_key` tie (design §3 rule 3, §7c R9) and the
ranking's recency tie-break. The stamps are to the second, and two notes of
one second tie on them and fall to the lowest of the two minted ids, so a
note that shares its scope and `fact_key` with one already stored waits out
the rest of the second that note was written in (at most a second per such
note; the eight changed-fact and conflict pairs). Global notes land in the owner's home
partition, project notes in the scratch project's partition. The service mints
its own ids; the report maps them back (`conditions.seeded_ids`). Each run
uses its own owner (`memory-eval-owner-<8 hex>`), so a reused scratch store
never lets an earlier run's notes into a later run.

Systems (`OPENALPACA_MEMORY_EVAL_SYSTEM`):

| Id | What the reader gets | How the harness sets it up |
|---|---|---|
| `none` | No memory: the context-cost reference | No service installed: no notes, no per-turn recall, no memory tool |
| `D` | **The new service as the daemon runs it**: the automatic per-turn recall (`MemoryService::recall`, lexical FTS5, no model) plus `memory_search`/`memory_read`/`memory_list` and the main loop's three writers on the surface | Notes seeded as above; the main loop's shipped caps (`[orchestrator.routing]`: 8 rounds × 4 tool calls) |
| `E` | **D bounded to design §5**: the per-turn recall plus at most two model-directed follow-up searches | D with the main loop's existing tool budget narrowed: `main_loop_max_rounds = 3`, `main_loop_max_tools_per_round = 1` |
| `L` | **Conversations in, learning on** (design 09 §7.2): the notes the extractor learned from the corpus, then the service as D runs it | Nothing seeded; the corpus fed through the gateway with a manual learning queue, flushed after every 10th user turn of a session and at its end; the questions then asked as D asks them, learning off |
| `L0` | **Conversations in, learning off**: only what the reader's own `memory_store` calls kept | L with no flush |
| `L-S` | No questions: the **MD-11/MD-12 check** | The change pairs' 8 old notes seeded as the user's own — as saved at the time, see below — the suggestion sessions fed with learning on, each suggestion confirmed through the user-directed confirm |
| `A` `B` | The deleted engine: per-turn FTS; the `memory_search` tool path with E5 vectors | **Archived** (`reports/`). Selecting either refuses before any call and names the archived file. |
| `C` | One bounded `MEMORY.md` | Never built; selecting it refuses. |

### What D and E do and do not isolate (read before quoting a number)

- **D is the product, not a component.** The per-turn recall cannot be
  switched off while a service is installed, so D measures the recall and the
  model's own `memory_search` calls together, like B did (B ran with the old
  per-turn path live too). Every row says which delivered what: `memory_used`
  lists each note that reached the model with its reason (`recall`, `pinned`,
  `tool`), `per_turn_fts_hits` counts the recall's notes, `memory_search_calls`
  the model's searches, and `relied_notes_delivered` how many of the row's
  `relies_on` notes arrived by either route — retrieval, separately from the
  answer.
- **E's bound is an approximation**, expressed with the caps the main loop
  already has rather than a new mechanism. One tool call per round and three
  rounds means at most two tool calls can inform an answer written in the
  last round. But: the bound counts **every** tool call (`memory_read`
  included), not only `memory_search`; nothing tells the model which round is
  its last, so a model that spends the third round on a tool exits
  `MaxRounds` and the turn answers with the runtime's no-answer line (judged
  like any other answer); a second call in one round is answered "max tools
  per round exceeded" instead of running; and the answer guard's one
  corrective round can add a round to a turn that states a task id nothing
  answers to. `conditions.follow_up_search_bound` is `2` on an E report and
  `null` on D.
- **The main loop's writers are on the surface** in D and E, as in the
  product. A model that stores, edits or forgets a note mid-run changes what
  later questions see; each row lists its tool calls and
  `totals.memory_write_calls` counts those three. It should be 0.
- **Both fictional projects share one project partition**: every question is
  asked from one workspace, so project notes about Lantern and Moss sit side
  by side, as they did (in one global pile) for A and B.
- **Validity is evaluated at run time.** A search that is not historical
  keeps only notes whose `valid_from`/`valid_to` interval holds now
  (`memory_index/search.rs`), and one gold note — the Stockholm trip,
  2026-10-05 to 2026-10-19 — is outside that interval before and after those
  dates, so the per-turn recall does not deliver it then; seven held-out
  questions rely on it. `run_started_at` is recorded.
- **The reader's temperature is not pinned.** The main loop sets no
  temperature (`runner/agentic_loop/backend.rs`), so the reader runs at the
  provider's default; the report records `reader_temperature: null`. The judge
  *is* at temperature 0 with reasoning disabled.
- Background learning is switched off for the questions of every system
  (`[orchestrator.learning] enabled = false`, `task_extract_enabled = false`;
  L and L-S learn only in their learning phase, before),
  `start_workflow` is refused at `max_workflows_per_lane = 0` (so a stray call
  can never dispatch a paid lead run), and every turn is `unattended` (a tool
  that would need a confirmation — `memory_forget`, `memory_update` — is
  refused instead of hanging).

## Prerequisites

1. **Build with the feature.** `--features live-eval` compiles the LLM
   providers into the test binary. There is no second feature any more: the
   new engine needs no model of its own.
2. **The eval's own `llm.toml`** in `OPENALPACA_CONFIG_DIR`. The harness never
   falls back to the repo's `config/` or to a default. Use `secret_env` keys;
   `secret_ref` (keychain) is not resolved by a test binary. A local Ollama is
   reached **only** if this file enables it — that is the one way
   `localhost:11434` can ever be touched, and it never happens under plain
   `cargo test`. **The test model is `qwen3.8:27b-mlx`** on a local Ollama
   (owner, 2026-09-24): the reader, the extractor and — owner answer LQ1 — the
   judge. The archived baselines used `qwen3.8:27b-mtp-q8_0` as reader and
   judge, so `compare.py` refuses to compare a new report with them until
   one is re-judged (below). The file for the test model:

   ```toml
   [orchestrator]
   model = "qwen3.8:27b-mlx"
   fallback_models = []          # the judge's provider is never a reader rung

   [providers.ollama]
   enabled = true
   base_url = "http://localhost:11434/v1"
   default_model = "qwen3.8:27b-mlx"

   [providers.anthropic]
   enabled = false

   [providers.openai]
   enabled = false
   ```
3. **A scratch store** in `OPENALPACA_HOME_STORE`, absolute, never the real
   `~/.openalpaca` (the harness refuses both an unset variable and the real
   path). What is written there: the owner's home memory partition under
   `memory/owners/`, its control snapshot under `state/memory-control/`, and
   the index cache `state/cache/memory-index-v1.sqlite`. Give each
   **concurrent** run its own store (they would share the index file); runs
   one after another may share one.
4. **CWD outside the repository** when you run the daemon or CLI for anything
   related; the test itself resolves the data files from `CARGO_MANIFEST_DIR`,
   so its CWD does not matter, and it uses a tempdir project as the workspace.
5. `OPENALPACA_LIVE_EVAL=1`. Without it the test exits early even under
   `--ignored` (no calls, no cost).

## Run

```sh
export OPENALPACA_LIVE_EVAL=1
export OPENALPACA_HOME_STORE=/tmp/memory-eval/store       # scratch, absolute
export OPENALPACA_CONFIG_DIR=/tmp/memory-eval/config      # holds llm.toml
mkdir -p "$OPENALPACA_HOME_STORE" "$OPENALPACA_CONFIG_DIR"

# D (the new service as the daemon runs it):
OPENALPACA_MEMORY_EVAL_SYSTEM=D OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-D.json \
  cargo test -p openalpaca_core --features live-eval \
  --test memory_eval -- --ignored memory_eval --nocapture

# D in both recall modes (design 09 §13.4): the same D run once per
# [orchestrator.memory] recall_mode — the report records the one used in
# conditions.recall_mode — then the two compared on the same judge.
for mode in excerpts index; do
  OPENALPACA_MEMORY_EVAL_SYSTEM=D OPENALPACA_MEMORY_EVAL_RECALL_MODE=$mode \
  OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-D-$mode.json \
    cargo test -p openalpaca_core --features live-eval \
    --test memory_eval -- --ignored memory_eval --nocapture
done
python3 scripts/eval/memory/compare.py /tmp/memory-eval/report-D-index.json \
  /tmp/memory-eval/report-D-excerpts.json
# Accuracy is compare.py's; the cost side is each report's totals:
# reader_tokens_in_mean_per_turn / _median_per_turn, reader_rounds_mean_per_turn,
# tool_calls_mean_per_turn (see "Reading a report"). For retrieval, read
# rows_with_every_relied_note_read_or_excerpted and each row's
# relied_notes_read_or_excerpted: relied_notes_delivered (and its totals) counts a
# title-only memory_search or memory_timeline row as delivered, so it does not
# compare the modes like for like; relied_notes_read_or_excerpted (a memory_read
# body, a snippet or a recall excerpt) does. relied_notes_read alone counts only
# memory_read bodies, which an excerpts run rarely needs.

# E (D bounded to two follow-up searches) and none (no memory): the same, with
# OPENALPACA_MEMORY_EVAL_SYSTEM=E or none and their own report paths.

# Compare (candidate first, baseline second). The archived baselines were
# graded by qwen3.8:27b-mtp-q8_0: compare.py refuses a report graded by another
# judge until one side is re-judged (below), or --allow-judge-mismatch says so.
python3 scripts/eval/memory/compare.py /tmp/memory-eval/report-D.json \
  scripts/eval/memory/reports/baseline-B-2026-09-24.json
python3 scripts/eval/memory/compare.py /tmp/memory-eval/report-D.json \
  scripts/eval/memory/reports/baseline-A-2026-09-24.json --margin -10
python3 scripts/eval/memory/compare.py --self-test   # the judge refusal, the recall-mode printout
```

Optional variables:

| Variable | Effect |
|---|---|
| `OPENALPACA_MEMORY_EVAL_QUESTIONS` | Another question file (the tuning split, or a subset). Default: `questions-heldout.json`. With `LEARN_FROM`, naming a file makes the replay write the conversation summaries and ask it (the episode split on a replay, below); a replay without it asks nothing. |
| `OPENALPACA_MEMORY_EVAL_LIMIT` | Take only the first N selected questions — for a smoke run. Recorded in the report. |
| `OPENALPACA_MEMORY_EVAL_CATEGORY` | One category only. Recorded. |
| `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` | Judge model id; default: the reader's model, the router's effective default (owner answer LQ1). `none` makes no judge call: every row is `not_judged` (the dry run below). See [The judge](#the-judge). |
| `OPENALPACA_MEMORY_EVAL_KEEP` | `1` keeps the tempdir (the scratch DB and project, with the project partition) and prints its path. L always keeps it. |
| `OPENALPACA_MEMORY_EVAL_SESSIONS` | L, L0, L-S: feed only the first N corpus sessions — a smoke run. Recorded (`conditions.learning.session_limit`). |
| `OPENALPACA_MEMORY_EVAL_LEARN_FROM` | With `SYSTEM=L`: **replay** the learning phase of the L run whose scratch directory this names. See below. |
| `OPENALPACA_MEMORY_EVAL_REJUDGE` | An archived report to **re-judge**; `SYSTEM` is then not needed, and neither is a store. See below. |
| `OPENALPACA_MEMORY_EVAL_RECALL_MODE` | `index` or `excerpts`: the `[orchestrator.memory] recall_mode` of the run (design 09 §13.2). D, E and the learning systems' questions recall in it; a replay without a question file, or a `none` run (no memory service), records it only. Unset: the config's default, `index`. Any other value refuses the run before any model call, naming the two. Recorded (`conditions.recall_mode`). |

The data-validation and harness unit tests run without any of this:

```sh
cargo test -p openalpaca_core --features live-eval --test memory_eval
```

They check ids, categories, languages, note kinds and scopes, `fact_key`
grammar, validity timestamps, `supersedes` links (at most one per note, in the
same scope and project, because the service's `supersedes` names one note in
the same partition), that `created_at` is RFC 3339, that the seed order is
chronological and stores a replaced note first, that every note maps to a
store input in that order, that every `relies_on` resolves, that the two
splits share no id, that the judge prompt still carries its five placeholders,
that E's caps express the two-search bound, and that A, B and C point at their
archive or refuse. For L8's recall-mode comparison (design 09 §13.4) they
check that `OPENALPACA_MEMORY_EVAL_RECALL_MODE` reaches every system's config
and the report's `conditions.recall_mode` (a `none` run and a replay's
conditions, offline) and that any other value is refused before anything
else, and — on the stub reader, over three notes a question relies on — that
an index-listed note counts as `listed`, not delivered, until a `memory_read`
in the same turn delivers it, and a title-only search counts as delivered but
not as read; and that the recorder around the read tools leaves every name,
definition and result as the unwrapped tool gives it. The learning evaluation's own tests run beside
them on an offline stub model — no network, nothing reaches `localhost:11434`: the judge
guard (an unroutable judge refused; the reader, or its family, accepted and
recorded; a judge answering as another model caught), the coverage verdict and
prompt, the change-pair, restatement and alias measures, the invariants
(a forced re-run that checked nothing fails), the L-S seed set, a re-judge
of a fixture-sized archived report and a replay of a fixture-sized scratch
database, both with no reader call — and the live phase's own path: two short
sessions fed through the gateway as a run builds it (every turn mapped to its
row, the flushes at the 10th turn and at each session's end, the reader's
calls read off the bus, then the forced re-run), and L-S's suggestion raised,
nothing retired, and the user's confirm retiring the old note. The L8 fix
slice's tests run there too: the summaries' daily cap lifted and a capped or
erroring feed warned about; a `switch` and a summary that stop `BadOutput`
flushed again at once (a `turns` window waits for the session's next
flush, and which labels are retried is pinned), a switch and a summary over
two chunks each retried until each chunk is read or skipped, and a forced
re-run whose window the queue skipped after its unusable answers unchecked;
a flush record keeping each unusable answer's excerpt; unusable answers,
skipped windows and a window a session's last flush left unread counted and
warned about; flush latency read over attempts; every learning measure
split by the path that wrote the note (a stub reader that calls `memory_store` beside a stub
extractor); the coverage judge's own cap and its `none`/`unparsed` totals;
a pair whose one matched note supersedes the old one, credited only when the
judge, asked again without it, names the old note, and those checks totalled
with a failed one told apart from a `NONE`; a restatement the reader
wrote or no flush read (a turn in a skipped window unread whatever a later
flush read); a replay's conditions and its labelled totals; a replay
that writes summaries and asks a question file; and the bus events a lagged
subscription lost — a fed turn's, a flush's, a question turn's, on a bus of
one slot — counted, totalled and warned about. The stub
router's rate limits are lifted, so the tests time the harness, not the
default 40 000-TPM bucket. Plain `cargo test -p openalpaca_core` does
not compile this file at all; it compiles and runs `memory_eval_corpus`, the
corpus's validation.

### A dry run with no model

Seeding, the per-turn recall and the report can be checked without spending
anything: point `OPENALPACA_CONFIG_DIR` at an `llm.toml` whose providers are
all `enabled = false` and whose `[cli_backends.claude_code]` and
`[cli_backends.codex]` say `enabled = false`, and set
`OPENALPACA_MEMORY_EVAL_JUDGE_MODEL=none` — the judge guard refuses a judge
the router cannot route, and with no provider there is none. Every turn then
ends at once with "No model is available" and no row is judged
(`not_judged`, so `compare.py` pairs nothing), but `conditions.notes_seeded`, each row's
`memory_used` and `relied_notes_delivered`, and
`totals.rows_with_every_relied_note_delivered` are real: they measure what the
per-turn recall alone delivers. Nothing in such a run can open a connection;
plan §6's `sandbox-exec` profile (`(deny network-outbound)`) can prove it.

## The learning evaluation

Design 09 §7. The questions and the judge are D's; what changes is where the
store comes from.

**The learning phase goes through the front door.** `Orchestrator::handle_message`
persists nothing, so L, L0 and L-S build a `Gateway` with persistence over the
orchestrator (the daemon's `OrchestratorHandler`, copied) and a learning queue
in `Manual` mode: no sweeper, no quiet rule, and no trigger starts a flush, so
the reader's timing cannot change what is learned when. Each corpus session is
opened as "New chat" opens one, a project session with the scratch project as
its workspace (Lantern and Moss share it, as the questions do), and every turn
goes through `Gateway::handle_event` — persisted, answered by the reader
(unattended), fed to the queue. The harness replays the production triggers
deterministically: `flush_session` after every 10th user turn of a session
(`turns`) and at its end (`switch`). The idle trigger has no deterministic
stand-in; L1's tests cover it. The extraction's daily caps are lifted
(`extract_max_calls_per_day = 0`), and so is the conversation summaries'
daily count, `[orchestrator.learning] episode_max_per_day`, to the most the
config's validation accepts (1000; never 0, which writes none): the whole
corpus is fed on one UTC day, and at the default of 20 every summary from
the corpus's 21st session on stopped `Cap` (L8, before the fix slice). All
three are recorded (`conditions.learning`), so a cap never stops the phase
part-way; if one ever does, the run says so first (`learning.warnings`,
printed as `!! WARNING` at the head of the summary block), as it does for a
fed turn that ended in an error (`learning.cost.feed_turn_errors`).

**A flush whose answer is unusable is flushed again**, as the production
queue retries a held window (ruling LR20): a `switch`, `replay`, summary or
forced re-run flush that stops `BadOutput` is flushed again at once —
`Manual` mode waits out no backoff — with the same request, until the
product has read or skipped each of its windows. The queue counts unusable
answers per cursor position and, on the product's `MAX_ATTEMPTS`-th (three),
skips that chunk and goes on to the next in the same call, so a flush over
several chunks may take more than three attempts — at most three a chunk
(since the fix slice's repair round 2: three in all left a later chunk held
after one answer, and never read). **A `turns` flush is not**: production
holds its window under a doubling backoff, so in a conversation that goes
on its next attempt reads the window grown, and the harness's stand-in is
the session's next flush (the next `turns`, or its `switch`), whose attempts
the queue counts on from the held one — in L8, three `turns` windows that
stopped `BadOutput` were read cleanly, grown, by their session's `switch`.
The stand-in's pace is not production's: a daemon retries after 60 s and
then 120 s, at a 20 s quiet gap — on a local model about one to three turns
later — where the harness retries at the session's next tenth turn or its
switch, so its retry sees a window grown further and, if every attempt
fails, skips a larger one; which way that biases coverage is unknown. Each flush record covers all
its attempts: `attempts`, `attempt_stops` (each attempt's stop), the counts
and tokens summed, `stopped` the last attempt's, and `unusable_answers`, each
answer the queue could not use with its reason (`cut at max_tokens (<n>)`
when the model stopped at the output ceiling, for an empty completion the
router's error — its `finish_reason` null — and the parser's reason
otherwise), its length, its finish reason and its first and last 400
characters — kept in the report only, never persisted by the product. The
third unusable answer skips the window, as in production. Read the retries
honestly: at temperature 0 the same prompt mostly fails the same way again
(L8's sceptics: 13 of 13 same-prompt pairs gave the same outcome), so an
immediate retry of a window that cannot grow — a finished session's — does
not rescue it. It is also the stricter retry: a daemon's retries read the
same rows but not the same prompt, since each attempt rebuilds its
known-notes section and a daemon retries 60 s or more later, after the next
conversation's learning — so the harness rescues fewer such windows than a
daemon might. Nothing perturbs the prompt to make it pass. A session whose
last flush still ended with a stop — whatever the stop — left a window
unread when the run ended: `learning.cost.windows_left_unread` (and
`episode_windows_left_unread`) counts it and `learning.warnings` names it.

Then learning is switched off and the questions are asked as D asks them. The bus is read around every flush and every turn, so the
report counts the two learning paths apart: `extraction` (the flushes) and
`model_tool` (the reader's own `memory_store` calls).

- **L** — learning on. At each session's end, after its `switch` flush, L
  also writes the session's **conversation summaries** (design 09 §13.3:
  `flush_episodes`, one `episode` note per stretch of at least 4 of the
  user's turns) — the one deterministic stand-in for the quiet period, the
  switch and the stop drain. Every fact measure below leaves the summaries
  out (`learning.learned` and the coverage search skip them); they are
  dumped apart as `learning.episodes`. After the questions, a **forced re-run**: the first
  learned note still active is forgotten, its session's cursor moved back to
  0, and the session flushed again (retried like any flush); a note citing a
  forgotten row is a resurrection. A re-run that could not run, whose flush
  made no extraction call, stopped, or ended with its window skipped after
  `MAX_ATTEMPTS` unusable answers fails the run: it checked nothing —
  `learning.invariants.forced_rerun_unchecked` says why (`unchecked after 3
  unusable answers`), apart from `forgotten_resurrected`.
- **L0** — the same feed, never flushed: only the reader's own writes learn.
- **L-S** — the change pairs' 8 old notes seeded as the user's own, then only
  `suggestion_sessions` (the 8 changes and the old notes' restatements).
  Four of those gold notes are written in hindsight ("Until June 2026 …",
  "Replaced by Helix", a `valid_to`: editor, laptop, commute, Moss's output
  directory), and a saved note that already records its own change lets a
  correct extractor read the change turn as a restatement of it. L-S seeds
  those four as the user would have saved them then — a present-tense title
  and body, no `valid_to` — and records their ids
  (`conditions.learning.seeded_as_saved_then`).
  Expected: 8 suggestions naming the right notes, 0 of the user's notes
  retired. Each listed suggestion is then confirmed through the service's
  user-directed confirm with its `target_revision`, and each old note must end
  `superseded`. No questions.

**Replays.** The reader answers at its default temperature, so the
transcript the extractor reads differs from run to run. An L run therefore
keeps its scratch directory (`conditions.learning.scratch_kept`: the
database, the scratch project and `learning-transcript.json`, the turn-to-row
map), and a replay learns that fixed transcript again with **no reader call**:

```sh
OPENALPACA_MEMORY_EVAL_SYSTEM=L \
OPENALPACA_MEMORY_EVAL_LEARN_FROM=/path/printed/as/scratch-kept \
OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-L-replay-1.json \
  cargo test -p openalpaca_core --features live-eval \
  --test memory_eval -- --ignored memory_eval --nocapture
```

A replay copies the kept database into a new tempdir, gives it a fresh owner
and a fresh scratch project (so both partitions start empty and the kept
scratch is only read), moves every cursor to 0, and flushes each stored
session whole, in order. It measures the learning side as L does, then the
forced re-run; it asks no question unless `OPENALPACA_MEMORY_EVAL_QUESTIONS`
names a file (below).

**A replay is extraction-only** (`conditions.learning.replay_is_extraction_only`).
It makes no reader call, so its store holds the learning queue's notes and
none of the live reader's own `memory_store` notes — which were most of
L8's live store (L: 80 of 116 active learned notes). The spread over three
replays is therefore the **extraction path's** variance on a fixed
transcript, never a sample of the pooled figures. Beside a live run it
compares **per note only**: the shares of active extraction notes with an
other-script alias, with any alias and with a key
(`aliases_by_surface.extraction`, `fact_keys_by_surface.extraction`, over a
different set of notes). A live run's `coverage.by_surface` and
`noise_by_surface` are not the extraction path's own coverage and noise:
the coverage judge names one note per gold, so a gold both paths learned is
credited to whichever note it named (L8's L: 34 of the reader's 58 matches
also had an extraction note among the hits) and the note it passed over
counts as noise; and the live extractor, listing the reader's notes among
the known ones, writes fewer notes (L: 36 active extraction notes against
the replays' 107–115, none dropped as already learned). L8's live
`extraction` coverage of 12 of 83 against the replays' 60–66 is that
attribution and pre-emption, not variance. A live run is the only sample of
the whole system — reader and extractor together; a second live run
(L-episodes was one) is the way to see its variance. Reader writes are not
re-applied in a replay: that would need a live feed that records them.

**What a learning report adds** (`learning`, `conditions.learning`):

| Field | What it is |
|---|---|
| `learning.warnings` | What to read first: fed turns that ended in an error, learning or summary flushes a daily cap stopped, each session whose learning or summary window was skipped after `MAX_ATTEMPTS` unusable answers (its turns never learned or summarised), each session whose last learning or summary flush ended with any other stop, with the stop (a window left unread when the run ended), and the bus events a lagged subscription lost (`learning.cost.events_lagged`), each subscription named — and, after the forced re-run, its own lost events, which could hide a resurrection from its check |
| `learning.turns` | Every fed turn: its corpus place and mode, its user row, the reader's calls, tools and own memory writes (`model_tool_memory_changes`, and the notes, `model_tool_notes`), all read off the bus, and `events_lagged`, the events its subscription lost to lag (those counts may be short by them) |
| `learning.flushes` | Every flush of the learning phase: its label (`turns`, `switch` or `replay`; the forced re-run's record is `learning.forced_rerun.flush`), `turns_fed` (a fed flush's place), `cursor_before` and `cursor_after` (the session's learning cursor, a message id, around it: the rows it moved past), its `attempts`, `attempt_stops` and `attempt_latencies_ms`, latency (over every attempt), the queue's counts per §2.4 outcome summed over the attempts, the notes it wrote (from its `memory_changed` events; `events_lagged`, the events its subscription lost), and `unusable_answers` |
| `learning.episode_flushes` | L, and a replay asked questions: every conversation-summary flush (label `switch`, or `replay`): its attempts, calls, the summaries written, the windows it skipped after `MAX_ATTEMPTS` unusable answers (`skipped_unusable`), tokens, latency, the notes it wrote and its unusable answers |
| `learning.learned` | The store after the phase, history and candidates included: status, origin, key, aliases, validity, sources, links, knowledge and `surface` — `extraction` when a learning flush's `memory_changed` created it, `model_tool` for any other learned note (the reader's `memory_store`) — conversation summaries left out |
| `learning.episodes` | L, and a replay asked questions: the conversation summaries the phase wrote, in the same shape |
| `learning.coverage` | Per relied-on gold note: a historical search on its title and aliases (at most 5 notes) and the coverage judge's `MATCH <n>` or `NONE`; `share` is the fact coverage; `none`, `unparsed` (a reply with no verdict line — an unread row, not a miss; `unparsed_cut_at_max_tokens` of them ran to the cap), `not_found` and `judge_errors` (a failed judge call or a failed search — the row's `judge_error` begins `search:` for the second) counted apart; `by_surface`, `matched` by the writer of the note the judge chose — in a live run the judge's single pick, not a path's own coverage (above). The judge's cap is its own, `conditions.learning.coverage_judge_max_tokens` (2048: at the QA judge's 400 every L8 unparsed verdict was a deliberation cut before its line). Against a report from before it (no `coverage_judge_max_tokens`), coverage can gain up to that report's `unparsed` count from the cap alone — L8's L 2, its replays 5–8 (up to +9.6 pp) — unless the old store is judged again: at temperature 0 a verdict that ended under 400 tokens is unchanged |
| `learning.noise`, `learning.noise_by_surface` | Active learned notes no coverage call chose, and the candidates raised — pooled, and by path (in a live run, a path's note the judge passed over for the other path's counts as its noise) |
| `learning.changed_facts` | The 8 pairs and the 7 corrections, each with its outcome (`new_active_old_superseded`, `new_active_old_same_key_loser`, `both_active_unlinked`, `new_missing`, `old_never_learned`, …) and whether the two notes share a key. When the judge matched one note for both golds and that note is active, no same-key loser, and supersedes a superseded note the old gold's search returned, the link is checked, not credited (since the fix slice's repair round 2: a link says what the store replaced, not what the note states): the coverage judge is asked about the old gold once more, with the same prompt and cap, shown its hits without the new note — at most one call a pair — and the pair is scored from that pick as any pair is: a superseded note (the linked one or another) gives `new_active_old_superseded` and a same-key loser `new_active_old_same_key_loser`, both correct; any other active note, `both_active_unlinked`; `NONE`, a reply with no verdict line, a failed call or no judge, `old_never_learned`. A checked pair is one whose `link_check` is set (the superseded note, the candidates, the verdict, the pick, the reply and its tokens); its `old_note_from` is `link_check` only when the check named a note — otherwise it stays `judge`, the old note being the new one — and its key reuse (`fact_keys.pairs_key_reused` of `pairs_with_two_notes`) is read over the note the check named. `learning.link_checks` totals the checks: `checks`, `calls` (all but `not_judged`), `credited`, the verdicts — `matched`, `none`, and apart from `none` the checks that `failed` with no verdict (`unparsed`, `unparsed_cut_at_max_tokens` of them, and `judge_errors`), which credit nothing without being the judge's reading — `not_judged`, and the judge tokens, which `coverage.judge_tokens_*` (the coverage rows' alone) leave out. The printed line carries them |
| `learning.restatements`, `learning.restatements_by_surface` | Per `restate` turn: `nothing_written`, `automatic_replacement`, `suggestion`, `duplicate_note`, `first_learned` from the notes citing its row; where none does, `model_tool_write` (the reader wrote a note during the turn — `reader_notes`; its notes mostly carry no sources) or `extraction_failed` (no flush of the session moved its cursor across the turn's row without skipping a window: a flush reads from the cursor, so a turn an earlier flush skipped past stays unread whatever a later flush read, and a flush that skipped a window counts none of its range read), and `nothing_written` only for a turn extraction read and wrote nothing for. `by_surface` gives each path's own outcome; the tally is pooled and by path |
| `learning.aliases`, `learning.fact_keys`, `…_by_surface` | The share of active learned notes with an alias in the other script (CJK against Latin), with any alias, and with a key — pooled, and by path |
| `learning.suggestion_check` | L-S: per seeded note, its suggestions and its status before and after the confirm |
| `learning.forced_rerun` | L and a replay: the note forgotten, its sources and session, the re-run's own flush record (`flush`, label `forced_rerun`, with its attempts, unusable answers and `events_lagged`), the notes it wrote citing a forgotten row (`resurrected`) or with the forgotten note's title or key from other rows (`rewritten_from_other_rows`), and `skipped` — why no check ran. Its counts are in no `learning.cost` total |
| `learning.invariants` | User notes retired without a confirm, forgotten notes resurrected — both must be 0 — and `forced_rerun_checked`: the re-run forgot a note and its flush made an extraction call and read its session to the end (no `stopped`, no window skipped). For L and a replay a re-run that checked nothing (skipped, no call, a flush that stopped — an unusable answer, no model — or one whose window the queue skipped after `MAX_ATTEMPTS` unusable answers) is a violation, never a 0 that reads as a pass; `forced_rerun_unchecked` says why. A violation fails the run after the report is written |
| `learning.cost` | Extraction calls and tokens, flush latency p50/p95 (over attempts, each one flush as a daemon runs it — as L8 measured it; a trigger's retries summed would read up to three times as long for each chunk), flushes by trigger, the conversation summaries' calls, count and tokens, memory changes by surface, the reader's feed cost and its turn errors (`feed_turn_errors`, `feed_turn_errors_at`); the bus events the phase's subscriptions lost to lag — the fed turns', the flushes' and the summaries' — and each subscription that lost any (`events_lagged`, `events_lagged_at`: non-zero means a note a flush created may count as the reader's in the surface split, and a reader write may be missing from a restatement's `model_tool_write`; `learning.warnings` says so); and, over every attempt of the learning phase (the forced re-run's are in its own record), for learning and for summaries apart (`episode_…`): `flush_attempts`, `unusable_answers` and `unusable_answers_by_reason` (`cut at max_tokens`, `empty completion`, or the parser's reason) and `windows_skipped_unusable`; and `windows_left_unread` (`episode_…` too): the sessions whose last learning (or summary) flush ended with a stop, a cap's among them — a window each left unread when the run ended. Count failures and unread windows from these, never from `flushes[].stopped` alone: a retried flush's `stopped` is its last attempt's, a window skipped after `MAX_ATTEMPTS` unusable answers ends with none, and a `turns` flush's stop is read again by its session's next flush |

The L8 fix slice reclassifies as well as adds (design 09 rev 3.26): H6
moves a pair whose one note supersedes the old one to
`new_active_old_superseded` (and into the pairs' key reuse) when the
judge, asked again without that note, names the old one (rev 3.28), and H7
moves a restatement no note cites to `model_tool_write` or
`extraction_failed`. A new report's `changed_facts_correct`,
`fact_keys.pairs_*` and `restatements_by_outcome` compare with an archived
L8 report's only through rev 3.26's re-scored figures (L's 10 of 15 is 11 of
15 under the new rule — if the check names the superseded note, as a
reading of the candidates suggests; offline it is 10 to 11), never as an
improvement.

The proposed targets (coverage ≥ 80 %, changed facts ≥ 12 of 15, other-script
aliases ≥ 60 %, restatements raising a suggestion or a duplicate ≤ 2, L within
−10 pp of D′) are never read against one run. The four samples are two
systems: the replays' spread is the extraction path's, to be read beside a
live run's per-note `extraction` shares only (above), and the whole system —
the pooled figures — has one sample per live run. Which of the two a target
applies to is the owner's question, not the harness's: the report gives
both.

**The episode split** (design 09 §13.4, for L8) is answered after a learning
run: an L run with `OPENALPACA_MEMORY_EVAL_QUESTIONS` naming
`questions-episodes.json` (an absolute path: the test runs in the crate's
directory) writes the summaries as its learning phase always
does, then asks the 20 questions as D asks them — with learning off, so the
reader finds a summary only through `memory_search` or `memory_timeline`:
automatic recall never delivers one. `conditions.learning.summaries` records
that the run wrote them.

```sh
OPENALPACA_MEMORY_EVAL_SYSTEM=L \
OPENALPACA_MEMORY_EVAL_QUESTIONS="$PWD/scripts/eval/memory/questions-episodes.json" \
OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-L-episodes.json \
  cargo test -p openalpaca_core --features live-eval \
  --test memory_eval -- --ignored memory_eval --nocapture
```

**The episode split on a replay**, without the three-hour live feed: a replay
with `OPENALPACA_MEMORY_EVAL_QUESTIONS` naming a file also writes each
session's conversation summaries at its end (label `replay`, as L does at
its `switch`) and then asks the questions as D asks them, learning off, before
its forced re-run (`conditions.learning.summaries` is then `true`):

```sh
OPENALPACA_MEMORY_EVAL_SYSTEM=L \
OPENALPACA_MEMORY_EVAL_LEARN_FROM=/path/printed/as/scratch-kept \
OPENALPACA_MEMORY_EVAL_QUESTIONS="$PWD/scripts/eval/memory/questions-episodes.json" \
OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-L-episodes-replay.json \
  cargo test -p openalpaca_core --features live-eval \
  --test memory_eval -- --ignored memory_eval --nocapture
```

It is not like for like with a live L-episodes run: the replay's store is
extraction-only (none of the live reader's `memory_store` notes, which
answered two of L8's twenty episode questions), its transcript is the
kept L run's, not L-episodes' (the live reader's replies differ run to run),
and its summaries are uncapped (H1) where L8's L-episodes had 27 summary
flushes stopped by the daily cap of 20 — every session from the 21st on
had no summary. The report's `system` is `L`: its printed totals say
`L (a replay of …: extraction-only store)`, and `compare.py` labels it
`(system L, replay)` and warns that it is not a live L run.

## The judge

**Owner answer LQ1 (2026-09-24): the reader judges, and the report says so.**
With no `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` the judge is the reader's own
model (`qwen3.8:27b-mlx` on the test machine). Every report records
`conditions.judge_model`, `judge_is_reader`, `judge_same_family` (the id
before `:` is the reader's) and `judge_is_extractor`; a report with
`judge_is_reader: true` is a model grading its own answers, and anything
written from it must say so.

The guard runs before any call and refuses only a judge the router cannot
route — the ladder would silently answer with another model in its place. It
does **not** refuse the reader or its family: it records them. Every judge
call pins the judge's id, and the model that answered is checked against it:
on the first call a mismatch stops the run, later it fails that row
(`judge_error`). After the questions, a row answered by any model but the
reader fails the run, once the report is written.

**Re-judge** grades an archived report's rows again — question, gold answer
and answer as recorded — with this run's judge and the frozen prompt, and no
reader call. The judge is the eval's `llm.toml` default model unless
`OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` names another, and `judge_is_reader` /
`judge_same_family` compare it with the archived report's reader:

```sh
OPENALPACA_MEMORY_EVAL_REJUDGE=$PWD/scripts/eval/memory/reports/system-D-prefix-2026-09-24.json \
OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-D-rejudged.json \
  cargo test -p openalpaca_core --features live-eval \
  --test memory_eval -- --ignored memory_eval --nocapture
```

The new report keeps the archived one's system, rows and reader figures;
each row gains `original_judge_verdict` and `original_judge_model`, the
totals are recomputed and gain `agreement_with_original` (raw and Cohen's κ
over the rows both judges decided), and the report records `rejudged_from`
and its SHA-256. `OPENALPACA_HOME_STORE` is not needed.

## Cost

Per system: 470 reader turns (one main-loop call each, more when the model
calls a tool) plus 470 judge calls of about 600 input tokens. On a local model
the cost is time, not money. At Haiku-class pricing expect roughly **$1–3 per
system**; at Sonnet-class roughly $5–15. The report records the measured
figures (`totals.reader_cost_usd`, `totals.judge_cost_usd`, tokens in/out,
latency p50/p95). The per-turn loop cost cap is $1 (`LoopConfig::default`), so
no single turn can run away. A smoke run with `OPENALPACA_MEMORY_EVAL_LIMIT=10`
costs cents.

On the local test model the cost is time (design 09 §7.5): L ≈ 4 h (300 fed
turns, about 40 flushes, 470 questions, 83 coverage calls), a replay ≈ 0.5 h,
L0 ≈ 3.3 h, L-S ≈ 0.5 h. Since the L8 fix slice a `switch`, `replay`,
summary or forced re-run flush that stops `BadOutput` is retried up to
twice more for each of its chunks (L8's fed flushes stopped `BadOutput` 11
times in L — 8 `switch`, 3 `turns` — and 16 in L-episodes; a `turns` one
waits for its session's next flush), an
extraction answer may run to 2,892 output tokens instead of 2,800, a
coverage verdict to 2,048, and H6's link check makes at most one more
coverage-judge call a pair — 8 a run, each up to 2,048 output tokens, on
top of the 83 (`learning.link_checks`). `OPENALPACA_MEMORY_EVAL_SESSIONS=2` with
`OPENALPACA_MEMORY_EVAL_LIMIT=5` is a smoke run of the whole L pipeline.

## Reading a report

One JSON file, rewritten after every question (a run that dies leaves
`"status": "running"` with the rows so far). The schema is the slice-1 one
(`"schema": 1`) with fields **added**, never changed, so `compare.py` pairs a D
or E report with the archived A and B reports:

- `status`: `complete` or `running`.
- `machine` (hostname, OS, arch, rustc), `build` (git head, `Cargo.lock`
  SHA-256, features), `conditions` (the owner, the `llm.toml` path and hash,
  reader default model, the main loop's round and tool-call caps, the
  tool-result byte cap and the context buffer ratio from the effective
  `DaemonConfig`, judge model/temperature/prompt hash, notes and questions
  paths with hashes, how many notes were seeded — and how many as a
  replacement — what was on the tool surface, the scratch store; new with 3.11:
  `system_description`, `memory_tools_offered`, `follow_up_search_bound`, the
  retrieved byte budget `recall_total_bytes` / `recall_pinned_bytes` /
  `recall_max_items` / `read_max_bytes`, `notes_scope_applied`,
  `notes_superseding` and `seeded_ids`; new with Learning v2's L10b (design 09
  §13.2, `3db79ce1`): the per-turn recall's `recall_mode`, `index` or
  `excerpts`) are the run's recorded conditions — the
  report is the record; nothing is a measurement without them. `embedder` is
  B's E5 record in the archived reports and always `null` from D on.
- `totals`: `correct`/`incorrect`/`unparsed`/`judge_errors`/`turn_errors`,
  `accuracy`, `by_category`, `by_language`, reader and judge tokens and cost,
  the models that answered, `memory_search_calls`,
  `rows_with_per_turn_fts_hits`, latency p50/p95/mean/max and wall time; new:
  `rows_with_every_relied_note_delivered` and `memory_write_calls`; new for
  L8's recall-mode comparison (design 09 §13.4):
  `rows_with_every_relied_note_listed_or_delivered` (every relied-on note
  delivered or listed — an `index` run's retrieval, where the field before it
  asks for the bodies), `rows_with_every_relied_note_read_or_excerpted`
  (every relied-on note excerpted or read — the like-for-like retrieval
  measure between the two modes), and the cost per question turn —
  `reader_tokens_in_mean_per_turn`, `reader_tokens_in_median_per_turn` (every
  round's input tokens), `reader_rounds_mean_per_turn` (model calls, ≈ rounds) and
  `tool_calls_mean_per_turn`; `null` with no rows; new with the L8 fix
  slice's nits round: `events_lagged` and `events_lagged_at` — the bus
  events the question turns' subscriptions lost to lag, and the rows that
  lost any (`<id> (<n>)`): those rows' tools, model calls, tokens and cost,
  and so `memory_search_calls` and `memory_write_calls`, may be short;
  printed as `!! WARNING` under the totals when non-zero.
- `rows`: one per question — the question, gold and actual answer, the judge's
  verdict and raw reply, latency, `ack_ms`, the routing `mode`, reader LLM
  calls/tokens/cost, the tools called, `memory_search_calls`,
  `per_turn_fts_hits` (for A, the old FTS query's result count; from D on, the
  notes the per-turn recall delivered **with their content** — reason `recall`
  or `pinned`, never an index line or a tool's fetch); new: `memory_used`
  (`note` — the gold id when it is a seeded note —, `gold`, `reason`: `pinned`,
  `recall`, `index` or `tool`) and `relied_notes_delivered` (relied-on notes
  whose content reached the model: any reason but `index`, the meaning of the
  archived reports; `tool` is any read tool's, so a `memory_search` or
  `memory_timeline` index row, which carries a title and no body, counts as
  delivered too); new for L8: `relied_notes_listed` (relied-on notes only an
  index line named — no content in that turn; a `tool` entry for the same
  note, a `memory_read` of it above all, makes it delivered),
  `index_lines` (entries with reason `index`), `memory_reads` (the notes
  `memory_read` returned with a body), `memory_listings` (each
  `memory_search`/`memory_timeline` call: `tool`, `detail` — `index` or
  `snippets` — and its `notes`), `relied_notes_read` (relied-on notes whose
  body `memory_read` returned), `relied_notes_read_or_excerpted` (relied-on
  notes that reached the model as more than a title: a `recall` or `pinned`
  entry, a `memory_read` body or a `snippets` excerpt) and
  `reader_rounds` (the model calls the router booked to the main loop in the
  turn, ≈ its rounds: a tool round, the answering round, the answer guard's
  corrective round and a max-tokens continuation each add one, and an in-loop
  compaction call adds one that is no round; a call that failed and was
  retried is missing, since the router books responses, not attempts).
  `reader_llm_calls` counts the main loop's
  `LlmCallCompleted` events, which it publishes **once per turn** with the
  whole turn's tokens — it is 1 on every answered row, the archived reports'
  included, whatever the rounds; read `reader_rounds` for those. New with the
  L8 fix slice's nits round: `events_lagged`, the bus events the turn's
  subscription lost to lag — its `mode`, `tools` and reader calls, tokens and
  cost are read off the bus and may be short by them (`totals.events_lagged`).
- New with the learning evaluation: `conditions.judge_model`,
  `judge_is_reader`, `judge_same_family`, `judge_is_extractor`, and
  `conditions.learning` (the corpus and its hash, what was fed, the flush
  rule, the lifted caps, the coverage prompt's hash, the kept scratch);
  `extraction_disabled` is now derived from it (true when no learning ran);
  and the top-level `learning` object described above. D and E reports carry
  both as `null`.

`unparsed` (the judge's last line was neither word) and `judge_error` rows
count as not correct in `accuracy`; `compare.py` pairs only rows both reports
judged, so a systematic judge failure shrinks `n` rather than hiding. A single
run is a measurement, never a verdict; the gate is the paired CI from
`compare.py`, which also flags any category more than 10 pp worse
(paraphrase and the Chinese rows are the ones to watch).

## Extending the set

Keep ids unique across both files, `expect_abstain` true exactly for
`abstention` rows, at least one `relies_on` for every other row, notes' ids as
hyphenated lowercase UUIDs, `fact_key` in `[a-z0-9][a-z0-9._-]{0,63}`, and at
most one `supersedes` per note, naming a note in the same scope and project.
Add gold notes first, then questions that cite them. Do not tune on the
held-out file — that is what the tuning split is for.
