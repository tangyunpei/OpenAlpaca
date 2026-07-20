# Agent Loop — Reference

> Whoever touches the loop next updates this doc. It's a spec, not generated.

OpenAlpaca's agentic loop maps 1-to-1 onto Hermes Research's [10-step
run_conversation loop](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop),
extended with cost budgets, parallel tool execution, three multi-agent
topologies, and mid-execution replanning.

## The Ten Inner Steps

Each step emits a `tracing::info_span!` named below. All spans are at
`Level::INFO`. The nine spans emitted from the loop body
(`runner/agentic_loop/mod.rs`) carry `agent_id` and `round` fields
(`llm_call` reports `round = state.rounds + 1`, one ahead of its
siblings); `cache_markers` lives in the provider layer and carries only
`breakpoints = 3`.

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
| Sequential pipeline | `orchestrator/dispatcher/pipeline_step.rs` (`execute_pipeline_step`, called per step from `dispatcher/pipeline.rs`) | One per agent in the pipeline |
| DAG | `runner/dag_executor/node_runner.rs` — parallelized up to `max_concurrent_agents` | One per DAG node |
| Lead agent | `runner/lead_agent/mod.rs` | One for the lead + one per subagent spawned via the `spawn_subagent` / `spawn_subagents_batch` (1–8 per call) tools |

The loop also runs outside the multi-agent topologies: the simple-query
handler invokes it directly for tool-enabled simple queries and the
deep-query path (`orchestrator/query_handler/simple_query_handler.rs`),
as does skill invocation (`orchestrator/skill/invocation.rs`,
`orchestrator/skill/invoke_executor.rs`).

## Hermes Compliance Matrix

```
Hermes step      OpenAlpaca      Notes
──────────────────────────────────────────────────────────────────────
1. task_id       Compliant       Pre-loop
2. append user   Compliant       Arc<Vec<ChatMessage>>
3. sys prompt    Compliant       Layered compose engine with two-tier memoization (global LRU + per-lane cache); 9 call sites unified via ComposeEngine::compose
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

## Loop Guardrails

Beyond the round/cost checks in steps 2–3, the loop enforces:

- **Per-round tool cap** — at most `config.max_tools_per_round` tool
  calls execute per round; and when a sandbox policy sets
  `max_tool_calls`, calls are partitioned into executable vs
  over-budget, with over-budget calls returning stub error results
  instead of executing.
- **Runtime model-access check** — the router may fall back to a
  different model than requested; after each response the actual
  `response.model` is checked against the agent's constraints via
  `CapabilityManager::check_model_access`. A violation ends the loop
  with `LoopFinishReason::Error`.
- **Thinking-block exclusion** — extended-thinking text is logged but
  explicitly omitted from the `ChatMessage` appended to history.
- **Transient LLM-error retry** — a `consecutive_llm_errors` counter
  drives exponential backoff on transient failures and resets to zero
  on any success.
- **Compaction telemetry** — each compaction publishes
  `SystemEvent::CompactionTriggered` on the event bus with utilization
  and summary metrics.

## System-Prompt Memoization

Implemented via the layered `ComposeEngine` (`compose/mod.rs`). All nine
production prompt-assembly call sites (simple query ×2, skill
invocation, pipeline step, DAG node, lead-agent prompt + subagent spawn,
planner, replanner) route through `ComposeEngine::compose`, which runs
five layers — Persona, Static Prompt, Dynamic Context, History, Assembly
— with two-tier memoization:

- **Tier 1, global LRU** — Layers 1+2, keyed by
  `GlobalCacheKey::Persona` / `GlobalCacheKey::StaticPrompt`
  fingerprints.
- **Tier 2, per-lane** — Layers 3+4, cached on
  `ConversationLane.caches`. When `compose` is called with
  `lane: None`, these layers are recomputed fresh each call.

Every layer lookup emits a `ComposeLayerCacheHit`/`Miss` event; misses
carry a structured `MissReason` (e.g. `PersonaChanged`,
`AgentConfigChanged`, `ToolsChanged`) obtained by diffing
sub-fingerprints against the most-recent cached entry.

Known gaps:

- Layers 3+4 report `MissReason::FirstBuild` on every miss — per-lane
  miss attribution is out of scope for this cycle.
- The pipeline-step call site passes `lane: None` (no natural lane key),
  so sequential-pipeline steps get no per-lane caching.
- The legacy `PromptBuilder` survives only as the internal backend of
  the static-prompt layer (`compose/static_prompt.rs`).

## Sources

- [Hermes Agent Loop Internals](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop)
