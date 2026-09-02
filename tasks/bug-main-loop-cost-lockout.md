# BUG — Main loop locks out after ~$1 of chat spend per day

**Found:** 2026-09-01, while verifying the knowledge base. **Severity:** high — chat stops working. **Status:** unfixed, not yet reproduced live.

## Symptom (predicted)

Once the day's cumulative LLM spend attributed to the `"orchestrator"` bucket passes `$1.00`, **every main-loop chat turn exits `CostExceeded` at round 0, before making any LLM call.** The user sees chat stop responding. It recovers only at local midnight, and a daemon restart does **not** clear it.

Workflows (lead agent + subagents) are unaffected — they get unique per-spawn agent ids.

## Mechanism

1. The main loop calls the agentic loop with a **fixed literal** `agent_id` and no cost accumulator — `orchestrator/query_handler/simple_query_handler.rs:651` (`"orchestrator"`) and `:661` (`cost_accumulator = None`, so a fresh zeroed `LoopCostAccumulator` each turn).
2. `LlmBackend::agent_cost()` returns the tracker's **cumulative** total for that agent id, not this turn's cost — `runner/agentic_loop/backend.rs` → `cost_tracker.get_agent_usage(agent_id).total_cost_usd`.
3. `LoopState::new()` sets `last_cost = 0.0`, so on round 0 of a new turn `cost_delta = cumulative − 0.0 = the entire day's orchestrator spend`, which is added to the fresh accumulator — `runner/agentic_loop/mod.rs`, cost-check block.
4. `if accumulated_cost > config.max_cost { return CostExceeded }` — and the orchestrator's `LoopConfig` comes from `LoopConfig::default()` (`apps/openalpacad/src/main.rs:357`, `..LoopConfig::default()`), where `max_cost = 1.00` (`runner/agentic_loop/config.rs:130`). No config key overrides it.
5. The tracker is **re-seeded at boot from today's `llm_usage`** (`apps/openalpacad/src/services/llm.rs:205-235`, `get_today_usage`), so restarting the daemon restores the same cumulative bucket.

The design intent is visible in the code comment — *"agent-scoped: the per-agent budget must not inherit other agents' spend"* — the per-**turn** accumulator is correct, but it is seeded from a cumulative source.

## Why it was not caught

Tests use a mock router whose `CostTracker` bucket starts empty, so `cost_delta` on round 0 is ~0 and the check never trips. It needs real accumulated spend under one agent id.

## Candidate fixes (not yet chosen)

1. **Baseline the delta** — capture `agent_cost()` once before the loop and initialise `state.last_cost` with it, so only *this turn's* spend accumulates. Smallest change; keeps the tracker as the source.
2. **Per-turn agent id** — give each main-loop turn a unique id (e.g. `orchestrator:{request_id}`), making the bucket genuinely per-turn. Changes cost attribution in `llm_usage` rows and the Settings usage view.
3. **Separate the budgets** — give the main loop its own `main_loop_max_cost` in `[orchestrator.routing]` and treat a per-day cap as a distinct feature.

Option 1 is the likely correct fix: the bug is that a per-turn budget is being compared against a cumulative measure.

## Verification plan

Reproduce first — start the daemon, chat until `llm_usage` for today under `agent_id = 'orchestrator'` exceeds $1.00, confirm the next turn returns `CostExceeded` with zero LLM calls. Then fix, and add a regression test that seeds the tracker with a non-zero bucket before running one main-loop turn.
