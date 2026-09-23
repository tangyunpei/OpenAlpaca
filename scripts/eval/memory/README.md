# Memory Eval Harness (memory redesign, plan slice 1 / §5)

A **developer benchmark**, not a test: it seeds a fixed set of gold memory
notes, asks a fixed set of questions through the chat front door, has a model
judge every answer against a gold answer, and writes one JSON report per
system under test. Two reports are compared pairwise to answer the plan's
question — is the new lexical memory service at least as accurate as the old
hybrid engine (−5 pp non-inferiority gate against **B**, or −10 pp against
**A** when B is unavailable)? Nothing here runs under plain `cargo test`.

## Layout

| File | Purpose |
|---|---|
| `notes-gold.json` | 83 gold notes about a fictional user (Lin Wei, 林薇) and two fictional projects (**Lantern**, a Rust ledger service; **Moss**, a Python static-site generator), in the shape of design §4's frontmatter (`id`, `title`, `kind`, `status`-less, `fact_key`, `aliases`, `tags`, `valid_from`/`valid_to`, `supersedes`, `created_at`) plus two eval-only fields: `project` and `language`. English and Chinese. Four **changed-fact** pairs carry `supersedes` + `valid_to` (editor, laptop, car→metro, Moss output dir) and four **conflict** pairs share a `fact_key` with no link (staging DB, Moss Python version, Lantern port, weekly-review time). |
| `questions-heldout.json` | **470** questions, the gate set: exact_recall 80, paraphrase 80, cross_language 70, changed_facts_time 55, project_procedures 55, rejected_failed_actions 45, abstention 45, source_conflicts 40 (en 303 / zh 125 / mixed 42). Each row: `id`, `category`, `language`, `question`, `gold_answer`, `relies_on` (gold note ids), `expect_abstain`. |
| `questions-tuning.json` | **150** more, same schema, for iterating on prompts, aliases or ranking. **The gate run never loads it** (it is loaded only through `OPENALPACA_MEMORY_EVAL_QUESTIONS`). |
| `judge-prompt.md` | The frozen binary judge protocol (CORRECT / INCORRECT). Its SHA-256 is recorded in every report; change it and every report before the change is a different experiment. |
| `compare.py` | The paired comparison: accuracy per system, the paired difference with its 95 % CI, the discordant-pair rate, per-category point differences, the gate verdict. |
| `crates/openalpaca_core/tests/memory_eval.rs` | The runner: a data-validation test (runs under `--features live-eval`, no model) and the `#[ignore]`d live eval. |

## How it works

`memory_eval` boots a real `Orchestrator` (real `LlmRouter` from the eval's
own `llm.toml`, a scratch SQLite database in a tempdir, a scratch project
directory as the turn's workspace), seeds every gold note through the
`memory_store` **tool** (the same backend a model tool call reaches), then
asks each question with `Orchestrator::handle_message` — exactly what the
daemon's `POST /v1/chat` handler calls — on a fresh lane per question, as one
owner. The answer text is handed to the judge model with the frozen prompt at
temperature 0, and the last line of the judge's reply is the verdict.

Systems (`OPENALPACA_MEMORY_EVAL_SYSTEM`):

| Id | What the reader gets | How the harness sets it up |
|---|---|---|
| `none` | No memory: the context-cost reference | No notes are seeded; no `memory_search` tool |
| `A` | The **per-turn FTS path** (`prompt_ctx::sources::memory::MemorySource`, FTS-only, whole-message match) | Notes seeded with no embedder; no `memory_search` tool on the surface |
| `B` | The **`memory_search` tool path**, hybrid FTS + E5 vectors | Notes seeded and embedded with the E5 model; `memory_search` registered exactly as the daemon registers it (`builtin_tools`); needs `--features live-eval-e5` |
| `C` `D` `E` | One bounded `MEMORY.md`; the new service; the new service plus follow-up searches | **Arrive with slice 3.** Today the run writes a report with `"status": "unavailable"` and returns. |

### What A and B do and do not isolate (read before quoting a B number)

- **The per-turn path cannot be switched off**: `Orchestrator::new` builds
  `MemorySource` whenever it has a database, and there is no knob. So B runs
  with the per-turn FTS path live *as well as* the tool. To keep that honest
  every row records `per_turn_fts_hits` — the result count of the same
  repository call `MemorySource::resolve` makes for that question (FTS only,
  limit 10, the turn's scope cascade) — and the totals carry
  `rows_with_per_turn_fts_hits`. Today's per-turn query requires every word of
  the message to match, so this is usually 0; when it is not, the B row had two
  sources of recall and the report says so. `memory_search_calls` per row says
  whether the tool was actually used.
- **The old `memory_store` takes one `content` string** and stores everything
  as kind `Preference`, global scope. A gold note is flattened for it as
  `# title`, the body as written, then `Aliases:`, `Tags:` and
  `Valid from/to:` lines — every field the new service will index, so B is not
  handicapped by the flattening. `scope`/`project` are carried in the data but
  not applied: today's workspace scoping needs a `--workspace` marker in the
  content and a per-turn workspace, and every question is asked from one lane.
  Slice 4's harness update passes the fields natively once the tool takes them
  (design §7a) and may partition project notes then.
- **The reader's temperature is not pinned.** The main loop sets no
  temperature (`runner/agentic_loop/backend.rs`), so the reader runs at the
  provider's default; the report records `reader_temperature: null`. The judge
  *is* at temperature 0 with reasoning disabled.
- The background user-trait extraction is switched off for the run
  (`extract_every_n_turns = usize::MAX`), `start_workflow` is refused at
  `max_workflows_per_lane = 0` (so a stray call can never dispatch a paid lead
  run), and every turn is `unattended` (a tool that would need a confirmation
  is refused instead of hanging).

## Prerequisites

1. **Build with the feature.** `--features live-eval` compiles the LLM
   providers into the test binary; `--features live-eval-e5` adds the local
   fastembed backend for B (heavier: it pulls `ort`).
2. **The eval's own `llm.toml`** in `OPENALPACA_CONFIG_DIR`. The harness never
   falls back to the repo's `config/` or to a default. Use `secret_env` keys;
   `secret_ref` (keychain) is not resolved by a test binary. Cloud models by
   default. A local Ollama is reached **only** if this file enables it — that
   is the one way `localhost:11434` can ever be touched, and it never happens
   under plain `cargo test`.
3. **A scratch store** in `OPENALPACA_HOME_STORE`, absolute, never the real
   `~/.openalpaca` (the harness refuses both an unset variable and the real
   path). The only thing written there is B's embedding cache,
   `state/cache/fastembed/`.
4. **CWD outside the repository** when you run the daemon or CLI for anything
   related; the test itself resolves the data files from `CARGO_MANIFEST_DIR`,
   so its CWD does not matter, and it uses a tempdir project as the workspace.
5. `OPENALPACA_LIVE_EVAL=1`. Without it the test exits early even under
   `--ignored` (no calls, no cost).

### The E5 download for B (G12 ruling)

B needs `intfloat/multilingual-e5-base` (about 1 GB) in the scratch store's
`state/cache/fastembed/`. The download is a **one-time, explicitly authorized
step**: when that directory is empty the harness refuses to start B unless
`OPENALPACA_MEMORY_EVAL_ALLOW_DOWNLOAD=1` is set, and records
`embedder.cache_was_empty` and `embedder.download_authorized` in the report.
If the download is refused or fails, the report is written with
`"status": "unavailable"` and the reason, and the gate is taken against A at
−10 pp (plan §5) — say so in the report's first paragraph.

`[embeddings]` in the eval's `llm.toml` overrides the model (`provider`,
`model`, `dimensions`); absent, the harness uses `local` + the E5 default at
768 dimensions.

## Run

```sh
export OPENALPACA_LIVE_EVAL=1
export OPENALPACA_HOME_STORE=/tmp/memory-eval/store       # scratch, absolute
export OPENALPACA_CONFIG_DIR=/tmp/memory-eval/config      # holds llm.toml
mkdir -p "$OPENALPACA_HOME_STORE" "$OPENALPACA_CONFIG_DIR"

# A (per-turn FTS) and none (no memory):
OPENALPACA_MEMORY_EVAL_SYSTEM=A OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-A.json \
  cargo test -p openalpaca_core --features live-eval \
  --test memory_eval -- --ignored memory_eval --nocapture

# B (memory_search tool, E5) — the first run downloads the model:
OPENALPACA_MEMORY_EVAL_SYSTEM=B OPENALPACA_MEMORY_EVAL_ALLOW_DOWNLOAD=1 \
OPENALPACA_MEMORY_EVAL_REPORT=/tmp/memory-eval/report-B.json \
  cargo test -p openalpaca_core --features live-eval-e5 \
  --test memory_eval -- --ignored memory_eval --nocapture

# Compare (candidate first, baseline second):
python3 scripts/eval/memory/compare.py /tmp/memory-eval/report-D.json /tmp/memory-eval/report-B.json
python3 scripts/eval/memory/compare.py /tmp/memory-eval/report-D.json /tmp/memory-eval/report-A.json --margin -10
```

Optional variables:

| Variable | Effect |
|---|---|
| `OPENALPACA_MEMORY_EVAL_QUESTIONS` | Another question file (the tuning split, or a subset). Default: `questions-heldout.json`. |
| `OPENALPACA_MEMORY_EVAL_LIMIT` | Take only the first N selected questions — for a smoke run. Recorded in the report. |
| `OPENALPACA_MEMORY_EVAL_CATEGORY` | One category only. Recorded. |
| `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` | Judge model id; default: the router's default model (the reader's model). Recorded either way, as the id the router reports answering. |
| `OPENALPACA_MEMORY_EVAL_KEEP` | `1` keeps the tempdir (the scratch DB and project) and prints its path. |

The data-validation test runs without any of this:

```sh
cargo test -p openalpaca_core --features live-eval --test memory_eval
```

It checks ids, categories, languages, note kinds, `fact_key` grammar,
`supersedes` links, that every `relies_on` resolves, that the two splits share
no id, and that the judge prompt still carries its five placeholders. Plain
`cargo test -p openalpaca_core` does not compile this file at all.

## Cost

Per system: 470 reader turns (one main-loop call each, more when the model
calls a tool) plus 470 judge calls of about 600 input tokens. At Haiku-class
pricing expect roughly **$1–3 per system**; at Sonnet-class roughly $5–15. The
report records the measured figures (`totals.reader_cost_usd`,
`totals.judge_cost_usd`, tokens in/out, latency p50/p95). The per-turn loop
cost cap is $1 (`LoopConfig::default`), so no single turn can run away. A
smoke run with `OPENALPACA_MEMORY_EVAL_LIMIT=10` costs cents.

## Reading a report

One JSON file, rewritten after every question (a run that dies leaves
`"status": "running"` with the rows so far):

- `status`: `complete`, `running`, or `unavailable` (with `unavailable_reason`).
- `machine` (hostname, OS, arch, rustc), `build` (git head, `Cargo.lock`
  SHA-256, features), `conditions` (the `llm.toml` path and hash, reader
  default model, the main loop's round and tool-call caps, the tool-result
  byte cap and the context buffer ratio from the effective `DaemonConfig`,
  judge model/temperature/prompt hash, notes and questions
  paths with hashes, how many notes seeded, what was on the tool surface,
  the scratch store) and `embedder` (B only) are the run's recorded
  conditions — the report is the record; nothing is a measurement without them.
- `totals`: `correct`/`incorrect`/`unparsed`/`judge_errors`/`turn_errors`,
  `accuracy`, `by_category`, `by_language`, reader and judge tokens and cost,
  the models that answered, `memory_search_calls`,
  `rows_with_per_turn_fts_hits`, latency p50/p95/mean/max and wall time.
- `rows`: one per question — the question, gold and actual answer, the judge's
  verdict and raw reply, latency, `ack_ms`, the routing `mode`, reader LLM
  calls/tokens/cost, the tools called, `memory_search_calls`,
  `per_turn_fts_hits`.

`unparsed` (the judge's last line was neither word) and `judge_error` rows
count as not correct in `accuracy`; `compare.py` pairs only rows both reports
judged, so a systematic judge failure shrinks `n` rather than hiding. A single
run is a measurement, never a verdict; the gate is the paired CI from
`compare.py`, which also flags any category more than 10 pp worse
(paraphrase and the Chinese rows are the ones to watch).

## Extending the set

Keep ids unique across both files, `expect_abstain` true exactly for
`abstention` rows, at least one `relies_on` for every other row, notes' ids as
hyphenated lowercase UUIDs and `fact_key` in `[a-z0-9][a-z0-9._-]{0,63}`. Add
gold notes first, then questions that cite them. Do not tune on the held-out
file — that is what the tuning split is for.
