# Pending decisions and limitations before a beta

**rev 2, 2026-10-12** (§4.3 updated: the upgrade's decisions taken, five owner questions open); **rev 1, 2026-10-11** — branch feat/markdown-memory at 7295f91b (pushed). A record, not a plan: nothing here is adopted; each decision's current behaviour is what ships.

This file gathers in one place what waits on the owner and what a beta tester will meet. It restates its sources and does
not replace them: each row cites the document that holds the full text, and that document stays authoritative. To close
an item, record the owner's answer here and in its source, with the commit that applies it, and bump this file's rev and
revision log. "Ships today" is the behaviour of the tree at 7295f91b. A recommendation is the source's, never applied;
"none recorded" means the source records none. Line numbers are as of 7295f91b and drift.

Short forms: *plan* = `tasks/api-fix-plan.md`; *ext design* = `tasks/extension-enable-design.md`; *lessons* =
`tasks/research/claude-code-design-lessons.md`; *storage design* = `tasks/storage-design/design.md`; *09*, *10*, *11*, *12*,
*03*, *08* = the files of `tasks/memory-redesign/` with those numbers; *hidden-chars* = `tasks/approval-hidden-chars.md`;
*deps plan* = `tasks/deps-upgrade/plan-rev3.md`; *code audit* = `tasks/code-audit-2026-07-19.md`.

## 1. Owner decisions pending

### 1.1 Tool governance and extensions

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| T1 | A per-tool deny rule set (`[security.permissions] deny = [...]`, tool names and `<server>__*` globs, enforced in the gate on every surface, subagents and builtins included), apart from the per-server and per-plugin toggle? | No rule set. C8 purged the old per-tool deny key; a hand-edited `daemon.toml` still carrying it gets one boot WARN. The per-agent ALLOW axis is the only way to narrow tools. | No. Also record the rejection as ext design §11.2 finding (4), which is not written yet. | plan §0 T1; ext design §13 Q5; lessons §6 |
| T3 | Defer extension tool schemas, a third axis LOADED beside ALLOW and ENABLE (names in the prompt, each schema loaded on request)? | Every tool of an `Enabled` extension is on the main loop's and the lead's surface from the start, in both `tool_selection` modes; the token estimate is from real bytes (A4). No loader, no key. | Record the rule, build nothing: load upfront while it fits in 10 % of the window, build the loader once that is seen crossed; any always-load override is per install unit, never per tool. | plan §0 T3; ext design §13 Q6 |
| T4 | A per-turn `<extension_status>` block naming extensions the owner left on that are failing (`Failed`, `Unapproved`, `ConfigInvalid`), reversing ext design §7.5's "log + WS only"? Moving slot or persisted `context_block`? | §7.5 as written: the loss reaches the log and WS events; the refusal in the tool result is the only channel to the model. | Yes, moving-slot shape first; it reverses a §7.5 row, so the owner ratifies. | plan §0 T4; ext design §13 Q7 |
| T6(a) | Keep respawning a crashed stdio MCP child, or adopt Claude Code's rule of never auto-reconnecting a local process (first failure is `Crashed`; only HTTP servers get a bounded reconnect)? | Transparent respawn (X-6, ext design §3.6 item 1): `ReconnectExhausted` on the fourth consecutive failed entry, so a child can be respawned three times before anyone learns it crashed. | Keep respawn: on an unattended daemon it is cheaper than a human retry. | ext design §13 Q8 (twin named in plan §0, no row of its own); lessons §6 T6(a) |
| T6(c) | Should the `mcp.toml` watcher `reload` an `Enabled` server whose declaration block changed with its bit untouched? Claude Code does not, so a yes goes beyond the reference. | The watcher applies the bit only and logs "declaration changed; reload to apply"; the change takes effect at the next `reload` or `enable` (ext design §10 case 15, §3.4.1). | Yes. The fingerprint is value-masked, so no credential value drives a teardown. | ext design §13 Q9; lessons §6 T6(c) |
| T7 | On a mid-session 401/403 from an HTTP MCP server, re-resolve the env credential once, reconnect and retry the call once before classifying `Failed{NeedsAuthorization}`? | Classified at once as the non-retriable `McpError::Unauthorized` (`crates/openalpaca_mcp/src/error.rs:29`); a rotated `bearer_env`/`api_key_env` takes effect only at `reload` or `enable`. | Yes: one re-resolve and one retry, never a ladder or a poller. | ext design §13 Q10, §3.6 item 1; lessons §6 T7 |
| T8 | May a project-scope MCP declaration, if one is ever read, enter as `Unapproved{NeverSeen}`, extending a plugin-only reason to MCP? Its premise, plan §1.2's `<project>/.openalpaca/config/`, is superseded by ST-1 and ST-9, so it is hypothetical today; no source marks it moot. | One home-store, owner-authored `mcp.toml`; MCP servers have no consent gate; no project-scope declaration is read. | Yes: a reason on an existing state, consent kept in the home store, no `resync` verb. | ext design §13 Q11, §3.3 E1; plan §1.2 (P-8); lessons §6 T8 |
| T15 | Is a fixed named ambient allow set, appended by the policy constructor to every fail-closed allowlist, acceptable against "allow is per-agent"? And may `read_result` join `{workspace_read, workspace_write}`? | The constructor appends the two. `read_result` is a grant (all nine shipped templates name it); an agent whose template omits it is refused and gets a spill stub it cannot follow. | Yes: `{workspace_read, workspace_write, read_result}`, beaten by `denied_capabilities`, never widened without a design revision. | plan §0 T15; ext design §13 Q14 |
| T35 | Should an extension tool that does not declare itself harmless ask for approval? | It asks only when its `destructive_hint` is true; no annotation counts as harmless (`effective_confirmation_set`, `security/sandbox/mod.rs:765`, `unwrap_or(false)` at `:784`). Plugin tools register with no annotations and never ask; an MCP tool whose server sends no hint never asks, unattended turns included. | None recommended; three options recorded: no change, MCP tools only, MCP and plugin tools. Under a yes an unattended turn using such a tool is refused. The answer decides B3's second line (§2). | plan §0 T35; storage design §0.3, §18 |

### 1.2 Credentials and security

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| T9 | Default store for plugin config values marked `sensitive`: `secret_encrypted` (in the root, under `state/.master_key`) or `secret_ref` (the OS keychain)? | Both exist and the caller must choose (`crates/openalpaca_plugins/src/permission_gate.rs:488`); `set_plugin_secret` has no production caller and a sensitive key written through the config route is refused, so only a hand-written `{ secret_ref = … }` plus `ext reload` works. | `secret_encrypted`, which keeps D1's single root. Decide T19 with it; choosing `secret_ref` makes T10's root scoping mandatory. | plan §0 T9, §12; ext design §13 Q12 |
| T10 | May a second `OPENALPACA_HOME_STORE` root (a test root) share the real keychain secrets? | Keychain accounts are `llm/<provider>/<uuid>` under service `OpenAlpaca`, not root-scoped; `install_id` is in the home `.layout` but names no account, so a second root naming the same `secret_ref` reads the real secret. | No: name accounts `openalpaca/<install_id>/<provider>/<key_id>`, so a second root starts with no credentials. | plan §0 T10, pre-check (c) |
| T19 | `[web_search] api_key` is written as plaintext into `config/llm.toml` (`PUT /v1/daemon/config/providers/web-search`) and copied into the five rotating backups under `state/backups/`. Bring it under the providers' secret resolution order? | Plaintext; nothing exports it; rotating a leaked key means editing `llm.toml` and its backups. | `api_key_env` / `secret_ref` / `secret_encrypted` in that order, so the daemon stores a reference; decide with T9. | plan §0 T19 |
| T21 | `discovery.json`'s `auth.expires_at` (issued + 24 h) is enforced only by the clients. Give the expiry to the daemon, or drop it? | The CLI refuses every call past it (`apps/openalpaca/src/client.rs:24`); the daemon compares the token only (`apps/openalpacad/src/middleware.rs:28`); the GUI prefers liveness. A daemon up for more than a day is reachable from the GUI, not the CLI, until `daemon restart`. | Rotate `discovery.json` (token and `expires_at`) on a schedule while the daemon runs. | plan §0 T21 |

### 1.3 Tasks, lanes and routes

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| T11 | Reserve `/new` on connector lanes as a deterministic-tier op mapped to `POST /v1/sessions`? N1 is not reopened. The row's "if yes" names `session.summary` / `last_summarized_message_id`, which migration 002 replaced with the working state. | Each Telegram, iMessage or Discord lane has one perpetual session; `idle_archive_hours` reserved and off; no `/new`. | Reserve the name and keep N1; revisit N1 only if a connector lane's `messages_7d` outgrows the prompt window. | plan §0 T11, §10 |
| T16 | Scope `GET /v1/tasks`, `GET /v1/tasks/{id}`, `…/timeline` and the cancel/pause/resume actions to the local user, as steer is? | Unscoped; `start`, the replay `resume` and steer go through `owned_run` (`apps/openalpacad/src/routes/tasks.rs:638`). | Leave unscoped until a multi-user lane flow exists (a steer injects text, a read or cancel does not); decide T22 with it. | plan §0 T16 |
| T18 | The CLI and the GUI share `{owner}:gui`, and a CLI turn carries its cwd as the project, so under R48 a one-shot `openalpaca chat` from another checkout archives the GUI's conversation. Since Stage 0's F4 a lead-queued follow-up carries its run's root but is pinned to the lane's active session when queued. Keep? | As landed (`repository/followup/mod.rs:111-118`); the CLI manual says a resumed conversation's own project governs the turn. | Keep and document; if archiving proves disruptive, pin the GUI's session on its own turns. For later: pin a lead-queued follow-up to `task.session_id`, as `recover_unprocessed_steering` does (`:265-268`). | plan §0 T18; storage design §18 Stage 0 item 3 |
| T22 | `GET /v1/chat/confirmations` is unscoped and copies `tool_arguments` verbatim; since L4 a `memory_confirm` row also carries the suggestion's `preview` (note titles and excerpts). Scope it, or redact rows the caller does not own? | Unscoped with arguments (`list_confirmations`, `apps/openalpacad/src/routes/chat.rs:902`; the arguments copied at `:929`); `event_log` keeps ids only, but the live `/v1/events` frame has always carried the arguments to every client. | Scope it, decided with T16; document until then. | plan §0 T22; 09 §10 "Noted, not asked" |

### 1.4 Costs and retention

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| T12 | Should `[orchestrator.sessions] log_retention_days` default to 0 or 90, and should a `persist` opt-out be reserved? | 0 and inert: parsed, clamped, served and warned about, but no age sweep exists (`crates/openalpaca_core/src/daemon_config/mod.rs:112-131`); plan §5.4's "the sweep machinery ships regardless" was not built. Only the two byte caps sweep. | 90 ("age is for exposure, size is for disk"), and reserve `persist` as a direction only. | plan §0 T12, §5.4 |
| T13 | Charge subagent spend to the lead's `max_cost` accumulator so "per-workflow" bounds the whole workflow? A new enforcement point beyond N4's relabel; pre-check (d), which would measure today's split, was never run (no provider keys). | The lead's cap ($5 default; the shipped `lead_agent` sets $3) bounds the lead's own calls; subagent spend shows in `get_task_usage` and is not counted; each subagent keeps its own $1 turn cap. | Lessons: yes, with the double-count rule written down; N4 as settled is relabel-only, so nothing ships until the owner decides. If no, relabel `workflow_max_cost_usd` "lead agent spend". | plan §0 T13, N4, pre-check (d) |

### 1.5 Files and storage

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| T17 | A retention policy for `plugins/.trash/<name>-<ts>/`? | Uninstall moves the plugin there (`crates/openalpaca_plugins/src/install.rs`, `fn trash`) and nothing purges it; the CLI manual says empty it by hand. | No auto-purge by default; an age sweep or a `store purge` target is the owner's call. | plan §0 T17 |
| T37 | On a turn with no project, offer `file_read` and `file_list` on every turn, as on a project turn? Live: "Call the file_read tool with path note.txt" got no reader; "Read file note.txt" did. | Held only when the message says `read file`, `open file`, `show file` or `cat ` (`orchestrator/intent/tool_suggest.rs`; R42, R-T6); the working-folder line names only tools held (R-T22), so the model usually cannot read what its own commands wrote in `scratch/<id>/`. | Yes (recommended, not adopted): both act only in the conversation's scratch folder and never ask (ST-16); `offer_file_readers` on every turn with a working folder, with a test. | plan §0 T37; storage design §18 Stage 2 item 28; `stage2-live-report.md` (h) |
| agent.config | Drop the `agent.config` column, orphaned when the removals pass deleted `AgentRepository`? It needs a migration, now 005. | The column stays (`001_baseline.sql:90`) and nothing reads it. | None recorded ("so it stays"). | CLAUDE.md, removals-pass paragraph; `tasks/todo.md:432` |

### 1.6 Local model and first run

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| T20 | Should a first boot enable `[providers.ollama]` by itself when Ollama answers on `base_url` and no provider holds a key? | The seeded template ships `enabled = false`; with a running Ollama and no key the first turn fails `NoRoutableModel` until the owner flips one bit; the first-run card points to Settings → Models & keys. | Keep `enabled = false`: enabling a provider is consent, and the cost is one documented action. | plan §0 T20 |
| T24 | Offer the OAuth tokens found from Claude Code (`~/.claude/.credentials.json` or the Keychain item) and Codex (`~/.codex/auth.json`) on the GUI's first-run surface? | `GET /v1/settings/llm/credentials` and `…/rescan` exist and the GUI wraps them, but nothing calls them; metadata only. | Not offered: it would show on screen that the machine is signed in elsewhere and use a credential minted for another product; it needs its own design with its own consent step. | plan §0 T24 |

### 1.7 GUI and CLI ergonomics

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| T25 | A new filled `dangerSm` button for `Stop daemon`'s confirm, or the existing `dangerGhost`? | `dangerGhost` (`1d08365d`). | Reuse `dangerGhost`; a filled danger style the design never drew is a design change. | plan §0 T25 |
| T26 | Should `openalpaca config reset --factory` ever take `--yes` / `--force`? | No flag; the typed word `factory-reset` at a terminal, so no GUI button or installer can script it (`73a0698d`). | Never, for now; a surface that needs it gets its own route. | plan §0 T26 |
| T27 | Should `openalpaca config list` fall back to a partial listing when the database is refused? | It refuses and names the rescue verb (`73a0698d`). | Keep the refusal: a listing silently missing the database third would be a lie. | plan §0 T27 |
| T32 | Post a farewell to the connector lanes before the daemon shuts down? | Silence (`1d08365d`); the stop dialog says connector users get no reply until the daemon starts again. | Keep the silence: a farewell spends the 10 s force-exit window and only works on the graceful path. | plan §0 T32 |
| T33 | Require a typed word on the GUI's `Stop daemon`, matching D-B's `factory-reset`? | A confirmation dialog with no typed word (`1d08365d`). | No: a stop destroys a process and the turn in flight, not data. | plan §0 T33 |
| drafts | Per-conversation composer drafts? F6 cleared the attachment chips on a conversation change; the draft text was left window-level on purpose. | One draft per window, carried across conversations. | None recorded ("per-conversation drafts are a decision, not a fix"). | CLAUDE.md, PR #31 follow-ups (F6) |

### 1.8 Memory and learning

Every numbered memory question (MD-1…MD-16, LQ1–LQ21, CQ-1…CQ-11, G2/G30/G38) is taken. What is left:

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| eq-014 | Should the conversation-summary (episode) prompt record a conversation's outcome and skip facts already stored as notes? Every measured run so far missed the outcome eq-014 asks about (cs-018): L8's first run kept the stored facts and dropped it, and 12's summary recorded the quiz as the assistant's rounds. | Since CQ-11 the episode note is written by its own call over the pre-slice-4 `episode_prompt.txt`, restored byte for byte and pinned by `the_episode_prompt_matches_its_fixture`. | None recorded ("the owner's; none is implemented"). | 10 §5, §9; 12 §5; 11 rev 1.9 item 1 |
| N9 | Trim the long gold answers of the episode split (`scripts/eval/memory/questions-episodes.json`), the §7.1 corpus review's N9? Evaluation only. | The golds stand as the rev 3.21 corpus review left them (N9 not applied); eq-017's gold was corrected separately (`c6db079f`). Beside it, 12 §6.4 (still open): the self-grading judge failed eq-020 for naming more of the conversation than the gold does, and no gold was rewritten. | None recorded. A change makes later episode scores incomparable with earlier ones. | 09 rev 3.21; 10 §5, §9; 12 §6.4 |
| stop drain | Keep learning's 3 s stop drain, or rely on the boot catch-up alone (a review's proposal: the drain rarely finishes on a local model)? Noted in 09 §10, never raised as a numbered question. | The drain stays: learning runs in a 3 s drain at stop and a catch-up at boot. | None recorded; removing it narrows MD-10's "before the daemon stops" trigger, so it is the owner's. | 09 §10 "Noted, not asked"; 03 §0 MD-10 |

Left to the lead, not the owner (recorded as measured; listed so they are not mistaken for settled):

- **Prose around an extraction answer.** `task_extraction::read_json_answer` takes the last value of the expected shape found in prose, so a complete quoted `{"items": [...]}` followed by a final non-items object is still taken. Yielding only the last value would close it. "Not decided in this batch; the reading is unchanged." (09 rev 3.34; 10 §9)
- **The 256-candidate cliff.** A word held whole in more than 256 eligible note bodies reaches the candidates only through the prefix query's 256, which can all be longer forms (`login` for `log`), losing every note that holds the word itself; no bench or proxy query shows it. A ranked body query beside the header query closes it at +11 ms p95 at 30K, +34 ms at 100K. (09 §6.5.4, §6.5.8; 10 §7.4, §9)
- **Header relevance's length bias.** It shrinks with a header's length; at 100K learned notes three identity questions (hq-0001, -0005, -0039) fall outside the 50 hits. Five variants were measured and each lost 1–4 proxy rows. (09 §6.5.5, §6.5.8; 10 §7.4, §9)

### 1.9 LLM router and usage accounting

The two questions of the paused "providers as a table" design (§4.1), then the router and call-side owner questions its
surveys listed. The surveys were taken at `7561f152` (2026-09-23); only Q3, Q4, Q5 and Q10 were re-checked at 7295f91b.

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| no-key-free | Should a local server given an API key (vLLM `--api-key`) still count as free, one "no key means free" rule or a separate local flag? | Design paused; only Ollama is keyless and priced 0. | One rule: such a server reads as unpriced until its model prices are set to 0. | `tasks/llm-simplify/design.md` §1 |
| id-collision | One model id served by two providers (OpenRouter's `openai/gpt-4o`; one local model on LM Studio and Ollama), while the registry maps an id to one provider? | An id maps to one provider. | None recorded (section 3, not yet presented). | `tasks/llm-simplify/design.md`, sections still to present, item 3 |
| Q1 | Book the routed model id rather than the provider's echoed one on the non-streaming path? | The echoed id is booked. | None recorded. | `tasks/llm-simplify/map-router.md` §7 |
| Q2 | Should core budgets account for the template's `fallback_models`? | They ignore it. | None recorded. | map-router §7 |
| Q3 | Give the streaming path the circuit breaker, the per-key RPM/TPM/concurrency limiter, success reporting, backoff and `EmptyCompletion`? Every main-loop turn streams. | Streaming skips all of them; `check_circuit` runs only on the non-streaming retry path (`router/retry.rs:18`, re-checked). | None recorded; code audit M3: funnel streaming through the same acquire/report path. | map-router §7; code audit M3 |
| Q4 | Refuse requests while the breaker is HalfOpen? | `retry.rs:18` refuses `Err(Open)` only (re-checked). | None recorded. | map-router §7 |
| Q5 | One breaker per provider (ideally per key) instead of one global one? | One global breaker (`rate_limiter/mod.rs:458-462`, re-checked): five failures from a dead Ollama open it for Anthropic too. | None recorded; code audit M5: per provider. | map-router §7; code audit M5 |
| Q6 | Should Ollama's keyless slot be rate-limited like a cloud key? | 50 RPM / 40,000 TPM / 5 concurrent. | None recorded. | map-router §7 |
| Q7 | Fallback after a substitution: keyed on the substituted model, or the requested one? | Keyed on the substituted model. | None recorded. | map-router §7 |
| Q8 | Should a CLI backend block without `timeout_secs` honour `[timeouts] cli_backend_timeout_secs`? | It uses 120 s and ignores the key. | None recorded. | map-router §7 |
| Q9 | Should core callers price with the cache-aware cost? | They use non-cache `calculate_cost`. | None recorded. | map-router §7 |
| Q10 | `list_models_for_provider` holds a `DashMap` `Ref` across `.await` (RT-13, the shape R59 forbids): fix it? | Still so (`routing/router/capacity.rs:242-246`, re-checked): a disable during key validation waits for the network. | Use `provider_entry()` (`router/mod.rs:234`); "a timing fix, not a cleanup". | map-router §6 RT-13, §7 |
| OQ-1 | What cost a usage row carries. | Interactive rows reprice at the last model without the cache discount; dispatcher rows use the tracker's cumulative total, over-reporting for reused ids (`lead_agent`, `skill:<id>`, `orchestrator`). The per-run budget check is unaffected. | CORE-04: pass the cost explicitly now, decide the algorithm separately. | `tasks/llm-simplify/map-callside.md` §9 |
| OQ-2 | Close the coverage gaps? | Utility jobs publish no `LlmCallCompleted`; compaction and nested-skill loops write no `llm_call_log` row; a streamed empty answer is never `EmptyCompletion`. | None recorded. | map-callside §9 |
| OQ-3 | Should `model_used` / `effective_default_model` match the router's pick when `[orchestrator] fallback_models` is set and the configured default is unroutable? | As surveyed at `7561f152`, not re-checked: the daemon's prediction can differ from the model the router picks; fixing it changes `POST /v1/chat`, `GET /v1/status` and the GUI composer's seed (G9). | None recorded. | map-callside §3c, §9 |
| OQ-4 | Should window and attachment predictions use the template's `fallback_models`? | They do not. | None recorded. | map-callside §9 |
| OQ-5 | One policy for `ContextBudgetComputed.model`? | As surveyed at `7561f152`, not re-checked: three policies — the main loop its configured model or `"default"`, the skill tier the router's default (its window is the answering model's), the lead the routed model. | None recorded. | map-callside §3e, §9 |
| OQ-6 | Which model a usage row names when none answered? | As surveyed at `7561f152`, not re-checked: the interactive writers name the router's configured default, possibly `""` or unroutable, not the turn's override; the lead its template pin; a subagent its access-checked model. | None recorded. | map-callside §3d, §9 |
| OQ-7 | Short-circuit `NoRoutableModel` on the streaming path? | Not short-circuited: it is retried on the non-streaming path (a log-only difference). | None recorded. | map-callside §9 |
| OQ-8 | Should nested skills honour the turn's model pin? | They ignore it. | None recorded. | map-callside §9 |

### 1.10 Approval surfaces (hidden characters)

Each is left to the owner in `tasks/approval-hidden-chars.md`: most in its words "an owner decision, not taken", ESC % G
as "left for the owner" and the CLI renderers as "its own task if the owner wants"; several sit inside its
accepted-residue blocks, where the residue itself is not to be re-raised.

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| bidi | Force logical order (LRO…PDF in plain text, `unicode-bidi: bidi-override` in the GUI) inside a value? | Right-to-left letters reorder the digits and neutrals next to them on every approval surface; item 8's isolate protects only the text around the block. | None recorded. | hidden-chars, accepted residue, first bullet |
| spaces | Show runs of U+0020 in a one-line field or an argument? | Not shown: a run still wraps and can fake an extra prompt line; the CLI manual says read an argument whole. | None recorded ("a rule about neighbours, not a set"). | hidden-chars, residue (round 2, S3 round 2) |
| confusables | Flag confusables and mixed scripts (a Cyrillic а for a Latin a, two tool names a homoglyph apart)? | Shown as written; the escape makes code points exact, not meaning. | None recorded. | hidden-chars, residue (round 2) |
| arming | Ignore Approve, Always allow, ↵ and the palette row for about 1 s after the answered request changes? | Not armed: an Enter meant for the composer, or a click after the queue head changed, can answer a request nobody read. | None recorded; the reviewer's fix is the option, and it adds an enforcement point to every approval and changes DESIGN_SPEC §4.5's ↵ contract. | hidden-chars, "Not taken in S3" |
| heading | A heading line in model-written suggestion content hides the rest of it from the card, the CLI count and the preview, while Replace adopts the whole body. | As described. | (a) 09 §2.4 rule 2 refuses content with a heading-like line (the implementer's pick), or (b) the wire carries a count of the rest and a flag. Either amends an accepted design row. | hidden-chars, S4 round 1 |
| learned escape | Escape and count a learned note in force (`extracted_*` origin) in the detail and in `openalpaca memory show`? | A note in force is shown as written (S4's R2), so tag characters or a bidi override in a learned note are escaped nowhere. | The implementer's: escape by origin; reverses R2 for a class of notes. | hidden-chars, S4 round 2 |
| ESC % G | Add ESC % G (UTF-8) to the CLI's terminal reset? | `TERMINAL_RESET` selects G0, not UTF-8; a terminal switched out of UTF-8 reads 0x9B as a CSI. Named in the reset's doc and the CLI manual. | None recorded ("left for the owner"). | hidden-chars, residue added in S5 |
| CLI renderers | Apply the escape rule to every CLI renderer of model or third-party text (`tasks log`, `tasks status`, the REPL `/agents`)? | Those print stored text as it comes. | None recorded; "worth its own task if the owner wants it". | hidden-chars, residue added in S2 round 1 |

### 1.11 Routing and observability

| ID | Question | Ships today | Recommendation | Source |
|---|---|---|---|---|
| batch work | Do graph-shaped jobs need more than the lead sequencing batch spawns, now the DAG executor is gone? | `spawn_subagents_batch` + `wait_for_subagents` + the lead's own sequencing; no `run_dag` tool. | If it proves insufficient, a lead-agent tool, not a resurrected executor. | Vault `11-roadmap-and-open-questions.md`; `tasks/design-routing-v2.md` §2 T5 |
| telemetry | Wire or delete the skill-router suggestions and the log-only events (`IntentClassified`, `DispatchDecision`, `OrchestrationStage`, the skill trio)? | Suggestions computed and debug-logged only (`orchestrator/intent/skill_match.rs:69-74`); the events reach `tracing::debug` only (`event_bridge.rs:276-380`). | None recorded. | `tasks/wiring-audit-2026-08-30.md` §2 |

### Closed without a decision

- **T14** — still a row of plan §0's pending table, and that row does not say moot; its question is moot in fact: GAP-22 (Phase 0 item 6) was never started and the extension design's C7 deleted the six `plugin_*` variants it would have stamped; the `Extension*` events carry `ts`/`instance_id` from birth (plan §0 T14, §6 Phase 0's *Landed* line).
- **T31** — still a row of plan §0's pending table, whose own text says "Moot since SQ-2": nothing reads, writes or sweeps `state/assets/` (plan §0 T31).
- **T2, T5, T6(b)** — applied as written by the extension design the owner accepted at rev 15 (2026-09-02): a bundled sub-unit of a plugin is a contribution, never its own row (§2.3, X-4); a disable tears down live and costs one prefix miss per lane (§6.2 #2, X-32); no automatic retry ladder for a failed server (§3.4). No T-row records an answer, and lessons §6 says nothing in it is decided; read literally, all three are pending.

Taken, for orientation only: T23, T28 (moot since SQ-2), T29, T30, T34, T36, ST-23 and FW-2 — plan §0, *Owner decisions
taken*; and the 2026-09-22 ruling that `openalpaca daemon stop` stops the daemon only — CLAUDE.md (whole-branch review
paragraph) and `tasks/todo.md:428`, not plan §0.

## 2. What a beta waits for

Which stages a beta waits for is the owner's call; Stages 0–2 are built. Three things no stage covers (storage design §21):

1. **B1 — the branch reaches `main`.** Pushed: `origin/feat/markdown-memory` is at 7295f91b (checked 2026-10-11). PR #34
   is open and still a draft. It takes PR #34 leaving draft and being merged, and the beta build made from `main`,
   never from a working tree. **The owner** pushes and merges.
2. **B2 — a fresh-install check of the packaged app**, after the last stage the beta includes, as 08 §4.1 did at
   `8fde2925`: the bundle from `scripts/release/package-macos.sh` on a clean home (`OPENALPACA_HOME_STORE` and
   `OPENALPACA_CONFIG_DIR` on a scratch directory, the daemon under `scripts/eval/memory/loopback-only.sb` from its
   first launch, the local model), walking a tester's path — the first-run card; Ollama switched on or a key added from
   Settings; a streamed chat; an upload; a workflow that writes a file and a report naming its path; the memory round
   trip; an approval card answered; Stop and Start daemon; a second start on the same store — with the evidence
   archived beside 08's and the result in the PR. Not started; the last packaged check predates Stages 0–2. It is also
   the first live check of Stage 2's app surfaces and the stale approval card (`stage2-live-report.md` (i), vitest
   only so far) and of the four memory GUI fixes 08 found (`81912aba`, `69fe47d4`, `de2c409a`, `623bf13c`; 08 §4.1,
   §6), and the GUI process has never run under the no-network profile. It does not cover the Windows or Linux
   packages (§3.13). **Claude** can run it with screen access to the app, which the owner grants.
3. **B3 — the manuals say what the beta cannot do.** One list in `docs/Installation_Manual.md`: the shell is not
   confined to the project (ST-14); extension tools never ask (T35); learning is below its own targets (10); a store
   from before 2026-09-28 is refused and the way through is the factory reset. Docs only, not started (no such list as
   of 2026-10-11). T35's answer decides its second line. **Claude** can write it; §3 below is its raw material.

## 3. Limitations a beta tester meets

### 3.1 Shell and processes

- The shell is a working directory, not a confinement: an absolute path, `~` or `cd /` reaches anywhere, and an approved command runs as the user — it can read `state/discovery.json` and answer later prompts, edit `daemon.toml` to switch prompting off, and read the key file. The approval card is the only stop. Accepted for this version. (storage design §0.1 ST-14, §11)
- A shell command's changes keep no pre-edit copy; only `file_write` and `file_edit` keep one, so a command's damage cannot be undone from the conversation's log. (§18 Stage 2 item 13)
- A command that times out or is cancelled is killed with its process group by an immediate SIGKILL (R-T23), so it may leave its own lock files, such as `.git/index.lock`; the Daemon Manual says so. The idea for later is a SIGTERM grace with a detached reaper. (§18 Stage 2 item 9)
- The kill does not reach everything: a process that leaves its group (`setsid`, a double fork) escapes; on Windows only the direct child dies; the daemon's forced exit kills nothing; a command in its own group no longer gets a terminal's Ctrl-C; a background command that reads `/dev/tty` stops until its time limit. (§18 Stage 2 item 8)

### 3.2 Files and the working folder

- Reading never asks: in a project an agent can read any file under its top folder, `.env` and other secrets included, and what it reads goes to the model. Accepted (ST-16); proven live. (storage design §0.1 ST-16; `stage2-live-report.md` (d))
- Replacing a file larger than 10 MB is refused, because its old content must be kept first. Accepted (ST-17).
- Inside a skill or a plugin agent, where no copy can be kept, `file_write` creates a new file but refuses to replace an existing one, and says why (R46).
- In a conversation with no project the model is offered the file readers only when the message says "read file", "open file", "show file" or "cat " — the open question is T37 (§1.5). (§18 Stage 2 item 28)
- Plugin agents and plugin skills get no working-folder line, so they are not told which folder the file tools and the shell act in. (§18 Stage 2 item 7)

### 3.3 Approvals

- An extension tool (MCP or plugin) asks only when it declares itself destructive; plugin tools never ask — the open question is T35 (§1.1).
- "Always allow" covers one tool for one subagent: each subagent of a workflow raises its own card for the same tool; a run-wide allow is not built (R43). (storage design §0.3, §12, §19)
- With no project, `file_write` asks for a by-product while `artifact_write` saving the same kind of file does not (ST-15's consequence; the `file_write` card seen live in (h), a by-product saved with no card in (e)). (storage design §0.5; `stage2-live-report.md` (e), (h))
- In Telegram, Discord and iMessage, `/yes` answers the chat's oldest pending prompt, keyed by chat and not by sender, so any participant of a group chat or channel can approve a tool (`crates/openalpaca_connectors/src/common/mod.rs:206`). Code audit H2 recommends binding a prompt to the person who raised it. A Discord preview line cut between a backslash and the character it escapes leaves that character bare. (§18 Stage 2 item 2; code audit H2; `tasks/patch-plan.md`, deferred)
- The GUI draws integers past 2^53 in tool arguments rounded on approval cards, while the tool receives the exact value; the CLI and the connectors print exact digits. Fixing it is its own design. (hidden-chars, "Not taken in S3")
- Accepted hidden-character residue, recorded not to be re-raised: right-to-left letters reorder neutrals inside a value; the GUI's plain-text forms (the palette's Approve row, the Always-allow toast) can wrap inside an escape; the CLI reset does not reset the palette or the scroll region; a letter a particular font draws blank (U+FA2E/U+FA2F on macOS) cannot be closed by any set. (hidden-chars, accepted residue)

### 3.4 Refusals added in Stage 2's final wave

Each enforces a rule the design already had, but a person will meet it (storage design Part II, *New refusals the final wave added*):

- **Open on a result file** answers `409 OPEN_NOT_OFFERED` (Reveal only) unless the file its row resolves to is a regular file of a kind the store keeps — a folder, or a link to a script or an app, is refused (R-T25, ST-18; `open_asset_for_user`, `routes/files_types.rs:160`).
- **Open on a by-product** is decided on its stored file: a link out of its folder is refused (`500 OPEN_FAILED`, R-T33), a kind Open does not hand to an app is `409 OPEN_NOT_OFFERED`, and a `.bin` by-product (FW-1's fallback name) is not offered Open (`ArtifactRecord::openable`; `d01c9260`).
- **The run-end notifier** delivers to a chat app nothing `send` would refuse: a result file, another project's row, or a by-product whose file is not inside its own folder in the store (R-T3, R-T20; `deliverable_path`, `notification/artifacts.rs:87`).
- **An explicit confirmation list naming `file_write`** (a template's `require_confirmation_for`, a skill's `permissions.confirm.tools`) gets a card for `file_edit` too (R-T35; `effective_confirmation_set`, `security/sandbox/mod.rs:765`).
- **A missing project's result file** answers `410 ARTIFACT_PROJECT_MISSING` on Preview, Open and Export — the folder is gone or has moved, re-point it — and Reveal refuses with the same sentence (R-T26).
- **The file tools** refuse a leaf swapped to a link after the check (`O_NOFOLLOW` on unix) and a target that is not a regular file — a named pipe, a socket, a device — before any card (R-T30, X6; `open_leaf`, `tools/builtins/helpers/mod.rs:274`).

### 3.5 Memory and learning

All figures below are the local reader model grading itself (`qwen3.8:27b-mlx`, LQ1), one sample of a live system each. Recall from saved notes passed its gate: D-index +6.6 pp over the old engine, CI [+3.7, +9.5] (10 §1).

- Learned memory answers below curated notes: 88.1 % (414/470) against 96.4 %, −8.3 pp, CI [−11.3, −5.3]; the CI misses the −10 pp target, and cross-language (−12.9 pp) and rejected or failed actions (−13.3 pp) fall below the block line. No re-target or re-measurement is recorded. (10 §1, §3.4, §8)
- Other-script aliases on learned notes (what makes cross-language recall work): median 56.1 % against a proposed 60 %; LQ18 keeps 60 % a target, not a gate. The gap sits on three replayed sessions, the build's extraction is likely not the cause, and the figures were not re-scored under LQ18. (10 §4.2, §4.8, §9; 09 §10 LQ18)
- Changed facts: 10 of 15 against 12 (`user.weekly_review` wrong in every final run). A suggestion was raised for 7 of 8 changed saved notes; the commute was missed because the reader stored the new value as a second active note beside the saved one. LQ19 answers the commute; it is built and unmeasured. (10 §4.2, §4.4, §4.5, §9)
- Scale: at the 30K design point recall p95 is 81.8 ms but a learned store takes 189.6 ms, an edit 252.2 ms and a forget 356.4 ms, with no write-path target set. At 100K recall p95 is 261.3 ms, a learned store about 3.1 s, and the 100K bench's peak RSS about 2.1 GB, a rise nobody has explained. A missing index rebuilds in the background (4.2 s at 30K, about 17 s at 100K), during which search answers from a file scan and `memory_timeline` is refused. Accepted by LQ15 (design point about 30K). (10 §7.2, §7.4, §9)
- Continuity: a new conversation answers questions about the previous one at 38/60 = 63.3 % against 80 %, +10.0 pp over carry-over off against +30 pp; `doing` + `decided` 36/40, `next` 2/20. Open findings: the writer puts the daemon's conditions (a refused approval, the workflow cap) into `next` (12 §6.2), and a line it reads cut looks unfinished (§6.3); the carry-over folding into the next conversation's state (§6.1) was answered by C1, and C1 and C2 were built after and not re-measured. The owner accepted `next` as measured on 2026-09-30 (`tasks/todo.md:112`; CLAUDE.md); 11, 12 and plan §0 do not record it. (12 §1, §3.2, §6, §7; 11 rev 1.8)
- Episode notes (conversation summaries) are unverified on this build: CQ-11 restored the call that answered 17 of 20 on the L8 build (scored before eq-017's gold was corrected), and nothing has been re-run since. (12 §5, §8; 10 §5; 11 §10.3)
- Memory search is lexical: no embedding and no model. A term inside a word of a space-separated body is not found — `book` in `runbook`, `wechsel` in `Zeitzonenwechsel`, `3.12` in `python3.12`, `4090` in `RTX4090`. Aliases are the remedy, by design (MD-1). (03 §0 MD-1; 09 §0.3, §6.5.4–§6.5.5)

### 3.6 Connectors

- iMessage runs every accepted message as the Mac owner: `resolve_owner` ignores the sender (`imessage/handler.rs:219`), a direct message from a handle not in `owner_handles` is processed once it starts with the trigger prefix (`imessage/routing.rs:132-155`), and group messages follow the prefix setting. Code audit H1, "security-critical": a per-sender principal with a linked-identity check. (code audit H1; `tasks/patch-plan.md`, deferred)
- A connector lane (`{owner}:{platform}`) mixes every DM and group on the platform and learns and recalls as an app lane does, so the owner's notes can be recalled in a group chat. Group privacy is its own unwritten design (MD-4; §4.6). (03 §0 MD-4, §3 rule 5; 09 §0.3)
- `/yes` per chat, not per sender: §3.3.
- Plugin connectors do not work: the bridge `get_plugin_connector` has no production caller and `ConnectorHandle::Plugin` is built nowhere. (CLAUDE.md, *Extensibility*)

### 3.7 Models and the LLM router

- The streaming path, which every main-loop turn takes, bypasses the circuit breaker and the per-key RPM/TPM/concurrency limiter; CLAUDE.md's Router Flow steps 5–7 describe the non-streaming path. Q3 (§1.9). (code audit M3)
- One circuit breaker serves every provider, so a dead Ollama blocks Anthropic too, and a HalfOpen breaker is not refused. Q4, Q5 (§1.9). (code audit M5)
- Per-key `allowed_models` and `monthly_budget` do not exist; they were removed, never enforced. (Vault 11; `tasks/wiring-audit-2026-08-30.md` §4)
- Plugin LLM providers do not work: `get_plugin_provider` has no production caller; D-I of the paused LLM design removes the switch with one install warning, unbuilt. (CLAUDE.md, *Extensibility*; `tasks/llm-simplify/design.md` D-I)

### 3.8 Extensions

- MCP resources and prompts are not implemented, and serving MCP is a non-goal: OpenAlpaca imports MCP tools only. (CLAUDE.md, *Extensibility*)
- Plugin-contributed skills cannot stream and cannot take attachments: the protocol returns a finished answer and takes a query string, so the turn gets one fallback delta and every attachment is recorded skipped. (CLAUDE.md, local-model rounds 7 and 9)
- The MCP HTTP transport has no redirect policy (`openalpaca_mcp` `transport/http.rs`), re-verified 2026-10-10; "recommended soon as its own fix". (deps plan §3.3)

### 3.9 Store, data and retention

- A store from before 2026-09-28 (schema 42–45) reads as newer than this build and is refused with nothing changed; the way through is `openalpaca config reset --factory`, which deletes the database and `llm.toml`'s keys. Keep a copy of `state/openalpaca.db` first, and never start a pre-squash build against the new store. (`tasks/store-revision/design.md` §0 SQ-1, §1.4, §5; storage design §21 B3)
- `event_log` grows forever: every `ServerEvent` but three is persisted, plus the sandbox's rows, and only a factory reset or a project purge removes any. (CLAUDE.md, *Gotchas*, W6)
- Session logs are bounded by size only; `log_retention_days` is inert. T12 (§1.4).

### 3.10 Security

- `web_fetch`'s SSRF check does not resolve hostnames (`validate_url`, `tools/url_validation.rs:39`, synchronous; redirects re-checked the same way), so a name resolving to a private or metadata address passes. Code audit M10: validate every resolved IP. (code audit M10)
- HEAD's locks carry 5 RustSec vulnerabilities — lopdf (a crafted PDF aborts the daemon; reachable), rustls (reachable), quick-xml ×2, RUSTSEC-2026-0194/0195 (likely reachable), rmcp (server transport, not compiled) — and 24 `bun audit` advisories; zip2's three GHSAs touch `extract()` paths the daemon does not call. The dependency wave clears them (measured 5→0, 24→0); DU-1's option (C) is a security slice first if an untrusted PDF or spreadsheet upload reaches anyone sooner. (deps plan §0 item 2, §4.1 DU-21, §4.2 DU-1)
- Also under §1: keychain secrets shared across roots (T10), the web-search key in plaintext (T19), the unscoped confirmations read (T22); and iMessage and per-chat `/yes` above.

### 3.11 Tasks and routing

- `/pause` only changes a run's status; execution and spend continue, since nothing in the agentic loop checks a pause (`orchestrator/task_ops.rs`). Code audit M16: honour a pause token or report "pause unsupported"; Routing V2 §3: a pause check at the drain seam, or document `/pause` as advisory. (code audit M16; `tasks/design-routing-v2.md` §3)
- The CLI and the GUI share one lane, so a CLI turn from another project can switch the conversation an open GUI window shows. T18 (§1.3).

### 3.12 The app

- Two surfaces stay "unavailable": adding a connector (GAP-17 — no route adds one) and the per-template enable toggle (GAP-20 — no field and no enforcement in the spawn path) (`apps/openalpaca-gui/src/lib/unavailable.ts:18`). (CLAUDE.md, plan paragraph)
- The run-report and written-file cards die with the chat view (views are swapped, not stacked), and the "Not sent to the model" note under an answer is live only. By decision; the manual says so. (CLAUDE.md, local-model rounds 8 and 12)

### 3.13 Packaging and platforms

- Only the macOS bundle has been packaged and checked (08 §4.1). The `windows-latest` CI job runs only the Pester suite, CI compiles nothing under `src-tauri`, whether the workspace builds on Windows or on a clean Ubuntu without OpenSSL headers is unanswered, and no supported OS or webview floor is written anywhere. (storage design §21 B2; deps plan Appendix B #9, #28, DU-18, DU-31)
- The release archive ships two different `openalpacad` builds of one commit — `libexec/openalpacad` and the app's sidecar — because the scripts build the CLI and the daemon jointly; and `prepare-sidecar.ts`'s freshness check compares a `tauri-build` copy with itself. Until DU-30's Q02/Q03, build the daemon alone, once, and hash it before `tauri build`. (deps plan §3.3; 08 §4.1, §6)
- A scratch store (`OPENALPACA_HOME_STORE`) does not isolate `$HOME`: with the seeded `llm.toml` the daemon runs credential discovery and detects the Claude Code CLI at boot, so an isolated run that may reach a model must switch both off. (08 §4.1, §6)

## 4. Planned work that is paused or never started

### 4.1 LLM providers as a table — paused in section 1 of 5

Users would add any OpenAI- or Anthropic-compatible service from Settings, the CLI or `llm.toml`, the three built-ins
becoming rows of one table; D-H…D-N were taken on 2026-09-23. Nothing is implemented or deleted and there is no spec;
the survey maps `map-construction.md`, `map-identity.md` and `map-wire.md` were never written and must be regenerated
before section 2. To resume: re-present section 1 with its open question (§1.9, *no-key-free*), then sections 2–5 with
the owner's approval after each; the deps plan says land the dependency wave first (it overlaps P11, P12, P29).
(`tasks/llm-simplify/design.md`, *Where to resume*; `tasks/todo.md`, Chunk 24; deps plan §3.4)

### 4.2 The medium- and high-risk dedup items — not started

CORE-04/05/07/09/10, L1–L3, P1/P2, CON-04, D02/D03, S2, CLI-02/03/04 (CORE-04 usage recording and the cost algorithm,
CORE-07 utility JSON parsing, CORE-09 `llm_metadata_map`, L1 one key resolver, L2 one provider factory, L3 the Anthropic
`_done` re-encode — the L items fold into §4.1). D-scope (2026-09-22) kept them out of the simplification wave; each
needs characterization tests first. **Conflict, not resolved here:** CLAUDE.md lists D03 and CLI-04 as not started,
while `tasks/todo.md:431` puts them in the low-risk batch, which landed whole as `22924d8f..143c29ad` (`:424`); the review report that
defines the ids is not in the tree. (CLAUDE.md, whole-branch review paragraph; `tasks/llm-simplify/map-router.md` §8,
`map-callside.md` §4–§5)

### 4.3 The toolchain and dependency upgrade — decided; five owner questions open (rev 2, 2026-10-12)

**Since 2026-10-11 every DU is taken** (DU-1 → `main`, DU-2 → Rust 1.99.0, DU-13 → Bun 1.4.2, the rest as recommended; plan-rev3 rev 3.4), DU-33 (macOS first) on 2026-10-12, and the owner's no-compromise rule of 2026-10-12 (`CLAUDE.md` core principles) re-answered the compromises (plan-rev3 rev 3.9, `modernization-plan.md` rev 2.0; the record is `tasks/deps-upgrade/reports/2026-10-12-no-compromise-replan.md`). **Open, each with its recommendation** (nothing that depends on one lands before its answer):

| Question | Options | Recommendation |
|---|---|---|
| (c)-1 `<Activity>` for the view swap vs G1/P2's word "mount" (DU-W2; M70) | (a) adopt M70 in the form that keeps G1 and P2 (a reveal stamp, the daemon's-word recorder always mounted, P2's cards cleared on hide); (b) keep the unmounting swap | (a) |
| (c)-2 the macOS floor (DU-31 (f-a) gives 10.13, which Tailwind 4 and Vite 8's output cannot run on; Q18) | (a) `minimumSystemVersion` 13.3; (b) 11.0 plus "Safari 16.4 or later" stated | (a) |
| (c)-3 MCP elicitation answered by a person (DU-R4 (ii), with T35; M84) | (a) an approval card through the `ConfirmationBroker`, the capability advertised; (b) keep `Decline` | (a), decided with T35 |
| (c)-4 MCP tasks (DU-R4 (iii); M83) | (a) a short design mapping a task onto the run model first; (b) no surface | (a) |
| (c)-5 window chrome (DESIGN_SPEC §3.1) | (a) keep today's decorated window; (b) the overlay title bar | (a) — no change |

The history below is rev 1's text, kept as written.


DU-1…DU-32: see `tasks/deps-upgrade/plan-rev3.md` (rev 3.2, 2026-10-10, re-measured at 7295f91b on Rust 1.99.0; Part I
the dependency wave P01–P40 with P27a/P31a, Part II the "flexible apps" programme Q00–Q10; `plan.md` rev 2.1 is frozen
and superseded). Today: Rust 1.93.0, rmcp 0.16.0, Bun unpinned (1.3.13 locally, floating in CI). Before the first
commit the owner takes DU-1, 2, 13, 23, 25 and 26 at least, DU-24, and DU-4, 6, 10 and 21 as rev 2.1 asked; Part II's
DU-27…DU-32 can wait, except DU-30 (w-i) if the provenance work is to precede P04 (deps plan §5.1 item 1). Each answer
is recorded as an *Owner decisions taken* row in plan §0. The owner runs this wave in
another session; CLAUDE.md's "do not start them unasked" is superseded by that request, which the deps plan records.

| DU | Question | Plan's recommendation |
|---|---|---|
| 1 | Where and when the wave happens | (A′) cut `chore/toolchain-and-deps` from `feat/markdown-memory` at a recorded post-Stage-2 commit (today 7295f91b), PR against it while #34 is a draft; the security slice (C) if an untrusted upload reaches anyone first |
| 2 | Which Rust | 1.99.0 (measured: 0 errors, clippy +23, tests unchanged); 1.99.1 if it appears before the cut |
| 3 | MSRV policy | `rust-version` = the pin's minor (`1.99`), moved with the pin |
| 4 | `serde_yaml` successor | serde-saphyr 1.3 with compat options, LastWins |
| 5 | rmcp scope | Superseded in part by DU-25; default features, response cache and `client_info` unchanged |
| 6 | infer's new upload refusals | (b) P17; (m-b) P17a |
| 7 | zip features, zip 9 | `['deflate']` at 8.6.0; zip 9 when calamine requires ^9 |
| 8 | pdf-extract's Form-XObject regression | Accept; an ignored test; file upstream |
| 9 | calamine C0 and in-cell separators | (a) for C0; a space for TAB/LF/CR inside a cell |
| 10 | keyring 4 gate | Count the owner's `secret_ref`/`use_keychain` uses first; a keychain round trip if any |
| 11 | tower-http 0.7 / tokio-tungstenite 0.30 | Latest (0.7.1, 0.30) |
| 12 | teloxide over rustls | (a) if a live bot round trip can run, else (c) |
| 13 | Bun version and lockfile | Pin `bun@1.4.2` and commit the v2 lock it writes; fall back to (b) if the first hosted Dependabot bun run fails |
| 14 | Vite 8 / Vitest 5, the plugin swap, other webviews | Take them; (w-a) unless a machine is at hand or DU-31's native-smoke runs |
| 15 | Maintenance bot | (a) with (p-i), plus P31a's narrowed Tauri requirements |
| 16 | Pester version | Exactly 6.2.0, fallback 5.9.1 |
| 17 | Clippy cleanup and a warnings gate | Skip in this wave |
| 18 | Windows coverage | (a) now (P01a/P40); later superseded by DU-31 |
| 19 | Repository hygiene (`rust_out`, `pnpm-workspace.yaml`, three scripts) | Take P37 |
| 20 | Migration SQL rule for older SQLite | No rule |
| 21 | Advisories gate; zip2's GHSAs | Adopt on PRs and weekly, the owner answers a red run; activate P34's recorded ignore for the GHSAs until calamine requires zip ^9 |
| 22 | `--locked` in CI | Adopt |
| 23 | The 7-day freshness rule | Cut on or after 2026-10-17 with the measured set; `--precise` step-backs if the PR must open sooner |
| 24 | opener 0.9.0 | Take it as P27a, gated by the files routes' tests and a manual Open on Linux |
| 25 | rmcp version, commits, protocol offer | 3.5.0, one commit, offer 2025-11-25 through `get_info`, caret requirement with the lock at 3.5.0 |
| 26 | Tauri Rust/JS pairing; the Windows 7 floor | Hold the sub-graph (tauri to 2.11.6) in P04, move both locks to 2.12 in P31a; accept and record the Windows 7 drop |
| 27 | Extract `openalpaca_app_support` (Part II) | Two commits, Q05/Q06 |
| 28 | HostBridge and a browser dev mode (Part II) | (a) now, a fixture daemon later; flagged: the dev middleware hands the token to any local client on :1420 |
| 29 | A daemon protocol version and a mismatch state (Part II) | (a) with a hand-written health probe, no new dependency |
| 30 | Sidecar staging and the release scripts (Part II) | (a) with (w-i), before P04 |
| 31 | Native CI and OS/webview floors | Three-OS shell-check per PR, native-smoke nightly; Tauri's documented floors |
| 32 | Streaming render, history paging, lazy previews | Adopt the gates and the §7.1 protocol; implement each only when measured |

### 4.4 The live-model steer-routing eval — never run

Routing V2's only remaining item: the `#[ignore]`d eval `crates/openalpaca_core/tests/steer_routing_eval.rs` (corpus
`scripts/eval/steer_routing/`) needs a resolvable `llm.toml` key, `--features live-eval` and `OPENALPACA_LIVE_EVAL=1`;
it runs no model discovery, so a local Ollama's models are unknown to it. Bar 0.8; about $0.10–$0.50 a run.
(`scripts/eval/steer_routing/README.md`; Vault 11)

### 4.5 Memory evaluations not re-measured

- LQ19 (the commute's answer) and LQ20 (the half-items retry and the cut read): built, never measured live. (10 §9; 09 §10)
- Continuity after C1 and C2: not re-run since rev 2's figures. (11 rev 1.8; 12 §7)
- Episode notes after CQ-11: the restored call not re-run on this build. (11 §10.3; plan §0, CQ-11)
- The live-eval harness's four offline tests still expect the episode projection CQ-11 removed: left for whoever next runs it (the owner's rule of 2026-09-29: the offline harness is never a gate). (11 rev 1.9 item 9)

### 4.6 Designs not written, and later work

- **Connector audience** (MD-4): group privacy for `USER.md`, transcripts, notices and memory together. Not written. (03 §0 MD-4, §9)
- **Sandboxing shell commands** (ST-14): what confines a command to the project on each platform, and its cost. Research for a later version. (storage design §0.1 ST-14, §18)
- **Merging two projects on a re-point** (ST-20): today refused with `409`, naming both ways out. (storage design §0.1 ST-20, §18)
- **Memory scale levers**: O5 (map the whole file, about −20 to −25 % of search time), `prefix=` indexes, O6/O7, O9, O13–O16 — unbuilt, "each is the owner's". (10 §7.4, §9)

## 5. Recorded residue (not built)

One line each; the full text is in storage design §18 (by stage and subsection) and in each cited source. §18 holds 4
items from Stage 0, 31 from Stage 1 and 46 from Stage 2 (numbered here in §18's order), besides its opening paragraph
(sandbox research and merging projects, §4.6; T35, §1.1; R43, §3.3; T36 and ST-23, taken); every item is below, in §3, or
in the closed line further down.

**Storage design — Stage 0** (§18 Stage 0)
- `ToolContext`'s doc omits `prompt_ctx/sources/memory.rs` among `request_workspace_root`'s readers; the field collapse at `tools/registry/mod.rs:27-28` owns it.
- A lead-queued follow-up is pinned to the lane's active session, not its run's (see T18).

**Projects and registry** (§18 Stage 1 *Registry*; Stage 2 item 16)
- A control snapshot whose project id names no registered folder is skipped by rebuild and doctor, unlisted (R-S11).
- R11 is kept by `list` and `by_id`, not by `lookup`; two folders with one id confuse a re-point or delete.
- A stranded `projects/.<name>.<pid>-<n>.tmp/` after a crash in `register` is never swept (R-S14).
- No test of a case-sensitive project volume beside a case-insensitive home.
- With the home store outside `$HOME`, a `~/.git` makes `$HOME` a project (R-F7).
- A root that is not valid UTF-8 can alias another (Linux).
- An absolute path whose missing tail ends in `..` is refused rather than resolved to no project.
- While `projects/` cannot be listed every workspace route answers `500 IO_ERROR`; the error arm has no route-level test.
- A root asked for in another case or Unicode normalisation than recorded is resolved by its canonical form (R-T29's remainder).

**Re-point** (§18 Stage 1 *The re-point*)
- After a live re-point fails past step 2, both roots look up as nothing until the next boot.
- Recovery before step 3 re-runs `name_for` instead of reading the name step 2 wrote.

**Memory and learning, storage side** (§18 Stage 1 *Memory and learning*)
- A memory write for a root no folder records is refused before it takes a slot; the `unregistered:` slot is left only to keep two such writes apart, and a failed lookup takes it with a WARN.
- `root_gone` reads a failed lookup as "not gone", spending a model call per learning retry while `projects/` is unreadable.
- Task extraction across a re-point or a delete can lose a run's items (R-F4's remainder).
- A re-point between the gateway and the handler leaves the turn with no project (review R7's window).
- `POST /v1/memories` answers a stale root `500 MEMORY_INTERNAL`, not a typed 409.
- A deleted conversation's summary in a third project's partition stays until forgotten by hand.
- A project delete publishes no `memory_changed`; another window's Memory view keeps the deleted notes.

**Project delete** (§18 Stage 1 *The delete*; Stage 2 item 6)
- An in-flight chat turn is not refused; a project only an earlier build's rows name cannot be deleted; work runs inside the async handler.
- Review R16's and R3's windows: a put landing after the commit leaves a row of the deleted project; a late entry leaves a folder no list shows.
- Kept files' edges: empty `kept/` folders, NFD or case-folded targets refused, a crash-cut kept copy adopted, a permanent `500 DB_ERROR` for a taken address.
- Nothing tells "the folder was kept" from "it was already gone" (R-F2).
- The delete card counts no result-file rows (needs a wire change).

**The app and the CLI** (§18 Stage 1 *The app and the CLI*; Stage 2 items 39–41; live report)
- `openalpaca memory --project` reads the registry itself, so a CLI and daemon on different home stores can disagree (X9).
- `store rebase --dry-run` advises "delete that project's data first" where the daemon answers a 409 naming the restart.
- The CLI prints "1 partitions", "1 uploads".
- The Projects list: no registration event, roots drawn unescaped, no typed word on Delete data (T33 stays open).
- The project dialogs' small defects (`windowFollows`, dropped `onSuccess`, silent late refusal, stale dialog, lagging header).
- Following a project: an id-less `oa-project`, a misleading note after a delete, a pinned conversation re-synced from the loaded page only.
- `FilePanelSlot` fetches versions and a diff for a pointer row that shows Preview only.
- The palette's Find and the panel's picker still hide marked result rows.
- Clearing a result file's missing mark by an image load, Open or Export waits for the list's next refetch.
- `openalpaca daemon tail` prints "? unknown event" for `daemon_shutting_down`.

**Approvals** (§18 Stage 2 items 1, 3, 35)
- An explicit confirmation list is used as written (it still asks for `artifact_write`/`workspace_write`), and `GET /v1/agent-templates` shows it without the added `file_edit`.
- An edit's card can go stale before the click; nothing compares the approved plan with the body's.
- A CRLF `old_string`/`new_string` shows `<U+000D>` at each line end on the card — accepted, the escape set stays.

**Files and confinement** (§18 Stage 2 items 4, 5, 17–20, 29–34, 36–38; live report)
- The bytes read and the bytes copied before an edit can differ; the edit is not a compare-and-swap.
- A result file's row follows its latest writer, so an earlier run's report may not list it.
- Reveal given an ancestor-link spelling of a missing project's root reads `OutsideStore`.
- The card/send race: a link swapped in between `send`'s card and the send is followed.
- A link swapped in at a middle folder is still followed, and off unix the leaf too (needs an `openat` walk).
- A named pipe swapped in between the check and the open blocks the call until its timeout (X6's window).
- R-T3 also refuses another project's upload by id.
- R-T20's `content_root` follows a link standing at `<store>/artifacts` or `<store>/uploads` itself.
- `refresh_project_file` reads a result file whole to hash it.
- Rows an earlier build recorded under a `docs/./plan.md` spelling stay separate.
- `workspace_write` stores any `file_asset_id` silently; the notifier checks it later.
- A dangling link out of a by-product's folder answers `410 ARTIFACT_GONE` on Open.
- `file_edit`, `file_read` and `file_list` reach the bootstrap and attachment-only arms of a project turn, untested there.
- A skill is pointed at an `artifact_read` it may not hold.
- A store seeded before S5.8 keeps `writing_agent.md` without `file_read` (the switch-over note's step 8 has the line).
- Live: the local model twice answered from memory instead of calling the tool.

**Shell and processes** (§18 Stage 2 items 21–22)
- A plugin agent's spawn and step, and the loop's step to its tool batch, are not raced against cancellation.
- A script's failed run shows only the first 500 characters of its stderr.

**Working folder** (§18 Stage 2 items 23–25, 27)
- R28's working-folder line is a message of its own off the main loop, so compaction keeps one row fewer verbatim.
- `remove_scratch` and `ensure` check, then act.
- A deleted conversation's scratch folder can come back while a tool runs; orphans are never swept.
- A raced turn reads the lane's active session for its history and state (R-T27's remainder).

**Docs and tests** (§18 Stage 1 *Registry*; Stage 2 items 10, 14, 15, 26, 42–46)
- The test home-store guard is a backstop, not the protection; the guard audit is.
- No test-build tripwire in `WorkingFolder::ensure`.
- `reconcile_owner`'s `NotRegistered` arm is untested.
- The case-only tests do not assert the spelling the folder is listed in.
- Unguarded tests compute a real-home scratch path into mock requests.
- `plan-stage2.md` keeps "git keeps its history" as written.
- `DESIGN_SPEC.md` rev 31 was amended in place.
- The plan's live-check driver passed `kind` twice.
- rustfmt drift stays in test files Stage 2 touched.
- An episode test flaked once; to watch.

**Closed by the storage design itself, listed so nothing is dropped:** Stage 0 items 1 (worded in Stage 2) and 4
(fixed in `32292608`); Stage 1's review R1 cross-wiring and the skipped reconciliation (closed by ST-23, S0) and the
shell route-around during a re-point (closed by `NotAProject`); Stage 2 items 11 and 12 (moot, unreleased commits only).

**Memory** (each from its source)
- No verb restores a note a confirmed suggestion retired (09 §0.3, §10).
- A project note does not outrank a global note with the same key in recall (09 §0.3).
- A confirmed store under another saved note's key outranks it unnamed (03 rev 7.6).
- "Notes used in this answer" is lost on reload; `GET /v1/memories/recently-used` has no GUI reader (03 §7c R8; 08 §6).
- The live-eval harness still expects the episode projection (§4.5; 11 rev 1.9 item 9).

**Elsewhere**
- Daemon stop: in-flight work is not drained; a pending confirmation may settle `Timed out` rather than `Cancelled` (Round G's drop guard not re-checked for a stop); a forced exit skips `remove_discovery()`; the MCP hang tests leave `sleep 3600` children. (CLAUDE.md, daemon-lifecycle paragraph)
- `ToolContext` carries `workspace_id`, `request_workspace_root`, `workspace_path` and `conversation_id`; collapsing the first three is a deferred cleanup. (Vault 11)
- The schema refusal says "it creates schema version 1" though migrations 2–4 run in the same transaction; the store-revision design's status line still reads S3–S6 as not built. (`tasks/store-revision/design.md` rev 1.1 item (6))
- Pre-existing problems the deps plan found: an unbounded MCP reconnect ladder (`client.rs:325`), `client_info` never sent, `config set|unset daemon.*` dropping comments, Linux-only unused imports, the README's missing OpenSSL headers, two rustls crypto backends, credential discovery reading the real keychain from an isolated daemon, `discovery.json` with no byte pin. (deps plan §3.3)
- Code-audit deferrals whose status at HEAD was not verified: M1, M2, M7, M15, M20, M21, L4, L5 — re-audit before the beta. (`tasks/patch-plan.md`, deferred; code audit)
- `docs/tools/DESIGN.md` and `TECHNICAL.md` date from 2026-07-19, and the vault's roadmap page from 2026-09-20. (CLAUDE.md, *Code Navigation*; Vault 11)
- Clippy keeps a 78-warning baseline and `cargo fmt --check` is not a gate (DU-17 skips the cleanup). (CLAUDE.md, Stage 2 gate; deps plan DU-17)
- Telegram's confirmation-before-rate-limit ordering is correct but untested (`handler.rs:71` before `:83`). (`tasks/todo.md:432`)
- `scripts/gen_api_docs.py` does not handle `ALTER TABLE … RENAME COLUMN`; no migration uses it yet. (CLAUDE.md, F8)

## 6. Sources

- `CLAUDE.md` — current-work paragraphs, settled rules, *Gotchas*.
- `tasks/api-fix-plan.md` — §0 (the pending T rows, every *Owner decisions taken* subsection), §1.2, §5.4, §10, §12.
- `tasks/extension-enable-design.md` — §13 Q5–Q14 and the body sections they point to (§2.3, §3.3–§3.6, §6.2, §7.5, §10, §11).
- `tasks/research/claude-code-design-lessons.md` — §6 (rows T1–T15).
- `tasks/storage-design/design.md` — §0.1–§0.5, §11, §12, Part II's *New refusals the final wave added*, §18, §21.
- `tasks/storage-design/stage2-live-report.md` — (d), (h), (i) and *Observations*.
- `tasks/memory-redesign/03-design.md`, `08-evaluation-report.md`, `09-learning-design.md`, `10-learning-evaluation.md`, `11-continuity-design.md`, `12-continuity-evaluation.md`.
- `tasks/store-revision/design.md` — SQ-1, §1.4, §5.
- `tasks/approval-hidden-chars.md` — the owner decisions not taken and the accepted residue.
- `tasks/deps-upgrade/plan-rev3.md` (rev 3.2) — §3.3, §3.4, §4, Appendix B.
- `tasks/llm-simplify/design.md`, `map-router.md`, `map-callside.md`.
- `tasks/code-audit-2026-07-19.md`, `tasks/patch-plan.md` — the deferred findings.
- `tasks/wiring-audit-2026-08-30.md`, `tasks/design-routing-v2.md`, `tasks/todo.md`.
- `scripts/eval/steer_routing/README.md`.
- Obsidian vault `~/Valuts/Main/Projects/OpenAlpaca/11-roadmap-and-open-questions.md` (owner's machine; as of 2026-09-20).
- `gh pr view 34` and `git rev-parse origin/feat/markdown-memory` on 2026-10-11.

## Revision log

- **rev 1, 2026-10-11** — written at 7295f91b, after the push, from four read-only extractions of the sources above
  (the plan's decisions, memory, the storage design, everything else) before the owner starts the dependency upgrade
  in another session. Nothing adopted. Duplicates merged: T35 and T37 (plan §0 and storage design), B1 (storage design
  and the other sources), per-chat `/yes` (storage design §18 and code audit H2), the two daemon binaries (08 and the
  deps plan), the pre-squash store (B3 and the store revision). T2, T5 and T6(b) are listed as closed with the caveat
  that no T-row records an answer. Verified against its sources the same day and corrected in place: stale line
  numbers (T16, T22), the `daemon stop` ruling's source, the record of `next`'s acceptance, the deps plan's pre-wave
  decision list, the hidden-character and memory wording, the LLM survey cells filled from map-callside §3c–§3e, the
  advisory hedge, the closed-list and residue citations, and §5's §18 counts; map-router's Q10 (RT-13) added.
