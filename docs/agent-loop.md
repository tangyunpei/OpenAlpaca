# Agent Loop — Reference

> Whoever touches the loop next updates this doc. It's a spec, not generated.

OpenAlpaca's agentic loop maps 1-to-1 onto Hermes Research's [10-step
run_conversation loop](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop),
extended with cost budgets, parallel tool execution, a lead-agent
multi-agent topology (subagents spawned singly or in batches), and a
mid-workflow steering rail. The legacy pipeline/DAG topologies and
mid-execution replanning were deleted in Routing V2 Phase 5.

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
| 2 | `loop.step.max_rounds_check` | Return MaxRounds if the round counter reached the effective cap (`config.max_rounds` plus any bonus rounds) | step 2 |
| 3 | `loop.step.cost_check` | Accumulate round cost; return CostExceeded if over `config.max_cost` | step 3 |
| 4 | `loop.step.compaction` | Graduated budget-aware history compaction | step 4 |
| 5 | `loop.step.build_request` | Assemble `RouterRequest` for this iteration | step 5 |
| 6 | `loop.step.pressure_layer` | Compute the ephemeral budget notice (fires only at >=80% cost or rounds, flag-gated) | nested in step 5 |
| 7 | `loop.step.cache_markers` | Apply the three Anthropic `cache_control: ephemeral` markers (system, last tool, last message) | `providers/anthropic/request.rs` |
| 8 | `loop.step.llm_call` | Dispatch `backend.complete(...)` under `tokio::select!` with the cancel token | step 8 |
| 9 | `loop.step.response_parse` | Parse the `ChatResponse`; split into tool-call branch or final-text branch | step 9 |
| 10 | `loop.step.persist_or_tools` | Execute tools (parallel via `join_all`) OR persist final text and return | step 10 |

Two details of the budget checks:

- **The cost cap is per invocation.** Before round 0 the loop records the
  agent's cumulative cost as a baseline, and step 3 adds only the growth
  since then. The main loop reuses one agent id (`orchestrator`) across
  every turn, so without the baseline a long-lived daemon would trip
  `CostExceeded` before its first call. The defaults are $1 per turn or
  subagent (`[execution.agent_defaults] max_cost`) and $5 per lead
  (`[execution.lead_agent_defaults] max_cost`); an agent template's
  `max_cost_per_task` overrides either (the shipped `lead_agent`
  template sets `3.0`). There is no overall daily budget for turns or
  workflows. Three background jobs that run outside this loop — memory
  extraction, conversation summaries and task-output extraction — each
  carry a small daily ceiling under `[orchestrator.costs]`.
- **The round cap can stretch, within a ceiling.** The effective cap is
  `max_rounds` plus any steering bonus and the answer guard's one bonus
  round, never more than `2 × max_rounds`.

## How a Loop Ends

Every invocation returns a `LoopResult` whose `finish_reason` is one of
six values (`runner/agentic_loop/config.rs`). With a session log
attached, the exit is also written as a `workflow_done` record using the
lower-case word in the second column.

| `LoopFinishReason` | Log word | When |
|---|---|---|
| `Complete` | `complete` | The model answered with no tool calls, and neither the steering completion guard nor the answer guard asked for another round. |
| `MaxRounds` | `max_rounds` | Step 2: the round counter reached the effective cap. |
| `CostExceeded` | `cost_exceeded` | Step 3: this invocation's accumulated cost passed `max_cost`. |
| `Truncated` | `truncated` | The model stopped on its output limit three times running. The first two times the loop appends the partial text and a "continue from where you left off" message (`MAX_TOKENS_RETRIES = 2`); the third returns the partial text. |
| `Cancelled` | `cancelled` | The cancel token fired — at the top of a round, during the LLM call, or during a retry backoff. |
| `Error(msg)` | `error` | A non-transient LLM error, a transient one that outlived its retries, or a runtime model-access denial. |

`final_content` is the model's answer on `Complete` and the partial text
on `Truncated`. On every other exit it is the last assistant text the
loop saw, which may be empty. Drained steering messages that never
reached an LLM call are pushed back to the inbox on every early exit, so
the cleanup path can file them.

**The no-answer line.** A chat turn must not end with nothing.
`LoopResult::no_answer_line()` returns `None` when there is answer text,
and `None` for `Cancelled`; otherwise it returns one runtime-authored
sentence:

| Exit | Sentence |
|---|---|
| `MaxRounds` | "I stopped after N tool round(s) without reaching an answer." |
| `CostExceeded` | "I stopped before reaching an answer: this turn hit its cost limit." |
| `Truncated` | "I stopped before reaching an answer: the reply hit the model's output limit." |
| `Error(e)` | "I could not finish this turn: e" |
| `Complete` | "I finished this turn without writing an answer." |

When a tool failed during the turn, " The last tool error was: …" is
appended — the last failing result, capped at 600 bytes, with the loop's
internal `[tool_error]` marker stripped. The line is used by the tiers
that answer a user's turn with a model: the main loop and the social
fast path (both in `query_handler/simple_query_handler.rs`) and the
file-based skill tier (`skill/invocation.rs`). It becomes the turn's content everywhere — the
stored assistant message, the one fallback delta, `done.content`. An
`Error` exit with no text is the exception in one respect: the handler
returns it as an error with that line as the message, so the turn still
ends on the SSE `error` frame and no assistant message is stored. A
lead-agent workflow does not use this line; it has its own fallback (see
[The Completion Report](#the-completion-report)).

## How Topologies Invoke the Loop

| Topology | Entry point | Loop invocations per task | Status |
|---|---|---|---|
| Lead agent | `runner/lead_agent/mod.rs` | One for the lead + one per subagent spawned via the `spawn_subagent` / `spawn_subagents_batch` (1–8 per call) tools | The only multi-agent topology (the legacy sequential pipeline and DAG executor were deleted in Routing V2 Phase 5; batch work goes through `spawn_subagents_batch` + `wait_for_subagents`) |

**Lead tool surface** (`runner/lead_agent/mod.rs::run_lead_agent`):
coordination tools (`spawn_subagent`, `spawn_subagents_batch` when
enabled, `check_subagent_status`, `wait_for_subagents`, plus
`post_update` + `queue_followup` under steering) + workspace tools +
`memory_search`, plus `artifact_write` and `read_result` when the lead's
own template declares those capabilities (the shipped `lead_agent`
template does), unioned with the same extension set as the main loop —
every installed MCP-bridged (`<server>__<tool>`) and plugin-provided
(`<plugin>::<tool>`) tool whose extension is enabled — and a per-request
`invoke_skill` instance, so the lead can run catalog
skills and connected integrations itself or delegate them. The lead's
`SandboxPolicy` allowlist is extended from the final tool definitions at
run time (template denials still win); subagents stay template-scoped —
they get only the capabilities their own template declares, never the
lead's blanket grant.

Subagents can be plugin-backed: when the spawned template's source is
`AgentSource::Plugin` (contributed by a plugin manifest), the spawn path
skips the internal loop entirely and `runner/plugin_agent.rs` drives the
plugin's external reasoning loop instead — `spawn()` with the composed
instructions, then `step()` polls capped at 50 iterations, with requested
tool calls proxied through the same `SandboxManager::execute_tool` path
(capability checks, sanitization, confirmation, timeouts) as internal
subagents. Depth, concurrency-permit, and cancellation semantics are
unchanged; the cancel token is checked between steps.

The loop also runs outside the multi-agent topology: the main-loop
front door (below) IS a direct loop invocation per user turn
(`orchestrator/query_handler/simple_query_handler.rs`); skill invocation
invokes it too (`orchestrator/skill/invocation.rs`,
`orchestrator/skill/invoke_executor.rs`).

## Streaming

The invocations that answer a user's turn — the main-loop front door,
the social fast path beside it (a one-round, no-tools loop), and
`skill/invocation.rs` — set `LoopConfig.stream_callback` from that
turn's sink when a client is watching one
(`chat::delta_forwarder` in `chat/turn_sink.rs`, one forwarder shared by
all of them), so a `/slash` skill's answer and its reasoning reach the client
delta by delta exactly like the main loop's. The forwarder passes on two
provider events and nothing else: `TextDelta` becomes the SSE `delta`,
`ThinkingDelta` becomes the SSE `reasoning`. Tool-call, usage, done and
error events stay inside the loop.

A **plugin-contributed** skill does not stream: `invoke_plugin_skill`
gets a finished answer back over the plugin protocol, so it keeps the
chat service's single fallback delta — nothing simulates chunks for it.
Every caller with nobody watching a stream (scheduled skills,
connectors, the follow-up runner, the nested `invoke_skill` tool, the
lead and its subagents) passes no sink and runs the non-streaming path.

With a callback set and a router backend, step 8 tries
`LlmRouter::complete_streaming` first and **falls back to the
non-streaming call** — same round, no extra round charged — when:

- the streaming request itself fails;
- collecting the stream fails, which includes 90 s of silence between
  chunks (`STREAM_IDLE_TIMEOUT`, `openalpaca_llm/src/streaming.rs`);
- the collection outlives `LoopConfig.max_stream_duration` (600 s).

`complete_streaming` returns `RoutedStream { model, stream }`. A stream
carries no model id of its own, and the router is the only code that
knows which rung of its fallback ladder it settled on, so the loop takes
`.stream` and reports `.model`. That id is what reaches
`LoopResult.model_used`, the cost tracker, the usage row and the SSE
`done` frame — the model that answered, not the model that was asked
for.

Because a multi-round turn streams the text written before each tool
call, the deltas of a turn need not add up to its answer.
`LoopResult.final_content` — `done.content` on the wire — is
authoritative.

## The Main-Loop Front Door (Routing V2)

Routing is a tool call, not a pre-classifier (the only routing since
Routing V2 Phase 5 deleted the planner ladder and the
`[orchestrator.routing] mode` key). `handle_message_internal`
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
`tool_selection = "core_union"`, or the whole registry under `"full"`)
unioned with a per-request set —
`start_workflow`, `task_status`, `memory_store` + `memory_forget`
(DB-gated), the globally-registered `memory_search` definition,
`read_result` (only when the daemon has both a session log and a
database — the one case where a result can spill, see
[Tool Results and the Spill](#tool-results-and-the-spill)), every
installed extension tool whose extension is enabled (MCP-bridged
`<server>__<tool>` and plugin-provided `<plugin>::<tool>`; a disabled
server or plugin contributes nothing on either surface), and
`invoke_skill` (catalog-skill invocation through the nested-skill
executor; present when an LLM router is configured). So installed
MCP/plugin tools and `invoke_skill` are on the DEFAULT surface, not just
under `"full"`. When the lane has active workflows AND steering is
enabled, `steer_workflow` + `queue_followup` join the surface.
Per-request instances go into a per-request registry clone, never the
global registry. Budgets come from `main_loop_max_rounds` /
`main_loop_max_tools_per_round`.

**Workflow context**: lanes with active workflows get a per-turn
`<active_workflows>` block (`query_handler/workflow_context.rs`) — task
id, title, status, progress counters — injected deliberately outside the
compose-engine layers (Tier-1/Tier-2 caches would serve stale status) —
plus `<workflow_relay_rules>` relay guidance.

**History provenance**: a turn that delegated stores its assistant
row carrying the run's id (`gateway/persistence.rs`). When that row is
replayed into a later turn's history it carries one fixed extra line —
`[This run was started by a start_workflow tool call, which returned task
<id>.]` (`orchestrator/context_builder.rs::delegation_provenance_line`) —
so the model can tell its own reported delegations from prose it could
imitate. The line exists **only in the replay**: the stored row, the
transcript and every client read the text as written, and an assistant
row with no `task_id` gets nothing.

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
  auto-run. Instead the lane's next main-loop turn injects them as an
  `<unprocessed_steering>` context block (up to 5 rows per turn,
  `query_handler/unprocessed_steering.rs`) that tells the model they
  were NOT acted on, and marks them done so each surfaces exactly once.

## The Completion Report

When a lead-agent workflow finishes
(`dispatcher/lead_agent.rs`), the lead agent's own final message IS the
user-facing completion report: the lead prompt carries an unconditional
`<completion_report>` contract (`runner/lead_agent/prompt.rs`), and the
spawn persists `LoopResult.final_content` verbatim to the lane
conversation (`persist_completion_report`), into the session the run was
started from. The legacy `format_task_result`
template is only the fallback for empty final content (budget / cancel /
error exits). Non-`Complete` finish reasons get a one-line status prefix
either way (`outcome.rs::completion_status_line`).

When a tool was refused because the run could not ask anyone for
approval (an `unattended` run — see the Daemon Manual's
[Tool confirmations](Daemon_Manual.md#tool-confirmations)), the runtime
appends one line of its own at the end of the report: *"Not run — this
run could not ask anyone for approval, so the following tool(s) were
refused: …"* (`outcome.rs::unapprovable_note`). The tool names are read
from the run's audit rows, which the lead's and every subagent's sandbox
write, so a refusal inside a subagent is reported too.

The task's `result_summary` and the `TaskCompleted` event carry the
report cut to 2 000 characters (`MAX_SUMMARY_LENGTH`); the cut keeps the
runtime's "Not run" line whole. The full report lives in lane history
and the task outcome record.

## The Follow-up Runner

`queue_followup` (lead-agent tool, plus a main-loop variant on
workflow-attached lanes) writes `followup` rows into `lane_followups`
(migration 033) with the originating principal/scope/workspace. When a
workflow finalizes and `followup_autostart = true` (default), the spawn
claims the lane's next `followup` row (`FollowupRepository::claim_next`
— which never claims `unprocessed_steering`; those rows are surfaced by
the lazy context-block injection described under Steering above) and
hands it to the
`FollowupRunner` (trait in `orchestrator/mod.rs`; daemon impl
`apps/openalpacad/src/followup.rs::GatewayFollowupRunner`). Each item
re-enters through `Gateway::handle_event` as a fresh turn —
`EventSource::Internal` with `lane_override` for lane continuity — so it
can answer inline or start its own workflow through the normal front
door. A queued row keeps the `unattended` declaration of the turn that
queued it (`lane_followups.unattended`, migration 042), so a follow-up
started hours later still knows whether anybody can answer an approval
prompt.

## Routing Mode Strings

`OrchestrationStage.mode` vocabulary after the Routing V2 default flip
(`planner_ms` is kept in the event schema and is always 0 in tool mode):

| Status | Mode strings |
|---|---|
| **Retired** — deleted with the planner ladder in Phase 5; no longer emitted, but may appear in historical `orchestrator_latency` rows | `two_phase_simple`, `two_phase_complex`, `two_phase_deep_query`, `two_phase_triage_failed`, `planner_simple_query`, `planner_complex_task`, `planner_unknown`, `planner_failed`, `fast_path`, `no_llm` |
| **New** | `main_loop` (the tool-mode front door), `steered` (the deterministic `/steer ` prefix path), `skill_command` (the deterministic skill tier, both modes), `task_ops` (task queries/control, incl. bare `/cancel` etc.) |
| **Unchanged** | `bootstrap`, `forced_simple_query`, `social_fast_path` |

There is no `workflow_started` mode string: a turn that starts a
workflow is `main_loop` with `delegation{task_id, title}` on the
response and a `DispatchDecision` with reason `model_tool_call`.
Model-routed steering (`steer_workflow`) is likewise a `main_loop` turn;
only the `/steer ` prefix emits `steered`.

## Hermes Compliance Matrix

```text
Hermes step      OpenAlpaca      Notes
──────────────────────────────────────────────────────────────────────
1. task_id       Compliant       Pre-loop
2. append user   Compliant       Arc<Vec<ChatMessage>>
3. sys prompt    Compliant       Layered compose engine with two-tier memoization (global LRU + per-lane cache); 5 call sites unified via ComposeEngine::compose
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

```text
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

## Context Budget and Compaction

Step 4 compacts history against a `ContextBudgetManager`
(`context_budget/budget.rs`), one per loop invocation. The main loop, the
file-based skill tier, the lead and every subagent each get one; a caller
that passes none (the social fast path, tests) never compacts.

- **The window is the answering model's.** Each caller resolves it
  through `runner::routed_model` / `routed_context_window`
  (`runner/model_window.rs`), which walk the router's substitution ladder
  the same way the router does. The skill tier resolves the model once and
  reads both the window and the media capabilities off it, so it cannot
  budget against one model while adapting attachments for another. On a
  local-only install the template's pinned cloud model is not routable,
  the call is answered by a local model with a much smaller window, and
  that smaller window is what the budget uses. When the registry knows no
  window for the model, the caller's own default applies (200 000 tokens
  in all four callers).
- **Trigger.** `fixed zone + message tokens >= window − buffer`, where the
  fixed zone is the system prompt plus the tool definitions and the buffer
  is `window × [execution.context] autocompact_buffer_ratio` (default
  0.165).
- **Tiers.** The starting tier is picked from utilization
  (`total / window`), and the compactor climbs one tier at a time — never
  down — until the messages are back under the trigger or the tiers run
  out (`prompt_ctx/compaction/graduated.rs`). With the default buffer the
  trigger sits at 83.5% of the window, so a default install starts at
  `HeuristicSummary` or `LlmSummary`; the lower tiers are the starting
  point only with a larger `autocompact_buffer_ratio`.

  | Utilization | Tier | What it does |
  |---|---|---|
  | ≥ 60% | `TruncateToolResults` | Cuts tool results longer than 200 bytes to half their size. |
  | ≥ 70% | `DropMultimedia` | Replaces image, audio and document parts with one-line text placeholders. |
  | ≥ 75% | `DiscardSocial` | Removes small-talk user messages and the replies to them. Keeps the system message, the first query and the last `min_recent_messages` (default 4). |
  | ≥ 80% | `HeuristicSummary` | Replaces older rounds with a compact summary built without a model (`runner/agentic_loop/context.rs::compress_context`). |
  | ≥ 85% | `LlmSummary` | Extracts memories and summarises older messages with an LLM call. Cancellable: a cancel restores the messages untouched. |

- **Never compacted away.** `<user_interjection>` steering messages are
  exempt from discard and truncation.
- **Narration.** Each compaction publishes
  `SystemEvent::CompactionTriggered` (utilization and message counts) and,
  with a session log attached, writes a `compaction` record with the tier,
  the token counts before and after, and the running total of dropped
  tokens.

## Tool Results and the Spill

One threshold decides what happens to a large tool result:
`[orchestrator.sessions] tool_result_inline_bytes` (default 32 KiB,
`MAX_TOOL_RESULT_SIZE` when a caller sets nothing).

| Result | Loop has a session log | Loop has none |
|---|---|---|
| At or under the threshold | Inline, unchanged. | Inline, unchanged. |
| Over it, succeeded | **Spilled**: the whole result is written to the session's `results/` directory, and the model receives a stub — the size, the first 2 KB, and `result_ref=file:results/…` with the instruction to page it with `read_result`. | Cut inline at the threshold, at a sentence, line or word boundary, with a marker naming how much is shown. |
| Over it, failed | The model's copy keeps the **head and tail** (half the threshold each), because a compiler or test failure sits at the end. The full bytes are still spilled to the log. | Head and tail, same cut. |

If the spill record could not be handed to the log writer, the model gets
the inline cut instead of a stub — a stub must never promise a file that
will not exist.

`read_result` is a scoped builtin (`tools/builtins/read_result.rs`) and a
**grant, not ambient**:

- the main loop offers it when the daemon has a session log and a
  database;
- the lead offers it when its own template declares the capability;
- a subagent gets it when its template declares it. All nine shipped
  templates in `config/agents/` do. An agent whose template does not name
  `read_result` is refused it and reads a stub it cannot follow.

## Loop Guardrails

Beyond the round/cost checks in steps 2–3, the loop enforces:

- **Per-round tool cap** — at most `config.max_tools_per_round` tool
  calls execute per round; the rest get a "max tools per round exceeded"
  error result. When a sandbox policy sets `max_tool_calls`, calls are
  partitioned into executable vs over-budget, with over-budget calls
  returning stub error results instead of executing.
- **Runtime model-access check** — the router may fall back to a
  different model than requested; after each response the actual
  `response.model` is checked against the agent's constraints via
  `CapabilityManager::check_model_access`. A violation publishes
  `SystemEvent::ModelAccessDenied` and ends the loop with
  `LoopFinishReason::Error`.
- **Thinking-block exclusion** — extended-thinking text is logged but
  explicitly omitted from the `ChatMessage` appended to history. It can
  be streamed to a watching client (see [Streaming](#streaming)) and is
  persisted nowhere.
- **Transient LLM-error retry** (router backend only) — a transient
  failure is retried up to 3 times in a row, with a 2 s, 4 s, 8 s backoff
  (capped at 30 s) that a cancellation interrupts. Each retry **costs a
  round**, and retries stop once `max_rounds` is used up. The counter
  resets to zero on any success. Anything else ends the loop with
  `Error`.
- **Approval prompts live in the sandbox, not the loop.** A tool on the
  confirm list blocks inside `SandboxManager::execute_tool` until it is
  approved, denied, times out or is withdrawn; the loop only sees the
  tool's result. A run that declared itself `unattended` has such a tool
  refused at once. The contract is in the Daemon Manual's
  [Tool confirmations](Daemon_Manual.md#tool-confirmations).
- **Compaction telemetry** — see
  [Context Budget and Compaction](#context-budget-and-compaction).
- **Answer guard** — `LoopConfig.answer_guard`
  (`runner/agentic_loop/answer_guard.rs`), consulted at the same point as
  the steering completion guard: after the model returns text with no
  tool calls, before the loop returns `Complete`. The guard owns the
  judgement and supplies both strings; the **loop owns the policy**:
  1. The first rejection buys exactly one corrective round. The rejected
     answer stays in history and the guard's `note` follows it as a user
     message. The round is paid for by a bonus round, so a turn on its
     last affordable round still ends with content.
  2. If the second answer is rejected too, the guard's `runtime_note` is
     **appended** to that answer (`"{answer}\n\n{runtime_note}"`; the note
     alone when the answer is blank). The answer is never taken away.

  It is `None` for every caller but the main loop, and an un-guarded loop
  does not even branch.

### The run-claim guard

The only production guard is `RunClaimGuard`
(`orchestrator/query_handler/run_claim_guard.rs`). It is the last of
three layers that keep a chat turn from announcing a workflow it never
started:

1. **Provenance** — a delegating assistant row replays into later
   history with one fixed line naming the `start_workflow` call and the
   task it returned, so the model can tell its own reported delegations
   from prose it could imitate (replay only; see *History provenance*
   above).
2. **The rules** — `<workflow_relay_rules>` and `start_workflow`'s own
   description agree that a run starts ONLY through a call in this turn,
   that a task id may be stated only when that call returned it, and that
   an explicit workflow request or an ask to write or save an artifact IS
   that call.
3. **The guard** — because prompting is not a guarantee.

The guard's trigger is three **facts**, not a reading of the sentence. It
does not try to decide whether the prose *means* to claim a start:

- The turn's `start_workflow` result cell is empty. A turn that did
  delegate is skipped; its id is on `delegation`.
- The answer states a token in an **id position** — after a `task id` /
  `task_id` / `task-id` / `taskid` / `run id` / `run_id` / `task` cue and
  the punctuation a model wraps an id in — that **looks like an id**: a
  UUID, or a run of 8 or more hex digits that is not a plain number.
  Every decimal digit is a hex digit, so a date (`20260919`) and a counter
  (`12345678`) are excluded by requiring one of `a`–`f`.
- The token matches no run of this turn's owner
  (`TaskRepository::owner_has_task_id_prefix`, a prefix so the 8-hex short
  form counts). The scope is the owner (`created_by`), not the lane, so
  relaying a run the CLI lane started into the GUI lane is true and is
  left alone, while another owner's run never excuses a claim. The
  identity is `ToolContext::created_by()`, the same one `start_workflow`
  stamps on the row and `task_status` reads back.

An answer that states no id touches no database, and a failed lookup
makes the guard say nothing at all. Both sentences the guard writes are
true whether the model invented a start or mis-recalled a status:

- The corrective note names the id(s), says no task of this user has them
  and that no `start_workflow` call was made in this turn, and gives both
  ways out — call `start_workflow` now, or correct or remove the id.
- The runtime line is *"Note from OpenAlpaca: no workflow was started in
  this turn, and no task with id `<id>` exists. Ask again to start one."*

The broad trigger is safe because the consequence is proportionate: a
false positive costs one round and one true line under the answer.

**Streaming**: the first answer's text deltas have already been sent
when the guard rejects it. `done.content` is authoritative — the GUI
replaces the bubble on `done`, and the corrected or annotated answer is
what is persisted as the assistant message. A client that only appends
deltas shows the rejected answer, then the second one, until `done`
settles it.

### The send-confirmation check

One more post-hoc check lives in the main-loop handler rather than in the
loop (`query_handler/simple_query_handler.rs`). When the turn carried a
send intent, a send tool was on the surface, no tool ran, and the answer
still reads as a send confirmation, the answer is **replaced** by a
bilingual warning that the message was NOT actually sent. Unlike the
run-claim guard it takes the answer away, which is why it only fires
with an actual send-intent signal.

## System-Prompt Memoization

Implemented via the layered `ComposeEngine` (`compose/mod.rs`). All
five production prompt-assembly call sites (simple query + social fast
path, skill invocation, lead-agent prompt + subagent spawn — the latter
reusing the `DagNode` compose mode) route through
`ComposeEngine::compose`, which runs
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
- The legacy `PromptBuilder` survives only as the internal backend of
  the static-prompt layer (`compose/static_prompt.rs`).

## Sources

- [Hermes Agent Loop Internals](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop)
