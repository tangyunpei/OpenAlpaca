# Agent Loop — Reference

> Whoever touches the loop next updates this doc. It's a spec, not generated.

OpenAlpaca's agentic loop maps 1-to-1 onto Hermes Research's [10-step
run_conversation loop](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop),
extended with cost budgets, parallel tool execution, three multi-agent
topologies, and mid-execution replanning.

## The Ten Inner Steps

Each step emits a `tracing::info_span!` named below. All spans are
at `Level::INFO` and carry `agent_id` and `round` fields.

| # | Span | Responsibility | Code |
|---|------|---|---|
| 1 | `loop.step.cancellation_check` | Check the CancellationToken; return Cancelled if set | `runner/agentic_loop/mod.rs` step 1 |
| 2 | `loop.step.max_rounds_check` | Return MaxRounds if the round counter reached `config.max_rounds` | step 2 |
| 3 | `loop.step.cost_check` | Accumulate round cost; return CostExceeded if over `config.max_cost` | step 3 |
| 4 | `loop.step.compaction` | Graduated budget-aware history compaction | step 4 |
| 5 | `loop.step.build_request` | Assemble `RouterRequest` for this iteration | step 5 |
| 6 | `loop.step.pressure_layer` | Compute the ephemeral budget notice (fires only at >=80% cost or rounds, flag-gated) | nested in step 5 |
| 7 | `loop.step.cache_markers` | Apply the three Anthropic `cache_control: ephemeral` markers (system, last tool, last message) | `providers/anthropic/request.rs` |
| 8 | `loop.step.llm_call` | Dispatch `backend.complete(...)` under `tokio::select!` with the cancel token | step 8 |
| 9 | `loop.step.response_parse` | Parse the `ChatResponse`; split into tool-call branch or final-text branch | step 9 |
| 10 | `loop.step.persist_or_tools` | Execute tools (parallel via `join_all`) OR persist final text and return | step 10 |

## How Topologies Invoke the Loop

| Topology | Entry point | Loop invocations per task |
|---|---|---|
| Sequential pipeline | `runner/dag_executor/node_runner.rs` | One per agent in the pipeline |
| DAG | Same, parallelized up to `max_concurrent_agents` | One per DAG node |
| Lead agent | `runner/lead_agent/mod.rs` | One for the lead + one per subagent spawned via `spawn_subagent` tool |

## Hermes Compliance Matrix

```
Hermes step      OpenAlpaca      Notes
──────────────────────────────────────────────────────────────────────
1. task_id       Compliant       Pre-loop
2. append user   Compliant       Arc<Vec<ChatMessage>>
3. sys prompt    Compliant       Layered compose engine with two-tier memoization (global LRU + per-lane cache); 8 call sites unified via ComposeEngine::compose
4. preflight     Compliant       Graduated compaction
5. build API     Compliant       3 providers via adapter crate
6. ephemeral     Compliant       Flag-gated (spec P0)
7. cache marks   Compliant       3 Anthropic breakpoints (spec P1)
8. interrupt     Compliant       tokio::select + CancellationToken
9a. tools+loop   Compliant       Parallel tool exec
9b. text+return  Compliant       Background memory extraction
10. persistence  Compliant       Optimistic-lock state_version
```

## Ephemeral Pressure Layer

Enabled via `config/daemon.toml`:

```toml
[experimental]
ephemeral_pressure_layer = true
```

Triggers when `max(cost_ratio, rounds_ratio) >= 0.8`. Notice content:

```
[budget_notice]
Budget status: {rounds_used}/{max_rounds} rounds ({rp}%), ${cost_used}/${max_cost} spent ({cp}%).
Prefer concluding the current task over opening new tool calls. ...
[/budget_notice]
```

Placement: Anthropic — second element of `system` array, no `cache_control`.
OpenAI/Ollama — tail `system`-role message in the `messages` array.

The notice is stateless (recomputed per iteration), never persisted to
conversation history, and never mutates the `Arc<Vec<ChatMessage>>` the
loop holds.

## Cache Breakpoint Topology (Anthropic)

Three of Anthropic's four allowed `cache_control: ephemeral` markers:

1. System prompt block.
2. Last tool definition.
3. Last message's last content block (spec P1).

Verify via `usage.cache_read_input_tokens` in the `ChatResponse`.

## System-Prompt Memoization — Deferred

Per-lane system-prompt memoization was designed (spec §Component 3) but
deferred from this work on 2026-04-18. The system prompt is built
downstream of `build_context` via `PromptBuilder` at six call sites
(simple-query / skill-invocation / pipeline-step / planner / replanner /
social fast path), and a safe fingerprint must also capture tool set,
bootstrap, and context-bundle variance — not just persona + agent_config
+ skills + memory. It will be re-planned alongside the broader context
and prompt-engine rebuild.

## Sources

- [Hermes Agent Loop Internals](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop)
- `docs/superpowers/specs/2026-04-18-hermes-agent-loop-refinements-design.md`
- `tasks/hermes-agent-loop-comparison.md`
