# Steer-Routing Eval Harness (Routing V2)

Live-model eval for the tool-mode main loop's mid-workflow routing: given a
message arriving while a workflow is running, does the model steer the
workflow (`steer_workflow`), answer inline, or queue a follow-up
(`queue_followup`) — and do control commands (`/cancel`, `/steer …`) stay on
the deterministic tier without ever reaching the model?

## Layout

| File | Purpose |
|---|---|
| `corpus.json` | ~24 labeled mid-workflow messages (mixed EN/中文). Each item: `{id, message, context, expected}` where `context` is the fake active workflow's title and `expected` ∈ `steer` \| `answer` \| `queue` \| `deterministic`. |
| `crates/openalpaca_core/tests/steer_routing_eval.rs` | The runner: a corpus-validation unit test (always runs) and the `#[ignore]`d live eval. |

## How it works

The live test boots a real `Orchestrator` (real `LlmRouter` from your
`llm.toml`, scratch SQLite DB), registers a fake Running workflow + steering
inbox on a fresh lane per corpus item, replays the message through
`Orchestrator::handle_message`, and scores the outcome by observation:

- **steer** — the steering inbox received a message (`WorkflowSteered`);
- **queue** — a `lane_followups` row was written (`FollowupQueued`);
- **deterministic** — the turn's `OrchestrationStage.mode` was not
  `main_loop` (task ops / `/steer ` prefix answered without the model);
- **answer** — a `main_loop` turn with none of the above;
- **start_workflow** — a misroute bucket: `max_workflows_per_lane` is pinned
  to 1 in the eval config, so a stray `start_workflow` call is refused at the
  per-lane cap (observable, but never dispatches a real — costly — lead
  agent).

Per item it prints a verdict line with mode, `ack_ms`, LLM call count, token
usage, and cost; at the end it prints totals from the `llm_usage` table and
per-mode ack latency from the `orchestrator_latency` table, then asserts
`accuracy >= bar`.

> **Note:** the original spec's "cost comparison vs planner" leg is obsolete —
> the planner ladder was deleted in Routing V2 Phase 5. The harness instead
> reports an **absolute** cost/latency baseline for the tool-mode front door.

## Prerequisites

1. **API keys** — a resolvable `llm.toml`, found via `$OPENALPACA_CONFIG_DIR`
   or the repo's `config/llm.toml` (not checked in; the daemon seeds it from
   `scripts/release/templates/config/llm.toml` on first start). Secrets
   resolve per the config docs — for the eval, `secret_env` (or a plain
   legacy `api_key`) is the simplest path; `secret_ref` (OS keychain) is not
   resolved by the test.
2. **Provider features** — the LLM providers are feature-gated; build the
   test with `--features live-eval`.
3. **Opt-in env** — `OPENALPACA_LIVE_EVAL=1`. Without it the test exits
   early even under `--ignored` (no calls, no cost).

## Run

```sh
OPENALPACA_LIVE_EVAL=1 cargo test -p openalpaca_core --features live-eval \
    --test steer_routing_eval -- --ignored steer_routing_eval --nocapture
```

Default `cargo test -p openalpaca_core` runs only the corpus-validation test
and skips the live eval.

## Cost ballpark

~24 items → one main-loop turn each (deterministic items are free). With
prompt caching on and a Sonnet-class default model, expect roughly 1–2 LLM
calls and ~2–6k input tokens per model-routed turn: **well under $1 per full
run** (typically $0.10–$0.50). The per-turn loop cost cap is $1
(`LoopConfig::default`), so a runaway turn cannot exceed that.

## Adjusting the bar

The accuracy bar defaults to **0.8**. Override per run:

```sh
OPENALPACA_EVAL_ACCURACY_BAR=0.9 OPENALPACA_LIVE_EVAL=1 cargo test ...
```

CI can gate on this later by exporting the env vars and providing keys via
`secret_env`.

## Extending the corpus

Add items to `corpus.json` keeping: unique `id`s, non-empty `message` and
`context`, `expected` one of the four labels, deterministic items starting
with `/` and model-routed items not. The always-on
`steer_routing_corpus_parses_and_validates` test enforces this shape (and a
minimum of 3 items per label, ≥20 total).
