# Agent Loop — Reference

> Whoever touches the loop next updates this doc. It's a spec, not generated.

OpenAlpaca's agentic loop maps 1-to-1 onto Hermes Research's [10-step
run_conversation loop](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop),
extended with cost budgets, parallel tool execution, three multi-agent
topologies (lead agent by default; pipeline/DAG legacy), mid-execution
replanning (legacy), and a mid-workflow steering rail.

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

| Topology | Entry point | Loop invocations per task | Status |
|---|---|---|---|
| Lead agent | `runner/lead_agent/mod.rs` | One for the lead + one per subagent spawned via the `spawn_subagent` / `spawn_subagents_batch` (1–8 per call) tools | **Default** — the only front-door topology under `mode = "tool"` |
| Sequential pipeline | `orchestrator/dispatcher/pipeline_step.rs` (`execute_pipeline_step`, called per step from `dispatcher/pipeline.rs`) | One per agent in the pipeline | **Legacy** — reachable only under `orchestrator.routing.mode = "planner"`; scheduled for deletion (Routing V2 Phase 5) |
| DAG | `runner/dag_executor/node_runner.rs` — parallelized up to `max_concurrent_agents` | One per DAG node | **Legacy** — planner-mode only; scheduled for deletion (Routing V2 Phase 5) |

The loop also runs outside the multi-agent topologies: under the default
`mode = "tool"` the main-loop front door (below) IS a direct loop
invocation per user turn
(`orchestrator/query_handler/simple_query_handler.rs`); skill invocation
invokes it too (`orchestrator/skill/invocation.rs`,
`orchestrator/skill/invoke_executor.rs`), as do the legacy planner-mode
simple-query and deep-query paths.

## The Main-Loop Front Door (Routing V2)

With `[orchestrator.routing] mode = "tool"` (the default), routing is a
tool call, not a pre-classifier. `handle_message_internal`
(`orchestrator/handlers.rs`) runs a short deterministic ladder and hands
everything that survives it to `handle_simple_query` with
`LoopOverrides::MainLoop` — one agentic-loop invocation per user turn,
**including while workflows run** (chat-by-default; lanes are never
captured):

1. **Task ops** — `/status`, `/tasks`, `/cancel|/pause|/resume`. Bare
   control commands (no task id) resolve against the lane's active
   workflows (`task_ops.rs::handle_bare_task_control`): exactly one
   running → act on it; zero → say so; multiple → list ids and ask.
2. **`/steer <msg>`** — deterministic injection into the lane's sole
   running workflow, bypassing the model
   (`task_ops.rs::handle_steer_prefix`; gated on `steering_enabled`).
3. **Skills** — slash commands and SkillRouter auto-mode, executed in the
   deterministic tier.
4. **Bootstrap / forced-simple-query** — unchanged special cases.
5. **Social fast path** — exact-phrase match, send-hints guarded,
   answered before the main loop so "thanks" stays cheap mid-workflow.
6. **Main loop** — everything else. Chat vs. task vs. steer is the
   model's tool choice.

**Tool surface** (`tools/builtins/main_loop.rs::main_loop_tool_set`):
the base picks (keyword-suggested tools under
`tool_selection = "core_union"`, or the whole registry minus the global
deny list under `"full"`) unioned with a per-request core set —
`start_workflow`, `task_status`, `memory_store` + `memory_forget`
(DB-gated), and the globally-registered `memory_search` definition.
When the lane has active workflows AND steering is enabled,
`steer_workflow` + `queue_followup` join the surface. Per-request
instances go into a per-request registry clone, never the global
registry. Budgets come from `main_loop_max_rounds` /
`main_loop_max_tools_per_round`.

**Workflow context**: lanes with active workflows get a per-turn
`<active_workflows>` block (`query_handler/workflow_context.rs`) — task
id, title, status, progress counters — injected deliberately outside the
compose-engine layers (Tier-1/Tier-2 caches would serve stale status) —
plus `<workflow_relay_rules>` relay guidance.

**Delegation contract**: `start_workflow`
(`tools/builtins/start_workflow.rs`) enforces `max_workflows_per_lane`
(tool mode only — never inside `dispatch_lead_agent`), dispatches the
lead agent detached, records the routing decision **unconditionally**
(`DispatchDecision` reason `model_tool_call`, not gated on
`dispatch_analysis_enabled`), publishes `WorkflowStarted`, and stores
the `DispatchOutcome` in a result cell. After the loop the handler reads
the cell and populates structured `delegation{task_id, title}`
(Orchestrator `delegation_map` → `HandleResult`/`GatewayResponse` → SSE
`done`). The model's own text is the ack — there is no canonical ack
string on this path.

## The Steering Rail

Mid-workflow interjections, gated on `steering_enabled` (default on).
Lead-only and text-only — subagents never see an inbox.

- **Data** (`runner/steering.rs`): a bounded, closable `SteeringInbox`
  (cap `steering_inbox_cap`, default 16) registered on `SharedContext`
  per running lead-agent task, right next to the cancellation token
  (`dispatcher/lead_agent.rs`). The lane→task index
  (`workflows_for_lane`) registers unconditionally — it also backs the
  per-lane cap and the workflow-context block.
- **Producers**: the `/steer ` prefix (deterministic) and the
  `steer_workflow` tool (model-routed). Push results: `Ok` reports queue
  depth; `Full` directs the model to offer `queue_followup`; `Closed`
  means the workflow already detached.
- **Two drain points** in the loop body (`runner/agentic_loop/mod.rs`):
  1. The round-boundary drain — after the cancellation/max-rounds/cost
     checks and compaction, immediately before `build_request`. Draining
     any earlier would lose messages on MaxRounds/CostExceeded exits.
  2. The completion guard — at the no-tool-calls Complete exit: if the
     inbox is non-empty, keep the assistant's answer in history, inject,
     and continue (budget checks still apply).
- **Injection format**: `<user_interjection ts="…">…</user_interjection>`
  user messages. The compactor exempts them from discard/truncation
  (`prompt_ctx/compaction/graduated.rs`,
  `runner/agentic_loop/context.rs`).
- **Budget**: +5 bonus rounds per non-empty drain
  (`STEERING_ROUNDS_BONUS`), capped at 2× `max_rounds`. `max_cost` is
  never extended. Transient-LLM retries also consume rounds, bonus
  included — accepted.
- **Wait interrupt**: `WaitForSubagentsTool`
  (`runner/lead_agent/tools.rs`) selects on the inbox's lost-wakeup-safe
  `notified()` alongside subagent completion, so a queued interjection
  breaks the up-to-600s wait instead of sitting until timeout.
- **Detach**: close-then-drain (closed is set under the queue lock), so
  a push racing detach gets `Err(Closed)` instead of vanishing. Leftover
  (drained-but-unsent or never-drained) messages become
  `unprocessed_steering` rows in `lane_followups` — they are never
  auto-run.

## The Completion Report

When a lead-agent workflow finishes
(`dispatcher/lead_agent.rs`), the lead agent's own final message IS the
user-facing completion report: the lead prompt carries an unconditional
`<completion_report>` contract (`runner/lead_agent/prompt.rs`), and the
spawn persists `LoopResult.final_content` verbatim to the lane
conversation (`persist_conversation`). The legacy `format_task_result`
template is only the fallback for empty final content (budget / cancel /
error exits). Non-`Complete` finish reasons get a one-line status prefix
either way (`outcome.rs::completion_status_line`). `TaskCompleted`
events carry a 500-char excerpt; the full report lives in lane history
and the task outcome record.

## The Follow-up Runner

`queue_followup` (lead-agent tool, plus a main-loop variant on
workflow-attached lanes) writes `followup` rows into `lane_followups`
(migration 033) with the originating principal/scope/workspace. When a
workflow finalizes and `followup_autostart = true` (default), the spawn
claims the lane's next `followup` row (`FollowupRepository::claim_next`
— which never claims `unprocessed_steering`) and hands it to the
`FollowupRunner` (trait in `orchestrator/mod.rs`; daemon impl
`apps/openalpacad/src/followup.rs::GatewayFollowupRunner`). Each item
re-enters through `Gateway::handle_event` as a fresh turn —
`EventSource::Internal` with `lane_override` for lane continuity — so it
can answer inline or start its own workflow through the normal front
door.

## Routing Mode Strings

`OrchestrationStage.mode` vocabulary after the Routing V2 default flip
(`planner_ms` is kept in the event schema and is always 0 in tool mode):

| Status | Mode strings |
|---|---|
| **Retired** — emitted only under `mode = "planner"`, gone with the ladder in Phase 5 | `two_phase_simple`, `two_phase_complex`, `two_phase_deep_query`, `two_phase_triage_failed`, `planner_simple_query`, `planner_complex_task`, `planner_unknown`, `planner_failed`, `fast_path`, `no_llm` |
| **New** | `main_loop` (the tool-mode front door), `steered` (the deterministic `/steer ` prefix path), `skill_command` (the deterministic skill tier, both modes), `task_ops` (task queries/control, incl. bare `/cancel` etc.) |
| **Unchanged** | `bootstrap`, `forced_simple_query`, `social_fast_path` |

There is no `workflow_started` mode string: a turn that starts a
workflow is `main_loop` with `delegation{task_id, title}` on the
response and a `DispatchDecision` with reason `model_tool_call`.
Model-routed steering (`steer_workflow`) is likewise a `main_loop` turn;
only the `/steer ` prefix emits `steered`.

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
