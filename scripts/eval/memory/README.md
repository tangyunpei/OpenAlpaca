# Memory Eval Harness (memory redesign, plan slice 1 / §5; rewritten for the new engine in 3.11)

A **developer benchmark**, not a test: it seeds a fixed set of gold memory
notes, asks a fixed set of questions through the chat front door, has a model
judge every answer against a gold answer, and writes one JSON report per
system under test. Two reports are compared pairwise to answer the plan's
question — is the new lexical memory service at least as accurate as the old
hybrid engine (−5 pp non-inferiority gate against **B**, or −10 pp against
**A** when B is unavailable)? The old engine is gone; its two baselines were
measured before it was deleted and are archived in `reports/`. Nothing here
runs under plain `cargo test`.

## Layout

| File | Purpose |
|---|---|
| `notes-gold.json` | 83 gold notes about a fictional user (Lin Wei, 林薇) and two fictional projects (**Lantern**, a Rust ledger service; **Moss**, a Python static-site generator), in the shape of design §4's frontmatter (`id`, `title`, `kind`, `status`-less, `fact_key`, `aliases`, `tags`, `valid_from`/`valid_to`, `supersedes`, `created_at`) plus two eval-only fields: `project` and `language`. English and Chinese. Four **changed-fact** pairs carry `supersedes` + `valid_to` (editor, laptop, car→metro, Moss output dir) and four **conflict** pairs share a `fact_key` with no link (staging DB, Moss Python version, Lantern port, weekly-review time). |
| `questions-heldout.json` | **470** questions, the gate set: exact_recall 80, paraphrase 80, cross_language 70, changed_facts_time 55, project_procedures 55, rejected_failed_actions 45, abstention 45, source_conflicts 40 (en 303 / zh 125 / mixed 42). Each row: `id`, `category`, `language`, `question`, `gold_answer`, `relies_on` (gold note ids), `expect_abstain`. |
| `questions-tuning.json` | **150** more, same schema, for iterating on prompts, aliases or ranking. **The gate run never loads it** (it is loaded only through `OPENALPACA_MEMORY_EVAL_QUESTIONS`). |
| `judge-prompt.md` | The frozen binary judge protocol (CORRECT / INCORRECT). Its SHA-256 is recorded in every report; change it and every report before the change is a different experiment. |
| `compare.py` | The paired comparison: accuracy per system, the paired difference with its 95 % CI, the discordant-pair rate, per-category point differences, the gate verdict. |
| `reports/` | The archived baselines of the deleted engine, measured on 2026-09-24 on a local Ollama as reader and judge: `baseline-A-2026-09-24.json` (13.0 %), `baseline-B-2026-09-24.json` (90.6 %), and their comparison. **B is the comparator** for D and E. |
| `crates/openalpaca_core/tests/memory_eval.rs` | The runner: a data-validation test and four harness unit tests (run under `--features live-eval`, no model), and the `#[ignore]`d live eval. |

## How it works

`memory_eval` boots a real `Orchestrator` (real `LlmRouter` from the eval's
own `llm.toml`, a scratch SQLite database in a tempdir, a scratch project
directory with a `.git` marker as the turn's workspace) and, for D and E, the
daemon's own memory service (`MemoryService::open_default`, its index cache
under the scratch store) with the read tools registered the way the daemon
registers them. It creates one owner (a `global_user` row — the service
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
- The background user-trait extraction is switched off for the run
  (`extract_every_n_turns = usize::MAX`, `task_extract_enabled = false`),
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
   `cargo test`. The archived baselines used a local Ollama as reader and
   judge; use the same model for a comparable D or E report.
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

# E (D bounded to two follow-up searches) and none (no memory): the same, with
# OPENALPACA_MEMORY_EVAL_SYSTEM=E or none and their own report paths.

# Compare (candidate first, baseline second) against the archived baselines:
python3 scripts/eval/memory/compare.py /tmp/memory-eval/report-D.json \
  scripts/eval/memory/reports/baseline-B-2026-09-24.json
python3 scripts/eval/memory/compare.py /tmp/memory-eval/report-D.json \
  scripts/eval/memory/reports/baseline-A-2026-09-24.json --margin -10
```

Optional variables:

| Variable | Effect |
|---|---|
| `OPENALPACA_MEMORY_EVAL_QUESTIONS` | Another question file (the tuning split, or a subset). Default: `questions-heldout.json`. |
| `OPENALPACA_MEMORY_EVAL_LIMIT` | Take only the first N selected questions — for a smoke run. Recorded in the report. |
| `OPENALPACA_MEMORY_EVAL_CATEGORY` | One category only. Recorded. |
| `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` | Judge model id; default: the router's default model (the reader's model). Recorded either way, as the id the router reports answering. |
| `OPENALPACA_MEMORY_EVAL_KEEP` | `1` keeps the tempdir (the scratch DB and project, with the project partition) and prints its path. |

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
archive or refuse. Plain `cargo test -p openalpaca_core` does not compile this
file at all.

### A dry run with no model

Seeding, the per-turn recall and the report can be checked without spending
anything: point `OPENALPACA_CONFIG_DIR` at an `llm.toml` whose providers are
all `enabled = false` and whose `[cli_backends.claude_code]` and
`[cli_backends.codex]` say `enabled = false`. Every turn then ends at once
with "No model is available" and every judge call fails (`judge_error`, so
`compare.py` pairs nothing), but `conditions.notes_seeded`, each row's
`memory_used` and `relied_notes_delivered`, and
`totals.rows_with_every_relied_note_delivered` are real: they measure what the
per-turn recall alone delivers. Nothing in such a run can open a connection;
plan §6's `sandbox-exec` profile (`(deny network-outbound)`) can prove it.

## Cost

Per system: 470 reader turns (one main-loop call each, more when the model
calls a tool) plus 470 judge calls of about 600 input tokens. On a local model
the cost is time, not money. At Haiku-class pricing expect roughly **$1–3 per
system**; at Sonnet-class roughly $5–15. The report records the measured
figures (`totals.reader_cost_usd`, `totals.judge_cost_usd`, tokens in/out,
latency p50/p95). The per-turn loop cost cap is $1 (`LoopConfig::default`), so
no single turn can run away. A smoke run with `OPENALPACA_MEMORY_EVAL_LIMIT=10`
costs cents.

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
  `notes_superseding` and `seeded_ids`) are the run's recorded conditions — the
  report is the record; nothing is a measurement without them. `embedder` is
  B's E5 record in the archived reports and always `null` from D on.
- `totals`: `correct`/`incorrect`/`unparsed`/`judge_errors`/`turn_errors`,
  `accuracy`, `by_category`, `by_language`, reader and judge tokens and cost,
  the models that answered, `memory_search_calls`,
  `rows_with_per_turn_fts_hits`, latency p50/p95/mean/max and wall time; new:
  `rows_with_every_relied_note_delivered` and `memory_write_calls`.
- `rows`: one per question — the question, gold and actual answer, the judge's
  verdict and raw reply, latency, `ack_ms`, the routing `mode`, reader LLM
  calls/tokens/cost, the tools called, `memory_search_calls`,
  `per_turn_fts_hits` (for A, the old FTS query's result count; from D on, the
  notes the per-turn recall delivered); new: `memory_used` (`note` — the gold
  id when it is a seeded note —, `gold`, `reason`) and
  `relied_notes_delivered`.

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
