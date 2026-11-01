# ZCode prior-art audit — tools, plugins, skills, subagents, MCP, desktop, supply chain (2026-10-17)

**Status: rev 1, 2026-10-17.** ZCode was read at `/Users/tangyunpei/dev/AlpacaCode/ZCode`, commit `aac47556` (tag v3.15.1, committed 2026-10-10), licensed Apache-2.0 (ZCode:LICENSE, ZCode:NOTICE.md, ZCode:THIRD-PARTY-NOTICES.md). OpenAlpaca was read at HEAD `3598a6fb` on `chore/toolchain-and-deps`. Plan line numbers are HEAD's (`git show HEAD:tasks/deps-upgrade/plan-rev3.md`, rev 3.12; `git show HEAD:tasks/deps-upgrade/modernization-plan.md`, rev 2.1; `git show HEAD:tasks/pending-decisions.md`, rev 3). The 2026-10-17 plan review (`tasks/deps-upgrade/reports/2026-10-17-plan-review.md`) is untracked in the working tree and is cited from there. Nothing here is adopted: this file feeds the plan's second revision and the owner's pending decisions.

## 1. Scope, method and licence

**How it was done.** Eight areas, one auditor each:

| Area | Scope |
|---|---|
| Z1 | Tools, permissions and approvals |
| Z2 | Plugins, hooks and UI plugins |
| Z3 | Skills |
| Z4 | Subagents, background agents and dynamic workflows |
| Z5 | The MCP client |
| Z6 | Desktop shell, app architecture and the GUI |
| Z7 | Toolchain, dependencies, supply chain, CI and the test harness |
| Z8 | A sweep of the pending decisions |

Each auditor read ZCode's code and docs and OpenAlpaca's code and records. It wrote patterns (what ZCode does, what OpenAlpaca does today, the gap, a recommendation, the plan target) and answers for the pending decisions its area touches.

An adversarial verifier then re-read every citation on both sides. It gave each pattern a verdict (confirmed, partly or refuted) and, where needed, a corrected recommendation and a corrected owner-decision flag. Refuted patterns were dropped before this edit. Their number was not passed to the editor, so the raw counts are not known.

What was kept:
- **129 patterns:** Z1 20, Z2 16, Z3 16, Z4 17, Z5 18, Z6 15, Z7 18, Z8 9.
- **94 confirmed and 35 partly.** "Partly" means real but narrower. Those patterns appear here in the verifier's reading: its corrected recommendation and owner-decision flag replace the auditor's.
- **107 pending-decision answers across the areas**, consolidated in §5 to one row per decision. Twenty-five answers had no verifier pass (Z2's five, one of Z3's, five of Z4's, fourteen of Z7's). They are marked † in §5.

ZCode's CI configuration is absent from this export (no `.gitlab-ci.yml`, no `.gitlab/ci/`, no `scripts/ci/`), so every claim about ZCode's CI is doc-only and says so. Nothing was built, run or launched in either repository. Every claim rests on reading source, docs and the local cargo registry sources.

**Licence.** ZCode is Apache-2.0. Ideas, designs, contracts, behaviour, and rule lists read as data are free to use.
- **What counts as a code port.** Copying or transliterating ZCode code, including turning a TypeScript table or a regex set into Rust.
- **What a code port requires.** Shipping the Apache-2.0 licence text and the content of ZCode's NOTICE in an OpenAlpaca THIRD-PARTY-NOTICES file, and marking the files that were changed.
- **Why none can land yet.** OpenAlpaca ships no LICENSE file and no notices today (Z7-02).

Every recommendation below is therefore written as a design transfer. Six places considered a port, and each says "write it fresh":
- the guarded command matcher tables (Z1-02 (c));
- the safe-flag tables (Z1-20);
- the stderr redaction patterns (Z5-09);
- the login-shell capture (Z6-02);
- the deterministic tar writer (Z7-12);
- the malformed-argument wording (Z1-14).

Z1-07 was tagged a code port by its auditor; its verifier reclassified it as a design transfer.

---

## 2. Summary — the lessons worth most

**1. An approval must not grant more than the person saw.** (Z1-02, Z1-03, Z2-03, Z3-01, Z1-09/Z8-01, Z1-08) This is the highest-value cluster, and every item in it is an owner decision. Today:
- One "Always allow" on a harmless `shell_execute ls` waives `rm -rf` for the rest of that run (`crates/openalpaca_core/src/security/confirmation.rs:187-221`).
- An MCP server that sends no hints has every tool run with no card (`crates/openalpaca_core/src/security/sandbox/mod.rs:765-790`).
- A plugin cannot declare its tools destructive, because their annotations are discarded (`crates/openalpaca_plugins/src/manager.rs:1569`).
- A model can write a skill, through the shipped create-skill, whose shell commands never ask, and then run it.
- In a group chat, anyone's `/yes` answers the owner's prompt (`crates/openalpaca_connectors/src/common/mod.rs:206-223`).

ZCode asks for every MCP tool whatever its hints (ZCode:apps/zcode-cli/packages/core/src/mcp/index.ts:148). It narrows what "always allow" can mean per tool (ZCode:apps/zcode-cli/packages/contracts/src/tools/contract.ts:106-114) and lets only the bound owner approve in a group.

**2. No subagent may outlive its run, and the lead's contract must match the code.** (Z4-01, Z4-02, Z4-04, Z4-05) ZCode aborts every in-flight ask before it writes a run's terminal status (ZCode:apps/zcode-cli/packages/dynamic-workflow/src/engine/engine-settlement.ts:26). OpenAlpaca's problems:
- Its subagents run detached and nothing cancels them when the lead returns. The run's token is removed without being cancelled (`crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs:1068`), so orphans keep spending, writing and prompting, and `/cancel` can no longer reach them.
- The lead is told spawning is immediate and queued; the code waits 30 s for a permit and then fails.
- Every wait re-delivers earlier answers at a shrinking budget.
- A resumed run forgets its subagents.

All four are fixes. Z4-01 comes first.

**3. The MCP client must not send a call twice when the first may have reached the server, and must not cut what streams.** (Z5-02, Z5-03, Z5-04, Z5-06, Z5-08) ZCode keeps one deadline per call and retries only on a definite "never sent" (ZCode:apps/zcode-cli/packages/adapters/src/mcp/index.ts:778, :828-830). Its HTTP fetch has no total deadline, and progress resets the timeout. OpenAlpaca's problems:
- It reconnects and sends a timed-out call again (`crates/openalpaca_mcp/src/client.rs:552-584`), so a non-idempotent tool runs twice.
- reqwest's 30 s total timeout (`crates/openalpaca_mcp/src/transport/http.rs:96-98`) cuts every streamed body. P19b's listen stream and M15's 300 s window cannot work over HTTP.
- A mutex serialises every call to a server.
- A refused redirect would read as "retry".

Z5-02, Z5-03 and Z5-08 move no version and belong before the cut, together with D6. Z5-04 follows them, and Z5-06 is an amendment of M15, which lands after the cut.

**4. P19's `Auto` needs an escape hatch and a named failure; M84 needs a form, not an approval card.** (Z5-01, Z5-13, Z1-12, Z5-07) ZCode's dual-era negotiation has a per-server `legacy`, `auto` or pinned mode, a probe bounded by its connect budget, and a typed negotiation failure. It added that failure after a real server's odd answer was shown as "network unreachable" (ZCode:apps/zcode-cli/packages/adapters/src/mcp/index.ts:1726-1747). ZCode's elicitation is form-only, with one typed field per primitive, attributed to the call in flight, and declined otherwise. P19 hard-codes `Auto` behind rmcp's private 10 s probe. M84 plans an Approve/Deny card, which cannot return the `content` a schema with fields asks for. Amend both before they are built.

**5. Keep the thin shell; fix its startup and environment.** (Z6-01, Z6-02, Z6-03, Z6-05)
- A booting daemon reads as alive today and would read as dead under Q04's health probe. That is the review's C-09, and the root cause is that `discovery.json` is written about 560 lines of boot before `axum::serve`.
- A daemon the app spawns inherits launchd's PATH, so `npx`, `uvx` and `claude` disappear (reasoned from macOS behaviour, not measured).
- An agent-written path link in a preview navigates the app's only page away, and M77 as written admits it.
- A WebKit content crash leaves a blank window with no log line.

ZCode's fat Electron shell is not the model. Its startup, environment and navigation lessons are.

**6. Make the gates trustworthy, and gate the primary platform.** (Z7-01, Z7-02, Z7-03, Z7-10)
- CI never compiles OpenAlpaca's macOS-only code: the `getattrlistbulk` scan, iMessage, keychain discovery.
- No LICENSE file and no third-party notices ship.
- The programme's own gate tools pass when they are broken, and none of them runs in CI.
- Nothing keeps M39's `#[expect]` state from regressing.

ZCode keeps a licence gate tied to lockfile hashes (ZCode:scripts/third-party-notices.mjs:15-30) and a fail-closed checker with its own tests (ZCode:scripts/test/architecture-governance.test.mjs:142). Its docs report primary-platform CI.

**7. A call's real limits belong where they are enforced, and a call that cannot succeed must not ask first.** (Z1-04, Z1-05, Z1-13, Z1-14, Z1-15) ZCode treats a tool as a declared contract: permission, result budget, timeout, cancellation. Its executor validates and resolves before hooks and approval (ZCode:apps/zcode-cli/packages/core/src/tool/executor/call-runner.ts:196-260). OpenAlpaca's problems:
- `shell_execute` tells the model 300 s while the main loop kills at 60 s.
- The recovery hint says "increase timeout", but no parameter exists.
- Errors are inferred from a text prefix.
- The card is raised before the arguments are validated.

The fixes are cheap. The per-tool timeout policy is the owner's.

**8. Port ZCode's mechanisms, never its trust defaults.** (Z2-13, Z4-17, Z8-09, Z6-15, Z8-07) ZCode:
- enables a plugin on install;
- runs third-party plugin hooks unconditionally;
- writes sensitive plugin config in plain text;
- has no cost budget and falls back to YOLO when no mode is recorded;
- selects memory with a model;
- has no CSP on its main renderer.

OpenAlpaca's settled rules are stronger on each. One ZCode default is evidence for tightening OpenAlpaca: it never reads another agent's credentials in the background, while OpenAlpaca's daemon polls Claude Code's and Codex's every 30 s by default. That is T24.

---

## 3. The eight areas

Legend for the tables: **owner** = yes, no, or split (part fix, part decision), using the verifier's flag where it differs from the auditor's. **port** = idea, design, or code-port. **verdict** = confirmed or partly.

### Z1 — Tools, permissions and approvals

**ZCode's design in brief.** A tool is a declared contract (ZCode:apps/zcode-cli/packages/contracts/src/tools/contract.ts:168-187). It carries:
- a permission spec: risk level, side-effect scope, `needsApproval`, `alwaysAsk`, and `askOptions.allowAlways` false or `"session"` (:87-115);
- a result budget with a head or tail preview (:117-135);
- a timeout policy `{defaultMs, maxMs, allowCallOverride, cleanupGraceMs}` (:141-153).

The executor (ZCode:apps/zcode-cli/packages/core/src/tool/executor/call-runner.ts) runs lookup, strict schema validation (:196), `validateInput` (:211), `resolveInput` (:231), PreToolUse hooks (:250), permission (:312), then a timeout-wrapped handler. Calls run in parallel only when concurrent-safe, read-only or free of side effects (ZCode:apps/zcode-cli/packages/core/src/tool/scheduler.ts:85-104, :154-194).

`PermissionService.checkPermission` applies a fixed order (ZCode:apps/zcode-cli/packages/core/src/permission/service.ts:101-238). In the default build mode it allows a tool only when it is read-only and needs no approval (:495-560). Every MCP tool is `needsApproval = true`, and its hints set only the risk level (ZCode:apps/zcode-cli/packages/core/src/mcp/index.ts:129-217).

A card offers allow once, allow in this project, and deny with optional feedback (ZCode:docs/permission-project-approval.md:9-90). A match against the 13-rule dangerous-command catalog forces a single-use ask (service.ts:136-147). The shell kill sends SIGTERM to the group and its descendants, then SIGKILL after 1.5 s (ZCode:apps/zcode-cli/packages/adapters/src/exec/process-tree.ts:3-90).

ZCode does **not** confine tool paths to the workspace (ZCode:apps/zcode-cli/packages/core/src/tool/path-policy.ts:36-39). OpenAlpaca's R31/R32 confinement is stricter and must not be relaxed toward ZCode's.

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z1-01 | Serialise tool calls that are not read-only within a round | medium | no (presented for latency) | design | partly | new commit N-TOOL-3 |
| Z1-02 | Always-allow scope declared per tool; no whole-tool allow for commands and writes | high | yes | design | confirmed | new pending row beside T35; new commit |
| Z1-03 | T35: extension tools ask unless they declare themselves read-only | high | yes | design | confirmed | T35 (§5) |
| Z1-04 | Validate a call and take the extension gate before raising its card | low | no | design | partly | N-TOOL-2 |
| Z1-05 | One shell timeout, stated where it is enforced | high | split | design | confirmed | N-TOOL-1; owner half §4.4 |
| Z1-06 | Spawn MCP servers and plugins without the daemon's secrets (= Z5-11) | medium | yes | design | partly | new commit; pending §1.2 |
| Z1-07 | SIGTERM grace and a detached reaper before the group kill | medium | yes (reverses R-T23) | design | partly | storage design §18 residue |
| Z1-08 | Compare-and-swap between the edit card and the write (S4b) | low | yes | design | partly | storage design §18 S4b |
| Z1-09 | Chat-app approvals answered only by the lane owner (= Z8-01) | medium | yes | design | confirmed | code audit H2 |
| Z1-10 | Deny with a reason the model reads | medium | yes | design | confirmed | new commit |
| Z1-11 | T22: scope the read, keep the answer by id, bind a widening answer | low | yes | design | partly | T22 with T16 |
| Z1-12 | M84: attribute an elicitation to the call in flight; form mode only | medium | yes (via M84) | design | confirmed | M84 (§4.3) |
| Z1-13 | Tool errors marked `is_error` on the wire; hints owned by the tool | low | no | design | confirmed | N-TOOL-4 |
| Z1-14 | Malformed tool arguments: warn once, accept stringified objects, report every issue | low | no | design (own wording) | confirmed | N-TOOL-5 |
| Z1-15 | Placeholder for an empty successful result | low | no | idea | confirmed | N-TOOL-6 |
| Z1-16 | Command output spill stub previews the tail too | low | no | idea | confirmed | N-TOOL-7 |
| Z1-17 | MCP image budget (= Z5-12) | low | no | idea | confirmed | M14 follow-up |
| Z1-18 | Background a long command at its budget instead of killing it | low | yes | design | confirmed | later design |
| Z1-19 | Stream a tool's start to the GUI while its arguments arrive | low | no | idea | confirmed | later GUI work |
| Z1-20 | Shell read-only auto-allow: do not adopt (reverses ST-14) | low | yes | design | confirmed | none (§6) |

**Z1-01.** The gap is narrower than the auditor stated.
- *Today:* every executable call in a round runs under `join_all` (`crates/openalpaca_core/src/runner/agentic_loop/mod.rs:1133`, `:1157`, `:1171`). Neither `file_ops.rs` nor `pre_edit.rs` takes a lock, and `file_edit`'s body has await points (`file_ops.rs:933-1012`).
- *When it bites:* bodies interleave only after each call has passed its gate. That happens on an approval-cache hit after Always allow, under `auto_approve`, in a sandbox with no broker, or when two answers arrive within the body's run time. Normally the GUI shows one card at a time, and the second edit plans again after the first write lands (`file_ops.rs:974` against `:1068`). Several cards still appear at once.
- *Recommendation:*
  - Partition each round in model order. Consecutive calls whose tool declares `read_only_hint = true` (for MCP, also `idempotent_hint = true`) share a group. Every other call is a group of one. Groups run in order; calls within a group run concurrently.
  - `spawn_subagents_batch`, `wait_for_subagents` and `check_subagent_status` stay concurrent-safe. Plugin tools, which carry no annotations, run alone.
  - Results keep model order, and cancellation is unchanged.
  - The regression test runs two `file_edit` calls on one file under a pre-approved (EntireTool-cached) or `auto_approve` policy and asserts both edits land. A test with cards answered would likely pass today.
  - Correct the citation: `RegisteredTool.annotations` lives in `tools/registry/mod.rs`, not `builtins/mod.rs:662-670`.

This is a fix. It is presented only because it changes round latency.

**Z1-02.**
- *Gap:* Always allow sends `entire_tool` (`apps/openalpaca-gui/src/views/chat/useChatSession.ts:1425-1444`). The cache keys on the tool name alone (`crates/openalpaca_core/src/security/confirmation.rs:187-221`; recorded after approval at `sandbox/mod.rs:440-454`). One approval of a harmless `shell_execute ls` therefore lets the same turn or agent run execute `rm -rf`, `git push --force` or `git reset --hard` with no card (`apps/openalpaca-gui/README.md:248`). R43 already bounds it to one run. Under ST-14 the card is the only stop for the shell.
- *Recommendation (owner):*
  - (a) The daemon declares on the card what Always allow may mean, and every client renders only what the card offers, as it already does for `per_call`:
    - `shell_execute`: these exact arguments (the existing, unoffered `TheseArgs` scope) or none;
    - `file_write` and `file_edit`: none or these arguments;
    - extension tools: the whole tool.
  - (b) With it, a dangerous-command catalog that makes a matching shell call ask every time, even under an existing allow. ZCode's thirteen rules (ZCode:docs/dangerous-command-approval-v1-plan.md:146-160; matcher wired at ZCode:apps/zcode-cli/packages/core/src/tool/handlers/bash.ts:120-126; forced single-use at ZCode:apps/zcode-cli/packages/core/src/permission/service.ts:136-147) may be reused as data.
  - (c) Later: command-prefix rules from a shell parser. Porting ZCode's `guarded/*.ts` or `bash-command-permission-policy.ts` would be a code port.
  - Recommend (a) now and (b) with it. This changes the accepted Always-allow contract and its Round G toast, and it stays within R43.

**Z1-03.** `effective_confirmation_set` asks only for a tool with `destructive_hint = Some(true)` (`crates/openalpaca_core/src/security/sandbox/mod.rs:765-790`, `unwrap_or(false)` at `:784`). An MCP server that sends no hints, which is common, has every tool run with no card, including on unattended turns. ZCode asks for every MCP call in build mode (ZCode:apps/zcode-cli/packages/core/src/mcp/index.ts:136-148; permission/service.ts:495-550). The recommendation is consolidated in §5's T35 row: ask unless the tool declares `read_only_hint == Some(true)` and not `destructive_hint == Some(true)`. Under it, plugin tools, which carry no annotations, always ask until Z2-03 lands, and unattended turns refuse such tools.

**Z1-04.**
- *Today:* the sandbox raises the card after `confirmation_preview` (`sandbox/mod.rs:353-372`), which returns `Ok(None)` for any tool without a preview hook (`tools/registry/mod.rs:713-723`, default `:337-343`). The extension gate and `validate_tool_arguments` run only at dispatch (`:792`, `:797`, `:1523-1583`). A person can therefore approve a call that is refused after the click.
- *Severity:* it fails closed: the refusal still happens, and the click was wasted. The sentence at `sandbox/mod.rs:353-357` is design 09 §4.4's rule for preview-hook refusals, not a general settled rule.
- *Recommendation:* add a `ToolRegistry` precheck that runs `validate_tool_arguments` and a non-holding extension-ledger check, and call it in the sandbox before the card. Dispatch keeps its guarded check, and the drain guard is still taken at dispatch. The same refusals, earlier (ZCode: call-runner.ts:196-260).

**Z1-05.**
- *Today:*
  - The main loop's tool timeout is `LoopConfig::default()`'s 60 s (`crates/openalpaca_core/src/runner/agentic_loop/config.rs:179`; set at `apps/openalpacad/src/main.rs:478-483`; read at `simple_query_handler.rs:340`; enforced at `sandbox/mod.rs:543`). The daemon key `agent_defaults.max_tool_runtime_secs` (`daemon_config/execution.rs:36`) is not read on this path.
  - `shell_execute` tells the model "Timeout: 300 seconds" (`shell_execute.rs:18`, `:126`).
  - Its description allows chaining while its parameter forbids it (`:124-125` against `:135`).
  - The loop's recovery hint says "increase timeout", and no such parameter exists (`tool_helpers.rs:83-85`).
- *Recommendation, in two halves:*
  - (1) Fixes, apply: generate the stated timeout and the hint from the timeout the sandbox enforces; fix the chaining contradiction; make the main loop read `agent_defaults.max_tool_runtime_secs`, or say why it does not.
  - (2) Owner: a per-tool timeout policy on `RegisteredTool` `{default, max, call_override}`, as in ZCode:apps/zcode-cli/packages/contracts/src/tools/contract.ts:141-153, taking the lower of it and the agent's cap; a model-settable `timeout_secs` capped at max; and the main loop's cap value, 60 s or 300 s.
  - R45's kill at the timeout is unchanged either way.

**Z1-06 (= Z5-11).**
- *Today:* Stage 0's F3 removes only `OPENALPACA_MASTER_KEY` (`apps/openalpacad/src/main.rs:160`). Keys resolved through `secret_env` stay in the process environment (`crates/openalpaca_llm/src/config/key_pool_builder.rs:73`). MCP stdio children (`crates/openalpaca_mcp/src/transport/stdio.rs:62-67`) and plugins (`crates/openalpaca_plugins/src/process_pool.rs:37-49`) inherit the whole environment plus their declared variables.
- *Exposure:* every credential the daemon received through its environment: other servers' `*_env` and `*_from` variables, and LLM keys resolved through `secret_env`. Keychain and encrypted keys are not exposed.
- *What ZCode does:* it strips its own runtime and credential keys through a denylist, not an allowlist (ZCode:apps/zcode-cli/packages/adapters/src/mcp/network.ts:10-21; ZCode:packages/shared/src/runtimeEnv.ts:212-225, :286-292). It scopes only its computer-use broker token away from other children (ZCode:docs/cua-permission-broker/zcode-cua-permission-broker.md:22).
- *Recommendation (owner):*
  - The minimal form first: at spawn, `env_remove` every `secret_env` name, every mcp.toml `*_env`/`*_from` name and `OPENALPACA_*`.
  - A full allowlist (`env_clear` plus PATH, HOME, LANG, TMPDIR) is the stricter option, and it can break a server that relied on an inherited variable.
  - The shell keeps inheriting under ST-14.
  - The Daemon Manual names the change.

**Z1-07.**
- *Today:* `KillGuard` sends an immediate `killpg` SIGKILL (`crates/openalpaca_core/src/tools/platform.rs:175-195`). The recorded residue: lock files such as `.git/index.lock` are left behind (R-T23); a process that leaves the group escapes; on Windows only the direct child dies (`tasks/storage-design/design.md:1069-1076`; `tasks/pending-decisions.md:190`).
- *Recommendation (owner; it reverses the lead's ruling R-T23):*
  - On a timeout or cancel, send `killpg` SIGTERM.
  - Hand the unreaped `Child` to a detached task. The task keeps `run_bounded`'s order (both pipes to their end, then wait), sends `killpg` SIGKILL after a 1–1.5 s grace if the child is still unreaped, and then reaps it. The group id stays the child's until it is reaped, so R45's guarantee holds.
  - Do **not** signal enumerated descendant pids, as ZCode does. That would signal pids the daemon neither owns nor holds unreaped, and break R45's pid-reuse guarantee (`platform.rs:70-77`).
  - Windows stays deferred (DU-33).

**Z1-08.**
- *Today:* the recorded residue S4b is real (`tasks/storage-design/design.md:1056-1061`). The card is planned when it is raised, the body plans again after the click (`file_ops.rs:974` against `:1068`), and nothing compares the two. The window is narrow, and the pre-edit copy keeps the replaced content.
- *Recommendation (owner; it adds a refusal):* record the target's (size, mtime, SHA-256) when the card is planned. `file_edit`, and `file_write` when it replaces a file, recompute them before writing and refuse on a mismatch: "the file changed after you approved; ask again".
- *ZCode's guard is a different mechanism:* it checks read state, Read before Edit (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/edit.ts:60-64, :421-458). Read-before-edit is a separate later option.

**Z1-09.** `/yes` or `/no` answers the chat's oldest pending prompt, whoever sends it (`crates/openalpaca_connectors/src/common/mod.rs:186-223`; code audit H2, deferred, `tasks/pending-decisions.md:206`). Folded into Z8-01. ZCode's owner-only rule was confirmed in docs by this area's verifier and in code by Z8's.

**Z1-10.** `ConfirmationResponse` carries only `approved` and `approval_scope` (`crates/openalpaca_core/src/security/confirmation.rs:48-53`). A denial reaches the model as "Tool 'x' denied by user" (`sandbox/mod.rs:457-472`), so the model usually retries the same call. Recommendation (owner; it adds a wire field and model-visible text, but no refusal):
- an optional `feedback` on `POST /v1/chat/confirmations/{id}`: at most 4,096 characters, trimmed, blank falling back to the plain denial, ignored on approve;
- the feedback appended to the denial after a fixed lead-in in our own wording, and escaped wherever it is displayed;
- a GUI input under Deny, `openalpaca tasks confirmations deny --reason` in the CLI, and `/no <text>` in the connectors;
- `docs/api` regenerated.

ZCode offers this on its V4 desktop and web surfaces only (ZCode:docs/permission-project-approval.md:30-50).

**Z1-11.**
- *Today:* `list_confirmations` is unscoped and copies `tool_arguments` (`apps/openalpacad/src/routes/chat.rs:902-937`). `confirm_tool` checks only the id (`:943-960`). Only the GUI refuses to widen a foreign run (`useChatSession.ts:409-421`).
- *Recommendation:*
  - With T22 and T16: scope the read to the caller's lanes and owned runs, and keep answering by id, as ZCode does (ZCode:apps/zcode-cli/packages/bootstrap/src/zcode-protocol-v4/commands/handlers/interaction-background.ts:35-62).
  - If a widening guard is wanted, accept `entire_tool` only when the body names the prompt's `stream_id` or `lane_key` (main-loop prompts) or its `task_id` checked through `owned_run` (workflow prompts, which carry neither of the first two). A plain approve or deny stays id-only.
  - The auditor's rule, `lane_key` or `stream_id` only, would block Always allow on every workflow card.
  - On a single-token daemon this guard is about correctness, not a security boundary.

**Z1-12.** Folded into the M84 amendment (§4.3). ZCode, all built:
- elicitation is advertised in form mode only, and other modes are declined (ZCode:apps/zcode-cli/packages/adapters/src/mcp/index.ts:1093-1103, :1543);
- a request is attributed through a map of in-flight calls (:1104, :1117-1131); the map is keyed by server name, so two concurrent calls to one server overwrite each other;
- with no call in flight the request is declined (ZCode:apps/zcode-cli/packages/bootstrap/src/zcode-protocol/interaction-broker.ts:255-260);
- the card is live-only and never persisted (:270-288).

**Z1-13.**
- *Today:* the Anthropic `tool_result` is sent without `is_error` (`crates/openalpaca_llm/src/providers/anthropic/request.rs:153-161`). Success is inferred from a `[tool_error]` text prefix (`agentic_loop/mod.rs:1183`). Recovery hints are chosen by tool name and English substrings (`tool_helpers.rs:67-90`), and one of them is wrong (Z1-05).
- *Recommendation:*
  - Carry `is_error` on the tool-result message and map it to Anthropic's field. OpenAI-compatible providers keep the text prefix, because that wire has no field.
  - The loop's success test reads the flag.
  - Move each hint into its tool's own error text.

**Z1-14.** Two sites silently replace unparsable arguments with `{}`: `crates/openalpaca_llm/src/streaming.rs:93-94`, and `crates/openalpaca_llm/src/providers/openai/response.rs:63`, which the auditor missed. `validate_tool_arguments` reports only the first problem and rejects a stringified object (`tools/registry/mod.rs:1523-1583`). Recommendation, at both sites:
- warn once with the tool name and the byte length, never the content;
- accept a top-level string that parses to an object;
- report every issue in one result, in our own wording.

The contract follows ZCode:docs/model-tool-call-validation.md:89-172.

**Z1-15.** A successful shell command with no output returns `""` (`shell_execute.rs:47-60`), and so does an MCP result with no content (`tools/mcp/bridge.rs:84-107`). In one place in the loop, an empty `Ok` result becomes `(<tool> completed with no output)` (ZCode:apps/zcode-cli/packages/core/src/tool/executor/result-serialization.ts:57-68).

**Z1-16.** `spill_stub` carries the first 2 KB and a `result_ref` (`crates/openalpaca_core/src/session_log/record.rs:185-190`). Only the Err path keeps head and tail (`utils/text.rs:77`). For `shell_execute` and command tools, the stub should preview head and tail with the Err path's budget. `spill_stub` is §5.4's stub "verbatim", so amend §5.4's text and its pinned tests in the same commit.

**Z1-17.** Folded into Z5-12.

**Z1-18.** R45 kills a timed-out command with its group (`platform.rs:57-115`). ZCode instead commits an eligible command to the background at its budget and notifies later. The auditor read this as doc-only (ZCode:apps/zcode-cli/docs/design/v2/tool/04-bash.md:223-260); the verifier found it in code (`core/src/tool/executor/background-tasks.ts`). It is a later option only, with a design: it reverses R45 for the shell and needs a task registry, output files and a notification rail.

**Z1-19.** Calls are assembled at the end of the stream (`crates/openalpaca_llm/src/streaming.rs:85-100`), so the commit boundary already holds. Note only: a `tool_input_start {name}` frame, plus a path once it parses, for GUI display (ZCode:docs/streaming-tool-input-preview.md:12-35). Never execute from deltas.

**Z1-20.** ZCode auto-allows an AST-checked read-only Bash command (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/bash.ts:128). ST-14 says every shell command asks (`shell_execute.rs:120-126`; `tasks/pending-decisions.md:188`). Do not adopt; see §6.

### Z2 — Plugins, hooks and UI plugins

**ZCode's design in brief.** A ZCode plugin is a declarative directory in the Claude Code plugin format, not a process with its own protocol. The loader reads `.zcode-plugin/`, `.claude-plugin/` or `.codex-plugin/` `plugin.json` (ZCode:apps/zcode-cli/packages/adapters/src/plugins/index.ts:107-109). A plugin contributes some combination of:
- skills, fed to the host's ordinary skill scanner;
- agents, run by the host's own subagent runtime (ZCode:apps/zcode-cli/packages/bootstrap/src/subagents.ts:93-109);
- hooks;
- MCP servers, namespaced `plugin:<name>:<server>` (ZCode:apps/zcode-cli/packages/adapters/src/plugins/mcp.ts:67-69);
- `userConfig`;
- UI surfaces.

Keys it recognises but does not run produce visible `plugin_unsupported_component` diagnostics (index.ts:112, :378-390). Install activates a tree atomically, with a transaction marker, a backup and recovery on read (ZCode:apps/zcode-cli/packages/adapters/src/plugins/atomic-directory.ts:19-120).

Agents get the canonical name `<plugin>:<agent>`, with a bare alias only when that name is unique and not reserved (subagents.ts:135-148, :207-213). Every plugin tool is an MCP tool and therefore needs approval. Repository-sourced hooks need a persistent trust record keyed by a digest of the declaration (ZCode:packages/shared/src/workspace-hook-digest.ts:192-248).

**Read ZCode's plugin code, not its docs.** The code drifted from its own compat spec on three points:
- third-party hooks run unconditionally (index.ts:371-376);
- plugin agents load;
- uninstall deletes the plugin's data by default (ZCode:apps/zcode-cli/packages/bootstrap/src/plugins.ts:782-784).

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z2-01 | Plugin skills and agent templates need a name-collision rule (= Z3-07) | high | split | design | confirmed | N-PLUG-6; ext design §10 case 13 |
| Z2-02 | The plugin tool bridge reports `isError` results as successes | medium | no | idea | confirmed | N-PLUG-1, before M58 (a) |
| Z2-03 | Plugin tool annotations are discarded, so the confirmation rule cannot reach plugin tools | high | yes (T35) | idea | confirmed | T35 |
| Z2-04 | Recognised-but-unsupported manifest content gets visible warnings | high | no | idea | confirmed | N-PLUG-2 |
| Z2-05 | The approve surface says what approving actually grants | medium | no | idea | partly | N-PLUG-3 |
| Z2-06 | Declarative skills and agent templates inside a plugin directory | medium | yes | design | confirmed | later design |
| Z2-07 | Plugin tools as a bundled MCP server under the plugin's identity | medium | yes | design | confirmed | later design after P19, P19b, M14, M82 |
| Z2-08 | `mcp_compatible = true` skips every handshake, so a real MCP server cannot load | medium | no ((a)); owner ((b)) | idea | confirmed | N-PLUG-5 after P19 |
| Z2-09 | A plugin update rolls back a failed swap and recovers from a crash mid-swap | medium | no | idea | confirmed | N-PLUG-4 |
| Z2-10 | Bind plugin consent to a digest of what runs | medium | yes | design | confirmed | new pending row; ext design §13 Q4 |
| Z2-11 | The approve preview lists a plugin's skill, agent and slash claims | low | yes | design | confirmed | later design |
| Z2-12 | Decide the unwired connector and provider bridges before M13 and M58 | low | split | idea | partly | M13, M58; new connector row |
| Z2-13 | Do not adopt ZCode's weaker trust defaults | medium | no | idea | confirmed | §6 |
| Z2-14 | Hooks as an extension point, if ever wanted | low | yes | design | confirmed | later design |
| Z2-15 | UI plugins as MCP Apps, if the GUI ever hosts plugin pages | low | yes | design | confirmed | later design |
| Z2-16 | Remote install sources, if ever added | low | yes | idea | confirmed | later design |

**Z2-01 (= Z3-07).** This is a confirmed defect, and no malice is needed to trigger it.
- *Today:*
  - A plugin's skill id, slash command and aliases are inserted with no check for an incumbent (`crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs:721-757`).
  - `register_template` refuses a duplicate agent id by returning `false` (`crates/openalpaca_core/src/agent/registry/mod.rs:83-92`). The plugin manager ignores the result and records the id as its own (`crates/openalpaca_plugins/src/manager.rs:1432-1434`).
  - At T2, entries are removed by id whoever owns them, and a tombstone naming the plugin is written (`manager.rs:953` → `catalog/mod.rs:199-224`; `manager.rs:962` → `registry/mod.rs:118-126`).
  - `remove` falls back to a scan by display name (`catalog/mod.rs:672-707`).
  - The reverse also happens: `load_entry` inserts over an existing key (`catalog/mod.rs:420`), so a hot-reloaded file skill takes a live plugin skill's id, and the plugin's later unload deletes the file skill.
- *Examples:* a plugin that answers `agent/info` with `writing_agent` deletes the shipped template at its next disable. A plugin skill with slash `/review` replaces the shipped one at once, and the plugin's disable deletes both.
- *What ZCode does:* canonical `<plugin>:<agent>` names, an `agent_ambiguous_name` diagnostic instead of a bare alias (ZCode:apps/zcode-cli/packages/bootstrap/src/subagents.ts:135-148, :207-213), and `plugin_duplicate_id` for a duplicate plugin (ZCode:apps/zcode-cli/packages/adapters/src/plugins/index.ts:160-170).
- *Recommendation:*
  - **Apply now:** honour `register_template`'s return value. At T2, remove an entry only when its source is this plugin (`SkillSource::Plugin{plugin_id}`, or the template's source), and never through `remove`'s name-scan fallback.
  - **Present:** the skip rule. At E4, a plugin skill id, slash, alias or agent id that a live incumbent holds (a file skill or template, or another enabled plugin) is skipped and recorded on the row beside `skipped_tools`. The rule sits on both insert paths, `register_plugin_skill` and `load_entry`.
  - Z2's verifier read the skip rule as mirroring §10 case 13's tool rule (`tasks/extension-enable-design.md:2459`), so not a decision. Z3's verifier read it as an amendment of case 13, to be presented. This edit takes the stricter reading.
  - Canonical `<plugin>:<id>` naming stays a separate owner option, because it changes naming and the protocol.
  - Land before any plugin-surface refactor. The M58 ordering argument is weak, because the fix touches the catalog and the registry, not the executor traits.

**Z2-02.** `PluginToolProxy::execute` joins `content[].text` and returns `Ok` whatever `isError` says (`crates/openalpaca_plugins/src/bridge/tool_bridge.rs:50-63`). The MCP bridge maps `is_error` to `Err` (`crates/openalpaca_core/src/tools/mcp/bridge.rs:102-104`). The defect bites when a plugin follows the MCP result shape, as `mcp_compatible` plugins are meant to. Recommendation:
- map `isError: true` to `Err(joined text)`, with a test using a single-frame error result;
- state in the bridge comment that a plugin's `tools/call` result follows MCP's `CallToolResult`, since no other document says so.

**Z2-03.** Every plugin tool registers with `annotations: None` (`manager.rs:1569`), and `discover_tools` reads only the name, description and input schema (`:1529-1545`). The rule MCP servers can trigger with `destructiveHint` is therefore unreachable for plugins. Reading the annotations is the precondition for any T35 answer to treat plugins consistently. It creates prompts, and refusals on unattended turns, where none existed, so it goes to the owner as part of T35 (§5) and does not land alone.

**Z2-04.** Two failures that both pass the validate dry run as clean:
- *A manifest typo.* `PluginManifest` has no `deny_unknown_fields` and no warning for an unknown key (`crates/openalpaca_plugins/src/manifest.rs:7-72`). A typo such as `tool = true`, where the key is `types.tools` and tool discovery runs only under it (`manager.rs:1237`), loads a plugin with zero tools and no message.
- *Unwired types.* `types.connector` and `types.provider` are probed (`:1247`, `:1268`) and listed in the preview with no caveat (`crates/openalpaca_plugins/src/install.rs:196-202`), but neither bridge has a production caller (`tasks/pending-decisions.md:238`, `:245`).

Recommendation: warn, never refuse. Warn on an unknown key, and on `types.connector` or `types.provider` while their bridges are unwired, in four places: the validate dry run, the install/approve preview, the row's `warnings`, and the log. This builds the warning half of the owner's taken decision D-I (revised), which rules such a warning for `provider = true` (`tasks/llm-simplify/design.md:18`; the file is git-ignored and exists on disk only), and extends it to connectors and unknown keys (ZCode: index.ts:112, :378-390).

**Z2-05.** The GUI manual says "`Approve` grants exactly those strings" (`docs/GUI_Manual.md:1332`; also `apps/openalpaca/src/commands/ext.rs:602`, `:1144`). In context the sentence describes what the consent records, not a sandbox. Still, the child is spawned as the owner, with the daemon's environment and its own directory as working directory (`crates/openalpaca_plugins/src/process_pool.rs:36-48`).

Recommendation: add one sentence under the approve preview in the CLI and the GUI, and in both manuals:

> Approving starts the plugin's program as you, with the daemon's environment (less the master-key variable, Stage 0 F3) and full file and network access; its capabilities name what it contributes to agents, not what it can do.

Cross-reference `docs/Daemon_Manual.md:185`. ZCode states the same boundary plainly (ZCode:docs/plugin-hooks-compat.md:23-29).

**Z2-06.** ZCode hands a plugin's `skills/` to the host's scanner and its `agents/*.md` to the host's own profile parser (ZCode:apps/zcode-cli/packages/adapters/src/plugins/index.ts:392-396; subagents.ts:93-109). OpenAlpaca contributes skills and agents only through the `skill/info` and `agent/info` RPCs (`manager.rs:1296-1347`). That path loses streaming and attachments (`tasks/pending-decisions.md:250`), the working-folder line (`:199`), and the ability to replace a file (R46, `:197`). As an owner option and a later design: declarative `skills/<id>/SKILL.md` and `agents/<id>.md` inside a plugin directory, loaded through the existing parsers, named under Z2-01's collision rule, and with consent covering the declared files (Z2-10, Z2-11). It widens what a plugin is.

**Z2-07.** OpenAlpaca plugins speak a bespoke, MCP-shaped JSON-RPC (`process_pool.rs:105-147`; `tool_bridge.rs:46-66`). So every MCP improvement in the programme (P19, P19b, M14, M15, M82) reaches MCP servers but not plugins.
- *Owner option:* a plugin may declare `[mcp_servers.<name>]`. The plugin supervisor starts each server through `openalpaca_mcp` as a contribution registered at E4 and withdrawn at T2, never as its own `/v1/extensions` row, which keeps X-4.
- *Why it is the owner's:* it reverses an accepted design statement, "OpenAlpaca plugins do not bundle MCP servers" (`tasks/extension-enable-design.md:224`).
- *Sequencing:* after P19, P19b, M14 and M82.
- *If declined:* take Z2-02, Z2-03 and Z2-08 (a) as point fixes.

**Z2-08.** When `mcp_compatible` is true, the manager skips `initialize` and sends `tools/list` first (`manager.rs:1165-1190`, `:1518-1521`). An rmcp server refuses any first message other than `initialize` (rmcp-0.16.0 `src/service/server.rs:195-201`, the version in `Cargo.lock`). The test fixture `tests/fixtures/echo-plugin` is a tolerant shell script, which is why the tests pass.
- Prefer (a): the real lifecycle (`initialize`, then `notifications/initialized`) through `openalpaca_mcp`, after P19. State that `mcp_compatible` plugins receive no `[config]`, and warn at validate time if one declares config keys (Z2-04).
- Deleting the flag, (b), would remove a design-stated feature, so it is the owner's alternative if Z2-07 is declined.

**Z2-09.** `update_from_path` trashes the old tree and then commits the staged one, with no rollback between the two steps (`manager.rs:2096-2099`). If the commit fails, `Staged`'s Drop sweeps the staged copy too (`install.rs:471-486`). The plugin is then absent, and its last good tree sits only in `.trash/`. Recommendation:
- restore the trashed tree when the commit fails, before returning the error;
- write a one-line marker, `.staging/<id>.swap`, before the trash move, and remove it after the commit;
- a boot reconcile pass that finds the marker restores the trashed tree and sweeps `.staging/` copies older than the process.

D1 and the extension design's §1.3 rule 3 (nothing the owner dropped in is deleted) are kept. ZCode: atomic-directory.ts:19-120; ZCode:apps/zcode-cli/packages/adapters/src/plugins/marketplace.ts:836-847.

**Z2-10.** Consent records only the capability list (`crates/openalpaca_plugins/src/permission_gate.rs:57-85`). E1 re-prompts only when capabilities grow (`manager.rs:1104-1125`), and an update resets consent only when the declared set changes (`:2058-2068`). A remote update that swaps `entry` to another program, or adds `types.agent = true`, therefore runs under the old approval.

Recommendation (owner; it adds an enforcement point):
- record a `consent_digest` at approve time over `entry`, `args`, `env` keys and values, `types` and capabilities, virtual ones included;
- treat a mismatch at E1 like capability growth, with a new reason such as `DeclarationChanged` and a preview of the delta;
- offer no escape hatch, consistent with Q4;
- hash the entry file's bytes only as an option, because a plugin that rewrites its own program at runtime would be re-prompted for ever.

ZCode's digest covers the source path, event, matcher, execution, timeout and output cap (ZCode:packages/shared/src/workspace-hook-digest.ts:192-248).

**Z2-11.** `ManifestSummary` (`install.rs:153-206`) cannot show which skill id, slash, aliases or agent id a plugin will claim. Those arrive from `*/info` only after approval. As an owner option: declare `[[contributes.skill]]` and `[[contributes.agent]]` in `plugin.toml`, show them escaped in the preview, and have E4 refuse an `*/info` answer that claims anything undeclared. Z2-06 would give this for free.

**Z2-12.** The verifier corrected the premise.
- *Provider half:* the owner already took D-I (revised) on 2026-09-23 (`tasks/pending-decisions.md:286`). Schedule its deletion of `PluginLlmProvider`, `get_plugin_provider` and `ProviderType::Plugin`, with the install warning, before M58's `LlmProvider` commit (`modernization-plan.md:803`), or record in M58 that the plugin implementation is deleted, not converted.
- *Connector half:* this one is genuinely undecided, and needs a new owner row. Either delete `PluginConnector`, `get_plugin_connector` and the `types.connector` probe, keeping Z2-04's warning, or design the wiring.
- M13's touch on `connector_bridge.rs` is one attribute, so it can proceed either way.

**Z2-13.** ZCode has five defaults weaker than OpenAlpaca's: enable on install, unconditional plugin hooks, sensitive config in plain text, workspace config enabling plugins, and uninstall deleting data. Recorded as rejected prior art in §6, so a later borrowing is presented rather than slipped in.

**Z2-14, Z2-15, Z2-16.** These are notes for later designs.
- *Z2-14, hooks:* ZCode's contract is seven events; exit code 2 blocks; deny > ask > allow; a per-session snapshot (ZCode:docs/hooks-runtime-contract.md:9-204). A hook that can deny adds an enforcement point. A plugin's hooks would ride its approve gate, bound to Z2-10's digest. Never let "enabled" imply "hooks run" without consent, which is ZCode's open gap.
- *Z2-15, UI plugins:* if pages are ever wanted, use MCP Apps, with every page action routed through the ConfirmationBroker (ZCode:UI_PLUGIN.en.md:296-363). That needs MCP resources, which are a recorded non-goal.
- *Z2-16, remote install:* a future source would be new egress, so an owner decision. ZCode's rules would be the floor: a mandatory sha256, symlink entries refused, the local copy's budgets, and staging (ZCode:apps/zcode-cli/packages/adapters/src/plugins/zip-source.ts:14-19, :408-415). It enters `Unapproved{NeverSeen}`.

### Z3 — Skills

**ZCode's design in brief.** A ZCode skill is the agentskills / Claude Code shape: a directory with a SKILL.md whose body is loaded **into** the running conversation by one Skill tool. It carries no tool grants and no permission policy. The frontmatter keys are name, description, `when_to_use`, license and metadata (ZCode:apps/zcode-cli/packages/adapters/src/skills/index.ts:21-27).

Discovery merges the home, workspace, plugin and bundled roots (ZCode:apps/zcode-cli/packages/adapters/src/skills/roots.ts:20-105). Every scan error except ENOENT becomes a `skill_scan_failed` diagnostic (ZCode:apps/zcode-cli/packages/adapters/src/skills/scan.ts:11-15).

The catalog is a dynamic context section listing every skill. It has a 20,000-character budget and falls back to a names-only list, so no skill silently disappears (ZCode:apps/zcode-cli/packages/core/src/context/sections/skills.ts:9-10, :36-78). The Skill tool returns the body capped at 100 KB with an explicit truncation marker (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/skill.ts:17-35, :77-127). Product skills ship as a read-only bundled pack outside the plugin system (ZCode:apps/zcode-cli/packages/bootstrap/src/app/bundled-skills.ts:16-163).

OpenAlpaca's skill is a different, settled design: a separate sandboxed sub-loop with its own declared tools, its own confirmation list, scripts exposed as `skill_script:*` tools, auto-routing and cron (`crates/openalpaca_core/src/orchestrator/skill/invocation.rs:180-441`). Switching to ZCode's in-loop model would reverse P1-1 and Routing V2, and is not recommended.

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z3-01 | A skill may add approval prompts, never remove them; skill scripts ask like the shell | high | yes | design | confirmed | new T-row beside T35; before B3 |
| Z3-02 | Every available skill reaches the model's catalog, and the lead gets the catalog | high | no | design | confirmed | N-SKILL-1, before M07 |
| Z3-03 | Typed skill diagnostics on the API, the CLI and Settings | medium | no | design | partly | N-SKILL-2, before P30 and M36 |
| Z3-04 | Unknown frontmatter keys recorded and reported | medium | no | idea | confirmed | P30 |
| Z3-05 | Frontmatter edge cases belong in P30's pin test | medium | no | idea | partly | P30, E6 corpus, D4 |
| Z3-06 | Ship OpenAlpaca's own skills as a read-only bundled pack | medium | yes | design | confirmed | later design (pending-decisions §4.6) |
| Z3-07 | A plugin skill never replaces a file skill (= Z2-01) | medium | yes | design | confirmed | N-PLUG-6 |
| Z3-08 | create-skill writes user-scope skills to a root nothing scans | medium | no | idea | confirmed | N-SKILL-3 |
| Z3-09 | Say when a skill's instructions were cut | low | no | idea | confirmed | N-SKILL-4 |
| Z3-10 | Bound the level-1 scan: regular files and a size cap | low | no | idea | confirmed | in N-SKILL-2 |
| Z3-11 | A subagent skill allowlist | low | yes | design | confirmed | later design |
| Z3-12 | Project-scope skills, a standard-skill bridge, import from other agents | low | yes | design | confirmed | later design with T8, T24 |
| Z3-13 | A composer `/` picker fed by the same catalog | low | no | idea | confirmed | later GUI design |
| Z3-14 | Per-skill enable toggle: recommend no change | low | yes | idea | confirmed | none (§6) |
| Z3-15 | Key skill telemetry and the execution log by catalog identity | low | yes | design | confirmed | migration 005 |
| Z3-16 | Gate a tool on its teaching skill having been read | low | no | idea | confirmed | later, with T3 |

**Z3-01.**
- *Today:*
  - A skill's `permissions.confirm.tools` becomes its sandbox's explicit list, which **replaces** the annotation defaults (`crates/openalpaca_core/src/security/sandbox/mod.rs:765-775`). This is true on all three skill paths: `invocation.rs:388-393`, the nested executor (`invoke_executor.rs:453`) and the plugin-skill tier (`invocation.rs:1264`).
  - `skill_script:*` tools register with `annotations: None` (`invocation.rs:825-834`), and `annotations_for_builtin` has no arm for them (`crates/openalpaca_core/src/tools/builtins/mod.rs:629-646`), so they never ask.
- *How a model gets there with no card at any step:*
  1. The shipped create-skill writes SKILL.md through such a script (`config/skills/create-skill/SKILL.md:65`, `:122`).
  2. The script takes `confirm.tools` from its arguments (`config/skills/create-skill/scripts/write_skill_md.sh:156-163`) and writes into `<root>/config/skills` (`:32`, `:84-87`).
  3. The watcher hot-loads the new skill (`apps/openalpacad/src/hot_reload.rs:554-560`).
  4. The main loop's `invoke_skill` can run it (`crates/openalpaca_core/src/tools/builtins/main_loop.rs:292-307`).

  That is T34's shape, fixed for `system_agent` by Stage 0's F1, coming back through skills.
- *What is and is not recorded:* the explicit-list rule for skills is recorded as R30 (`tasks/storage-design/design.md:84`, `:652`, `:1046-1052`; `tasks/pending-decisions.md:217`). The model-authored path and the unannotated scripts are not.
- *Recommendation (owner, beside T35; it reverses R30 for skills):*
  - (1) Union a skill's `confirm.tools` with the annotation-derived set on all three paths. Agent templates keep R30 and R-T35 unchanged.
  - (2) Give `skill_script:*` the shell's annotation: destructive, open-world.
  - Cost: scheduled skills (`apps/openalpacad/src/scheduled_skills.rs`) are unattended, so under (2) a cron skill that relies on its scripts stops. Offer the narrower form too: ask on attended turns, and follow T35's answer for unattended ones.
  - ZCode backs (2): scripts run through its shell tool under that tool's own approval. ZCode has no analogue for (1).

**Z3-02.**
- *Today:*
  - `<available_skills>` has a 500-character budget and `break`s at the first line that does not fit (`crates/openalpaca_core/src/orchestrator/query_handler/mod.rs:255-278`), so later skills are dropped without a word.
  - Its header says the *user* can invoke them with slash commands (`:262`), although the main loop holds `invoke_skill`.
  - A skill with no slash is listed by display name (`:269`), while `invoke_skill` resolves only a slash, an alias or the id (`invoke_skill.rs:159-162`; `catalog/mod.rs:527-551`), so the listed name fails as unknown.
  - The lead holds `invoke_skill` and is told to use catalog skills (`crates/openalpaca_core/src/runner/lead_agent/prompt.rs:16-22`), but its prompt passes `skills_catalog: None` (`:248`) in `SubagentMinimal` mode, which renders no catalog (`crates/openalpaca_core/src/compose/static_prompt.rs:195-214`).
- *Recommendation:* one renderer for the main loop and the lead, built from the availability-filtered `catalog_summary` (`catalog/mod.rs:576`):
  - each line gives the id and the `/slash`, and the description cut with an ellipsis;
  - over the budget, it falls back to names only;
  - the budget lives in `[orchestrator.prompt_budgets]`;
  - the wording names `invoke_skill`;
  - for the lead, the same block is a raw block beside `extension_tools_suffix`.

  Lead prompt goldens will move. Land before M07, which edits `skill/catalog/mod.rs`, or rebase M07.

**Z3-03.**
- *Today:* a SKILL.md that cannot be read or parsed, or that conflicts on its slash, is skipped with a warning only (`catalog/mod.rs:272-281`, `:296-300`, `:336-345`, `:433-442`). `GET /v1/skills` lists only loaded skills (`apps/openalpacad/src/routes/skills.rs:110-172`), and the CLI has no skills command.
- *Recommendation:*
  - The catalog keeps the last scan's diagnostics `{code, severity, path, skill_id?, message}`, with the codes `read_failed`, `missing_frontmatter`, `invalid_yaml`, `missing_field`, `id_mismatch`, `deprecated_field`, `slash_conflict`, `unknown_key` (Z3-04) and `too_large` (Z3-10).
  - Serve them on a new `GET /v1/skills/diagnostics`, so `GET /v1/skills` stays a bare array (plan §7).
  - Add `openalpaca skills list|info` and a count in Settings → Tools, and regenerate `docs/api`.
  - The diagnostics' messages are `SkillParseError`'s Display strings, which M36 golden-tests, so land this before M36 (`modernization-plan.md:757`) or reuse M36's golden test.
  - Treat it as a complement to E6's rebuilt `yamlcmp` comparator (`plan-rev3.md:933`), which already reports parse failures over the owner's files, not as the gate P30 lacks.

**Z3-04.** `SkillFrontmatter` is `#[serde(default)]` with no capture of unknown keys (`crates/openalpaca_core/src/middleware/skill/types.rs:303-334`). A one-character typo such as `require_capabilities` leaves a skill with no tools (`invocation.rs:253-255`, `:438-441`) whose body still tells the model to use them. Recommendation, in P30 or the commit before it:
- an `unknown_key` warning diagnostic;
- a warning when a skill declares no tools at all;
- keep the lenient parse (`middleware/skill/mod.rs:8-9`): no `deny_unknown_fields`, which would refuse legacy and third-party files.

Whether serde-saphyr supports `flatten` the same way is part of D4's research.

**Z3-05.** Three edge cases, read but not run:
- *BOM:* `extract_frontmatter_str` uses `trim_start()` (`crates/openalpaca_core/src/middleware/skill/parser.rs:6`), and U+FEFF is not White_Space, so a BOM-prefixed SKILL.md reads as having no frontmatter.
- *Closing delimiter:* the frontmatter ends at the first newline followed by `---` (`:17`), so a `----` or `---x` line also closes it.
- *CRLF:* very likely handled today, but untested.

`middleware/skill/tests.rs` has no BOM, CRLF or block-scalar case, although the shipped create-skill uses `description: >`. Recommendation, into P30's pin test and E6's corpus:
- fixtures written fresh for BOM, CRLF, `>`/`>-`/`|`/`|+`, a quoted value containing `: `, a nested map, an unknown key and an empty body, asserted equal under serde_yaml 0.9 and the chosen serde-saphyr;
- strip one leading BOM, and match the closing delimiter as a whole line;
- keep **one** serde parser: ZCode's own two-parser bug (ZCode:docs/external-agent-skills-import.md:63) is the argument;
- fix or retire `config/skills/create-skill/scripts/list_skills.sh`, whose `grep -m1 'slash:'` matches anywhere in the file (`:39-42`).

**Z3-06.** The four shipped skills are seeded once into `<config>/skills`, and an existing directory or file is never overwritten (`apps/openalpacad/src/bootstrap/content.rs:61-71`, `:102-114`). So no fix to a shipped skill reaches an existing install. That is the same shape that forced the switch-over note's hand edit after F1.
- *Owner option (it changes ruling L9's seeding):* embed the shipped skills and materialise them read-only under `state/bundled-skills/<sha256>/` as the lowest-precedence scope, as ZCode does at priority 1,000,000 (ZCode:apps/zcode-cli/packages/bootstrap/src/app/bundled-skills.ts:50-59). The owner overrides one by creating the same id under `config/skills`. The switch-over note names the old seeded copies.
- Agent templates share the seeding gate.

**Z3-07.** Folded into Z2-01.

**Z3-08.** create-skill's `scope=user` writes to `$HOME/.config/openalpaca/skills` (`write_skill_md.sh:84-86`; `scaffold.sh:59`; `check_exists.sh:31`; `list_skills.sh:58`). The daemon never scans that directory: it reads `<config>/skills` only (`apps/openalpacad/src/services/mod.rs:195-209`). The directory also lies outside `~/.openalpaca` (D1), and the skill still reports success. Recommendation:
- drop the `user` scope, or refuse it naming the one root;
- fix `SkillScope::User`'s comment (`types.rs:13`);
- because of the seeding gate, the switch-over note says that existing installs keep the old scripts.

**Z3-09.** `skill_to_prompt_block` keeps `budget_tokens*4` characters (16,000 by default) with no marker (`crates/openalpaca_core/src/middleware/skill/renderer.rs:41-57`). Add one line naming the cut: "instructions cut at N of M characters".

**Z3-10.** The boot scan follows links and reads each SKILL.md whole and synchronously (`catalog/mod.rs:286-292`, `:339-340`; `services/mod.rs:195-209`). By reading, not by execution: a FIFO or device would block or exhaust memory at boot. The directory is owner-authored, so the risk is low. Require a regular file (following the owner's own links stays allowed), and cap the level-1 read with a `too_large` diagnostic. Fold into Z3-03's commit.

**Z3-11 to Z3-16.**
- *Z3-11:* a template `skills: [ids]` granting a filtered `invoke_skill` (ZCode:apps/zcode-cli/packages/core/src/subagent/runner.ts:2127-2140). It widens a subagent's surface against "subagents stay template-scoped", so it is an owner option only.
- *Z3-12:* project skills and import from other agents (ZCode:apps/zcode-cli/packages/adapters/src/skills/roots.ts:45-105; ZCode:packages/services/src/settings-sync/settingsSyncService.ts:139-169). If ever adopted: they enter unapproved, may carry no scripts and no confirmation list, are copied rather than linked (D1), and are recorded with a content hash. Decided with T8 and T24. This also meets ST-9.
- *Z3-13:* a GUI `/` picker from `GET /v1/skills`, which already serves `triggers.slash` (`apps/openalpacad/src/routes/skills.rs:156-160`). Needs a DESIGN_SPEC §5.6 entry.
- *Z3-14:* do not adopt ZCode's per-path toggle. It would be a third governance axis, and `invoke.mode: disabled` stays the switch (`routes/skills.rs:7-13`).
- *Z3-15:* `skill_execution_log.skill_id` holds the display name, so a rename splits a skill's history (`crates/openalpaca_storage/src/repository/skill_execution/mod.rs:444-471`). Take the recorded fix with migration 005, bundled with the `agent.config` drop.
- *Z3-16:* ZCode gates four workflow tools on their teaching skill having been loaded (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/workflow-skill-gate.ts:4-44). Note only, with T3.

### Z4 — Subagents, background agents and dynamic workflows

**ZCode's design in brief.** Both of ZCode's delegation mechanisms run in process: a subagent is a child runtime with its own session (ZCode:apps/zcode-cli/docs/design/v2/tool/07-subagent.md:18).

The Agent tool:
- resolves a profile, and gives the child that profile's tool pool minus a global denylist: no recursion, no AskUserQuestion (07-subagent.md:154-169);
- replaced its fixed 600 s wait with an activity watchdog (ZCode:docs/subagent-timeout-policy.md:5-19; ZCode:apps/zcode-cli/packages/core/src/subagent/runner.ts:211-220, :1196-1320);
- registers every launch as a task before it runs (runner.ts:184-192);
- delivers a background completion as a notification on the parent's command queue. An active turn absorbs it at its next step; an idle parent gets one model-only wake turn, delivered at most once (ZCode:apps/zcode-cli/packages/core/src/runtime/methods/runtime-command-queue.ts:37-70, :317-394).

Every admitted input is pinned in a durable ledger (ZCode:apps/zcode-cli/packages/contracts/src/interfaces/session-store.port.ts:849-858, :1142-1192).

Dynamic workflows are a model-written TypeScript script. It is typechecked, drawn as a static graph, confirmed, and journaled to SQLite ("the journal is the truth", ZCode:docs/dynamic-workflow.md:8-33, :200-203). Every terminal path aborts the run's in-flight asks before its status is written: "no ask may outlive its run" (ZCode:apps/zcode-cli/packages/dynamic-workflow/docs/execution-engine.md:513-540, :762-764). There is no cost budget anywhere; usage is telemetry summed over the run's lineage (ZCode:docs/dynamic-workflow/authoring.md:1291-1307).

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z4-01 | No subagent outlives its run | high | split (the cancel is a fix; the completion guard is the owner's) | idea | confirmed | N-LEAD-1; sibling of M40 |
| Z4-02 | Queue a spawn under the bound instead of blocking the lead and failing it | high | no | idea | confirmed | N-LEAD-2, after N-LEAD-1 |
| Z4-03 | Seal follow-up admission at shutdown; settle follow-ups a crash left claimed | low | split | idea | partly | N-DAEMON-1; owner half §4.4 |
| Z4-04 | Deliver each subagent result to the lead once | medium | no | idea | confirmed | N-LEAD-3 |
| Z4-05 | On resume, rebuild the subagents' state from a durable record | medium | no | design | confirmed | N-LEAD-4 |
| Z4-06 | Hand finished subagents' results to the lead at its round boundary | low | split (`until: "any"` is a fix; injection is the owner's) | design | partly | later design |
| Z4-07 | Let the lead stop or redirect one subagent | medium | yes | design | confirmed | later design |
| Z4-08 | A subagent can ask the lead a question | low | yes | design | confirmed | later design, after N-LEAD-1 |
| Z4-09 | A workflow's spend is one figure across subagents and resumes (T13) | medium | yes | design | confirmed | T13 |
| Z4-10 | Approval windows run one at a time | low | yes | design | confirmed | later design beside R43 |
| Z4-11 | Bind a run's promised work to the conversation that started it (T18) | low | yes | idea | confirmed | T18 |
| Z4-12 | Race every wait on a child against cancellation | low | no | idea | partly | beside M60 |
| Z4-13 | Rows the daemon wrote carry provenance the working-state writer reads | low | yes | idea | partly | continuity design 11 |
| Z4-14 | A subagent's failure text gives a cause the lead can act on | low | no | idea | confirmed | N-LEAD-5 |
| Z4-15 | Shutdown gives each stage its own deadline | low | no | idea | confirmed | N-DAEMON-1 |
| Z4-16 | Typed subagent results; the dynamic workflow as the batch-work answer | low | yes | design | confirmed | later design reusing M82 |
| Z4-17 | ZCode choices OpenAlpaca has settled against | low | yes (information) | idea | confirmed | none (§6) |

**Z4-01.**
- *Today:*
  - Each subagent runs in a detached `tokio::task::spawn` on `cancel_token.child_token()` (`crates/openalpaca_core/src/runner/lead_agent/tools.rs:651`, `:661`).
  - `run_lead_agent` returns when its loop exits and cancels nothing (`crates/openalpaca_core/src/runner/lead_agent/mod.rs:725-764`).
  - The dispatcher then calls `remove_cancellation_token` (`crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs:1068`), which deletes the token without cancelling it (`crates/openalpaca_core/src/context/shared/mod.rs:299-305`). Its comment assumes nothing is left to stop (`lead_agent.rs:1066-1067`), which is false while subagents run.
- *How a lead ends with subagents still running:* a MaxRounds or CostExceeded exit, or the 600 s wait cap (`tools.rs:1206`).
- *What the orphans do:* they keep calling the model outside every cap, write files in the project and raise approval cards. `/cancel` can no longer reach them, and the timeline shows them as interrupted (`lead_agent.rs:1012-1015`).
- *Recommendation:*
  - **Apply, as a fix:** when `run_lead_agent`'s loop returns, cancel the run's subagents (`ctx.cancel_task(&task_id)`, or the tracker-held child tokens) before `lead_agent.rs:1068`. Cancelling the token first matters, because R45's process-group kill rides on it. Wait a bounded time for their spans to close as Cancelled, with the detail "the run ended before this subagent finished", and list those subagents in the completion record. Hold the handles in a JoinSet, so a dropped run future aborts them too (M40's own reason).
  - **Present, as an option layered on top:** a completion guard that gives the lead one more round to wait for, or explicitly abandon, its running subagents. The precedent is the steering completion guard (`crates/openalpaca_core/src/runner/agentic_loop/mod.rs:1318`).
- ZCode: ZCode:apps/zcode-cli/packages/dynamic-workflow/src/engine/engine-settlement.ts:26.

**Z4-02.** The lead is told "Spawning is always immediate … queued if capacity is limited — this is handled automatically" (`crates/openalpaca_core/src/runner/lead_agent/prompt.rs:130-132`). `spawn_subagent` instead waits up to 30 s for a semaphore permit before registering, then fails (`tools.rs:235-249`; register at `:649`). The stock limit is 6 concurrent subagents (`config/daemon.toml:10`), while `spawn_subagents_batch` accepts 8 and spawns them one after another (`tools.rs:1001`, `:1026`). So on a stock install, entries 7 and 8 of a batch each block the lead's tool call for 30 s and then fail, unless a slot frees in time.

Recommendation: register the run as Queued and return its run id at once. Acquire the permit inside the spawned task with `child_token.run_until_cancelled(sem.acquire_owned())` (M60's form), and set Running when it arrives. The cap does not change. Land after Z4-01, which is what bounds a queued subagent by its run.

**Z4-03.**
- *Today:* `claim_next` flips a follow-up to `running` (`crates/openalpaca_storage/src/repository/followup/mod.rs:341-347`). The row is marked done only after a detached gateway turn returns (`apps/openalpacad/src/followup.rs:28-71`). Autostart at a run's end ignores shutdown (`lead_agent.rs:1073-1080`). Nothing ever settles a `running` row.
- *Apply:* skip autostart once the daemon's cancel token is set. This needs `AppState.cancel_token` threaded into the dispatcher, which holds only the run's token (`lead_agent.rs:531`).
- *Present (owner):* what happens at boot to a follow-up a crash left `running`. Either settle it to `cancelled` with a visible notice, which is ZCode's at-most-once choice (ZCode:apps/zcode-cli/packages/core/src/runtime/methods/background-notifications.ts:58-82), or re-queue it, as `recover_unprocessed_steering` does for steering (`followup/mod.rs:258-270`).

**Z4-04.** `summary()` lists every subagent the run ever spawned, on every call, and sizes each answer by `answer_budget(inline_bytes, map.len())` (`crates/openalpaca_core/src/runner/lead_agent/tracker.rs:47-52`, `:191-233`). A second wait therefore re-delivers batch 1, and each answer's budget shrinks to about 38 % of what it was: roughly 1,216 bytes for 16 entries against 3,200 for 8. Recommendation:
- keep a "reported" flag per entry;
- `wait_for_subagents` returns head and tail only for entries not yet reported, shares the byte budget among those, and adds one line naming the run ids already reported, so that a compacted lead knows to ask `check_subagent_status`;
- `check_subagent_status` keeps whole re-reads.

This refines Stage 0's F2 sizing, and the tool's description changes in the same commit.

**Z4-05.** Resume replays the lead's own rounds (`crates/openalpaca_core/src/session_log/replay.rs:152-158`). The tracker is built fresh, so `check_subagent_status` answers "Unknown subagent_run_id" (`tools.rs:1155`) and `wait_for_subagents` answers "No subagents have been spawned yet." (`tracker.rs:193-195`). Span rows keep only 200 characters of a subagent's answer (`crates/openalpaca_core/src/runner/span.rs:22`, `:100-102`). Recommendation:
- spill each subagent's whole final answer somewhere durable, for example under `sessions/<id>/results/`;
- on resume, rebuild the tracker from `SubagentOpen` and `SubagentClose`: a closed span becomes Completed or Failed with its kept answer, and an open span becomes Failed, "interrupted by the daemon's stop";
- the resume note names those subagents.

**Z4-06.** Today the lead learns of results only by waiting for all of them (or the cap, or a steering push) or by polling (`tools.rs:1116-1160`, `:1204-1285`). Two options:
- **Fix, offered first:** an additive `wait_for_subagents {until: "any"}`.
- **Owner:** inject a `<system_note>` at the round boundary through the steering inbox (`SteeringOrigin::Daemon`, `crates/openalpaca_core/src/runner/steering.rs:48-54`), as ZCode's active-loop drain does (ZCode:apps/zcode-cli/packages/core/src/runtime/methods/runtime-command-active-loop.ts:22-90). This widens the steering rail's job and changes the lead contract in `docs/agent-loop.md`.

**Z4-07.** No lead tool stops or messages one subagent. The lead's coordination tools are spawn, batch spawn, check, wait, `post_update` and `queue_followup`; `steer_workflow` is not among them, despite the auditor's list (`tools.rs:1579-1583`). Each child token is moved into its task and kept nowhere (`:651`). Owner option: two lead-only tools, `cancel_subagent(run_id)` and `message_subagent(run_id, text)`. The tracker keeps each child token, which Z4-01 needs anyway, and optionally a per-subagent inbox drained at the subagent's round boundary (ZCode:apps/zcode-cli/docs/design/v2/tool/07-subagent.md:401, :433-450).

**Z4-08.** OpenAlpaca has no ask-the-lead tool anywhere under `crates/openalpaca_core/src/tools`. ZCode's `escalate` blocks the subagent until the main agent answers. It allows at most 3 escalations per ask, and a refusal is a normal result, not an error (ZCode:apps/zcode-cli/packages/contracts/src/tools/escalate.ts:36-56; ZCode:apps/zcode-cli/packages/bootstrap/src/app/workflow-driver-helpers.ts:84). Later design, after Z4-01: `ask_lead` and `answer_subagent`, bounded per subagent, cancelled with it, refusals as plain results.

**Z4-09.** See T13 in §5.
- The lead's cost accumulator is created fresh on each run (`crates/openalpaca_core/src/runner/agentic_loop/mod.rs:449`), and the lead passes `None` (`crates/openalpaca_core/src/runner/lead_agent/mod.rs:737`). A resumed run therefore restarts its cap at zero, and orphaned subagents (Z4-01) escape it entirely.
- A shared accumulator already exists for nested skills (`crates/openalpaca_core/src/runner/agentic_loop/cost.rs:4-35`).
- If the owner wants an enforced whole-workflow bound, it needs a workflow-scoped accumulator checked at both the lead's and the subagents' round boundaries. Each loop's own cap stays unchanged and is never summed into the other. The default is re-derived from fan-out width, since 8 × $1 already exceeds the shipped $3, and the figure is carried across resume.
- Do not adopt ZCode's no-budget stance (N4).

**Z4-10.** Each prompt's clock starts when its own wait begins (`crates/openalpaca_core/src/security/confirmation.rs:107-113`). A fan-out raises one card per subagent (R43), and every clock runs at once. So a person answering cards one by one sees the later ones time out. ZCode starts only the head-of-queue question's clock (ZCode:docs/ask-user-question-auto-resolution.md:64). Owner option: start a prompt's clock when it reaches the head of its run's or its lane's queue. A timeout stays a denial: do not carry over ZCode's empty-answer auto-continue, which ZCode itself never applies to permissions.

**Z4-11.** See T18 in §5. A follow-up the lead queues is pinned to the lane's active session at queue time (`crates/openalpaca_storage/src/repository/followup/mod.rs:111-118`), while `recover_unprocessed_steering` pins to the run's own session (`:258-268`). ZCode records the launching session at launch (ZCode:apps/zcode-cli/packages/core/src/runtime/methods/subagent.ts:285).

**Z4-12.**
- *Today:* a plugin agent's `executor.spawn` and `executor.step` RPCs are awaited without racing the token (`crates/openalpaca_core/src/runner/plugin_agent.rs:143-145`, `:198`). They are bounded by the plugin channel's 60 s default timeout (`crates/openalpaca_plugins/src/process_pool.rs:85-90`), so the gap is up to 60 s of cancel latency per RPC with the permit held, not for ever.
- *Apply beside M60:* race both RPCs with `run_until_cancelled`.
- *Later design for the owner:* replacing `wait_for_subagents`'s 600 s wall clock with an inactivity watchdog is model-visible (ZCode:apps/zcode-cli/packages/core/src/subagent/runner.ts:211-220).

**Z4-13.** The working-state writer renders `[role]: text` from role and content only (`crates/openalpaca_core/src/orchestrator/working_state.rs:156-176`, `:472-476`). The `source` and `task_id` columns exist (`crates/openalpaca_storage/src/migrations/001_baseline.sql:231`, `:234`) and are not read. Owner decision: label whole daemon-authored rows (no-answer lines, completion reports) as `[daemon]`, and re-measure on report 12's split. Daemon lines appended inside an assistant row, such as RunClaimGuard's N3 line, are not covered. Reopening `next`, which the owner accepted as measured on 2026-09-30, is the owner's call (ZCode:docs/subagent-respond-to-coordinator.md:24-48).

**Z4-14.** A failed subagent reads `Agent '<id>' failed: <LoopFinishReason Debug>.` (`tools.rs:952-959`): a CostExceeded with no amount, or a debug-escaped `Error("…")`. Meanwhile the prompt tells the lead to retry failures (`prompt.rs:135-136`). Recommendation: render the failure with a lead-facing reason function that shares `LoopResult::no_answer_line`'s wording (`crates/openalpaca_core/src/runner/agentic_loop/config.rs:366`). Say when a retry cannot help: the cost cap, an auth failure, no routable model (ZCode:apps/zcode-cli/docs/design/v2/tool/07-subagent.md:215-216).

**Z4-15.** The extension sweep is bounded by what remains of the shutdown window (`apps/openalpacad/src/main.rs:891-895`). The connectors and the wake manager are not (`:899-904`; `apps/openalpacad/src/managers/connector.rs:254-259`), and they run before `remove_discovery` (`main.rs:908`). The force-exit watchdog calls `process::exit(1)` (`:805-813`). Recommendation: bound both by what remains of `FORCE_EXIT_GRACE`, and also remove `discovery.json` in the watchdog's exit path (ZCode:docs/cli-bounded-shutdown.md:30-39, :56-70).

**Z4-16.** Later design, owner: an optional `result_schema` on `spawn_subagent`, validated with M82's `jsonschema`, allowing a bounded repair and then failing (ZCode:apps/zcode-cli/packages/dynamic-workflow/docs/execution-engine.md:226-260). The schema must pass `strict_mode_compatible` before it is sent strict. A script engine would be a whole new component and is not recommended. This informs the batch-work question (§5).

**Z4-17.** No cost budget, inherited bypass modes, YOLO fallback, no delegation by subagents, and a confirmation window on every workflow start. Each is listed in §6 with the rule that excludes it.

### Z5 — The MCP client

**ZCode's design in brief.** ZCode's MCP client sits on @modelcontextprotocol/client 2.0, in `NodeMcpAdapter` (ZCode:apps/zcode-cli/packages/adapters/src/mcp/index.ts).

*Connecting.* A connect builds a client with deliberately declared capabilities: elicitation in form mode only, and only when a host port exists (:1537-1546). It runs under the server's timeout and classifies a failure by the phase it happened in (:1619-1652). Dual-era negotiation (ZCode:docs/mcp-dual-era-version-negotiation.md:21-116; code index.ts:2316-2370):
- a per-server mode: `auto` by default, `legacy`, or a strict `2026-07-28` pin;
- a probe bounded by 5 s and by half the connect budget;
- a typed `protocol_negotiation_failed` that tells the user to switch to legacy.

*Calling* (:770-850, :1118-1250):
- one deadline per call;
- a disconnected record is reconnected once before the call;
- the call is retried once, only on a definite "Not connected";
- progress resets the timeout.

*Results and lists.* Images over 200 KiB of base64 become artifacts, and third-party images are never decoded (ZCode:docs/mcp-tool-output-budget.md:8-42). Tool names are sanitised (ZCode:apps/zcode-cli/packages/core/src/mcp/name.ts:14-18). A list change marks the server stale, and the list is fetched again at the next turn boundary (index.ts:503-531, :733-741).

*Security and diagnostics.* stderr is kept as a redacted 4,000-character tail attached only to failures (index.ts:110). Credentialed fetches refuse every redirect (ZCode:apps/zcode-cli/packages/adapters/src/mcp/official-auth.ts:84-93, :222-228, :325-331). Every MCP tool needs approval (ZCode:apps/zcode-cli/packages/core/src/mcp/index.ts:148).

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z5-01 | A per-server lifecycle mode and a named negotiation failure: P19's `Auto` has no escape hatch | high | yes | design | partly | P19, DU-25 (p4), App. B #4/#42 |
| Z5-02 | Never re-send a tool call that may have reached the server; one deadline per call | high | no | idea | confirmed | N-MCP-1, before the cut |
| Z5-03 | The MCP HTTP client's hard-coded 30 s total timeout cuts every streamed body | high | no | idea | confirmed | N-MCP-2 with D6, before the cut |
| Z5-04 | Every call to a server is serialised by a mutex held across the RPC | medium | no | idea | partly | N-MCP-3, with or after N-MCP-1 |
| Z5-05 | Map remote MCP tool names into a provider-safe alphabet | medium | no | idea | partly | N-MCP-4 |
| Z5-06 | M15 against the sandbox clock | high | split ((c) is the owner's) | design | confirmed | M15 |
| Z5-07 | Elicitation for M84: form-only, per-call attribution, typed fields, clock suspension | medium | yes | design | confirmed | M84; (c)-3 |
| Z5-08 | D6: a refused redirect is a typed, non-retriable failure | medium | yes (refusing every redirect) | idea | confirmed | N-MCP-2 / D6 |
| Z5-09 | Capture stdio stderr as a bounded, redacted tail attached to the failure | medium | no | design | confirmed | N-MCP-5, after P19 |
| Z5-10 | Parse mcp.toml one server at a time | medium | yes (amends ext design §2.1/§5.1) | idea | partly | owner option |
| Z5-11 | MCP stdio children inherit every credential (= Z1-06) | medium | yes | idea | partly | see Z1-06 |
| Z5-12 | Keep MCP images, audio and blobs as by-product artifacts (= Z1-17) | medium | yes | design | confirmed | commit after M14 |
| Z5-13 | Show the negotiated version and timings on the row; send our own identity | low | yes | idea | confirmed | P19 (`get_info`) plus a row field |
| Z5-14 | Descendant cleanup: test a grandchild that leaves the group | low | no | idea | confirmed | P19 gate |
| Z5-15 | A liveness check only when the user asks | low | yes | idea | confirmed | later, ext design §3.6 |
| Z5-16 | Boot waits for MCP connects; ZCode's catalog cache is a plan | low | yes | idea | confirmed | note |
| Z5-17 | Keep app-only (MCP Apps) tools off the model's list | low | no | idea | confirmed | note |
| Z5-18 | Per-session process isolation and a workspace working directory | low | yes | idea | confirmed | note (§6) |

**Z5-01.**
- *Today:*
  - P19 hard-codes `ClientLifecycleMode::Auto` (`plan-rev3.md:791`; prep `refs/prep/deps-wave-2026-10-12:crates/openalpaca_mcp/src/client.rs:701-706`).
  - rmcp's 10 s discover timeout is a private constant, passed by every public entry point (rmcp-3.5.1 `src/service/client.rs:657`, `:755`), so ZCode's "5 s or half the budget" rule cannot be applied.
  - A server that closes the probe or answers it with anything but a correlated JSON-RPC error fails with no fallback (`:1044-1057`).
  - The probe runs on every connect, including mid-call respawns (`crates/openalpaca_mcp/src/client.rs:438-491`) and boot, which waits for MCP before serving (`apps/openalpacad/src/main.rs:381`, `:785`).
  - Every bring-up failure other than 401/403 reads `Unreachable`, "not actionable — retry" (`crates/openalpaca_core/src/tools/mcp/classify.rs:28-36`).
- *Recommendation (owner; a new `mcp.toml` key and a new `FailureReason` word in the extension design's total enum, §4.2):*
  1. Add `protocol = "auto" | "legacy" | "2026-07-28"` to `[servers.<name>]`, mapped to rmcp's Auto, Initialize and Discover. The pin never downgrades. Use `skip_serializing_if` so every existing `config_fingerprint` stays byte-identical.
  2. Classify `NoCompatibleProtocolVersion`, `ExpectedInitResult`, `LegacyFallbackFailed` and a transport close during the probe as one distinct failure whose text names `protocol = "legacy"`.
  3. Pin an in-session respawn to the era this load negotiated. This is OpenAlpaca's own choice: ZCode renegotiates on every reconnect (doc :98).
  4. **Warn**, not refuse, on `connect_timeout_secs <= 10` combined with `auto`.
  5. File an rmcp issue for a configurable auto-discover timeout, the named way out of the fixed 10 s.

**Z5-02.**
- *Today:* `call_tool` loops. On any `is_retriable()` error (TransportClosed, Transport, Timeout, ServerInternal; `crates/openalpaca_mcp/src/error.rs:83-91`) it reconnects and sends again with a fresh `request_timeout` (`client.rs:552-584`). `reconnect` kills the stdio child (`:483-484`). A good handshake resets the attempt counter (`:325`), so a server that handshakes but fails calls loops until the sandbox cut: 60 s for agents, 300 s for the lead.
- *Consequence:* a non-idempotent tool runs twice after a timeout. This is plan §3.3's unbounded reconnect ladder (`plan-rev3.md:449`).
- *Recommendation:*
  - Before sending: if the service is absent or `running.is_transport_closed()` (rmcp-3.5.1 `src/service.rs:1053`), reconnect once and send. This keeps the extension design §3.6 item 1's transparent respawn of a dead child, and T6(a). ZCode does the same (ZCode:apps/zcode-cli/packages/adapters/src/mcp/index.ts:916-928).
  - After the request may have left, never send it again. Return Timeout, ServerInternal, Transport and a mid-call TransportClosed to the model, and let the next call respawn.
  - One deadline spans the pre-send reconnect and the call.
  - Count consecutive failed handshakes toward `ReconnectExhausted`, without resetting on a handshake alone.
  - Update the text of extension design §3.6 item 1.
  - Leave out "re-send when `idempotent_hint` is true", or present it with T35: it trusts a server's own annotation.

**Z5-03.** `StreamableHttpTransport` builds its reqwest client with `.timeout(self.request_timeout)` (`crates/openalpaca_mcp/src/transport/http.rs:96-98`), defaulting to 30 s (`:30`). `build_transport` never calls `with_timeout` (`client.rs:619-656`), so a configured `request_timeout_secs` never reaches it. reqwest's `timeout` is a total deadline that runs until the response body ends (reqwest-0.13.5 `src/async_impl/client.rs:1444-1450`).

Every streamed HTTP body is therefore cut at 30 s: SSE tool answers, the legacy GET stream, P19b's `subscriptions/listen`, and M15's progress window. A cut listen stream is resumed only where the server supports Last-Event-ID (rmcp-3.5.1 `src/transport/streamable_http_client.rs:787-806`). This is silent degradation of a configured value.

Recommendation: build the MCP client with `connect_timeout` and an idle `read_timeout` only, as the LLM client already does, and leave per-request deadlines to `McpClient` and M15. Test that an SSE stub streaming for 45 s completes and that a listen stream outlives 30 s. Land it in D6's commit, since both edit `http.rs`.

**Z5-04.** `call_tool`, `list_tools`, `list_tools_once` and `ping` hold the `service` mutex across the whole RPC (`client.rs:367`, `:387`, `:503`, `:556`). The lock is taken inside the timeout wrapper (`:553-560`).
- *Correction to the audit:* a queued call's timeout does **not** close the service under the in-flight call, because `reconnect` takes the same lock (`:483`).
- *The actual harm:* a queued call spends its 30 s waiting for the lock and times out unsent. Under Z5-02's old loop it then kills a healthy server. Parallel subagents gain nothing.
- *Recommendation:* clone the `Peer` under the guard and drop the guard before awaiting (`Peer` has `call_tool` and `list_all_tools`, rmcp-3.5.1 `src/service/client.rs:1760`, `:1966`). Take the lock outside the timeout wrapper, or not at all, for calls.
- *Before landing:* confirm T3's registry-level drain bounds calls running on a cloned peer, and amend the extension design's §3.2 T4 and disable-latency text (`tasks/extension-enable-design.md:2195-2197`). Test that two concurrent calls to a stub holding the first for 2 s both finish in about 2 s. Land with or after Z5-02.

**Z5-05.** The remote half of a tool name is used raw in `format!("{server_name}__{remote_name}")` (`crates/openalpaca_core/src/tools/mcp/bridge.rs:33-34`). The registry refuses only empty names, names over 256 bytes and names containing NUL (`tools/registry/mod.rs:548-566`), and both providers send the name verbatim. The providers' name rules are external and were not checked here. Recommendation:
- after checking each provider's documented rule, map the remote half to `[A-Za-z0-9_-]` and cap the combined length;
- keep `remote_name` raw for the call;
- detect collisions within one server's mapped list **before** registration: keep the first and warn naming both originals.

The case-13 skip does not apply here, because it ignores incumbents of the same extension (`apps/openalpacad/src/managers/mcp.rs:1245`), so the second tool would silently overwrite the first. Test `read.file`, and `read.file` beside `read_file`.

**Z5-06.**
- *Today:* two clocks run independently. `McpClient` has its 30 s `request_timeout`. The sandbox has `max_tool_runtime_secs`, 60 s for agents and 300 s for the lead, and enforces it by dropping the future (`crates/openalpaca_core/src/security/sandbox/mod.rs:543-547`); extension tools cannot be exempt (`tools/registry/mod.rs:574-581`). The registry passes `cancel_token: None` (`:852`). rmcp's `RequestHandle` has no Drop impl (rmcp-3.5.1 `src/service.rs:542-549`), so a dropped call sends no `notifications/cancelled`.
- *Consequence:* M15's planned `max_total_timeout_secs = 300` (`modernization-plan.md:741`) is capped at 60 s for agents, and neither a sandbox cut nor a run's `/cancel` tells the server anything.
- *Recommendation, inside M15:*
  - (a) The registry hands the call a CancellationToken that the sandbox fires at its deadline and on run cancel, so `cancel(reason)` always goes out.
  - (b) Clamp `max_total_timeout` below the effective `max_tool_runtime_secs`, or document that the smaller one wins.
  - (c) Owner: whether progress may extend the sandbox clock, as ZCode's single clock does (ZCode:apps/zcode-cli/packages/core/src/mcp/index.ts:149, :226-229). That changes the sandbox contract.

**Z5-07.** This feeds the M84 amendment (§4.3). rmcp declines elicitations by default (rmcp-3.5.1 `src/handler/client.rs:179-190`). Its capability offers `form`, with `schema_validation`, and `url` (`src/model/capabilities.rs:80-133`). M84's Approve/Deny card (`modernization-plan.md:878-881`) cannot produce `ElicitResult.content` for a schema with fields. An elicitation arriving mid-call would wait inside the 30 s and 60 s clocks. rmcp's `RequestContext` carries no related-request id (`src/service.rs:1208-1216`).

**Z5-08.** The default redirect policy applies, and every auth and extra header is a default header (`crates/openalpaca_mcp/src/transport/http.rs:57-89`, `:96-104`). This is recorded as a leak (review SF-6 and D6). An rmcp HTTP error maps to the retriable `TransportClosed` unless it is a 401 or 403 (`error.rs:154-174`).

Recommendation, in D6's commit:
- classify a refused redirect as its own non-retriable error carrying the target origin. Bring-up then reads `ConfigInvalid`, "server redirected to <origin>; set url to the canonical endpoint", and a call never enters the reconnect ladder;
- **owner:** whether to refuse **every** redirect when auth or `extra_headers` are configured, as ZCode's credentialed fetch does, following same-origin redirects only for unauthenticated servers;
- test with a loopback redirect to a second port.

**Z5-09.** `TokioChildProcess::new(cmd)` inherits stderr (`crates/openalpaca_mcp/src/transport/stdio.rs:79`; rmcp-3.5.1 `src/transport/child_process.rs:124-130`). A server's stderr therefore goes raw into `daemon.log`, unattributed, unbounded and unredacted, and a Failed row's detail is the rmcp error alone (`apps/openalpacad/src/managers/mcp.rs:1132-1136`). Recommendation, after P19:
- spawn with `TokioChildProcess::builder(cmd).stderr(Stdio::piped())` and a reader task that always drains the pipe, keeping the last 4 KB per connection;
- redact by the exact secret values this load resolved, plus generic patterns, in OpenAlpaca's own code, not a port of ZCode's regex table;
- attach the tail to the Failed detail and to the crash warnings, wrapped with `wrap_untrusted_context` where it enters a tool result or a status rendering, and escaped with `escape_hidden` where a person sees it. That is the accepted extension design's existing rule (`tasks/extension-enable-design.md:1855-1859`), designed and never built.

**Z5-10.** `McpConfig::parse` deserialises the whole file and fails on the first bad server (`crates/openalpaca_core/src/tools/mcp/config.rs:195-203`). An unparseable file boots with one pseudo-record and no servers (`apps/openalpacad/src/services/mcp.rs:14-17`). Unknown keys are silently ignored (`config.rs:53-55`). That fail-closed shape is the accepted extension design's (`tasks/extension-enable-design.md:142-148`, `:1603-1610`, `:1847`), so this is an owner option amending its §2.1 and §5.1, not a fix:
- parse the servers block by block;
- an invalid block becomes `Failed{ConfigInvalid}` naming the field, and the other servers load;
- define the watcher's behaviour for a newly invalid block, by default keeping the running instance;
- keep the atomic writer's re-parse strict.

Warning on unknown keys per block can land on its own, because it adds no refusal.

**Z5-11.** Merged into Z1-06.

**Z5-12 (= Z1-17).** MCP images become a placeholder, and audio and resources a bracketed kind name (`crates/openalpaca_core/src/tools/mcp/bridge.rs:84-104`). M14 does not cover binary content (`modernization-plan.md:734-737`). Owner (it widens the MCP bridge to writing by-products, and by-products never ask under ST-15):
- in the registry's MCP arm, which has the ToolContext (`tools/registry/mod.rs:838-852`), save image, audio and embedded-blob content as a by-product through `put_tool_artifact`, within `max_artifact_bytes`;
- base64-decode to bytes for storage, but never image-decode, transcode or compress third-party content;
- put the artifact id and path in the text, so the model can `artifact_read` or `send` it;
- with no ToolContext, tell the model nothing was saved.

ZCode: ZCode:docs/mcp-tool-output-budget.md:8-42.

**Z5-13.** The negotiated version is stored (`crates/openalpaca_mcp/src/client.rs:262-268`) but appears nowhere under `apps/openalpacad/src`. `client_info` is built (`apps/openalpacad/src/managers/mcp.rs:2315-2319`) but never sent, because there is no `get_info` override, so rmcp's default identity goes out. Under 2026-07-28 that identity travels in every request's metadata (rmcp-3.5.1 `src/service/client.rs:971-974`). Recommendation:
- add `protocol_version` and the connect and list durations to the extension row and to `openalpaca ext info`, regenerating `docs/api`;
- in P19, override `get_info` with our `client_info` and an explicit capability set, empty until M84. These are the two lines plan §3.3 already offers the owner.

Appendix B's #42 is then read from the row: a connect of about 10 s means a server that ignored the probe.

**Z5-14.** Keep P19's spawn-time process group and JobObject (prep `stdio.rs:118-130`), which are stronger than ZCode's attach-after-spawn. Add to P19's forking fixture a grandchild that calls `setsid`, and warn, naming the pids, if any pid of the recorded tree outlives teardown. Add a parent-pid walk only if the fixture shows an escape (ZCode:apps/zcode-cli/packages/adapters/src/mcp/process-tree.ts:128-140, :196-216).

**Z5-15 to Z5-18.**
- *Z5-15:* `McpClient::ping` has no caller (`client.rs:386-394`). As an owner option: `openalpaca ext info --check`, one ping with a 5 s bound, where a JSON-RPC error counts as alive. No poller, so §3.6 stands.
- *Z5-16:* boot serves only after MCP reconciles (`apps/openalpacad/src/services/mcp.rs:11-13`, "never a pending record"). Z5-01's key is the cheaper fix. ZCode's catalog cache is a plan with no code (ZCode:docs/mcp-async-tool-catalog-cache.md:3-8).
- *Z5-17:* if a server in use ships app-only tools, filter them at registration with one check on `_meta` (ZCode:apps/zcode-cli/packages/adapters/src/mcp/descriptor.ts:33-34).
- *Z5-18:* ZCode gives each session its own server process (ZCode:apps/zcode-cli/packages/adapters/src/mcp/pool.ts:514, :536-540). OpenAlpaca's one process per server, with the daemon's directory as MCP servers' working directory, is settled by the storage design's Stage 2. Owner option only, if a stateful server needs it.

### Z6 — Desktop shell, app architecture and the GUI

**ZCode's design in brief.** ZCode's desktop is a fat Electron 41 shell (ZCode:packages/desktop/package.json:110).
- *Process model:* per window, main forks a "Local Host" utility process and talks to it over a MessagePort (ZCode:packages/desktop/src/main/desktopHostProcess.ts:755-756). The Host spawns the agent CLI. Agents live and die with the app, and quitting while sessions run asks first, with Cancel as the default (ZCode:packages/desktop/src/main/index.ts:1445-1487).
- *Environment:* startup captures the login shell's environment asynchronously, with markers, a hard deadline, a process-group kill and an allowlist of keys (ZCode:packages/services/src/runtime-tools/runtimeLoginShellEnvCapture.ts:6-11, :54-101, :173-216).
- *Measurement:* launch marks T0–T5 are carried from main into the renderer (ZCode:packages/desktop/src/main/desktopLaunchMarks.ts:1-17).
- *Database:* a startup state machine owned by the Host (ZCode:packages/desktop/src/host/databaseStartupCoordinator.ts:40, :89-143).
- *Crashes:* process-gone and unresponsive handlers log the event and keep minidumps (ZCode:packages/desktop/src/main/desktopCrashCapture.ts:421-450).
- *Links:* external opens go through an IPC that allows http, https and file (ZCode:packages/desktop/src/main/desktopMainIpcRemote.ts:41-47, :282-322).
- *No CSP:* the main renderer has no Content-Security-Policy.

OpenAlpaca is the opposite architecture by settled decision: a thin Tauri 2 shell with five commands (`apps/openalpaca-gui/src-tauri/src/lib.rs:416-427`), a strict CSP (`src-tauri/tauri.conf.json:24`), and a daemon that outlives the window (`lib.rs:354-410`). What transfers is startup, environment, link handling, crash recovery, measurement, guards and packaging practice, not the process model.

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z6-01 | A booting daemon reads as alive today and would read as dead under Q04 | high | split ((i) into C-09; (ii) owner) | design | partly | Q04, review C-09; DU-29 |
| Z6-02 | A daemon started by the app has a different PATH from one started in a terminal | high | yes | design | confirmed | shell commit beside Q12/Q14; B3 now |
| Z6-03 | A path link in an agent-written preview replaces the app's page; M77 as written admits it | high | no | idea | confirmed | M77 and its G-bundle gate |
| Z6-04 | Startup marks written to a file, so Q00's owed GUI rows need no person at a screen | medium | no | idea | confirmed | Q14; §7.5; D13 |
| Z6-05 | A WebKit content crash leaves a blank window with no log line | medium | no | idea | confirmed | Q14 (or M76) |
| Z6-06 | After an app update, the old daemon keeps running unnoticed | low | no (the installer prompt is the owner's) | idea | partly | Q04 row |
| Z6-07 | G1's boundary as a standing guard, not a one-time gate | medium | no | idea | confirmed | Q06 acceptance → Q08 |
| Z6-08 | First run imports configuration, never credentials; network features ship off (T24; = Z8-07) | medium | yes | design | partly | T24 |
| Z6-09 | DevServerHost as a same-origin proxy instead of a token endpoint | low | yes | design | partly | Q07 / SF-21 |
| Z6-10 | The webview's own storage sits outside `~/.openalpaca` | low | yes | design | confirmed | GUI commit after Q07; D1 record |
| Z6-11 | Signing and notarisation are neither recorded as a beta limit nor planned | low | yes | idea | partly | pending §3.13 / B3 |
| Z6-12 | macOS close hides the window instead of quitting (option) | low | yes | idea | confirmed | beside (c)-5 |
| Z6-13 | Remember the window size the person chose | low | yes | idea | confirmed | M78; later |
| Z6-14 | A bundle-size ratchet once Q10 has set the budgets | low | no | idea | confirmed | after Q10 |
| Z6-15 | Note: ZCode's shell choices OpenAlpaca should not copy | low | no | idea | confirmed | §6 |

**Z6-01.**
- *Today:* the daemon takes the singleton lock (`apps/openalpacad/src/main.rs:118`), binds (`:210`) and writes `discovery.json` (`:222`). Only then does it open the database (`:228`; a refusal there exits with discovery already written), reconcile every MCP server (`:381` → `services/mcp.rs:59`, with `connect_timeout_secs = 30`, `config/mcp.toml:10`), start plugins (`:439`) and connectors (`:649`), and serve (`:785`).
- *Consequence:* the shell's 300 ms TCP test (`apps/openalpaca-gui/src-tauri/src/lib.rs:257-268`) reports a daemon in that window as alive, and the webview's first requests hang. Under Q04 the health probe fails in that window, so `decide_reuse` spawns a second daemon. It meets the lock, and the wait reports a failure while the real daemon is still booting.
- This is the review's C-09 (`tasks/deps-upgrade/reports/2026-10-17-plan-review.md:877`). Z6-01 adds the root cause, a log rotation C-09 already names (`lib.rs:322-326`), and part (ii).
- *Recommendation:*
  - **(i) Apply, merged into C-09's Q04 row:** when `discovery.json`'s pid is a live `openalpacad` (`running_daemon_pid` / `pid_is_daemon`) and health has not answered, the shell parks and retries within a bounded budget and shows "daemon is starting". It never spawns and never rotates `daemon.log`, and on timeout it reports with the log tail. Do **not** probe the singleton lock: `singleton_lock_is_free()` acquires it (`crates/openalpaca_storage/src/daemon_lifecycle.rs:135-136`) and can make a booting daemon exit. Gate it natively against a deliberately slow MCP server.
  - **(ii) Owner; additive wire format under Q01's byte pin:** write `discovery.json` just before `axum::serve`, or add a phase field to it, or add an early health route. C-09's row is the no-change answer.
  - The auditor's claim that "every step of the reconnect ladder can spawn" is wrong. The ladder spawns only through bootstrap or an `instance_id` change (`apps/openalpaca-gui/src/lib/events.ts:439-462`; `lib/connection.ts:319-331`).
- The database-gate idea is ZCode's (ZCode:docs/desktop/database-migration-startup-gate-plan.md:35-39; budget figures doc-level).

**Z6-02.**
- *Today:* the shell spawns the sidecar with its own inherited environment plus two variables (`lib.rs:344-350`). The daemon resolves every external program through PATH:
  - MCP stdio servers (`crates/openalpaca_mcp/src/transport/stdio.rs:64`);
  - the CLI backends `which::which("claude")` and `"codex"` (`crates/openalpaca_llm/src/cli_backend/mod.rs:54`, `:154`);
  - `sh -c` (`crates/openalpaca_core/src/tools/platform.rs:20`);
  - skill interpreters (`tools/builtins/mod.rs:576`).
- *Consequence:* an app opened from Finder inherits launchd's minimal environment (reasoned, not measured). So `npx`, `uvx`, Homebrew's `node` and `claude` are missing for the app-spawned daemon but present for `openalpaca daemon start`. No manual and no pending row records this.
- *Recommendation (owner; it runs the user's profile at boot):*
  - (a) the shell captures the environment once, asynchronously, under ZCode's rules (markers, a deadline, a process-group kill, an allowlist of keys, not values) and patches `spawn_daemon`;
  - (b) the daemon captures it when its PATH is launchd's default;
  - (c) a fixed bootstrap list (`/opt/homebrew/bin`, `/usr/local/bin`, `~/.cargo/bin`), which is ZCode's own darwin list (runtimeLoginShellEnvCapture.ts:8-10) and also the fallback under (a);
  - (d) document only.
  - Recommend (a) with (c) as its fallback.
  - Decide whether credential-pointer keys (`AWS_PROFILE`, `GOOGLE_APPLICATION_CREDENTIALS`, `KUBECONFIG`), which ZCode inherits, are allowed.
  - Announce where PATH came from: one WARN in `daemon.log` and a line in Settings → Connection, as ZCode logs it (ZCode:docs/desktop/startup-critical-path.md:96).
  - API keys are not inherited, so `secret_env` stays a terminal-launch feature, and the docs say so.
- **Now:** add the limitation to B3's list (`tasks/pending-decisions.md:179`) with the workaround, an absolute `command` or `env.PATH` in `mcp.toml` (`stdio.rs:65-67`).

**Z6-03.**
- *Today:* DocumentPreview renders agent markdown through marked and DOMPurify into the window's only document (`apps/openalpaca-gui/src/components/work/preview/DocumentPreview.tsx:21`, `:139`). DOMPurify keeps path-only hrefs. M77 admits "only the app's own origin" (`modernization-plan.md:853-856`). wry calls the policy for same-origin navigations too, and Tauri answers an unknown asset path with `index.html` (tauri-2.12.3 `src/manager/mod.rs:392-415`).
- *Consequence:* a click on `[notes](/Users/me/proj/notes.md)` navigates the app away. The expected result is a blank window or a cold re-bootstrap (not run). ZCode tells its agents to write exactly such links (ZCode:docs/desktop/zcode-desktop-context.md:38-40).
- *Recommendation (apply, inside M77; it completes M77's own stated intent):*
  - `on_navigation` admits only the exact entry document, and the devUrl root in dev. It refuses every other same-origin path and every scheme other than http(s), and never hands `file:` to the OS. ZCode's `file:` branch opens any file with no check on its kind (ZCode:packages/desktop/src/main/desktopMainIpcHelpers.ts:28-58), and ST-18's Open-by-id stays the only way to open a local file.
  - DocumentPreview intercepts anchor clicks: http(s) goes to M77's browser path; a local path gets no navigation, with Open or Reveal optionally offered through the existing daemon routes only; a fragment stays in the page.
  - G-bundle clicks an absolute path, a relative path, a fragment, `mailto:` and an http link, and checks the app is still on its page afterwards.

**Z6-04.** §7.5 scenario (1) asks for first paint and first useful content (`plan-rev3.md:1058`) and names no instrument. The review says it needs an instrumented build or a person (`2026-10-17-plan-review.md:739`). Recommendation:
- the shell records T0 and the `on_page_load` marks into Q14's `gui.log`;
- the webview's marks (bundle start, first commit, first paint, first useful content) reach the log **without** widening M74's five-command manifest (`modernization-plan.md:838-839`): through a debug or instrumented build, or the console. If a command is used instead, amend M74's manifest and capability in the same commit;
- add to §7.5's rules (`plan-rev3.md:1060`): alternate baseline and candidate trial by trial, on copies of one fixture (ZCode:docs/desktop/startup-critical-path.md:183-199).

D13's owed rows can then be taken by instrument.

**Z6-05.** The shell registers no handler for the web content process ending and no panic hook (`lib.rs:416-427`). Tauri 2.12 offers `Builder::on_web_content_process_terminate` on macOS (tauri-2.12.3 `src/app.rs:1903-1917`). In Q14 or M76: log to `gui.log`, then `reload()`. Reloading is safe because the state lives in the daemon, an advantage of the thin shell. Also install a `std::panic::set_hook` that writes to `gui.log`. G-bundle: kill the WebContent pid; the window reloads and reconnects to the same `instance_id`.

**Z6-06.** The shipped `install.sh` already stops the running daemon before replacing the app (`scripts/release/install.sh:159-184`, called at `:309`). The gap is therefore narrow: a hand-replaced `.app`, a daemon that ignored SIGTERM for 10 s, or a daemon started between install and first launch.
- *Apply:* a Q04 row, "same protocol, different `build.version`": reuse the daemon, with a note naming both versions and Stop/Start. Never restart automatically.
- *Owner:* `install.sh` prints the daemon's `busy` counts and asks before stopping (ZCode:docs/desktop/auto-update-auto-download-install.md:14-16, doc-level).

**Z6-07.** G1's acceptance is checked once, in Q06's gate (`plan-rev3.md:966`). Q08's macOS leg runs check, clippy and tests only (`:1008-1010`). Make Q06's acceptance permanent: Q08's job and a local script assert that `cargo tree -p openalpaca_gui -e normal --locked --prefix none` contains nothing on G1's deny-list, and that `openalpaca_app_support` depends on no workspace crate (ZCode:architecture-policy.yaml:4-50; ZCode:package.json:38, :73-75).

**Z6-08 (= Z8-07).** See T24 in §5. One correction to the ZCode evidence: ZCode does read another product's secret, Chrome's Safe Storage key, but only during a cookie import the user starts, with a timeout, and a denial ends the import (ZCode:packages/desktop/src/main/chromeCredentialManager.ts:51-61). It never reads another agent's tokens and never polls. A first-run "Import from Claude Code / Codex" of MCP declarations and skills, landing unapproved or disabled under the two-axis rule, is a later design.

**Z6-09.** DU-28 (a)'s DevServerHost hands the daemon's token to the page (`plan-rev3.md:588-591`). SF-21's amendment stands. As an owner option: a serve-only reverse proxy that adds the token server-side, so its value never leaves the dev-server process (ZCode:packages/web/vite.config.ts:56-70).

The trade-off must be stated. Server-side injection makes the daemon *ambiently* authenticated at `127.0.0.1:1420`. WebSocket upgrades bypass CORS, so any web page could open the proxied events socket, and simple cross-origin requests would carry the token. The proxy therefore needs an exact Origin check on **every** proxied request, upgrades included, and must refuse a missing Origin from browsers, on top of SF-21's `server.cors: false`. Without those it is weaker than DU-28 (a).

**Z6-10.** The project selection and pane widths live in WKWebView's localStorage (`apps/openalpaca-gui/src/stores/project.ts:13`, `:100-108`; `stores/pane-widths.ts:82-93`). That storage sits under the bundle identifier in `~/Library`, outside `~/.openalpaca`, and neither `OPENALPACA_HOME_STORE` nor the factory reset touches it. Tauri's `data_store_identifier` needs macOS 14 (tauri-2.12.3 `src/webview/webview_window.rs:1194-1205`), and `data_directory` is unsupported on macOS. Recommendation:
- **owner:** record the exception in CLAUDE.md's D1 bullet and the manuals;
- **apply:** namespace the `oa-*` keys by the served store's install id or root from `GET /v1/status`, so a different store starts empty (ZCode:packages/desktop/src/main/desktopRuntimeEnv.ts:60-80).

**Z6-11 (with Z7-11).** `package-macos.sh` states the bundle is unsigned (`scripts/release/package-macos.sh:12-17`), and G-ident's equality check says it assumes so (`:178-186`). Neither `tasks/pending-decisions.md` nor the plan names signing or notarisation. Recommendation:
- **now:** move the limitation into pending-decisions §3.13 and B3's list;
- **owner:** a signing decision, since it needs an Apple Developer account. When it is taken, sign the staged sidecar once before it is copied to both the bundle and `libexec/`, record the signed hash as provenance, keep Tauri from re-signing it (or re-derive G-ident's equality after signing), and verify with codesign, spctl and stapler (ZCode:packages/desktop/electron-builder.config.js:818-856; ZCode:docs/desktop/macos-dmg-signing-notarization.md:7-21, :88-103).

**Z6-12 to Z6-15.**
- *Z6-12:* an owner option beside (c)-5: on macOS, close hides the window and Cmd+Q quits (ZCode:packages/desktop/src/main/desktopDarwinCloseBehavior.ts:7-31). Decide only after Q10's idle-memory numbers.
- *Z6-13:* land M78 as planned. Persist the window size only if wanted, never through Tauri's window-state plugin (it writes outside the store and widens M74/M75). Persist the normal bounds, never the maximised ones (ZCode:packages/desktop/src/main/desktopWindowSize.ts:4-80).
- *Z6-14:* after Q10 sets the budgets, `package-macos.sh` compares the `.app` and the daemon with them plus a margin, and lists the largest items on failure (ZCode:docs/desktop-bundle-size-architecture.md:120-142). A build guard, not a product enforcement point.
- *Z6-15:* no CSP on ZCode's main renderer, a preload bridge of about 1,000 lines, agents that die with the app. Not to be copied (§6).

### Z7 — Toolchain, dependencies, supply chain, CI and the test harness

**ZCode's design in brief.** Read at `aac47556`. ZCode's CI configuration is absent from the export, so every CI claim is doc-only.
- *Tool pinning:* `mise.toml` pins node 24.14.0 and pnpm 10.33.2 (ZCode:mise.toml:1-3). The pins drift elsewhere: `.nvmrc` floats at `24`, root and nested engines differ, and the lint tools differ between root and CLI. ZCode records that a floating CI Node broke its packaging (ZCode:third-party/README.md:9).
- *No release-age rule anywhere,* and no update bot or advisory scanning.
- *What ZCode does have:*
  - an install-script allowlist (ZCode:pnpm-workspace.yaml:20-32);
  - a licence gate tied to the lockfile's hashes (ZCode:scripts/licenses.mjs:5-127; ZCode:scripts/third-party-notices.mjs:15-30);
  - an architecture policy checked against a baseline, with reasoned and optionally expiring exceptions, and inline lint disables counted as violations (ZCode:architecture-policy.yaml:4-195; ZCode:scripts/architecture/index.mjs:20-315). The checker has its own fail-closed tests (ZCode:scripts/test/architecture-governance.test.mjs:142, :195, :268);
  - Vitest with a serial fork pool for process-spawning tests (ZCode:vitest.config.ts:4-44, :109-118);
  - per its docs, primary-platform CI (ZCode:docs/ci-build-platform-selection.md:28-36).

OpenAlpaca is already ahead on release age (`bunfig.toml`), exact JS pins, single-sourced toolchains read by CI, `--locked` builds, SHA-pinned actions and planned advisory gates. ZCode is ahead on licensing, install-script allowlisting, gates over its own tooling, reasoned exceptions, primary-platform CI and test isolation by class.

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z7-01 | Primary platform in the automatic CI set; deferred platforms as manual legs | high | yes | design | confirmed | beside Q08; DU-31/DU-33 |
| Z7-02 | Third-party licensing: a LICENSE file, notices in the bundle, a licence gate | high | yes | design | confirmed | P34 widened; notices commit near Q03/Q09 |
| Z7-03 | Every gating tool fails closed, has its own tests, and runs in CI | medium | yes | idea | partly | MF-2/MF-6 amendments; new CI job |
| Z7-04 | One machine-read record of waivers, each with a reason | low | no (a detail of MF-2) | idea | partly | MF-2 item 1; P34 |
| Z7-05 | cargo-nextest for process-spawning tests and G-base's verdict | low | yes | idea | confirmed | later (Part III) |
| Z7-06 | Crate dependency policy checked against cargo metadata | low | yes | design | partly | after Q06 |
| Z7-07 | A tested repository-policy guard over workflows, scripts and gate texts | low | yes | idea | partly | with M63 and P34 |
| Z7-08 | An explicit install-script allowlist on the JS side | medium | yes | idea | partly | beside M44 |
| Z7-09 | A manual CI trigger | low | no (needs the owner's push) | idea | partly | SF-2 complement |
| Z7-10 | A ratchet on lint suppressions after M39 | medium | yes | idea | confirmed | M65 |
| Z7-11 | Measure the declared macOS floor in every Mach-O; record the signing status | low | split (record no; gate yes) | design | partly | Q18; pending §3.13 |
| Z7-12 | Deterministic release archives | low | no | design | partly | Q03, deferred |
| Z7-13 | One version source per tool; close the remaining floats | low | yes | idea | confirmed | P39 (owner); SF-8 |
| Z7-14 | Test-only filesystem fault injection for the store's write paths | low | no | design | partly | later design |
| Z7-15 | Provider stream fixtures captured with their frame boundaries | low | no | idea | confirmed | with P04/P41 gates, or later |
| Z7-16 | knip: mark deliberate exports with an inline tag | low | no | idea | confirmed | M67 |
| Z7-17 | A pre-push hook that runs fast gates | low | yes | idea | confirmed | owner option after M64/M65 |
| Z7-18 | A mechanical cross-check of the decision ledger | low | no | idea | confirmed | a `tasks/` tool |

**Z7-01.** CI builds, tests and lints only on ubuntu (`.github/workflows/ci.yml:17-46`), and the Windows job runs Pester only (`:82-98`). The macOS-only code is never compiled by CI: the `getattrlistbulk` scan (`crates/openalpaca_storage/src/memory_files/dirstat.rs:23`, `:38`, `:105`), iMessage (`crates/openalpaca_connectors/src/lib.rs:16`) and keychain discovery (`crates/openalpaca_llm/src/keys/credential_discovery/mod.rs:448`, `:501`). Q08's macOS leg covers the shell crate only (`plan-rev3.md:1008-1010`), and Q09 runs no suite (`:1012-1013`). Owner options:
- (a) a `rust-macos` job on a **pinned** label (for example `macos-15`) that runs the CI set's `cargo build|test|clippy --locked --workspace --exclude openalpaca_gui` as a PR check, beside ubuntu. Whether it is a required check is the owner's branch-protection setting. No apt step on macOS;
- (b) the same job on `schedule` and `workflow_dispatch` only;
- under M65, run the warnings gate on both operating systems (SF-28).

Cost: macOS minutes bill at ten times the Linux rate on a private repository (`plan-rev3.md:607`). A **separate** question reopens DU-33 (a) (`:617-618`): whether Q08's Windows and Linux legs should be written now as non-blocking, dispatch-only jobs. Present it; do not fold it in.

**Z7-02.** The tree holds no LICENSE file, although `Cargo.toml:40` and `apps/openalpaca-gui/package.json:6` declare MIT. The bundle carries no notices (`tauri.conf.json:37`; `scripts/release/package-macos.sh:214-217`). P34's `deny.toml` checks advisories only (`plan-rev3.md:862-865`). So every distributed archive ships about 700 crates and the JS bundle without the licence texts MIT and Apache-2.0 require. Recommendation, all the owner's:
1. Commit the MIT LICENSE the manifests already declare. This is the owner's legal act.
2. Widen `deny.toml` with a `[licenses]` allow list, and optionally `[sources]` (crates.io only) and `[bans]` (multiple versions at warn), as a DU-21 sub-row.
3. Before any beta distribution, generate THIRD-PARTY-NOTICES for Rust (tool not chosen) and for the JS production graph (evaluate `bun pm licenses` first, `modernization-plan.md:524`). Record the hashes of `Cargo.lock` and `bun.lock` it was built from, fail when they no longer match, and bundle it as a Tauri resource and in the archive. Whitelist the file in `.gitignore` (`.gitignore:98` hides `*.md`).

ZCode's strict release check is itself not yet wired into its CI (ZCode:third-party/README.md), so adopt its freshness idea, not a claim that ZCode enforces it.

**Z7-03.** CI runs neither `gen_api_docs.py --check` nor its tests; the plan accepts that as a local step (`plan-rev3.md:922`). `lockdates.py` exits 0 when a lookup fails (`tasks/deps-upgrade/tools/gbase/lockdates.py:13-16`, `:79-85`). MF-2 item 1 and MF-6 already close the crash and lookup holes for the cut. What is new is running the gates in CI. Owner option:
- a seconds-long `tooling` job on ubuntu running `python3 -I scripts/gen_api_docs.py --check`, `scripts/tests/test_gen_api_docs.py`, and lockdates' gate-mode tests against a recorded registry fixture, with no network;
- `gbase.sh --self-test` only if its toolchain needs are cheap on a runner;
- MF-2's gate mode gets tests for the lookup-failure and partial-output paths, as an addition to MF-2.

Consequences:
- P19 and P21 must land in one push, or P19 regenerates `docs/api`. G-base expects `openalpaca_mcp.md` to be stale between them (`tasks/deps-upgrade/tools/gbase/README.md:89`).
- DU-D3 then needs a pinned interpreter (`modernization-plan.md:329`).

ZCode's two script tests listed in its default run are absent from the export (ZCode:vitest.config.ts:99-100); its architecture-checker tests are present.

**Z7-04.** Fold into MF-2 item 1 (review `:145-150`): specify the waiver record's format, one file with id, package@version or advisory id, reason and recording commit, read by lockdates' gate mode. Mirror the advisory ignores into `deny.toml`. If Z7-03's job exists, have it fail when `cargo tree -i zip` no longer shows a zip 8 dependent while P34's dormant ignore remains (`plan-rev3.md:863`). A self-retiring expiry adds little, because a freshness waiver is simply unnecessary once the version turns seven days old.

**Z7-05.** G-base sums `test result:` lines, so a binary that dies by signal adds no failure (MF-6, review `:283-292`). The MCP hang tests leave orphaned `sleep 3600` children (`tasks/deps-upgrade/tools/gbase/gbase.sh:495`, `:553`). After the cut, as an owner option, cargo-nextest: a process per test, a serial test group for R45's KillOnFail tests, the MCP hang tests and `daemon_args`, leak detection and JUnit output. All of those capabilities are unverified on a pinned version. MF-6's amendment stays for the cut-day run.

**Z7-06.** `CLAUDE.md`'s graph says `openalpaca_gui ← openalpaca_api, openalpaca_storage`, while the shell's manifest names storage only (`apps/openalpaca-gui/src-tauri/Cargo.toml:24`). P38 fixes the edge by hand now. After Q06, as an owner option, check internal edges against a checked-in table over `cargo metadata --locked --format-version 1` (ZCode:docs/architecture/architecture-governance.md:18). Generating CLAUDE.md's block from that table is a separate owner choice.

**Z7-07.** Owner option, in Z7-03's job: one tested guard asserting that
- every cargo build, test, clippy and check in workflows and release scripts has `--locked`;
- every `uses:` is a 40-hex SHA with a `# vX.Y.Z` comment;
- every checkout sets `persist-credentials: false`;
- after M63, every `CARGO_TARGET_DIR=` is paired with `CARGO_BUILD_BUILD_DIR`;
- no `+1.9x.y` toolchain literal appears in gate scripts (review `:453`).

ZCode's equivalent guard is documented (ZCode:docs/ci-cache-strategy.md:59-69) but absent from the export.

**Z7-08.** Neither `package.json` declares `trustedDependencies`. `bun.lock` holds esbuild 0.27.2 and @swc/core 1.16.1, both of which declare postinstall scripts, so what runs at install time is decided by Bun's built-in default. The auditor's inference that the scripts did not run is invalid and is dropped. Owner option, beside M44:
- declare `trustedDependencies` explicitly in the root `package.json`, after checking whether an explicit list replaces Bun's default and whether esbuild or @swc/core need their scripts (unverified);
- a `bun pm untrusted` gate (unverified on 1.4.3);
- re-check after P33 removes @swc/core.

ZCode's allowlist: ZCode:pnpm-workspace.yaml:20-32.

**Z7-09.** `ci.yml` triggers only on push to main and on pull requests (`:3-6`). GitHub offers `workflow_dispatch` only for a workflow whose file is on the default branch (not re-checked against GitHub's docs). Optional: land the trigger on main through its own small PR (the owner's push), then dispatch with `gh workflow run ci.yml --ref chore/toolchain-and-deps`. Otherwise rely on SF-2's draft PR, which the review already requires.

**Z7-10.** M39 converts all 135 `#[allow]`s to `#[expect(…, reason)]` (`modernization-plan.md:761-763`), but M65's lint table (`:899`) keeps no new `#[allow]` out. Owner, widening DU-R3 (b)+(d): add `clippy::allow_attributes` and `clippy::allow_attributes_without_reason` at warn, denied through `CARGO_BUILD_WARNINGS`. Use `cfg_attr(<where>, expect(…))` for one-OS lints, and run the gate on ubuntu and macOS (SF-28; ZCode:scripts/architecture/index.mjs:165-178).

**Z7-11.**
- **Record now:** add "unsigned, not notarised" to pending-decisions §3.13 (with Z6-11).
- **Owner option for Q18's gate (`plan-rev3.md:1039`):** read `LC_BUILD_VERSION` minos from every Mach-O in the `.app` and in `libexec/`, and require it to be at or below the declared floor. Set `MACOSX_DEPLOYMENT_TARGET` explicitly for the sidecar and the CLI builds too, which are separate cargo invocations from `tauri build`. Rust's default minos for aarch64-apple-darwin is probably already below 13.3 (not measured), so the gate's value is as a guard (ZCode:docs/desktop-ci-cd.md:188, :223).

**Z7-12.** The archive embeds `built_at_utc` (`package-macos.sh:202-210`), and neither the bundle nor cargo's output is reproducible. Deterministic tar flags alone therefore cannot let a reviewer re-derive it. Defer until the bundle is reproducible; then use sorted names, zeroed mtime, uid and gid, and `gzip -n` (bsdtar flags unverified), written fresh, not ported (ZCode:scripts/deterministic-tar-archive.mjs:21, :96-106, :130-135).

**Z7-13.** Keep OpenAlpaca's single-source stance, and do not adopt mise. The remaining floats are `runs-on: windows-latest` (`ci.yml:84`), P39's planned revert to ubuntu-latest (`plan-rev3.md:885`), Pester installed by version without a hash (`ci.yml:93-96`) and an unpinned Python. Owner: keep pinned runner labels, which drops P39. A label pins the OS version, not the image contents, which still roll weekly. Gate texts name "the pinned toolchain" (SF-8's amendment).

**Z7-14 to Z7-18.**
- *Z7-14:* a test-only `test-fault-inject` feature in `openalpaca_storage`, compiled like `test-home-guard`, aimed at ENOSPC and EIO on `memory_files::fsx` and the artifact writer, with the atomic rename left unhooked. ZCode's version is built (ZCode:apps/zcode-cli/packages/adapters/src/storage/fs-fault-injection.ts). OpenAlpaca already injects logic faults in learning and EACCES by chmod (`crates/openalpaca_storage/src/memory_files/tests.rs:3966`).
- *Z7-15:* a fixture format that keeps each frame's bytes and timing, captured from a real Ollama or OpenAI-compatible response at each provider-path move and replayed chunk by chunk. Never record a key or an auth header (ZCode:docs/testing/llm-api-stream-capture-replay.md:9-47). Today there is one captured Ollama body (`crates/openalpaca_llm/src/providers/openai/tests.rs:821`).
- *Z7-16:* in M67, put each deliberate export's reason in an inline tag that knip excludes (ZCode:knip.json:3-5), so the exemption dies with the export.
- *Z7-17:* do not add a default hook: the Rust suite takes about 5.5 min. As an owner option, an opt-in hook running only `cargo fmt --all -- --check` (after M64) and `bun run format:check` (ZCode:.husky/pre-push:1-20).
- *Z7-18:* a script that lists every decision id with its state in `tasks/pending-decisions.md` and in its source, and fails on a mismatch, run by hand at each revision (ZCode:scripts/audit-conversation-session-case-coverage.mjs:35-60). SF-18 and SF-19 show the ledger drifting.

### Z8 — Pending decisions sweep

**ZCode's design in brief** (as it bears on OpenAlpaca's open decisions; confirmed in code unless marked).

*Permissions.* One synchronous `PermissionService` answers allow, ask or deny, and an async broker handles the ask (ZCode:apps/zcode-cli/packages/core/src/permission/service.ts:101-262). The yolo and guarded modes return an allow **before** the config deny list is checked (:152-154 against :166), as the file's own comment admits (:366). With no interactive client, the default broker denies (ZCode:apps/zcode-cli/docs/design/v2/permission-broker.md:100).

*Bots.* Each bot is bound to one actor by a bind code (ZCode:packages/services/src/bots/botConfigHelpers.ts:39-59). Group commands, approvals among them, are owner-only (ZCode:packages/services/src/bots/groupSessions.ts:26-37; botsService.ts:6241-6321). Each chat, group or topic is its own conversation (ZCode:docs/bots-feishu-group-collaboration.md:11-24, doc).

*Storage and credentials.* Product temp writes were moved out of TMPDIR into the data root (ZCode:docs/tmpdir-storage-paths-spec.md:9-35). Credentials are one AES-256-GCM file whose fallback key is derived from the platform, home and username (ZCode:packages/services/src/credential/providers/credentialCipherProvider.ts:20-39).

*Usage and retention.* Usage facts are one row per logical model request, from one write point, with cache tokens and no cost, pruned at 30 days (ZCode:apps/zcode-cli/packages/core/src/runtime/methods/usage-observability.ts:55-120). Transient event types are named in one set (ZCode:apps/zcode-cli/packages/contracts/src/events/session-event-retention.ts:9-25).

*Model calls and memory.* One retry policy serves the stream and generate paths, with no circuit breaker (ZCode:apps/zcode-cli/packages/adapters/src/model/retry-policy.ts:13-29). Memory recall is chosen by an auxiliary model (ZCode:apps/zcode-cli/packages/core/src/memory/recall/selector.ts:14-25).

| id | title | priority | owner | port | verdict | plan target |
|---|---|---|---|---|---|---|
| Z8-01 | Bind each connector to one person; approvals only from that person (= Z1-09) | high | yes | design | confirmed | code audit H1/H2; MD-4 |
| Z8-02 | A connector conversation is per chat, group or topic; the sender is the actor | high | yes | design | partly | MD-4 design; T11 |
| Z8-03 | No product writes to `$TMPDIR` | medium | no | idea | confirmed | N-STORE-1 |
| Z8-04 | One usage row per logical model request, from one write point | medium | yes | design | partly | CORE-04, OQ-2; migration 005 |
| Z8-05 | Retention by declared event class | low | yes | idea | confirmed | W6, later |
| Z8-06 | Show storage by retention class; clean only the safe classes | low | yes | idea | confirmed | T17, T12, later |
| Z8-07 | Use another product's credentials only through a user-started import | high | yes | design | confirmed | T24 |
| Z8-08 | A local, redacted diagnostics bundle for beta testers | medium | yes | idea | confirmed | B2/B3; new commit |
| Z8-09 | ZCode selects memory with a model: do not port | low | yes (information) | idea | confirmed | none (§6) |

**Z8-01 (= Z1-09).**
- *Today:* iMessage runs every accepted message as the Mac owner, because `resolve_owner` ignores the sender (`crates/openalpaca_connectors/src/imessage/handler.rs:219`). A non-owner DM that starts with the trigger prefix is processed (`imessage/routing.rs:132-155`). `/yes` pops the chat's oldest prompt, keyed by chat (`common/mod.rs:206-235`). These are code audit H1 and H2, deferred (`tasks/pending-decisions.md:206`, `:235`).
- *Recommendation (owner; it adds an enforcement point):* each connector names its bound person, for example by a bind code sent in a private chat. Work and `/yes` are accepted only from that principal, and any other sender gets a reply naming the reason, never silence. Optionally `/yes <short id>` names the request.
- A strict binding also drops non-owner participation in groups, unless MD-4 grants it, so decide this with MD-4.

**Z8-02.** A connector lane is `<owner>:<platform>` and mixes every DM and group on the platform (`tasks/pending-decisions.md:236`). The MD-4 design is not written (`:376`). ZCode keys a conversation by chat, group or topic, with the sender as the actor. Only the owner can enable a group, and enabling copies settings, not history or grants. As one option for MD-4's skeleton:
- key the conversation by (platform, chat[, thread]) and the principal by the bound sender;
- this reverses N1's one perpetual session per lane and the `{owner}:{platform}` key, which T11's row says is not reopened (`:45`);
- ZCode supplies only the conversation identity; the memory and privacy boundary per audience is OpenAlpaca's own design work, because ZCode has no audience-scoped memory.

**Z8-03.**
- *Today:* every Open of an upload copies it to `$TMPDIR/openalpaca-open/<id>-<name>`, and nothing removes the copy (`apps/openalpacad/src/routes/files_types.rs:66-75`, `:128-143`). `openalpaca memory edit`, `memory add` and `persona edit user` use TMPDIR buffers (`apps/openalpaca/src/commands/memory.rs:1277`, `:1526`; `commands/persona.rs:171`), and a refused edit keeps its file there (`memory.rs:1292-1296`). A project's "Delete data" and the factory reset cannot reach them.
- *Recommendation:* stage Open copies under `state/cache/open/<file_id>/` (0700, the original filename kept, which is the reason for staging), swept at boot and removed in the project purge by file id. Put the CLI editor buffers in a 0700 directory under `state/`. This applies D1.
- The residue is recorded in `tasks/storage-design/01-current-layout.md:105`, not in the design's §18.

**Z8-04.** `llm_call_log` has no cache-token columns and no cancelled status (`crates/openalpaca_storage/src/migrations/001_baseline.sql:370-384`), and utility jobs, compaction and nested skills write no row (OQ-2). `agent_id` already carries a source label (`orchestrator_summary`, `lead_agent`, `skill:<id>`). Recommendation (owner), inside CORE-04 and OQ-2:
- one usage write after every routed call, reusing `agent_id` as the label;
- cache read and write token columns and a cancelled status, in migration 005.

Leave retention out, or present it only together with a durable per-task cost rollup. An age prune would zero old tasks' cost on `GET /v1/tasks` (`LlmUsageRepository::cost_for_tasks`, `crates/openalpaca_storage/src/repository/llm_usage/mod.rs:259-270`) and change `/v1/usage/summary` (`:292-308`).

**Z8-05 to Z8-09.**
- *Z8-05:* if the owner ever wants an `event_log` retention pass, tag each variant transient or audit in the exhaustive persistence match (`apps/openalpacad/src/events/persistence.rs:12-25`), and age-prune transient rows. Build nothing unasked (W6).
- *Z8-06:* a read-only size view per retention class (`openalpaca store usage` and a Settings pane) on the README's classes (`crates/openalpaca_storage/src/store/mod.rs:772-790`), with cleanup only for regenerable classes, `plugins/.trash/` and orphaned scratch folders. Do not add a data-path setting that moves only part of the store, the defect ZCode documents (ZCode:docs/settings-storage-management.md:16-39).
- *Z8-07:* see T24 in §5.
- *Z8-08:* a local-only `openalpaca diag export`: a zip written under the store, never uploaded, holding the last N days of logs, versions, and a config projection with every secret-bearing key redacted. It excludes the database, notes and session logs, and is named in B3's list (ZCode:packages/services/src/feedback/feedbackLogArchive.ts:286-306; ZCode:packages/desktop/src/main/exportLogs.ts:138-160).
- *Z8-09:* do not port ZCode's model-chosen recall: MD-1 (§6).

---

## 4. The second revise — amendments for the upgrade plan

**How to read this list.** Each line has an id (A-nn), a target with its line at HEAD, the change, the patterns it comes from, and one of two tags:
- **apply:** work within settled rules; the next revision writes it in.
- **present:** an owner decision; the next revision records it as a question with this recommendation, and nothing ships before the answer.

Lines marked **before the cut** move no version, so DU-23 permits them before the cut, as Phase 0 did. The cut is at or after 2026-10-17T20:08Z as decided (DU-23 (a)); the review recommends 2026-10-21T19:53Z (D1, §5.4). None of these lines is a reason to delay the cut. Plan line numbers are from `git show HEAD:tasks/deps-upgrade/plan-rev3.md` (P), `git show HEAD:tasks/deps-upgrade/modernization-plan.md` (M) and the untracked review (R).

### 4.1 plan-rev3 Part I

- **A-01 · §3.3 (P:447-449) · apply · before the cut.** Two items in "found in passing" become scheduled commits:
  - the MCP HTTP redirect leak becomes N-MCP-2, in D6's commit with the stream fix;
  - the unbounded reconnect ladder (`client.rs:325`) becomes N-MCP-1.

  From Z5-02, Z5-03, Z5-08.
- **A-02 · DU-25, a (p4) sub-row (P:561, recommendation P:567) · present.** A per-server `protocol = "auto" | "legacy" | "2026-07-28"` key in `mcp.toml`, mapped to rmcp's Auto, Initialize and Discover; the pin never downgrades; `skip_serializing_if` keeps every `config_fingerprint`. Also a named negotiation failure: a new `FailureReason` word in the extension design's §4.2 enum, whose text names `protocol = "legacy"`. From Z5-01.
- **A-03 · P19 body (P:782; lifecycle P:787, P:791) · present (with A-02).**
  - Map the key, and classify `NoCompatibleProtocolVersion`, `ExpectedInitResult`, `LegacyFallbackFailed` and a transport close during the probe as A-02's failure.
  - Pin an in-session respawn to the era this load negotiated. State that this is OpenAlpaca's choice; ZCode renegotiates on reconnect.
  - **Warn** on `connect_timeout_secs <= 10` with `auto`.
  - Record an rmcp issue asking for a configurable auto-discover timeout, as the named way out of the private 10 s (rmcp-3.5.1 `src/service/client.rs:657`).
  - Override `get_info` with our `client_info` and an explicit capability set, empty until M84 (the two lines §3.3 already offers the owner).

  From Z5-01, Z5-13.
- **A-04 · P19 gates (P:794, P:795) · apply.**
  - The HTTP round trip checks that a listen stream outlives 30 s, which needs A-13.
  - The forking fixture gains a grandchild that calls `setsid`.
  - Teardown warns, naming the pids, if any pid of the recorded tree survives.

  From Z5-03, Z5-14.
- **A-05 · P19b (P:799) · apply.**
  - Precondition: A-13 has landed.
  - The listener reopens the subscription once after an unexpected end while the connection lives, where today it logs and stops (prep `client.rs:804-819`).
  - A check that a list change arriving mid-turn does not alter the running turn's tool surface, the invariant ZCode keeps (ZCode:apps/zcode-cli/packages/adapters/src/mcp/index.ts:503-531).

  Whether P19b goes ahead at all stays D5. From Z5 and Z8's D5 answers.
- **A-06 · Appendix B #4 (P:1223) and #42 (P:1270) · apply.** Both are answered from the extension row once A-27's field lands: a connect of about 10 s means a server that ignored `server/discover`. From Z5-13.
- **A-07 · P30 (P:843) · apply.**
  - An `unknown_key` warning and a no-tools-declared warning. No `deny_unknown_fields`.
  - Fixtures written fresh for BOM, CRLF, `>`/`>-`/`|`/`|+`, a quoted value containing `: `, a nested map, an unknown key and an empty body, asserted equal under serde_yaml 0.9 and the chosen serde-saphyr.
  - Strip one leading BOM, and match the closing `---` as a whole line (`crates/openalpaca_core/src/middleware/skill/parser.rs:6`, `:17`).
  - Keep one serde parser, and fix or retire `config/skills/create-skill/scripts/list_skills.sh`.
  - The gate adds "no new skill diagnostic over the E6 corpus" once N-SKILL-2 lands, as a complement to `yamlcmp`.
  - D4's research adds: `flatten` under serde-saphyr, the default-on comments feature, and the fixtures above.

  From Z3-04, Z3-05, and Z3's D4 answer.
- **A-08 · P34 (P:862; contents P:863; gate P:865) · present.**
  - Widen `deny.toml` with a `[licenses]` allow list, and optionally `[sources]` (crates.io only) and `[bans]` (multiple versions at warn).
  - Mirror the advisory ignores into A-11's waiver record.
  - If A-12 is taken, fail when `cargo tree -i zip` shows no zip 8 dependent while the dormant ignore remains.

  From Z7-02, Z7-04.
- **A-09 · P28 (P:826) and P41 (P:831) · present (with D10).** Add a mock-server gate (teloxide and twilight against a local stub) as a **supplement**. The live round trip stays the gate, because a stub cannot exercise rustls with OS roots or aws-lc-rs against the real endpoints. From Z8's D10 answer.
- **A-10 · P38 (P:880) · apply, with one present.**
  - Fix CLAUDE.md's `openalpaca_gui` edge by hand (Z7-06).
  - Add the B3 lines from A-41 to the manuals.
  - Present: record the webview-storage exception in CLAUDE.md's D1 bullet (Z6-10).
- **A-11 · Review MF-2 amendment item 1 (R:145-150) · apply.**
  - Gate mode's lookup-failure and partial-output paths get tests against a recorded registry fixture, never the network.
  - The waiver record is one file with the fields id, package@version or RUSTSEC id, reason and recording commit, read by `lockdates --gate`.

  From Z7-03, Z7-04.
- **A-12 · §6, the "docs/api staleness" row (P:922) · present.** A `tooling` CI job on ubuntu running `gen_api_docs.py --check`, its unit tests and lockdates' gate-mode tests, and optionally `gbase.sh --self-test`. Consequences: P19 and P21 land in one push or P19 regenerates `docs/api`, and DU-D3's premise flips (A-36). From Z7-03.
- **A-13 · New commit before the cut · apply (with a present).** `fix(mcp)` in D6's commit (review SF-6 and D6, R:406, R:951):
  - the MCP reqwest client gets `connect_timeout` and an idle `read_timeout`, and no total timeout;
  - a refused redirect is its own non-retriable error naming the target origin, reading `ConfigInvalid` at bring-up and never entering the reconnect ladder;
  - present: refuse **every** redirect when auth or `extra_headers` are configured.

  From Z5-03, Z5-08.
- **A-14 · §5.2 conventions (P:637) · apply.** Gate texts name "the pinned toolchain read from `rust-toolchain.toml`", never `cargo +1.99.x` (SF-8's amendment). From Z7-07, Z7-13.
- **A-15 · P39 (P:885) · present.** Keep pinned runner labels, which drops P39. Note that a label pins the OS version, not the image contents. From Z7-13.
- **A-16 · Review MF-6 (R:279) · apply.** Unchanged for the cut-day run. Record cargo-nextest as the later structural fix (A-45). From Z7-05.
- **A-17 · Review SF-2 (R:335) · optional.** `workflow_dispatch` lands on main through its own small PR, which is the owner's push, then `gh workflow run ci.yml --ref chore/toolchain-and-deps`. The draft PR is still opened. From Z7-09.

### 4.2 plan-rev3 Part II

- **A-18 · Q04 / DU-29 (P:593, option (a) P:595; G3-b P:972; Q04 P:994) · apply, merged into review C-09 (R:877).**
  - The "starting / unresponsive" row: `discovery.json`'s pid is a live `openalpacad` (`running_daemon_pid`, `pid_is_daemon`), and health has not answered.
  - The shell then parks and retries within a bounded budget and shows "daemon is starting". It never spawns, never rotates `daemon.log`, and never probes the singleton lock. On timeout it reports with the log tail.
  - Gate it natively against a deliberately slow stdio MCP server.

  From Z6-01 (i).
- **A-19 · Q04 (P:994) · present.** Write `discovery.json` just before `axum::serve`, or add a phase field to it (updating Q01's byte pin, P:982), or add an early health route. C-09's row is the no-change answer. From Z6-01 (ii).
- **A-20 · Q04 (P:994) · apply, with one present.**
  - A row: same protocol, different `build.version` → reuse with a note naming both versions and Stop/Start; never restart automatically.
  - Present: `install.sh` prints `busy` and asks before stopping the daemon (`scripts/release/install.sh:159-184`).

  From Z6-06.
- **A-21 · Q07 / DU-28 (P:588, P:1004) · present.** SF-21's amendment stands. The reverse-proxy alternative is offered with its trade-off stated: it makes the daemon ambiently authenticated at `127.0.0.1:1420`, so it needs an exact Origin check on every proxied request, WebSocket and SSE upgrades included, and must refuse a missing Origin. From Z6-09.
- **A-22 · Q06 acceptance (P:966) → Q08 (P:1008) · apply.** Q08's macOS job, and a local script, assert Q06's two `cargo tree` conditions on every run after Q06. From Z6-07.
- **A-23 · Q08 / DU-31 (P:605, cost P:607) under DU-33 (P:617) · present.**
  - A `rust-macos` job on a pinned label running the CI set's build, test and clippy with `--locked`, either as a PR check or on schedule plus dispatch.
  - M65's warnings gate on both operating systems (SF-28).
  - A **separate** question that reopens DU-33 (a): write Q08's Windows and Linux legs now as non-blocking, dispatch-only jobs.

  From Z7-01.
- **A-24 · New shell commit beside Q12 (P:1018) / Q14 (P:1024) · present.** PATH for the app-spawned daemon:
  - options (a) capture once in the shell, asynchronously (markers, a deadline, a process-group kill, an allowlist of keys); (b) capture in the daemon; (c) a fixed bootstrap list; (d) document only;
  - recommend (a) with (c) as its fallback;
  - announce the PATH source in `daemon.log` and Settings → Connection;
  - decide the credential-pointer keys.

  From Z6-02.
- **A-25 · Q14 (P:1024) · apply.**
  - `on_web_content_process_terminate` logs to `gui.log` and calls `reload()`. A panic hook writes to `gui.log`. This may land in M76 instead.
  - Startup marks: T0 and the `on_page_load` marks go into `gui.log`. The webview's marks arrive through a debug or instrumented build or the console, never a sixth command unless M74 is amended in the same commit.

  From Z6-05, Z6-04.
- **A-26 · §7.5 rules (P:1060; scenarios P:1058) · apply.** Alternate baseline and candidate trial by trial, on copies of one fixture (SF-22's amendment). D13's owed rows (R:983) are then taken from A-25's marks. From Z6-04.
- **A-27 · Extension row field (with P19) · present (with A-03).** `protocol_version` and the connect and list durations on the extension row and in `openalpaca ext info`, with `docs/api` regenerated. From Z5-13.
- **A-28 · Q10 follow-up (P:1042) · apply.** A size ratchet in `package-macos.sh`: the `.app` and the daemon against Q10's budgets plus a margin, listing the largest items on failure. From Z6-14.
- **A-29 · Q18 (P:1039) · present (with (c)-2).**
  - Read the minos of every Mach-O in the `.app` and in `libexec/`, and require it to be at or below the floor.
  - Set `MACOSX_DEPLOYMENT_TARGET` explicitly for the sidecar and the CLI builds.

  From Z7-11.
- **A-30 · Q03 (P:990) / Q09 (P:1012) · present (with A-08).** THIRD-PARTY-NOTICES generated from the lockfiles, with their hashes recorded and checked, bundled as a Tauri resource and in the archive. Deterministic archives wait until the bundle is reproducible. From Z7-02, Z7-12.

### 4.3 modernization-plan (Part III)

- **A-31 · M84 (M:878) · present.** It widens the ConfirmationBroker from approve/deny to a form, decided with T35 and (c)-3:
  - form mode only, with schema validation; `url` never advertised; any other mode declined with a warning;
  - one input per primitive field (string, number, integer, boolean, enum), server text escaped under the approval rule; at minimum, accept only requests that ask for no fields;
  - an MRTR round attributed exactly through M15's loop;
  - a legacy request attributed only when exactly one call to that server is in flight; otherwise declined. ZCode's map, keyed by server, misattributes concurrent calls;
  - the call's MCP and sandbox clocks suspended or extended while a person answers; otherwise legacy requests are declined;
  - declined on unattended turns and connector lanes;
  - answers never persisted, as with `preview`.

  From Z1-12, Z5-07, and Z4's, Z5's and Z8's (c)-3 answers.
- **A-32 · M15 (M:739) · apply, with one present.**
  - (a) The registry hands each MCP call a CancellationToken that the sandbox fires at its deadline and on run cancel, so `notifications/cancelled` always goes out.
  - (b) `max_total_timeout` is clamped below the effective `max_tool_runtime_secs`, or the smaller one is documented as winning.
  - Present (c): whether progress may extend the sandbox clock.
  - N-MCP-1 may fold in here, since M15 rewrites the call path.

  From Z5-06, Z5-02.
- **A-33 · M14 (M:734) · present, a follow-up commit.**
  - Image, audio and embedded-blob content saved as a by-product through `put_tool_artifact`, within `max_artifact_bytes`: base64-decoded for storage, never image-decoded, transcoded or compressed.
  - The artifact id and path go in the text.
  - With no ToolContext, the model is told nothing was saved.

  From Z5-12, Z1-17.
- **A-34 · M13 (M:730) / M58 (M:803) · apply, with one present.**
  - Schedule the deletion of `PluginLlmProvider`, `get_plugin_provider` and `ProviderType::Plugin` under the taken D-I (revised) before M58's `LlmProvider` commit, or record in M58 that the implementation is deleted, not converted.
  - N-PLUG-1 (`isError`) lands before M58 (a).
  - N-PLUG-6 (collisions) lands before any plugin-surface refactor.
  - Present: a new row for the connector half.

  From Z2-12, Z2-02, Z2-01.
- **A-35 · M60 (M:694) / M40 (M:766) · apply.**
  - Race the plugin agent's `executor.spawn` and `executor.step` RPCs (`crates/openalpaca_core/src/runner/plugin_agent.rs:143`, `:198`) with `run_until_cancelled`.
  - N-LEAD-2's permit acquire uses the same form.
  - N-LEAD-1's JoinSet is M40's sibling.

  From Z4-12, Z4-02, Z4-01.
- **A-36 · DU-D3 (M:329) · present (follows A-12).** If the tooling job is adopted, CI pins its Python interpreter, and that version is the declared floor. From Z7-03.
- **A-37 · M65 (M:898) after M39 (M:761) · present.**
  - Add `clippy::allow_attributes` and `clippy::allow_attributes_without_reason` at warn, denied through `CARGO_BUILD_WARNINGS`.
  - Use `cfg_attr(<where>, expect(…))` for one-OS lints.
  - Run the gate on ubuntu and macOS.

  From Z7-10.
- **A-38 · Ordering · apply.**
  - N-SKILL-1 (catalog) lands before M07 (M:673), which edits `skill/catalog/mod.rs`, or M07 is rebased.
  - N-SKILL-2 (diagnostics) lands before M36 (M:757, `SkillParseError`), or reuses M36's golden test.
  - M67 (M:908) marks deliberate exports with an inline tag that knip excludes.

  From Z3-02, Z3-03, Z7-16.
- **A-39 · M76 (M:848) / M77 (M:853) · apply.** This completes M77's own stated intent.
  - `on_navigation` admits only the exact entry document, and the devUrl root in dev. It refuses every other same-origin path and every scheme other than http(s), and never hands `file:` to the OS.
  - DocumentPreview intercepts anchors: http(s) goes to the browser path; a local path gets no navigation, with Open or Reveal optionally offered through the daemon routes only (ST-18); a fragment stays in the page.
  - G-bundle clicks an absolute path, a relative path, a fragment, `mailto:` and http, and checks the app is still on its page.

  From Z6-03.
- **A-40 · Notes, no change.**
  - M74 (M:838): A-25's marks must not add a command silently.
  - Z6-13's window-state plugin would widen M74 and M75; M78 (M:858) lands as planned.
  - M70 (M:818) stays (c)-1's (a), with G1 and P2 kept at module level.
  - M83 (M:873): design first. ZCode's task registry, completion notification and single read tool give a shape (ZCode:docs/task-output-contract.md). Its prerequisites are A-13 and A-32.
  - M82 (M:868): its validator is reused later by A-46's `result_schema`.

  From Z6-04, Z6-13, and the (c)-1 and (c)-4 answers.

### 4.4 New commits outside the version wave

**Fixes within settled rules (apply).** All move no version and may land before or after the cut. Each one's verifying test is named in its source pattern.

| id | commit | from | order |
|---|---|---|---|
| N-LEAD-1 | `fix(core): a lead's run cancels the subagents it leaves running` (cancel before `lead_agent.rs:1068`, a bounded wait, spans Cancelled, listed in the completion record, a JoinSet) | Z4-01 | first in the lead set |
| N-LEAD-2 | `fix(core): a subagent spawn queues under the lead's bound` | Z4-02 | after N-LEAD-1 |
| N-LEAD-3 | `fix(core): each subagent result reaches the lead once` (amends Stage 0 F2's sizing) | Z4-04 | any |
| N-LEAD-4 | `fix(core): a resumed run rebuilds its subagents from the session log` | Z4-05 | any |
| N-LEAD-5 | `fix(core): a subagent's failure names its cause and whether a retry can help` | Z4-14 | any |
| N-MCP-1 | `fix(mcp): a call that may have reached the server is never sent again` (one deadline; reconnect once before sending; extension design §3.6 item 1 text) | Z5-02 | before the cut, or folded into M15 |
| N-MCP-2 | = A-13 (the stream timeout and D6) | Z5-03, Z5-08 | before the cut, before P19b and M15 |
| N-MCP-3 | `fix(mcp): calls do not hold the service lock across the RPC` (extension design §3.2 T4 text) | Z5-04 | with or after N-MCP-1 |
| N-MCP-4 | `fix(core): remote MCP tool names mapped to a provider-safe alphabet; collisions within one server warned` | Z5-05 | any |
| N-MCP-5 | `feat(mcp): stdio stderr kept as a redacted tail on the failure` | Z5-09 | after P19 |
| N-TOOL-1 | `fix(core): the shell's stated timeout and hint come from the enforced one; chaining wording fixed; the main loop reads max_tool_runtime_secs` | Z1-05 (1) | any |
| N-TOOL-2 | `fix(core): validate and gate a call before its confirmation card` | Z1-04 | any |
| N-TOOL-3 | `fix(core): serialise non-read-only tool calls in a round` (presented for latency) | Z1-01 | any |
| N-TOOL-4 | `fix(llm,core): tool errors carry is_error; hints move into the tools` | Z1-13 | any |
| N-TOOL-5 | `fix(llm,core): malformed tool arguments are logged and every issue reported` (both sites) | Z1-14 | any |
| N-TOOL-6 | `fix(core): an empty successful result says so` | Z1-15 | any |
| N-TOOL-7 | `fix(core): a command's spill stub previews head and tail` (§5.4 amended) | Z1-16 | any |
| N-SKILL-1 | `fix(skills): every available skill reaches the catalog, and the lead gets it` | Z3-02 | before M07 |
| N-SKILL-2 | `feat(skills): skill diagnostics on the API, CLI and Settings; the level-1 scan bounded` | Z3-03, Z3-10 | before P30 and M36 |
| N-SKILL-3 | `fix(skills): create-skill writes only where the daemon reads` | Z3-08 | any |
| N-SKILL-4 | `fix(skills): say when a skill's instructions were cut` | Z3-09 | any |
| N-PLUG-1 | `fix(plugins): an isError result is an error` | Z2-02 | before M58 (a) |
| N-PLUG-2 | `feat(plugins): warn on unknown manifest keys and unwired types` (D-I's warning half) | Z2-04 | any |
| N-PLUG-3 | `docs(plugins): the approve surface says what approving grants` | Z2-05 | any |
| N-PLUG-4 | `fix(plugins): an update rolls back a failed swap and recovers at boot` | Z2-09 | any |
| N-PLUG-5 | `fix(plugins): mcp_compatible plugins get the real MCP lifecycle` (option (a)) | Z2-08 | after P19 |
| N-PLUG-6 | `fix(plugins,core): honour register_template's answer; unload removes only what the plugin owns` (the skip rule itself is presented, B-05) | Z2-01, Z3-07 | before any plugin-surface refactor |
| N-DAEMON-1 | `fix(daemon): each shutdown stage within the remaining window; discovery removed on forced exit; no follow-up autostart during shutdown` | Z4-15, Z4-03 (apply half) | any |
| N-STORE-1 | `fix(store,daemon,cli): Open copies and editor buffers live under state/, not $TMPDIR` | Z8-03 | any |
| N-GUI-1 | `fix(gui): the oa-* storage keys are namespaced by the served store` | Z6-10 (apply half) | after Q07 |

**Owner decisions with a commit shape (present).** Each becomes a row in `tasks/pending-decisions.md` where none exists.

| id | decision | from | pending row |
|---|---|---|---|
| B-01 | Per-tool Always-allow policy on the card (args, none or tool), with a dangerous-command catalog as data | Z1-02 | new, beside T35 and R43 |
| B-02 | T35's answer: read plugin annotations; extension tools ask unless they declare themselves read-only | Z1-03, Z2-03 | T35 |
| B-03 | A skill's confirmation list adds to the defaults; `skill_script:*` asks like the shell (narrower form for cron skills) | Z3-01 | new, beside T35 |
| B-04 | Extension children spawned without the daemon's secrets (the minimal `env_remove` first) | Z1-06, Z5-11 | §1.2, new |
| B-05 | The plugin collision skip rule (amends extension design §10 case 13); canonical `<plugin>:<id>` naming as a further option | Z2-01, Z3-07 | new |
| B-06 | Consent bound to a digest of the declaration (`DeclarationChanged`) | Z2-10 | new; informs §13 Q4 |
| B-07 | SIGTERM grace and a detached reaper (reverses R-T23; no descendant signals) | Z1-07 | storage §18 residue |
| B-08 | Compare-and-swap between the edit card and the write (S4b) | Z1-08 | storage §18 residue |
| B-09 | Connector approvals and work only from the bound person (H1, H2) | Z1-09, Z8-01 | §3.3, §3.6 |
| B-10 | Deny with a reason (`feedback`) | Z1-10 | new |
| B-11 | A per-tool timeout policy; a model-settable `timeout_secs`; the main loop's cap value | Z1-05 (2) | new |
| B-12 | T22: the scoped read and the corrected widening guard | Z1-11 | T22 with T16 |
| B-13 | Lead-only `cancel_subagent` and `message_subagent` | Z4-07 | new |
| B-14 | Boot settles leftover follow-ups (at most once) or re-queues them | Z4-03 | new |
| B-15 | The completion guard on top of N-LEAD-1 | Z4-01 | new |
| B-16 | Approval clocks start at the head of the queue | Z4-10 | new, beside R43 |
| B-17 | Parse `mcp.toml` per block (amends extension design §2.1/§5.1) | Z5-10 | new |
| B-18 | Credential discovery ships off, opt-in | Z6-08, Z8-07 | T24 |
| B-19 | `openalpaca diag export` (local only) | Z8-08 | B3 |
| B-20 | One usage write point with cache tokens and a cancelled status (migration 005) | Z8-04 | OQ-1, OQ-2 |
| B-21 | `skill_execution_log` keyed by catalog id (migration 005, with the `agent.config` drop) | Z3-15 | §1.5 agent.config |
| B-22 | A LICENSE file, the `deny.toml` licence policy, and shipped notices (A-08, A-30) | Z7-02 | §3.13, new |
| B-23 | The `tooling` CI job and the policy guard (A-12) | Z7-03, Z7-07 | new |
| B-24 | An explicit `trustedDependencies` with a `bun pm untrusted` gate | Z7-08 | new |
| B-25 | A dependency-edge check over `cargo metadata` after Q06 | Z7-06 | new |
| B-26 | A user-triggered liveness check (`ext info --check`) | Z5-15 | new |
| B-27 | The D1 record of the webview's own storage | Z6-10 | §3.9 |

### 4.5 Later designs (record, do not schedule)

- **Z1-18** a long command backgrounded at its budget (reverses R45).
- **Z1-19** `tool_input_start` frames.
- **Z2-06** declarative skills and agents in a plugin directory.
- **Z2-07** plugin tools as a bundled MCP server (reverses extension design :224).
- **Z2-11** declared claims in the preview.
- **Z2-14** hooks.
- **Z2-15** MCP Apps.
- **Z2-16** remote install sources.
- **Z3-06** a bundled skill pack (changes L9).
- **Z3-11** subagent skill allowlists.
- **Z3-12** project skills and import from other agents (with T8, T24).
- **Z3-13** a composer `/` picker.
- **Z3-16** tools gated on a teaching skill (with T3).
- **Z4-06** round-boundary result injection (`until: "any"` can land as a fix).
- **Z4-08** `ask_lead`.
- **Z4-09** a workflow-scoped cost accumulator (T13).
- **Z4-12** an inactivity watchdog for `wait_for_subagents`.
- **Z4-13** labelled daemon rows for the working-state writer (12 §6.2).
- **A-46 · Z4-16** a typed `result_schema` on `spawn_subagent`.
- **Z5-16** an asynchronous catalog at boot.
- **Z5-18** per-project MCP processes.
- **Z6-12** close-to-hide.
- **Z6-13** remembering the window size.
- **A-45 · Z7-05** cargo-nextest.
- **Z7-14** fault injection.
- **Z7-15** stream fixtures with frame boundaries.
- **Z7-17** an opt-in pre-push hook.
- **Z7-18** a ledger cross-check script.
- **Z8-02** MD-4's per-chat conversation skeleton.
- **Z8-05** event-class retention (W6).
- **Z8-06** storage by retention class (T17, T12).

### 4.6 Records to correct now (no code)

- **A-41 · B3's list (`tasks/pending-decisions.md:179`) and §3.13 (`:275`):**
  - a daemon the app spawns may lack the terminal's PATH; the workaround is an absolute `command` or `env.PATH` in `mcp.toml` (Z6-02);
  - "unsigned, not notarised" (Z6-11, Z7-11).
- **A-42 · New rows in `tasks/pending-decisions.md`** for B-01, B-03, B-05, B-06, B-10, B-11, B-13 to B-17, B-22 to B-26, and the connector half of Z2-12. The row text is the recommendation in this file.
- **A-43 · Storage-design citation:** the residue of the `$TMPDIR` Open copy is recorded in `tasks/storage-design/01-current-layout.md:105`, not in the design's §18 (Z8-03).
- **A-44 · `CLAUDE.md` Gotchas:** when a ZCode mechanism is borrowed, its trust default is not (§6 of this file). This is a pointer, added in the same commit as P38.

---

## 5. Pending decisions informed by ZCode

The table has one row per decision, consolidated across the areas that answered it. Where areas disagreed, the row follows the verifier's reading, and the disagreement is named.
- **Changes?** says whether the recommendation recorded in the decision's source changes. Several sources recorded none; for those, "yes" means a recommendation is now given.
- **†** marks a row whose every contributing answer had no verifier pass.
- Line numbers are those of `tasks/pending-decisions.md` at HEAD unless another file is named (P = plan-rev3, M = modernization-plan, R = review).

### 5.1 Pending-decision rows (plan §0 and `tasks/pending-decisions.md` §1)

| decision | ZCode's answer | recommendation in light of it | changes? | areas |
|---|---|---|---|---|
| **T35** (:30) | Every MCP tool, plugin-bundled ones included, has `needsApproval = true` whatever its hints; hints set only the displayed risk (ZCode:apps/zcode-cli/packages/core/src/mcp/index.ts:129-148). Build mode asks (ZCode:apps/zcode-cli/packages/core/src/permission/service.ts:495-551). Only plan mode admits non-destructive MCP tools (:458-466). The relief is persistent project allow rules (ZCode:docs/permission-project-approval.md:62-80). | **Precondition:** read plugin annotations (Z2-03; `crates/openalpaca_plugins/src/manager.rs:1569` drops them). **Options:** the three recorded (no change; MCP only; MCP and plugins), plus (iv), **recommended**: MCP and plugin tools ask unless they declare `read_only_hint == Some(true)` and not `destructive_hint == Some(true)`, between ZCode's ask-always and today's ask-never; and (v) ZCode's strict form, ask whatever the hints. **Consequences:** under (iii) to (v), unattended turns refuse such tools. Pair with B-01 so extension tools keep a whole-tool Always allow. **Kept apart:** a persistent per-server or per-project allow (Z5's and Z8's suggestion) is a separate surface that reverses R43; present it on its own. Skill scripts are a sibling class (B-03). B3's second line follows the answer. | yes (none recorded → (iv)) | Z1, Z2†, Z3, Z5, Z8 |
| **T1** (:22) | A per-tool deny set exists: config `disallowedTools`, and project deny/ask/allow rules by name and content (service.ts:166-193, :215-222). MCP tools named there are never registered (mcp/index.ts:88-96). Yolo returns an allow **before** the deny list (service.ts:152-154 against :166). | Keep "No", under the two-axis rule and C8. If it is ever taken, evaluate deny before every allow path and mode shortcut, and match legacy and current names. | no | Z1, Z8 |
| **T3** (:23) | No ToolSearch and no deferred loading (ZCode:docs/task-output-contract.md). | Confirms "record the rule, build nothing". | no | Z8 |
| **T4** (:24) | The model is told nothing about a failing server; a call fails with "MCP server is not connected" (ZCode:apps/zcode-cli/packages/adapters/src/mcp/index.ts:930-933). | No new evidence; the moving-slot recommendation is unchanged. | no | Z8 |
| **T6(a)** (:25) | No background respawn. The next call reconnects a disconnected record once, inside its own deadline. Only a definite "Not connected" is retried (index.ts:778, :828-830, :916-928). | Keep automatic recovery, but change the mechanism (N-MCP-1): reconnect once **before** sending, never send again a call that may have reached the server, one deadline over both. This keeps §3.6's transparent respawn and removes the unbounded ladder (P:449). Whether the ledger then learns `Failed` lazily at the next call, instead of after the respawns, is the owner's (Z8's option; it touches extension design §3.6 item 1). | yes (mechanism) | Z5, Z8 |
| **T6(c)** (:26) | Connections keyed by name, scope and the full stable-stringified config, credential values included (ZCode:apps/zcode-cli/packages/adapters/src/mcp/pool.ts:530-541). | Yes, as recommended. Keep the value-masked fingerprint, which is stricter than ZCode's key. | no | Z8 |
| **T7** (:27) | The credentialed fetch retries once after a 401, only when a credential was injected, and never after a 403 or a 3xx (ZCode:apps/zcode-cli/packages/adapters/src/mcp/official-auth.ts:84-93, :320-330). ZCode's OAuth recovery has no OpenAlpaca counterpart and is not evidence for this row. | One re-resolve and one retry, narrowed to a 401 on a request that carried the env credential. A 403 becomes `NeedsAuthorization` at once; `crates/openalpaca_mcp/src/error.rs:145-151` already separates the two. | yes (narrowed) | Z5, Z8 |
| **T8** (:28) | The design gates untrusted project MCP servers with consent keyed by project id and a config digest (ZCode:apps/zcode-cli/docs/design/v2/mcp.md:150-167), but the code ships them trusted (ZCode:apps/zcode-cli/packages/bootstrap/src/zcode-protocol/mcp.ts:74-75). Workspace hooks got a digest-keyed trust store after a deep-link attack chain (ZCode:docs/workspace-hook-trust-mr-scope.md:7-20). | Yes, as recorded, with consent kept in the home store. Bind consent to a digest of the declaration, so an edited checked-in file needs fresh approval (B-06). If project skills are ever read, the same gate applies, plus no scripts and no confirmation list (Z3-12; this also meets ST-9). ZCode reversing its own design for MCP is the argument for writing the gate before any project-scope read lands. | no (refined) | Z2†, Z3, Z8 |
| **T9** with T19 (:36, :38) | Sensitive plugin config is written in plain text and hidden only on the wire (ZCode:apps/zcode-cli/packages/bootstrap/src/plugins.ts:929-947). The cipher's fallback key comes from platform, home and username (ZCode:packages/services/src/credential/providers/credentialCipherProvider.ts:20-39). The one safeguard is a sink rule: env and header only (ZCode:apps/zcode-cli/packages/adapters/src/plugins/mcp.ts:384-430). | Confirms `secret_encrypted`. Do not copy the key derivation. Borrow the sink rule if plugin manifests ever gain templates. | no | Z2†, Z8 |
| **T11** (:45) | Every bot channel has `/new` and `/clear`, which return the chat to a draft (ZCode:docs/bots.md:87, :111-126; doc). | Reserve `/new` and keep N1. `/new` presupposes a per-chat boundary (Z8-02). | no | Z8 |
| **T12** (:54) | Usage facts pruned at 30 days, logs rotated at 14 and 7 days, trajectories bounded with an opt-in to keep everything (ZCode:docs/model-io-full-retention-setting.md:9-17). | Consistent with 90 days and with reserving `persist` as a direction. | no | Z8 |
| **T13** (:55) | No monetary budget anywhere. Usage is summed over the run's lineage (ZCode:docs/dynamic-workflow/authoring.md:1291-1307). A goal's token budget counts only the parent session (ZCode:apps/zcode-cli/packages/contracts/src/events/session.events.ts:1245-1258). | N-LEAD-1 is a precondition for any workflow bound: orphaned subagents escape every cap, and the lead's accumulator restarts on resume (`crates/openalpaca_core/src/runner/agentic_loop/mod.rs:449`). ZCode's evidence leans toward the relabel answer ("lead agent spend", with the task-wide figure beside it). If enforcement is wanted, Z4-09's rules apply. This departs from the lessons' unqualified yes; the decision stays the owner's. | yes (leans to relabel) | Z4, Z8 |
| **T15** (:29) | A spill envelope names a path that every relevant surface can Read (ZCode:apps/zcode-cli/packages/core/src/tool/result-persistence-format.ts:29-44). Read is admitted without approval (service.ts:499-506). | Confirms yes. | no | Z1, Z8 |
| **T17** (:61) | No trash: uninstall deletes the cache and the data by default (plugins.ts:775-790), against ZCode's own spec. | Confirms no auto-purge. At most, an explicit owner verb showing each entry's size and provenance. | no | Z2†, Z8 |
| **T18** (:47) | A CLI invocation starts its own session, and `--continue` resumes the directory's last root session (ZCode:apps/zcode-cli/docs/design/v2/session-persistence.md:92, :429, :458). Results go to the launching runtime (ZCode:apps/zcode-cli/packages/core/src/runtime/methods/subagent.ts:285). | Keep and document. Move the recorded "for later" pin of a lead-queued follow-up to `task.session_id` to now; that is the owner's call. A CLI one-shot that opens its own session is an extra option and a design change under R48, not the row's fix. | yes (timing of the pin) | Z4, Z8 |
| **T20** (:69) | No local-model provider, so no evidence. | Keep `enabled = false`. | no | Z6 |
| **T21** (:39) | Expiry is enforced only by the server: 30 s single-use capabilities checked on use (ZCode:packages/server/src/hostCapability.ts:4-52). Its long-lived token has no expiry. | An expiry means something only where it is enforced. Rotate the token as recommended, or drop `expires_at`. | no | Z8 |
| **T22** with T16 (:48, :46) | Pending permissions are a projection per session (ZCode:apps/zcode-cli/packages/contracts/src/events/event-reducer.ts:517-571). Answers go by unguessable id, and a miss is idempotent (ZCode:apps/zcode-cli/packages/bootstrap/src/zcode-protocol-v4/commands/handlers/interaction-background.ts:35-62). Only the full-access answer checks the session (interaction-registry.ts:165-178). | Scope the read with T16, as recorded, and keep answering by id. If a widening guard is wanted: accept `entire_tool` only with the prompt's `stream_id` or `lane_key` (main loop) or its `task_id` checked through `owned_run` (workflow prompts). The auditor's lane-or-stream-only rule would block Always allow on every workflow card. | yes (adds the corrected guard) | Z1 |
| **T24** (:70) | Other agents' config is imported only from a user-opened import step, per item, never overwriting (ZCode:docs/onboarding-external-agent-import-steps.md:9-23). ZCode never reads another agent's tokens. Its one keychain read is a user-started Chrome import with a 15 s timeout, and a denial ends it (ZCode:packages/desktop/src/main/chromeCredentialManager.ts:6-10, :50-61). Features that reach the network are off unless the build enables them (ZCode:docs/desktop/build-time-optional-capabilities.md:5-11). | Keep "not offered", and answer rev 3's open sub-question. Ship `[credential_discovery] claude_code = false, codex = false` in the seeded `llm.toml`, opt-in from Settings, and consider the same for `[cli_backends.*]` (B-18). If the feature stays, make it an explicit import with a consent card, a timeout and refusal as a stop. Review SF-13 already uses exactly this pre-seed for live checks. | yes | Z3, Z6, Z8 |
| **T25 / T33** (:76, :80) | Quit asks only while sessions run, in a native Quit/Cancel dialog with Cancel as the default and no typed word (ZCode:packages/desktop/src/main/index.ts:1445-1487). | Confirms `dangerGhost` and no typed word. | no | Z6 |
| **T32** (:79) | Disposal aborts and awaits everything, and sends no farewell (ZCode:docs/bot-channel-runtime-split.md:35; doc). | Confirms the silence. | no | Z8 |
| **T37** (:62) | Read is on every surface, subagent profiles included (ZCode:apps/zcode-cli/packages/core/src/subagent/profile.ts:35), and admitted without approval. Keyword-free offering is plausible but was not shown in code. ZCode's Read is unconfined. | Confirms yes, confined to the scratch folder as today (ST-16). | no | Z1, Z8 |
| **stop drain** (:91) | Session close cancels in-flight extraction (ZCode:apps/zcode-cli/packages/core/src/runtime/agent-runtime.ts:366-373; memory/extraction.ts:168-178) and waits only for the cancellation. The cursor is not advanced (:116-118). | Rely on the boot catch-up. OpenAlpaca's `learning_cursor` is durable, and its own code calls the catch-up the durable half of the stop drain (`crates/openalpaca_core/src/memory/learning/queue.rs:969-972`), so cancelling at stop loses nothing. It narrows MD-10's trigger, so it is the owner's. | yes (none recorded) | Z4†, Z8 |
| **OQ-1** (:118) | Usage rows store tokens with the cache read and write breakdown and the raw usage, and no cost (ZCode:apps/zcode-cli/packages/core/src/runtime/methods/usage-observability.ts:104-118). | Keep CORE-04, and store cache-token counts so cost can be recomputed later. | no | Z8 |
| **OQ-2** (:119) | One write point records every logical request: compaction, titles, workflow children, subagents, failures (ZCode:apps/zcode-cli/docs/design/v2/usage-observability.md:18-21, :86-95). | One usage write after every routed call, reusing `agent_id` as the label (B-20). The top-level skill tier labels its rows "orchestrator" today, so a `skill:<id>` label would change both tiers. | yes (none recorded) | Z3, Z8 |
| **OQ-8** (:125) | Skills run inside the loop, so they always answer on the session's model; skill frontmatter has no model key (ZCode:apps/zcode-cli/packages/adapters/src/skills/index.ts:21-27). | Yes: a nested skill runs on the turn's pinned model. The mechanism is to thread the turn's `model_override` into the skill tier **and** the nested executor. A1's resolution reads the configured pin (`crates/openalpaca_core/src/orchestrator/skill/invocation.rs:123-140`, `:642`) and would not carry the turn's. The cost implications are the owner's. | yes (none recorded) | Z3 |
| **id-collision** (:107) | A model is the pair (providerId, modelId) (ZCode:apps/zcode-cli/packages/contracts/src/model/model.ts:44-46), and usage books both. | Key the registry and selection by (provider, model). This touches `POST /v1/chat`'s `model`, `GET /v1/models` and the ladder, so it is the owner's design (llm-simplify §3). | yes (none recorded) | Z8 |
| **Q1** (:108) | Usage books the resolved model, never the id the provider echoes, with the raw usage beside it (usage-observability.ts:55-120). | Book the routed id, as V4 does on the streaming path, and keep the echoed id as a raw detail. | yes (none recorded) | Z8 |
| **Q3** (:110) | One retry policy for stream and generate, with retry-after honoured (ZCode:apps/zcode-cli/packages/adapters/src/model/retry-policy.ts:13-29). An empty completion is retried once (empty-completion-retry.ts:8-31). | Confirms code audit M3, with one boundary added: retry, or answer `EmptyCompletion`, only before the first delta reaches the user. | no | Z8 |
| **Q5** (:112) | No circuit breaker; per-request retry with jitter. | If a breaker stays, one per provider (M5). | no | Z8 |
| **telemetry** (:149) | No keyword router: the model chooses from the listed catalog, and the user picks with `/skill` (ZCode:apps/zcode-cli/packages/cli/src/command-center/slash-commands.ts:252-266). | Delete the computed-but-unused skill suggestions (`crates/openalpaca_core/src/orchestrator/intent/skill_match.rs:68-76`), and keep the deterministic tier. This answers only half the row; the log-only events are not addressed. | yes (half; none recorded) | Z3 |
| **batch work** (:148) | The dynamic workflow: a lead-level tool, a typechecked script with a confirmed static graph, typed asks and a journal (ZCode:docs/dynamic-workflow.md:9-33). | Keep "a lead-agent tool, not a resurrected executor". If one is built, take the core: typed results (Z4-16), a durable record per subagent (Z4-05) and queuing (Z4-02), and only once lead sequencing has measurably failed. | no | Z4†, Z8 |
| **arming** (:138) | Nothing time-based: a chord submits, and the dialog resets per request id (ZCode:docs/elicitation-keyboard-behavior.md:9-21; doc). | An alternative option, not a recommendation. It changes DESIGN_SPEC §4.5's Enter contract. | no | Z8 |
| **resolve_skill_key's deferred fix** (`crates/openalpaca_storage/src/repository/skill_execution/mod.rs:444-471`) | Telemetry is keyed on resolved identity, never on a name string (ZCode:docs/monitoring/skill-agent-step-telemetry.md:21). | Take it with migration 005 and the `agent.config` drop (B-21). | yes† (no number → 005) | Z3† |

### 5.2 The programme's five open questions (`tasks/pending-decisions.md` §4.3)

| decision | ZCode's answer | recommendation | changes? | areas |
|---|---|---|---|---|
| **(c)-1** `<Activity>` (:308) | No Activity; state lives in a store outside the view tree (ZCode:packages/ui/src/Root.tsx:499-503). | (a), with G1 and P2 kept at module level. | no | Z6, Z8 |
| **(c)-2** macOS floor (:309) | Electron 41 inherits Chromium's macOS 12 floor (ZCode:docs/electron/docs/breaking-changes.md:203-208). Sidecars are built with `MACOSX_DEPLOYMENT_TARGET=12.0` (ZCode:docs/desktop-ci-cd.md:188; doc). | (a) 13.3, stated once and shown in the app. A-29's minos gate is the owner's option. | no | Z6, Z7†, Z8 |
| **(c)-3** elicitation (:310) | Form mode only, advertised only with a host port; other modes declined; attributed to the call in flight and declined when there is none; one question per field; live-only (index.ts:1093-1131, :1543; ZCode:apps/zcode-cli/packages/bootstrap/src/zcode-protocol/interaction-broker.ts:250-288; ZCode:apps/zcode-cli/packages/bootstrap/src/zcode-protocol-v4/mcp-elicitation.ts:30-150). | Keep (a), but with A-31's shape: an Approve/Deny card cannot return `content`. Decided with T35. | yes (shape) | Z1, Z4, Z5, Z8 |
| **(c)-4** MCP tasks (:311) | No MCP tasks. Its own long work uses a task registry, notifications and one read tool (ZCode:docs/task-output-contract.md). | (a) design first. A-13 and A-32 come first. | no | Z5, Z8 |
| **(c)-5** window chrome (:312) | An overlay title bar, paid for with a drag strip on every view (ZCode:docs/automations-main-window-drag-region.md:1-10). | (a) keep the decorated window. | no | Z6, Z8 |

### 5.3 Design questions, recorded residue and code-audit items

| decision | ZCode's answer | recommendation | changes? | areas |
|---|---|---|---|---|
| **Extension design §13 Q4** | Workspace-hook trust v2 removed the one-time and trust-all states, keeping trust in one exact declaration (ZCode:docs/workspace-hook-trust-mr-scope.md:20-22, :70-72). | No escape hatch. With B-06, the prompt shows only the delta. | no† | Z2† |
| **R43** (:204) | A session allow covers only the subagent that asked; project rules persist; a parent's full access never widens a child (ZCode:docs/dynamic-workflow/launch.md:1060-1068; ZCode:docs/permission-project-approval.md:184). | Keep R43. Address the fan-out pain with B-16 (queue-head clocks) and B-01 (the argument-scoped allow). A run-wide or persisted allow only as an explicit reversal. | no | Z1, Z4†, Z8 |
| **H1 / H2** (:206, :235) | One bound actor per bot; others refused; group approvals owner-only (ZCode:packages/services/src/bots/botConfigHelpers.ts:39-59; botsService.ts:6241-6321; groupSessions.ts:26-37). | Take both (B-09), decided with MD-4. | no | Z1, Z8 |
| **R-T23 and R45's edges** (storage design §18; :190) | SIGTERM to the group and its descendants, then SIGKILL after 1.5 s through an escalation that outlives the call (ZCode:apps/zcode-cli/packages/adapters/src/exec/process-tree.ts:3-90). | Take the record's "idea for later" now (B-07), **without** descendant signals. It reverses R-T23. | no (timing) | Z1 |
| **S4b**, the stale edit card (storage design §18) | The input is resolved once before approval (ZCode:apps/zcode-cli/packages/core/src/tool/types.ts:334-351). Edit refuses a file changed since it was read (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/edit.ts:421-458): related, not the same mechanism. | Close it with a compare-and-swap (B-08). | yes (residue → close) | Z1 |
| **MD-4** connector audience (:376) | Each chat, group or topic is its own conversation, and the sender is the actor. Enabling a group copies settings, not history or grants (ZCode:docs/bots-feishu-group-collaboration.md:11-24). | One option for the skeleton (Z8-02), naming its reversal of N1 and the lane key. The memory and privacy rules are OpenAlpaca's own design work. | yes (none recorded → an option) | Z8 |
| **Continuity residue 12 §6.2** (:229) | Synthetic inputs carry a source and model-only visibility, filtered downstream (ZCode:docs/subagent-respond-to-coordinator.md:24-48). | Label whole daemon-authored rows `[daemon]` (Z4-13). Reopening `next` is the owner's. | no† | Z4† |
| **/pause is advisory** (:267) | No pause: stop leaves a run resumable, and resume replays the journal (ZCode:apps/zcode-cli/packages/dynamic-workflow/docs/execution-engine.md:513-581). | Report "pause unsupported", or map `/pause` to cancel-as-interrupted plus replay. No pause token. | no† | Z4† |
| **Code audit M10**, web_fetch SSRF (§3.10) | Only literal private addresses are blocked, with no DNS, to stay within a 1 s budget (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/webfetch-egress-guard.ts:14-27). | M10's check of every resolved IP stays the stronger answer. Resolve once and connect to the checked address. | no | Z8 |
| **W6**, `event_log` retention (§3.9) | Transient types are named in one set and evicted; audit events are kept (ZCode:apps/zcode-cli/packages/contracts/src/events/session-event-retention.ts:9-25). | Still nobody's default. If wanted, the shape is Z8-05. | no | Z8 |

### 5.4 The 2026-10-17 review and the programme's DU rows

| decision | ZCode's answer | recommendation | changes? | areas |
|---|---|---|---|---|
| **D1** cut instant (R:919) | No release-age rule; versions move by hand under the lockfile. | Unchanged: cut at or after 2026-10-21T19:53Z, with tooling that refuses to start early (MF-1 item 3). | no† | Z7† |
| **D2** rustls waiver (R:933) | No freshness waivers. Exceptions carry a reason and an optional expiry (ZCode:architecture-policy.yaml:187-195). | Unchanged (moot under D1). If waived, record it in A-11's waiver file. | no† | Z7† |
| **D3** Rust 1.99.1 (R:937) | The runtime is pinned exactly; a floating CI runtime broke packaging (ZCode:third-party/README.md:9). | Unchanged: DU-23 governs, and gate texts name the pinned toolchain (A-14). | no† | Z7† |
| **D4** serde-saphyr 2.0 (R:941) | A hand-written flat YAML subset, and a bug when two readers disagreed (ZCode:docs/external-agent-skills-import.md:63). | Unchanged: research 2.0 first, with A-07's items added. P30's live risk is on the reader side only. | no | Z3 |
| **D5** P19b (R:945) | A list change only marks the server stale, and the list is fetched again at the next turn. No `subscriptions/listen` in ZCode's code (its SDK is not vendored). | Take P19b with A-05's conditions: A-13 first, one reopen, the mid-turn invariant. | yes (conditions) | Z5, Z8 |
| **D6** MCP redirects (R:951) | The credentialed fetch sets `redirect: "manual"`, and any 3xx is a typed, never-retried failure (official-auth.ts:84-93, :222-228, :325-331). | Take it, with A-13's typed error. Refusing every redirect for credentialed servers is presented. | no | Z5, Z8 |
| **D7** symlinked ancestor (R:957) | Prefers the bundled runtime; falls back to an explicit env path before refusing; no rule on symlinked ancestors (ZCode:packages/desktop/src/main/desktopRuntimeEnv.ts:423-439). | Weak support for D7's canonicalise-and-keep-identity. The recommendation is unaffected. | no | Z6 |
| **D8** Reveal by file id (R:961) | Opens renderer-supplied paths after realpath with no check on the file's kind. Its unvalidated openExternal was its own audit's P2 (ZCode:docs/security/security-audit-2026-06-09.md:129-150). | Informs without deciding: it favours keeping validation away from webview-supplied paths. | no | Z6 |
| **D9** CSP and CORS (R:967) | The desktop avoids loopback HTTP. The debug server ships a global `cors()` against its own audit (ZCode:apps/zcode-cli/packages/debug/server/index.ts:31). | Record as residue now; revisit with a runtime-pinned `connect-src` after Q04. | no | Z6, Z8 |
| **D10** live bot gates (R:971) | Bot CI runs against a mock Feishu API; real-tenant evidence only where no fixture can prove it (ZCode:docs/bots-e2e-case-catalog.md:5-24; doc). | "Wait" stands. Add A-09's stub gate as a supplement. | no | Z8 |
| **D11** SQLite (R:975) | Native artifacts are vendored as tracked archives pinned by SHA-256, with provenance and a single source (ZCode:apps/zcode-cli/dependencies/README.md:9-69). | Unchanged: wait. If vendoring is chosen: a hash pin, provenance, a notice and one source. | no† | Z7† |
| **D12** Bun fallback (R:979) | One package manager, no fallback lockfile format. | Unchanged: record that (b) needs a fresh 1.3.x install. | no† | Z7† |
| **D13** Q00's GUI rows (R:983) | Launch marks T0–T5 and alternating A/B trials (ZCode:packages/desktop/src/main/desktopLaunchMarks.ts:1-17; ZCode:docs/desktop/startup-critical-path.md:183-199). | Do not hold the cut. Take the rows from A-25's marks. | no | Z6 |
| **SF-21** (R:708) | A same-origin Vite proxy (ZCode:packages/web/vite.config.ts:56-70), in front of a server that is unauthenticated by default (audit P1). | SF-21 stands. A-21's proxy alternative only with its trade-off stated. | no | Z6 |
| **SF-2** (R:335) | Manually created pipelines are first-class (ZCode:docs/ci-build-platform-selection.md:24, :35; doc). | A-17. | no† | Z7† |
| **MF-6** (R:279) | A serial fork pool for spawn-heavy tests, and tooling tests proving that a tool failure is never read as success (ZCode:scripts/test/architecture-governance.test.mjs:142). | As amended for the cut-day run; nextest later. | no† | Z7† |
| **DU-25 (p4)**, Appendix B #4 and #42 (P:1223, P:1270) | A per-server mode, a bounded probe and a typed failure (ZCode:docs/mcp-dual-era-version-negotiation.md:21-116). | Amend P19 (A-02, A-03), and answer #42 from the row (A-06). | yes | Z5 |
| **DU-31 (a) / Q08 under DU-33 (a)** (P:605, P:617) | Primary platforms run automatically; deferred ones are manual and non-blocking (doc). | A macOS workspace job as an option (A-23). Writing the Windows and Linux legs reopens DU-33 (a) and is presented separately. | yes | Z7 |
| **DU-D3** Python floor (M:329) | The runtime is pinned and checked in CI. | Conditional on A-12: pin the interpreter. | yes† | Z7† |
| **DU-R3 / M65** (M:898) | Inline lint disables count as violations; the baseline changes only explicitly (ZCode:scripts/architecture/index.mjs:165-178). | Keep (b)+(d), and present A-37's two lints. | yes† | Z7† |
| **DU-D2 / M67** | knip run by hand only, with inline tags (ZCode:knip.json:3-5). | A note; do not reopen. Use inline tags (A-38). | no† | Z7† |
| **DU-D1 / M66** | oxlint defaults plus `max-lines`; no type-aware rules. | Unchanged. Keep one oxlint version in the repository. | no† | Z7† |
| **DU-21 / P34** | No advisory scanning; licensing instead. | P34 as planned. Licensing is a separate option (A-08). | no† | Z7† |
| **DU-15 / P35** | No update bot. | Unchanged. | no† | Z7† |

**Counts.**
- ZCode informs **73 decisions**: 34 rows in §5.1, 5 in §5.2, 10 in §5.3, 24 in §5.4.
- **22 change a recommendation:** T35, T6(a), T7, T13, T18, T22, T24, stop drain, OQ-2, OQ-8, id-collision, Q1, telemetry, resolve_skill_key's fix, (c)-3, S4b, MD-4, D5, DU-25 (p4), DU-31/Q08, DU-D3, DU-R3.
- **8 of the 22 give a recommendation where the source recorded none:** T35, stop drain, OQ-2, OQ-8, id-collision, Q1, telemetry, MD-4.
- **3 of the 22 rest only on unverified answers:** resolve_skill_key's fix, DU-D3, DU-R3.
- **17 rows are marked †.**

### 5.5 Decisions ZCode does not inform

| decision | why ZCode says nothing |
|---|---|
| T10 | ZCode keeps no credential in the keychain. |
| T16, T19 | Only as halves of T22 and T9. |
| T26, T27 | No equivalent surface. |
| T2, T5, T6(b), T14, T31 (closed or moot); T36 (taken) | Not examined. |
| drafts | ZCode's old composer kept drafts per scope; its v4 composer keeps none. Inconclusive. |
| `agent.config` | No counterpart. |
| eq-014 | ZCode has no conversation-summary note. Its extractor is handed the manifest of existing memories, a possible mechanism for "skip stored facts", but there is no evidence on recording outcomes. |
| N9, and the three items left to the lead (prose around an answer, the 256-candidate cliff, header length bias) | ZCode's memory has no comparable lexical index or extraction parser. |
| no-key-free; router Q2, Q4, Q6–Q10; OQ-3 to OQ-7 | No keyless local provider, fallback ladder, cache-aware pricing, CLI backend or DashMap. OQ-7 is touched only through Q3's row. |
| The hidden-character items (bidi, spaces, confusables, heading, learned escape, ESC % G, the CLI renderers) | ZCode does no escaping on approval surfaces. OpenAlpaca is ahead. |
| Extension design §13 Q5–Q10, Q13, Q14 | Not examined; Q11 and Q12 are covered under T8 and T9. |
| B1, B2 | Branch and fresh-install gates. B3 is touched through A-41. |

---

## 6. What ZCode does that OpenAlpaca deliberately does not

These are options for the owner, never defects. Each names the OpenAlpaca rule that excludes it today; adopting any of them is a reversal the owner would make explicitly.

**Memory, skills and governance**

| ZCode does | excluded by |
|---|---|
| Memory recall chosen by an auxiliary model reading a manifest (ZCode:apps/zcode-cli/packages/core/src/memory/recall/selector.ts:14-25); extraction as an agent loop (memory/extraction.ts:45-80). Z8-09. | MD-1: memory search is lexical, with no model on any memory path. |
| Skills loaded into the running loop, carrying no tool grants and no permission policy (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/skill.ts:17-127). | The skill-isolation design (P1-1, Routing V2): a skill is a sandboxed sub-loop with declared tools, its own confirmation list, scripts and cron. |
| A per-skill enable toggle keyed by path (ZCode:apps/zcode-cli/packages/adapters/src/config/schema.ts:548-561). Z3-14. | The two-axis rule (ALLOW per agent, ENABLE per extension; no third axis). `invoke.mode: disabled` is the switch (`apps/openalpacad/src/routes/skills.rs:7-13`). |
| Per-tool deny lists and project allow/ask/deny rulesets, matched by name and content (ZCode:apps/zcode-cli/packages/core/src/permission/service.ts:166-222). T1. | The two-axis rule and C8's purge of the per-tool deny key. |
| Session-wide permission modes (build, plan, yolo, guarded), with yolo and guarded allowing **before** the deny list is read (service.ts:152-166). | Per-agent capabilities; no global bypass; the approval rules. |

**Tools, approvals and the shell**

| ZCode does | excluded by |
|---|---|
| Auto-allowing a read-only shell command checked by AST against safe-flag tables (ZCode:apps/zcode-cli/packages/core/src/tool/handlers/bash.ts:128). Z1-20. | ST-14: every shell command asks. |
| Tool paths not confined to the workspace (ZCode:apps/zcode-cli/packages/core/src/tool/path-policy.ts:36-39). | The storage design's R31/R32 confinement. This must not be relaxed toward ZCode. |
| Backgrounding a shell command at its budget instead of killing it (ZCode:apps/zcode-cli/docs/design/v2/tool/04-bash.md:223-260). Z1-18. | R45: kill with the process group at the timeout. A later option, with a design. |
| Auto-continuing an unanswered question with an empty answer after 5 minutes (ZCode:docs/ask-user-question-auto-resolution.md:5-7). | A confirmation timeout is a denial (round 10's T1). |
| A run-wide or project-wide persisted allow (ZCode:docs/permission-project-approval.md:62-80). | R43: one tool, one subagent. Presented only as an explicit reversal (§5.3). |

**Plugins and extensions** (Z2-13)

| ZCode does | excluded by |
|---|---|
| Install enables a plugin by default (ZCode:apps/zcode-cli/packages/bootstrap/src/plugins.ts:746-753). | ADR-030: installing grants nothing, and approve is the single act that starts a plugin (`crates/openalpaca_plugins/src/manager.rs:1950-2005`). |
| Third-party plugin hooks run unconditionally, and hook children inherit the agent's environment (ZCode:apps/zcode-cli/packages/adapters/src/plugins/index.ts:371-376). | OpenAlpaca has no hooks. A deny-capable hook would be a new enforcement point (Z2-14). |
| Sensitive plugin config written in plain text (plugins.ts:929-947). | D1 and T9's `secret_encrypted` (`crates/openalpaca_plugins/src/permission_gate.rs:43-49`). |
| A git-tracked workspace config that can enable plugins and inject secrets (ZCode:docs/plugins/workspace-level-plugin-architecture.md:24-27). | ST-1 and ST-9: nothing is read from a project folder. |
| Uninstall deletes the plugin's data by default (plugins.ts:782-784). | Extension design §1.3 rule 3: an uninstall moves the tree to trash and keeps the data (`apps/openalpacad/src/routes/extensions.rs:76-82`). |
| Project-scope MCP servers trusted and auto-connected (ZCode:apps/zcode-cli/packages/bootstrap/src/zcode-protocol/mcp.ts:74-75). | T8's recorded answer: such a declaration enters unapproved. |
| UI plugins as MCP Apps (ZCode:UI_PLUGIN.en.md:296-363). Z2-15. | MCP resources are not implemented, and serving MCP is a non-goal. |

**Subagents and workflows** (Z4-17)

| ZCode does | excluded by |
|---|---|
| No cost budget anywhere (ZCode:docs/dynamic-workflow.md:215-218, :237-238). | N4: caps per turn ($1) and per workflow. |
| A subagent inherits bypassPermissions, acceptEdits or auto mode and cannot lower it (ZCode:apps/zcode-cli/docs/design/v2/tool/07-subagent.md:298). A run with no recorded mode falls back to YOLO (ZCode:docs/dynamic-workflow/launch.md:1043-1045). | The approval rules: an unattended turn refuses (M6), and there are no silent permissive defaults. |
| Subagents may never delegate (07-subagent.md:164). | OpenAlpaca allows delegation to depth 3 (`crates/openalpaca_core/src/runner/lead_agent/tools.rs:71`). |
| A confirmation window on every workflow start (dynamic-workflow.md:134-137). | Routing V2: chat by default; chat against workflow is the model's own tool choice. |

**Desktop, storage and credentials**

| ZCode does | excluded by |
|---|---|
| A fat Electron shell; agents die with the app; no CSP on the main renderer; a preload bridge of about 1,000 lines (ZCode:packages/desktop/src/main/desktopWindowChrome.ts:604-605; ZCode:packages/desktop/src/preload/index.ts). Z6-15. | The thin shell (G1), a daemon that outlives the window (plan §7.1, §7.8), a strict CSP (`apps/openalpaca-gui/src-tauri/tauri.conf.json:24`) and M71–M75. |
| An overlay title bar (ZCode:packages/desktop/src/main/desktopWindowChrome.ts:139-148). | (c)-5, DESIGN_SPEC §3.1: the decorated window. |
| A server process per session, with the cwd resolved against the workspace (ZCode:apps/zcode-cli/packages/adapters/src/mcp/pool.ts:514, :536-540; index.ts:1918-1923). Z5-18. | The storage design's Stage 2: the daemon's directory is MCP servers' working directory. |
| Close-to-hide on macOS (ZCode:packages/desktop/src/main/desktopDarwinCloseBehavior.ts:7-31). Z6-12. | Tauri's default and today's tested quit/relaunch. An option after Q10. |
| A "data storage path" setting that moves only part of the data (ZCode:docs/settings-storage-management.md:16-39). | D1: one root. |
| A credential cipher whose fallback key is derived from platform, home and username (ZCode:packages/services/src/credential/providers/credentialCipherProvider.ts:20-39). | OpenAlpaca's random `state/.master_key`. |

**Toolchain and CI**

| ZCode does | excluded by |
|---|---|
| mise, and pins that float across files (ZCode:.nvmrc:1; ZCode:package.json:111-113). Z7-13. | Single-source pins that CI reads (`rust-toolchain.toml`, `packageManager`). |
| A default pre-push hook running lint, the architecture check and affected tests (ZCode:.husky/pre-push:1-20). Z7-17. | The full suite takes about 5.5 minutes and agent sessions commit often. At most an opt-in format hook. |

**Where ZCode is behind, and nothing is to be taken.**
- No release-age rule and no advisory scanning. OpenAlpaca has `bunfig.toml`'s minimum release age and P34.
- No hidden-character escaping on approval surfaces; ZCode strips bidi and zero-width characters only from memory file names (ZCode:apps/zcode-cli/packages/core/src/memory/memory-file-path.ts:82). OpenAlpaca has the approval escape set.

---

## 7. Not covered

**Not established by this audit**
- **ZCode's CI configuration** is absent from the export (no `.gitlab-ci.yml`, `.gitlab/ci/` or `scripts/ci/`). Every CI claim is doc-only. ZCode:vitest.config.ts:100 lists a guard test whose file is missing.
- **ZCode's MCP SDK** (@modelcontextprotocol/client 2.0) is not vendored. Whether it opens `subscriptions/listen` for 2026-07-28 servers is unknown; ZCode's own code makes no such call.
- **Read from docs or by function shape, not line by line:**
  - the background-bash lifecycle (the code exists, `background-tasks.ts`);
  - the guarded matcher's per-rule internals (`guarded/*.ts`);
  - the marketplace dependency resolver and GitHub-archive materialiser;
  - the workspace hook trust specs under `docs/specs/`;
  - the onboarding import wizard's steps;
  - ZCode's bots' owner-only enforcement, beyond `withAuthorizedContext` and `groupSessions.ts`.
- **Unverified on the versions that would be taken:**
  - Bun's default trusted-dependencies list, and whether an explicit `trustedDependencies` replaces it;
  - `bun pm untrusted` on Bun 1.4.3;
  - cargo-deny's `[bans]` `wrappers` syntax;
  - nextest's leak detection and test groups;
  - bsdtar's flags for deterministic archives;
  - which tool generates Rust notices;
  - the providers' documented tool-name alphabets (Z5-05);
  - whether rmcp's SSE reconnect resumes a body cut by reqwest's total timeout on real servers (Z5-03);
  - Rust's default minos for aarch64-apple-darwin (Z7-11).
- **Reasoned, not measured:**
  - the launchd PATH of a Finder-launched app (Z6-02);
  - the blank window after a path-link click (Z6-03);
  - the WebContent termination behaviour (Z6-05);
  - the BOM, FIFO and link behaviour of the skill scan (Z3-05, Z3-10);
  - the daemon's real-store boot time with MCP servers configured (Q00's 73 ms came from a fixture with none).
- **Pending answers without a verifier pass:** twenty-five (§5 †).

**Areas left out**
- **Out of scope:**
  - ZCode's updater and release server, gray release, deep links and protocol registration, OAuth for MCP (`oauth*.ts`) and plugin-bundled MCP OAuth;
  - the computer-use broker (only "authority is never derived from a tool or server name" was noted);
  - worktree isolation, forked subagents, agent teams and remote agents;
  - plan mode beyond its approval boundary, and automations beyond the creation limit;
  - WebFetch's egress guard beyond M10, Read's image and PDF paths, `apply_patch`, Edit's unicode-escape matching;
  - the Windows and Linux desktop work and process-tree behaviour (deferred by DU-33);
  - ZCode's e2e suites, telemetry stack and settings information architecture;
  - remote skill sync; the content of ZCode's bundled skills;
  - Gen UI and the plugin UI pointer-event spec;
  - the plugin creator and the plugin Docker evaluation;
  - importing external agents' plugin directories.
- **Approval "arming"** (pending-decisions §1.10): ZCode's dialog resets per request id and has no arming delay. The evidence is too weak for a recommendation beyond the alternative listed in §5.1.
- **One observation in passing that is not a ZCode pattern:** plugin children are spawned with `kill_on_drop` only and are not process-group leaders (`crates/openalpaca_plugins/src/process_pool.rs:36-44`). P19 makes MCP stdio children group leaders, and R45 kills shell process groups, but a plugin's grandchildren survive a disable. This is worth a row beside P19.
- **Nothing was installed, built, run, fetched or launched** in either repository.
