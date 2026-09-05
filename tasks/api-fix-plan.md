# Daemon Fix Plan — rev 3.1: single root, artifact store, sessions, the 23 GUI gaps

**Status:** design, rev 3.2 — **ready to implement; no carve-out.** D1–D5 and N1–N5 are resolved and carry no alternatives. N5's mechanism is `tasks/extension-enable-design.md` rev 15 (verified 2026-09-02) (design of record, ADR-030); this plan defers to it wherever the two overlap (§0 N5, Phase 0 A0–A3, Phase 8 items 2 and 9). The owner decisions the Claude Code lessons raised are **pending — listed in §0, decided nowhere, applied nowhere.** No production code written. · **Date:** 2026-09-02 · **Branch:** `feat/ui-rework`
**Rev 3 changelog (one line):** N5 → resolved to the extension-enable design; Phase 8 item 2 (GAP-18) re-specified as the design's read-only C6 `origin` shape; Phase 8 item 9 relabelled **GAP-24** per design §12.1; every plan-targeted adopt/adapt row of `tasks/research/claude-code-design-lessons.md` §5 applied at its named anchor (Phase 0 A0–A4, §1 reservations, §4.8 one-transaction rebase, §5.4 session-log records and sweep, Phase 8 items); four implementer pre-checks; surface-to-owner rows (T1, T3, T4, T9, T10, T11, T12, T13, T14, T15) collected in §0 and left unapplied. Re-verified 2026-09-02 against design rev 6's verification addendum: the N5 row now names §3.7 (`tools/list_changed`) beside the reaper; the lessons doc is cited at rev 4 (a re-verification with no change to any plan-targeted row); C-3 tagged beside P-3.
**Rev 3.2 changelog (one line), 2026-09-05:** the extension design's C1–C8 landed (`tasks/extension-enable-design.md` rev 16) — §7 records the `/v1/extensions*` flat-envelope exception (R20), Phase 8 item 2's `GET /v1/tools` half is marked shipped in C6 (the `/v1/skills` half is what remains of GAP-18), and the per-tool deny key is purged as design §11.1 specified; no other plan row moved, and T1 (a per-tool deny rule) stays pending.
**Rev 3.1 changelog (one line), 2026-09-05:** Phase 1 landed (the store module, the mover, the marker change, the purge, migration 035) — the documentation sweep is done; §3's P13 verdict changed from a legacy-parse deletion to **NOTHING TO PURGE** (`prune_backups` never parsed timestamps) and P15 now records that its normalising `UPDATE` shipped inside migration 035.
**Before starting:** four implementer pre-checks in §0 (verify, don't decide) — bundled SQLite ≥ 3.35; `LlmConfig` consumers before P4; `secret_ref` keychain account naming per root; whether the per-workflow cost cap counts subagent spend (a measurement, not a change).
**Inputs:** `tasks/gui-api-requirements.md` (the 23-gap brief) · `apps/openalpaca-gui/src/lib/unavailable.ts` (23-entry registry, verified) · `apps/openalpaca-gui/src/lib/api/unbacked.ts` (client contract) · `apps/openalpaca-gui/API_MAP.md` §3 · `tasks/extension-enable-design.md` rev 15 (verified 2026-09-02) (the N5 mechanism; owns Phase 8 item 2 and the GAP-24 rename) · `tasks/research/claude-code-design-lessons.md` rev 4 (§5 plan deltas, §6 tensions) · rev 2 of this document
**Evidence appendices (untracked — `.gitignore:92` blanket-ignores `*.md`):**
`tasks/research/A-artifact-store.md` · `tasks/research/B-run-data.md` · `tasks/research/C-surface.md` · `tasks/research/R-root-taxonomy.md` (root move, taxonomy, purge) · `tasks/research/S-sessions.md` (session persistence)

**Spine — three directives, all now decided:**

1. **Artifacts of all kinds live on disk under a `.openalpaca/` convention**, human-findable paths, DB stores only the address (rev 1's spine, kept).
2. **The app is not distributed.** Legacy-compatibility measures are **deleted from the code**, not merely avoided going forward (§3).
3. **New pillar: session persistence** — local persistence of session + project association + session recovery: SQLite tables + a per-session append-only JSONL event log + filesystem tiers (§5).

The `.openalpaca/` directory structure is designed as an **extensible contract** (§1): namespaces reserved deliberately, one Rust module owning every path, new content kinds added only through one enum.

**Verified against the tree** (not taken from any brief): migration head is **034** (`crates/openalpaca_storage/src/migrations/mod.rs`, tail `Migration { version: 34, name: "drop_context_compaction_log" }`); `unavailable.ts` holds exactly 23 gap ids; `walk_up_for_marker` prefers `.alpaca` then `.git` (`crates/openalpaca_core/src/memory/workspace.rs:43-56`); `workspace_id_from_root` returns the **canonical path string**, not a hash (`workspace.rs:60-65`); `app_dir()` = `directories::ProjectDirs::from("","","OpenAlpaca").data_dir()` and everything (`discovery_path`, `lock_path`, `database_path`, `assets_dir`) hangs off it (`crates/openalpaca_storage/src/paths.rs:10-83`); `conversations.lane_key` carries a **column-level `UNIQUE`** (`migrations/011_unified_conversations.sql:3-12`) — the single reason multi-conversation doesn't exist; `conversation_messages` is keyed by `lane_key`, not conversation id (`009_conversation_messages.sql`); the boot orphan sweep marks every non-terminal task `failed` (`apps/openalpacad/src/bootstrap/migration.rs:21-27` → `repository/task/mod.rs:187-202`); the agentic loop's transcript is memory-only (`runner/agentic_loop/mod.rs:205`, `LoopResult` returns no transcript — `runner/agentic_loop/config.rs:263-276`); `tool_execution_log` stores **no arguments and no results** (`030_skill_tool_execution_log.sql:29-40`); `chat.rs:462` is a literal `approval_scope: None`; `settings.rs:314` is a literal `daily_cost_usd = 0.0`; `list_orphaned` has no `origin` predicate (`repository/file_asset/mod.rs:95-112`); `total_storage_bytes` sums **all** rows (`mod.rs:76-85`); `SystemEvent::DagNodeStarted` is still produced at `runner/lead_agent/tools.rs:232-240`. The complete `paths.rs` consumer inventory and the session-side ground truth live in appendices R §0 and S §1 — every claim there carries a checked file:line.

---

## 0. Decisions (resolved)

Rev 1 §0 posed five either-way calls. All five are decided. The plan below is written against these; no alternatives are carried.

| # | Decision | Consequence the plan pays |
|---|---|---|
| **D1** | **`app_dir()` itself moves to `~/.openalpaca/`** — a single root for everything: DB, `discovery.json`, lock, master key, `plugins/`, upload assets, config, artifacts, sessions. | A live-DB relocation at boot. Paid cleanly by the one boot-time mover (§2.2): per-entry atomic rename, idempotent resume, live-daemon guard, WAL sidecars before the DB file, abort-on-first-failure — ~80 lines replacing `migrate_legacy_app_dir()` in the same call slot. Plus an **atomic three-binary rebuild** (daemon + CLI + GUI in one commit); no compatibility window exists and none is designed. |
| **D2** | **Chat/connector uploads also move into `.openalpaca/`** — human-named under `uploads/`, not content-addressed. | New uploads write via `upload_dir()`/`upload_file_name()`; dedup keys off the existing owner-scoped `sha256` DB query (`routes/files.rs:172-185`), not the path. Existing content-addressed blobs park at `state/assets/` (mover) with a boot-time `storage_path` prefix UPDATE; the full re-home into `uploads/` is a Phase 8 item (§6, Phase 8.3). The connector's duplicated sha/dedup write path (`connectors/src/common/mod.rs:213-262`) collapses into the single writer first. |
| **D3** | **`extracted_text` stays in the DB** — a derived text index, bounded at 50 000 chars (`daemon_config/upload.rs:28`), on the prompt-assembly hot path. | Nothing; this was already the schema in §4.4. |
| **D4** | **`~/.openalpaca/`**, with an **`OPENALPACA_HOME_STORE`** env override (absolute paths only — a relative value is rejected; it would re-introduce CWD-dependence). | One resolution function, `home_root()` (§2.1). |
| **D5** | **`start` keeps the same task id** (200). | `dispatch_lead_agent_with_id` + idempotent `TaskRepository::upsert_queued` — acknowledged as the least clean code in the plan; isolated and named in review (§6 Phase 5, risk table §9). |

Two calls carried from rev 1, still standing:

- **`daily_cost_usd` comes from the DB** (`query_daily_usage` for today), not `CostTracker::total_cost()` — the tracker means *since boot*.
- **`calls_7d` is renamed `messages_7d`** (GAP-17). The only measurable number is inbound user messages by `conversation_messages.source`.

### New calls raised by rev 2 — **N1–N5 all resolved.** N1–N3 took their default (2026-09-01); N4 is a product call answered by the owner (2026-09-01); N5's *model* was fixed 2026-09-01 and its *mechanism* accepted 2026-09-02 as `tasks/extension-enable-design.md` rev 15 (verified 2026-09-02) — implement N5 from that document, never from this table.

| # | Question | Decision |
|---|---|---|
| **N1** | **Connector session boundaries.** Telegram/iMessage lanes get one perpetual `active` session. Is an idle-auto-archive (e.g. 72 h) wanted, and/or an in-chat `/new` command? The config knob (`[orchestrator.sessions] idle_archive_hours`) is reserved either way. | **Perpetual session.** Knob reserved (`idle_archive_hours`), off. |
| **N2** | **`GET /v1/status.log_path`.** Rev 1 shipped `null` unconditionally (the daemon writes no log). After the move, the *CLI-managed* start pipes daemon output to `state/logs/daemon.log` (`apps/openalpaca/src/manager.rs:38`). Serve that path when the file exists, or keep `null` until GAP-14 Phase B? | **Serve it when present**; `null` for GUI-sidecar daemons. |
| **N3** | **Early S0.** If the GUI wants multi-conversation before the artifact Library completes, sessions Phase 7a can be pulled ahead of Phase 3 — its only hard prerequisite is Phase 6's `#[derive(Default)]` churn payment, which can be paid early. | **Ship in the §6 order.** |
| **N4** | **Cost-cap semantics.** `max_cost` is enforced per-workflow (`lead_agent_defaults` $5) and per-agent-turn (`agent_defaults` $1), but the Settings panel presents today's spend as if against a *daily* budget. Add a real daily cap, or relabel? | **Per-workflow — relabel; no daily cap is added.** `/v1/usage/summary.caps` carries `workflow_max_cost_usd` / `agent_max_cost_usd` and **no** `daily_*` key; today's spend is reported as an informational total with no denominator, so the design's progress bar stays omitted (it was never drawn). A daily budget would be a new enforcement point in the LLM router, not a label — out of scope. **Corollary:** the main-loop cost lockout (`tasks/bug-main-loop-cost-lockout.md`) is confirmed a bug, not a daily budget working as intended. **Fix shape for the corollary (P-3 / C-3, adopted):** option 1 of the bug note — baseline `state.last_cost` from `agent_cost()` before round 0 so the $1 budget is per turn again; no daily budget, no attribution-row change. **What "per-workflow" measures today (P-2a, verified — recorded, no enforcement change):** subagent spawns pass the parent `task_id` into the loop (`runner/lead_agent/tools.rs:592`), so `task_id`-keyed *reporting* (`get_task_usage`/`cost_for_tasks`) already includes subagent spend; but `agent_cost()` prefers the agent-scoped bucket over the task bucket (`runner/agentic_loop/backend.rs:167-189`) and every subagent starts a fresh accumulator, so the lead's $5 `max_cost` bounds only the lead's own calls. Pre-check (d) below measures exactly this. Charging subagent spend to the lead's accumulator would be a **new enforcement point** (workflows die sooner; a subagent dollar counts against its own $1 turn cap and the lead's $5) and is therefore **owner decision T13 — pending, not a default** (see "Owner decisions pending"). Precedent, for the record only: Claude Code's `/cost` is per session and every daily/monthly limit lives outside the tool; its one hard cap (`--max-budget-usd`) does count subagents. |
| **N5** | **Tool allow / enable model.** What governs which tools an agent gets, and what the GUI toggles? | **RESOLVED — model + semantics settled 2026-09-01; mechanism accepted 2026-09-02 as `tasks/extension-enable-design.md` rev 15 (verified 2026-09-02)** (design of record; ADR-030 supersedes ADR-029). Two axes: **allow** is per-agent (template `capabilities`, skills' `requires_capabilities` — exists today); **enable** is per-extension. Settled: **(S1)** the toggle sits on **each MCP server and each plugin** — *not* per tool; the install unit is the toggle unit. **(S2)** *disabled* means **unloaded** — kill the plugin child process, drop the MCP connection. A lifecycle state, not a display filter; a disabled extension runs nothing and must not attempt reconnect. **(S3)** extensions carry distinct states — **Disabled**, **Unapproved** (approval gate not passed), **Crashed** (internal error: won't run, can't connect, or needs authorization). **(S4)** withdrawn capabilities: the agent **cannot** use the tool **and a warning is emitted**, surfacing in the log and/or chat — silent degradation is rejected. **Builtins are never toggled**; agent config alone governs them. **Mechanism, in two lines:** the owner's bit is persisted *write-first* in the declaration itself (`mcp.toml` `[servers.<n>] enabled`; `.permissions.toml` `enabled` beside a tri-state consent) while an in-memory `ExtensionLedger` inside `ToolRegistry` holds observed state (`Enabled \| Disabled \| Unapproved{reason} \| Failed{reason, detail, since} \| Orphaned`, transient `Enabling/Disabling`; *needs authorization* is a `FailureReason` with an `actionable` bit, not a state) stamped with a per-load `generation`. Disable = W persist → T0 gate → T1 deregister → T2 withdraw contributed skills/templates/providers → T3 drain → T4 kill/disconnect (the MCP client's `closed` seal is checked at `reconnect`'s entry **and** at `do_handshake`'s install point) → T5 commit; enable = W → E0 CAS (a no-op on `Enabled`, never a reload) → E1 consent + drift check → E2–E5 load; `reload` is the third verb; crashes reach `Failed{Crashed}` through a generation-carrying reaper; a connected server that changes its tool set mid-session (`notifications/tools/list_changed`, design §3.7) is refreshed in place under the mutex only while `Enabled` at the notifying generation — removed names withdrawn through T1 with attribution and kept flagged `server_withdrawn`, added names admitted through E4, no generation bump; S4 fires at three moments (attempted use, withdrawal at declaration, the transition) under design §7.5's log-versus-chat rule. The design's owner-gated refinements (§13 Q5–Q14) are **not applied** there or here. |

Four **implementer pre-checks** (verify, not decide):

- **(a)** bundled SQLite ≥ 3.35 for `ALTER TABLE … DROP COLUMN` in migration 035 — else use the 024-style table rebuild.
- **(b)** before deleting the legacy flat `llm.toml` branch (P4), confirm `LlmConfig` and `build_provider_with_runtime` have no non-legacy consumers.
- **(c) Keychain `secret_ref` naming per root (P-1/T10).** Verify how `secret_ref` keychain accounts are named today. If they are keyed per provider/key id only (not root-scoped), a second `OPENALPACA_HOME_STORE` root — a test root, say — silently shares the real keychain secrets. **The id source, so the check is implementable:** `ensure_store()` writes a second line `install_id=<uuid-v4>` into the **home** root's `.layout` on first creation and never rewrites it (§1.1); the project root's `.layout` carries a separate `project_id` (§1.2, P-12); both files are parsed as `key=value` lines after line 1. Whether accounts are then root-scoped (`openalpaca/<install_id>/<provider>/<key_id>`) or sharing is accepted is **owner decision T10 — pending**; the pre-check establishes the fact and lands the id, nothing more. Not a change to D4.
- **(d) Does the per-workflow cost cap count subagent spend? (P-2a/T13) — a MEASUREMENT, not a change.** Run one two-subagent workflow; compare `get_task_usage(task_id)` (task-wide reporting) against the accumulator `agent_cost()` reads for the lead at `runner/agentic_loop/backend.rs:167-189`; write both numbers into the N4 row. The expected result (task-wide reporting, lead-only cap) is already stated there. No enforcement point is added or moved on the strength of this check — that is T13.

### Owner decisions pending — listed here, decided nowhere, applied nowhere

Raised by `tasks/research/claude-code-design-lessons.md` §6 against rules that are settled. Each carries the lessons' recommended default; **the default is a recommendation, not a decision, and this plan is written as if the answer were "no change" until the owner says otherwise.** Design-side twins (T1, T3, T4, T6(a), T6(c), T7, T8, T9, T14, T15) are also listed as `tasks/extension-enable-design.md` §13 Q5–Q14 and are unapplied there too.

| # | Question | Recommended default (lessons §6) | If **yes** → what changes in this plan |
|---|---|---|---|
| **T13** (P-2b) | Charge subagent spend to the lead's $5 `max_cost` accumulator so "per-workflow" bounds the workflow? | Lessons recommend yes — **but N4 as settled is relabel-only, so nothing ships until decided** | `runner/agentic_loop/backend.rs` accumulator change + a written double-count rule (does the subagent's own $1 cap still apply?); N4 row gains "counts subagents". If **no**: relabel `workflow_max_cost_usd` in GUI/docs as "lead agent spend". |
| **T12** (P-21, P-27) | `log_retention_days` default 0 or 90 — and reserve a `[orchestrator.sessions] persist` opt-out knob (per-session override, no `sessions/<id>/` dir at all)? | 90 as the privacy sweep ("age is for exposure, size is for disk"); reserve `persist` — direction only | §5.4 table default flips to 90; the `persist` key is reserved with a `POST /v1/sessions {persist:false}` override. The sweep machinery ships regardless (§5.4). If **no**: default stays 0 with §5.4's recorded rationale. |
| **T9** (P-30, design X-29) | Plugin config values marked `sensitive`: default store `secret_encrypted` (in-root, under `state/.master_key`) or `secret_ref` (OS keychain)? | `secret_encrypted` — D1-pure | Phase 8 item 14's `plugin config get` redacts either way. If **`secret_ref`**: pre-check (c)'s root-scoping becomes mandatory rather than advisory. |
| **T14** (P-26) | GAP-22 (`ts`/`instance_id` on the six `plugin_*` WS events) in Phase 0, or dropped because the design's C7 deletes those variants? | Drop it if the extension design lands before Phase 8 | Phase 0 item 6 and the §8 GAP-22 row are removed; the `Extension*` event family carries both fields from birth (design §7.3). If **kept**: it ships as written and is deleted again in C7. |
| **T11** (P-33) | Reserve an in-chat `/new` command for connector lanes (a deterministic-tier op mapped to `POST /v1/sessions` for that lane)? N1 itself is not reopened. | Reserve the name; keep N1 | §5.1 gains the reserved command name and a sentence that `session.summary`/`last_summarized_message_id` must keep working after a `log_trimmed`; revisit N1 only if a connector lane's `messages_7d` outgrows the prompt window. |
| **T10** (P-1) | May a second `OPENALPACA_HOME_STORE` root share the real keychain secrets? | No — root-scope the account name via `install_id` | `secret_ref` accounts named `openalpaca/<install_id>/<provider>/<key_id>`; a second root starts with no credentials. If **yes**: document the sharing as intended. |
| **T4** (P-18) | May a per-turn `<extension_status>` block on the main-loop/lead surfaces satisfy S4's chat leg for degraded-but-wanted extensions (reverses design §7.5's row)? Shape: moving slot vs persisted record? | Yes, moving-slot shape first | §5.4 gains one record type `context_block {name, text, ledger_generation}` written whenever a per-turn block is injected. The **ungated** half — extension transition events placed under the "System audit → `event_log`" row — is already applied in §5.4. |
| **T1** (P-28) | An owner-authored per-tool deny **rule** set (`[security.permissions] deny = [...]`, deny-class, gate-enforced) distinct from the S1 toggle? | No (record the rejection) | Phase 8 item 2's `/v1/tools` carries read-only `denied_by: "<rule>" \| null` — never a per-tool enable bit; design §11 becomes *migrate* rather than *purge*. |
| **T15** (P-16) | A named, fixed *ambient* allow set (`{workspace_read, workspace_write}` today) appended by the policy constructor to every fail-closed allowlist — and may `read_result` join it? | Yes; A0 ships with today's two-name set regardless | §5.4's `read_result` builtin is constructor-appended so no `Only(empty)` policy can be left unable to page into its own spilled result. If **no**: `read_result` is listed per template/skill and the spill stub omits its paging hint on surfaces that lack it. |
| **T3** (P-4) | A third LOADED axis — deferred extension-tool schemas — beside ALLOW and ENABLE? | Record the rule (load upfront when ≤ 10 % of the window, measured from real bytes — Phase 0 A4), build only when that threshold is observed crossed | An `extension_tool_loading = "auto" \| "always" \| "defer"` key documented beside `tool_selection` in `config/daemon.toml:74-81` — never a fourth meaning of `tool_selection`; design §6.2 #2 gains a bounded round-boundary exception. |

---

## 1. The `.openalpaca/` taxonomy — the contract

This section is normative. Every path in the system is built by one module (§1.4) and appears in one of the two trees below. New content kinds exist **only** when added to the `ContentKind` enum — the reservation mechanism is a code review on one enum, and `grep ContentKind::` enumerates every kind in the system.

### 1.1 `~/.openalpaca/` — the home root (app state *and* no-project content store)

```
~/.openalpaca/                        ← home_root(); OPENALPACA_HOME_STORE overrides (D4)
  README.md                           ← seeded once; explains every entry below
  .layout                             ← line 1: "1" — layout-version marker; line 2 (HOME root only): install_id=<uuid-v4>,
                                        written once by ensure_store(), never rewritten (pre-check (c); P-1)
  state/                              ← MACHINE STATE — opaque, never user-edited, never committed
    openalpaca.db  (+ -wal, -shm)
    discovery.json
    openalpacad.lock
    .master_key                       (0600)
    .last-sweep                       ← boot-time retention sweep stamp (§5.4; at most once per day)
    assets/                           ← interim: relocated content-addressed uploads, until the D2 re-home (Phase 8)
    backups/                          ← app-rotated copies of hand-edited files (config/*.toml, plugins/.permissions.toml):
                                        <name>.bak.<ts> ×5 + <name>.unparseable-<ts>; deletable, never user-edited (P-6)
    logs/
      daemon.log                      ← CLI-managed start (manager.rs:38); GAP-14 Phase B appender lands here too
  config/                             ← USER-EDITED runtime config (GUI/CLI-managed daemons):
                                        llm.toml, daemon.toml, mcp.toml, agents/, skills/, orchestrator/, tools/
  plugins/                            ← user-dropped plugin dirs + .permissions.toml + .config/<name>.toml (non-sensitive values)
    .data/<name>/                     ← per-plugin DURABLE state, survives update, removed on uninstall only with keep_data=false (P-7)
  artifacts/                          ← content store, home scope (no-project fallback) — §4 grammar
  uploads/                            ← content store, home scope (D2: uploads with no project signal)
  sessions/                           ← content store, home scope — session event logs (§5); ALL sessions live
                                        here, never in a project dir (transcripts must not be git-committable)
  memory/  skills/  scratch/  cache/  ← RESERVED names (§1.3); not created until used
```

The organising rule: **`state/` is the machine's; everything else at the root is the human's.** `config/` and `plugins/` stay top-level because the user edits and drops files there. The content-store kinds (`artifacts/`, `uploads/`, `sessions/`, …) sit flat at the root so the home root *is itself* a store with exactly the project-store shape — `content_dir(scope, kind)` is `root/kind` for both scopes, one code path, no special cases.

- **`.layout`** — line 1 is the layout-version integer, written by `ensure_store()`, read at boot; a future restructure gets the §2.2 mover pattern with a version gate instead of heuristics. Both roots carry one. Lines after the first are `key=value`: the **home** root's carries `install_id=<uuid-v4>` (written once, never rewritten — the id pre-check (c) needs and T10 would key keychain accounts on); the **project** root's may carry `project_id=<uuid>` (P-12). Nothing else is ever written there.
- **`README.md`** — seeded once per root; documents the state/content split, the reserved names, and "delete `state/` = factory reset; delete a content dir = lose those files only". **It carries a retention-class column (P-5)** — one class per entry, so the owner learns the rules from the directory and not from docs: `state/` (never swept; factory reset), `state/backups/` and `cache/` (regenerable; swept freely), `sessions/` (size-capped + optional age sweep, §5.4), `uploads/` + `artifacts/` (never garbage-collected, §4.5), `scratch/` (swept). `openalpaca store purge <project>|--all --dry-run` (Phase 8 item 15) prints the deletion plan in those terms and names what it will *not* touch.
- **`plugins/.data/<name>/` (P-7)** — the split Claude Code makes with `${CLAUDE_PLUGIN_ROOT}` (replaced on update) vs `${CLAUDE_PLUGIN_DATA}` (survives). Today a plugin runs with `current_dir(plugin_dir)` and only manifest-declared env (`process_pool.rs:36-48`), so anything it persists lands inside its own install dir and GAP-24's staged update (Phase 8 item 9) would erase it. The spawn passes `OPENALPACA_PLUGIN_ROOT=<plugin_dir>` and `OPENALPACA_PLUGIN_DATA=<home_root>/plugins/.data/<name>`; uninstall takes `keep_data: bool`, default **true** (§1.3 rule 3 argues for keep). Dot-prefixed, so rule 1 already claims the name. Stays inside D1.
- **`state/backups/` (P-6, P-11)** — the destination for the atomic writer's rotated `.bak.<ts>` copies (keep 5) and its copy-aside of anything unparseable; the design's fail-closed parse policy for `.permissions.toml`/`mcp.toml` is safe only because a typo can be undone from here. Machine-owned, so it lives under `state/`, not beside the human's `config/`.
- **`cache/` (P-10, forward guidance only)** — when derived previews/thumbnails/rendered pages ever exist they are the first `ContentKind::Cache` user (`cache/previews/<sha256-prefix>/…`), regenerated on miss and swept by the age/size pass — never beside D2's `uploads/`, where they would count against quota and survive forever. Phase 3 lands no server-side thumbnails (GAP-11 is inline `?token=` loading), so nothing uses it yet.

### 1.2 `<project>/.openalpaca/` — the project store

```
<project>/.openalpaca/
  README.md                           ← seeded once
  .gitignore                          ← store-owned, committable (contents below)
  .layout                             ← "1"
  artifacts/                          ← §4.2 grammar:
    <YYYY-MM-DD>-<task-slug≤48>-<taskid8>/NN-<slug>.<ext>
    loose/<YYYY-MM-DD>/…
    …/.versions/<stem>/vN.<ext>
  uploads/                            ← D2: chat uploads carrying x-workspace-path
    <YYYY-MM-DD>/NN-<orig-name-slug>.<ext>
  sessions/                           ← RESERVED, deliberately unused: session logs live under the HOME root
                                        only (§5.4); the name is reserved so nothing else ever claims it
  memory/                             ← RESERVED: future memory exports / project memory packs
  skills/                             ← RESERVED: future project-scope skills — this is the `project_dir` that
                                        SkillCatalog::scan_multi_scope + SkillScope::Project already implement
                                        and nothing calls (orchestrator/skill/catalog/mod.rs:150-169); whoever
                                        wires project skills points it HERE — do not invent a second resolution
  config/                             ← RESERVED: future per-project config overrides (daemon.toml fragments)
  scratch/                            ← RESERVED: agent working space that is neither artifact nor session
  cache/                              ← RESERVED: derived/regenerable data; always ignorable, always deletable
```

**Store-owned `.gitignore`** (supersedes rev 1's single-line `.versions/`):

```gitignore
/.layout
/uploads/
/sessions/
/scratch/
/cache/
.versions/
```

Rationale per line: artifact **heads stay committable** (rev 1's decision, kept — a produced `findings.md` is a document and git is strictly better history than `.versions/`); `uploads/` are copies of files the user already has elsewhere; `sessions/` would be private transcripts if anything ever landed there; `scratch/` and `cache/` are by definition regenerable; `.versions/` (unanchored — it appears per-run under `artifacts/`) is OpenAlpaca's private history. `memory/` and `skills/` are deliberately **not** ignored: an exported memory pack or a project skill is exactly what a project wants in git. The `.gitignore` is committable so the rules travel with the repo; `ensure_store()` writes it only when absent, so user edits stick.

**Rules reserved with the `config/` name (P-8 — one paragraph now, no machinery).** A cloned repository is untrusted input for a daemon that runs unattended, so when per-project fragments are designed: (1) a project fragment **never** carries extension enable or approval bits — those are home-store facts (`mcp.toml`, `.permissions.toml`; design §5); (2) any extension declaration found under a project store enters as `Unapproved{NeverSeen}` until approved, with its consent record in the **home** `.permissions.toml` keyed by project id (the design's §3.3 E1 precondition; whether that S3 reason may extend to MCP declarations is design §13 Q11/T8 — pending); (3) list keys (any future deny-class rule set, `require_confirmation_for`-style lists) **merge** across home and project scope, deny-class entries from either scope win, scalar keys take project over home; (4) `security.auto_approve_confirmations` and any per-agent `auto_approve` are **refused** from a project fragment (warn, keep the home value). Claude Code refuses `auto`/`bypassPermissions` from project files for the same reason.

**Consent rule reserved with the `skills/` name (P-9).** Anything under `skills/` that widens an agent's tool surface (a skill's `requires_capabilities`) is honoured only after a per-workspace consent row — `workspace_trust(workspace_id, accepted_at, digest)` — exists in the DB; unconsented project skills are listed but refuse to run, naming the reason. The table name is reserved now and built with project skills; the same rule applies to `config/` fragments touching `execution.*`. This goes *further* than Claude Code (which does not trust-gate a project skill's `allowed-tools`); OpenAlpaca's restrictive `tools.allow` semantics make the gate cheap to honour.

### 1.3 Naming rules and namespace reservation

1. Top-level entries in either store root match `^[a-z][a-z0-9-]*$` — lowercase, no spaces or underscores; plural nouns for content collections. Dot-prefixed names (`.layout`, `.gitignore`, `.versions`) are reserved for store metadata, forever.
2. **A new content kind exists when and only when it is added to `ContentKind`** (§1.4). No crate ever joins a literal directory name onto a store root.
3. Unknown directories found in a store root are left untouched and never swept — the store never deletes what it did not create (extends §4.5's "produced artifacts are never garbage-collected" to the whole tree). This is a retention class in all but name (P-5); the README table says so per entry, and §5.4's sweep of a rowless `sessions/<id>/` is consistent with it because that directory is store-created.
4. `state/` never gains user-facing content; content kinds never gain machine state. A future "machine state per project" kind (if one ever exists) gets a reserved `state/` name under the project store — reserved now, unused.

### 1.4 The single Rust module — `crates/openalpaca_storage/src/store/mod.rs`

`paths.rs` is **deleted, not aliased** (purge P1). Every path in both roots is built here and nowhere else. The rename fan-out is exactly the consumer inventory in appendix R §0 — every site is mechanical (`paths::app_dir()` → a specific accessor) and the compiler enumerates them.

```rust
// ── roots ────────────────────────────────────────────────────────────────
pub fn home_root() -> anyhow::Result<PathBuf>;          // $OPENALPACA_HOME_STORE (absolute) or ~/.openalpaca
pub fn state_dir() -> anyhow::Result<PathBuf>;          // home_root()/state — creates it
pub fn database_path() -> anyhow::Result<PathBuf>;      // state/openalpaca.db
pub fn discovery_path() -> anyhow::Result<PathBuf>;     // state/discovery.json
pub fn lock_path() -> anyhow::Result<PathBuf>;          // state/openalpacad.lock
pub fn master_key_dir() -> anyhow::Result<PathBuf>;     // = state_dir(); passed to KeyEncryptor::ensure_at
pub fn logs_dir() -> anyhow::Result<PathBuf>;           // state/logs — creates it
pub fn backups_dir() -> anyhow::Result<PathBuf>;        // state/backups — creates it; the atomic writer's rotation target (P-6/P-11)
pub fn interim_assets_dir() -> anyhow::Result<PathBuf>; // state/assets — dies with the D2 re-home (Phase 8)
pub fn plugins_dir() -> anyhow::Result<PathBuf>;        // home_root()/plugins (replaces main.rs:331's inline join)
pub fn runtime_config_dir() -> anyhow::Result<PathBuf>; // home_root()/config (GUI/CLI-managed OPENALPACA_CONFIG_DIR value)

// ── content stores (both scopes share one shape) ─────────────────────────
pub enum StoreScope { Project(PathBuf), Home }
pub enum ContentKind { Artifacts, Uploads, Sessions, Memory, Skills, Scratch, Cache }
pub fn store_root(scope: &StoreScope) -> anyhow::Result<PathBuf>;    // <project>/.openalpaca | home_root()
pub fn ensure_store(scope: &StoreScope) -> anyhow::Result<PathBuf>;  // creates + seeds README/.gitignore/.layout
pub fn content_dir(scope: &StoreScope, kind: ContentKind) -> anyhow::Result<PathBuf>;
pub fn layout_version(root: &Path) -> anyhow::Result<Option<u32>>;

// ── artifact grammar (rev 1 §1.3, carried; ArtifactScope → StoreScope) ───
pub fn run_dir(scope: &StoreScope, created: DateTime<Utc>, task_title: &str, task_id: &str) -> anyhow::Result<PathBuf>;
pub fn loose_dir(scope: &StoreScope, created: DateTime<Utc>) -> anyhow::Result<PathBuf>;
pub fn artifact_file_name(seq: u32, title: &str, ext: &str) -> String;
pub fn slugify(input: &str, max_bytes: usize) -> String;             // pure, total, never empty
pub fn version_file_path(head_path: &Path, version: u32) -> anyhow::Result<PathBuf>;
pub fn artifact_extension(kind: ArtifactKind, mime: Option<&str>, name_hint: Option<&str>) -> String;
pub fn confine_to_root(root: &Path, candidate: &Path) -> anyhow::Result<PathBuf>;

// ── D2 upload placement ──────────────────────────────────────────────────
pub fn upload_dir(scope: &StoreScope, created: DateTime<Utc>) -> anyhow::Result<PathBuf>;  // uploads/<YYYY-MM-DD>
pub fn upload_file_name(seq: u32, original_name: &str) -> String;    // NN-<slug(orig,60)>.<ext>

// ── session paths (§5) ───────────────────────────────────────────────────
pub fn sessions_dir() -> anyhow::Result<PathBuf>;                    // content_dir(Home, Sessions)
pub fn session_dir(id: &str) -> anyhow::Result<PathBuf>;
pub fn session_log_path(id: &str) -> anyhow::Result<PathBuf>;
pub fn session_result_path(id: &str, seq: u64, tool: &str, ext: &str) -> anyhow::Result<PathBuf>;

// ── the mover (store/migrate.rs) ─────────────────────────────────────────
pub fn move_app_root();                                  // §2.2; called from main.rs at the old :71 slot
pub fn rebase_asset_paths(db: &Database);                // §2.2 step 6; called from bootstrap after DB open
```

`asset_storage_path(sha256)` (`paths.rs:75-83`) is **deleted**: under D2 new uploads are human-named, dedup keys off the `sha256` column, and existing blobs at `interim_assets_dir()` are addressed purely via their stored `storage_path` until the Phase 8 re-home deletes even that.

**One atomic writer for hand-edited config (P-11).** Three writers of user-edited TOML in two crates — `persist_only` for `llm.toml` (Phase 8 item 4), the `mcp.toml` writer and the `.permissions.toml` writer (design §2.1/§2.2) — would otherwise grow three tmp+rename implementations with different crash semantics. There is one: `atomic_write_with_backup(path, bytes, keep = 5)` behind the design's `openalpaca_core::config_io::atomic_write_toml` (it must live in `openalpaca_core`, not `store/`, for the dependency-graph reason the design gives — `openalpaca_storage` is a leaf below it); `store/mod.rs` only exposes `backups_dir()`. Semantics: write `.tmp` → `fsync` → rotate the current file to `state/backups/<name>.bak.<ts>` (keep five) → rename; a file that fails to parse at read time is copied aside as `<name>.unparseable-<ts>` before the fail-closed policy applies. All three writers route through it.

---

## 2. The move to `~/.openalpaca/`

### 2.1 Root resolution

```rust
/// $OPENALPACA_HOME_STORE if set (absolute path), else <home>/.openalpaca — every platform.
pub fn home_root() -> anyhow::Result<PathBuf>;
```

- Read on every call, like `app_dir()` today — no caching, so tests set it per-process. Non-absolute values are rejected.
- Home dir via `directories::BaseDirs::home_dir()` (already a workspace dep — `crates/openalpaca_storage/Cargo.toml:14`). `ProjectDirs` survives only inside the mover, to compute the *old* root.
- `OPENALPACA_CONFIG_DIR` **semantics are untouched** (`bootstrap/config.rs:14-61`). Only the *value* the GUI/CLI pass changes automatically — both compute `app_dir()/config` → now `home_root()/config` (`apps/openalpaca-gui/src-tauri/src/lib.rs:114`, `apps/openalpaca/src/manager.rs:217`). Dev runs from the repo keep resolving `./config` via the exe/CWD walk-up and never notice the move — dev-run LLM keys and persona docs are entirely unaffected.

### 2.2 The one boot-time mover — `store::migrate::move_app_root()`

Replaces `migrate_legacy_app_dir()` at the same call slot (`main.rs:70-71`): **after logging, before the singleton lock** — the same ordering constraint documented at `paths.rs:24-27`, because the lock file itself moves and the DB must not be open mid-rename. `old` = `ProjectDirs::from("","","OpenAlpaca").data_dir()` (exactly today's `app_dir()`, per platform); `new` = `home_root()`.

Algorithm — every step idempotent, the whole function re-runnable:

1. **Fresh install / already moved:** `old` absent ⇒ return. `old == new` (paranoia under `OPENALPACA_HOME_STORE`) ⇒ return.
2. **Live-daemon guard:** non-blocking `file_lock` probe on `old/openalpacad.lock` (same mechanism as `discovery/mod.rs:201-217`). Held ⇒ abort startup: "an old daemon is still running from `<old>`; stop it first". This is the one race the mover refuses to paper over — renaming a WAL-mode DB out from under a live process is corruption.
3. **Entry ledger**, each moved by `std::fs::rename` (same volume on all three platforms, so rename is atomic; an `EXDEV` error aborts with a message rather than falling back to a non-atomic copy):

   | Old entry | New location | Note |
   |---|---|---|
   | `openalpaca.db-wal`, `openalpaca.db-shm`, then `openalpaca.db` | `state/` | **Sidecars first**: a crash mid-trio leaves split halves; the resume on next boot reunites them *before* `Database::open` runs — SQLite pairs WAL with DB only at open time, and the mover always completes before the open (`main.rs:186`) |
   | `.master_key` | `state/` | Replaces the inline legacy-key copy at `main.rs:102-127` (deleted — P2) |
   | `config/` | `config/` | **Per-child merge**, not skip-if-dir-exists: a rebuilt GUI pre-creates `home_root()/config` *before* spawning the daemon (`src-tauri/lib.rs:114-115`), so the destination existing is expected; each child (`llm.toml` with its encrypted keys, `daemon.toml`, `orchestrator/`, `skills/`, …) moves if absent at the destination. Runs before `seed_default_configs` (`main.rs:92`), so the moved `llm.toml` wins over a fresh seed |
   | `plugins/` | `plugins/` | Same per-child merge; carries the user-approved `.permissions.toml` files |
   | `assets/` | `state/assets/` | Interim home; the D2 re-home into `uploads/` is Phase 8's job, not the mover's |
   | `daemon.log` | `state/logs/daemon.log` | CLI-managed-start log (`manager.rs:38`) |
   | `discovery.json`, `openalpacad.lock` | **deleted** | Regenerated every boot (`main.rs:178-183`); moving a stale discovery file would only confuse `ensure_not_expired` |

4. **Partial failure:** every entry is one atomic rename, guarded by skip-if-destination-exists. The first failure **aborts startup** with the failing path in the error. No rollback, and none needed — the next boot resumes exactly where it stopped, and no consumer opens any of these files before the mover finishes.
5. **Old root disposal:** `remove_dir(old)` if empty; otherwise log a warning listing leftovers and leave them. No `MOVED.txt` ceremony — single user, one machine.
6. **Post-open fixup** — `rebase_asset_paths(db)`, called in bootstrap immediately after `Database::open`, before any ingress, beside `sweep_orphaned_tasks` (`main.rs:188-193`): one idempotent statement repairing the stored absolute paths broken by the move:

   ```sql
   UPDATE file_assets
      SET storage_path = replace(storage_path, '<old>/assets/', '<new>/state/assets/')
    WHERE storage_path LIKE '<old>/assets/%'
   ```

   Not a numbered migration — the prefixes are runtime-computed. Runs every boot, matches zero rows after the first. It keeps §4.3's "readers need zero changes" promise intact through the move.
7. **The `com.openalpaca` leg is not carried forward.** `migrate_legacy_app_dir` chained `com.openalpaca.OpenAlpaca → OpenAlpaca` (`paths.rs:28-51`); that rename has long since happened on the only machine that matters. The new mover reads only today's root; a surviving `com.openalpaca.*` dir would simply be ignored.

### 2.3 Rebuild coordination, and why the mover exists at all

**Discovery consumers need zero code changes** — everything goes through `openalpaca_storage::{paths,discovery}` (full inventory: appendix R §0). They need a **rebuild, atomic across daemon + CLI + GUI**: an old GUI binary would read discovery from the old root, conclude no daemon runs, and spawn-loop forever. One commit, all three binaries; no compatibility window is designed — deliberate (directive 2, single user).

Skipping the mover would not *malfunction* — the daemon would boot fresh — but it would lose: the entire DB (conversations, memories, tasks, telemetry, and the persisted `identity.local_user_id`, so the lane key changes); installed plugins and their approval state; uploaded bytes; and, for GUI/CLI-managed setups, the runtime `config/` with the encrypted LLM keys and live persona docs. The mover is ~80 lines and removes that worst case entirely.

---

## 3. The legacy purge — Phase 1's deletion list

Directive 2: previously-added legacy measures are **cleared from the code**. Verdicts: **DELETE** (this branch, Phase 1 unless noted), **DELETE-AFTER** (safe once a named dependency lands), **KEEP** (looks legacy, is load-bearing), **OWN-TASK** (a real refactor, not a deletion). Serde's general ignore-unknown-fields behaviour is not listed — it is load-bearing correctness.

| # | Item | Where | Verdict |
|---|---|---|---|
| P1 | `migrate_legacy_app_dir()` + the `com`/`openalpaca` qualifier constants | `paths.rs:24-51`, call at `main.rs:70-71` | **DELETE** — replaced by `move_app_root()` in the same slot (§2.2). `paths.rs` tests move to `store/`; `test_paths_are_consistent` (`paths.rs:89-102`) re-targets `state_dir()`. |
| P2 | Inline legacy master-key copy (`config_base_dir/.master_key` → `app_dir`) | `main.rs:102-127` | **DELETE** — the mover's `.master_key → state/` entry owns relocation; no config dir on the machine still holds a key. |
| P3 | `.alpaca` marker preference | `memory/workspace.rs:43-49` (incl. the redundant `.exists() \|\| .is_dir()`), tests `:108,125` | **DELETE — recognise `.openalpaca` first, `.git` second, `.alpaca` never.** Honest cost: a directory that was a workspace root *only* via a hand-made `.alpaca` stops resolving, and memories scoped to it (workspace id = canonical path) go quiet until the user runs `mv .alpaca .openalpaca` once. State it in the commit message. Tests flip to `.openalpaca`. |
| P4 | Legacy flat `llm.toml` format branch | `crates/openalpaca_llm/src/config/llm_config/router_builder.rs:26-53` (`build_router_from_legacy`) | **DELETE** — the seeded template has been hierarchical (`[providers.*]`, `scripts/release/templates/config/llm.toml:8-21`) for as long as `seed_default_configs` has existed. **Pre-check §0(b)** before removing `LlmConfig`/`build_provider_with_runtime` with the branch. Drop the "auto-detects format" doc comment. |
| P5 | Skill frontmatter legacy fields + bridge: `command`, `trigger_patterns`, `tools_required`, `auto_load`, `apply_legacy_compat()`, legacy half of `effective_slash_command()` | `crates/openalpaca_core/src/middleware/skill/types.rs:335-347,352-371,384-392`; call sites `skill/mod.rs:55,65`; tests `skill/tests.rs:44-54,117,262-289` | **DELETE** — no tracked skill uses them (grep over `config/skills/` empty); plugin-contributed skills build the *new* sections + `..Default::default()` (`openalpaca_plugins/src/manager.rs:940-956`), so they compile unchanged. |
| P6 | `TaskConstraints.pipeline_sequential` | `orchestrator/task_state/state.rs:36,74`; tests `task_state/tests.rs:29,295`, `dispatcher/tests.rs:531` | **DELETE** — serde vestige of the deleted sequential pipeline; zero non-test readers. Old `state_json` rows deserialize fine (unknown-field ignore). `test_backward_compat_no_workspace_field` stays (the DB genuinely holds pre-workspace rows); its fixture drops the key. |
| P7 | `planner_ms`/`dispatch_ms` schema-stability zeros, `mean_planner_ms`, `dispatch_decisions.planner_requested_mode` | event `events.rs:250-260`; writers `handlers.rs:155-157,365-395`, `event_bridge.rs:330-341`; repo+tables `repository/orchestrator_latency/mod.rs:13,36-63,141-161,183` (022), `repository/dispatch_decision/mod.rs:18,38-48,68,103` (024); `dispatcher/mod.rs:223`; GUI `lib/api/types.ts:432,446,462` | **DELETE end-to-end** — they exist only "for schema stability" (`handlers.rs:156-157`, verbatim) and **no GUI view renders them** (verified: only `types.ts`/`orchestrator.ts`/`useOrchestrator.ts` carry the types). **Migration 035** (`035_drop_planner_telemetry.sql`): `DROP COLUMN` ×3 (pre-check §0(a); else 024-style rebuild). Retired mode *strings* in historical rows are data — untouched. Tests: `orchestrator_latency/tests.rs:55-85`, `dispatch_decision/tests.rs:23-77`. |
| P8 | Legacy `assignments`/`assigned_agents` task payload | `routes/tasks_types.rs:35-36`, `routes/tasks.rs:8,32,173,513-531`; CLI reader `commands/tasks.rs:115-133` | **DELETE-AFTER Phase 4** — `subagent_span` + `GET /v1/tasks/{id}/timeline` replace the data. When the timeline lands, the `assignments` key, its serde rename, the GUI/CLI parsers, and `test_task_response_serializes_agent_runs_under_assignments_key` are deleted in the same PR; GAP-20's run counts re-point at `subagent_span`. A Phase 4 **exit criterion**, not a deferred maybe. |
| P9 | `DagNodeStatus`/`DagNodeStarted` double emission | producers `runner/lead_agent/tools.rs:232-240`; GUI `components/work/run-events.ts:63-71` | **DELETE-AFTER the GUI switches to `SubagentSpan`** (Phase 8). No soak ceremony. |
| P10 | Backward-compat re-exports in the LLM crate root | `crates/openalpaca_llm/src/lib.rs:12-23` (the TODO says exactly this) | **DELETE** — fix consumers to canonical `routing::…` paths; the compiler enumerates them. |
| P11 | `resolve_local_user_id`'s legacy `gui_user:gui` adoption | `bootstrap/migration.rs:40-60` | **DELETE the fallback branch** — `identity.local_user_id` is persisted on the only real install; fresh DBs mint a UUID. Keep the persisted-id read. |
| P12 | `migrate_preference_summaries` (one-time summary migration that runs every boot) | `bootstrap/migration.rs:80-…`, called `main.rs:190` | **DELETE** — completed one-time data migration; matches zero rows every boot. |
| P13 | Legacy second-precision persona-backup filename parsing | prune logic, `tools/builtins/helpers/tests.rs:125-145` | **NOTHING TO PURGE — verified 2026-09-05.** `prune_backups()` (`tools/builtins/helpers/mod.rs`) never parses a timestamp: it filters by `<PREFIX>.*.md` and sorts lexicographically, oldest-first — a plain string sort, no date-format branch to delete. "Second-precision" is only a comment in `test_prune_backups_with_mixed_filename_formats`, describing one of the fixture filenames it sorts against. |
| P14 | CLI key-removal "exactly one key (legacy)" fallback | `apps/openalpaca/src/commands/ai_config.rs:194-199` | **DELETE** — every key on the machine post-dates `{provider}_cli` naming. XS. |
| P15 | Dual timestamp formats in `event_log` | `repository/event_log/mod.rs:96-105` | **Shipped:** the normalising `UPDATE` landed inside migration 035 (`035_drop_planner_telemetry.sql`) alongside the P7 column drops. Id-based pagination (Phase 4) stays regardless — the right key even with clean timestamps. |
| P16 | `secret_encrypted` "legacy encrypted (read-only)" tier | `router_builder.rs:151-160`, `config/llm_config/migration.rs:8-40` | **KEEP** — despite the comment it is the functional no-keychain fallback (the CI dbus dance exists precisely because keychains are environmental), and CLAUDE.md documents the three-tier resolution as a feature. Relabel the comment; delete nothing. |
| P17 | `AgentConfigFile` TOML shape + "legacy instance" registration | `agent/config_service.rs:160-247`, `agent/config/mod.rs:14-66`, `routes/agents.rs:496,539`, `routes/agents_types.rs:28-40` | **OWN-TASK** — the "legacy" shape is the *live* HTTP contract: the GUI posts `AgentConfigFile` JSON today (`lib/api/agents.ts:31-48,93-97`) and the idle-instance registration backs `/v1/agents`. Collapsing template-vs-instance duality is an API redesign; flagged, not swept into this purge. |
| P18 | CLI structured-delegation fallback; retired routing-mode strings in GUI label maps | `apps/openalpaca/src/chat_stream/mod.rs:288-293`; GUI grep | **NOTHING TO PURGE — verified.** The CLI parses only the structured `delegation` object; no GUI label map carries the retired mode strings (only the P7 type fields). Recorded so nobody hunts again. |
| P19 | `/v1/conversations` routes | `router.rs:144-150` | **DELETE-AFTER Phase 7** — deleted, not aliased, in the sessions PR; `/v1/sessions` replaces them (§5.7) and rev 1's GAP-21 is built **once**, on sessions. GUI/CLI callers updated same PR; nothing outside this repo calls them. |
| P20 | `GET /v1/events/history` bare-array compat | rev 1 Phase 3 spec (never built) | **NEVER BUILT — revises rev 1.** Always return the envelope; update the one CLI call site (`apps/openalpaca/src/commands/tasks.rs:310`) in the same PR. The dual-shape route and its risk row are gone. |

**Explicitly not legacy — do not clear:** `conversation_map` (live platform-chat-id → lane mapping used by every connector), `lane_key` on `conversation_messages` (still the routing address and the index the prompt window reads — `context_builder.rs:30-38`), and `event_log` (system audit, §5.6).

**Rev 1 hedges the move dissolves:** `storage_path` staying an absolute resolved path survives — but as a *convenience with a boot-time rebase* (§2.2.6), no longer a compat constraint; `asset_storage_path` "unchanged" is reversed by D2; rev 1 §1.11's "no files move" is reversed wholesale (§4.10).

**Documentation sweep** (same PR as the mover): `docs/QuickStart_Manual.md:48`, `docs/GUI_Manual.md:41-42`, `docs/Installation_Manual.md:111-113,145,167,192`, `docs/Daemon_Manual.md:42-45`, `docs/CLI_Manual.md:47,96,222`, `apps/openalpaca-gui/README.md:126-127`, `apps/openalpaca-gui/API_MAP.md:37,777-778,932`, `scripts/release/install.sh:135`, `scripts/release/uninstall.sh:43`, CLAUDE.md's "Data directory" paragraph — all name `~/Library/Application Support/OpenAlpaca`. Pleasant side effect: the hint at `apps/openalpaca/src/commands/plugin.rs:225,230` (`~/.openalpaca/plugins/.config/`) becomes *literally* true — the `.config` suffix is **real** (`main.rs:331`, `permission_gate.rs:23,37`), not spurious; rev 2's "fix the suffix" would have made the hint wrong (P-30). Phase 8 item 14 replaces the file hint with a redacting `plugin config get` instead of correcting a path.

---

## 4. The artifact store

### 4.1 Layout

The store trees are §1.1/§1.2. The artifact-specific shape, unchanged from rev 1:

```
artifacts/
  2026-09-01-connector-audit-3f2a1b7c/    ← <YYYY-MM-DD>-<task-slug≤48>-<taskid8>
    01-connector-audit-findings.md        ← HEAD, currently v2
    02-migration-plan.md                  ← HEAD, v1
    03-screenshot-settings-drawer.png
    .versions/
      01-connector-audit-findings/
        v1.md                             ← SUPERSEDED only; head is never duplicated
  loose/2026-09-01/
    01-weekly-report.html                 ← produced outside any task (main loop)
```

**The daemon CWD is never used to place an artifact.** Under a Tauri sidecar or a LaunchAgent it is arbitrary. CWD-derived workspace ids remain fine for memory scoping (existing behaviour at `orchestrator/handlers.rs:90-97`) but must not decide where bytes land. The no-project fallback is the **home root's own content dirs** (single root — same shape, same code path).

### 4.2 Path grammar

```
run_dir  := <YYYY-MM-DD> "-" slug(task_title, 48) "-" taskid[0..8]      ≤ 68 bytes
file     := NN "-" slug(name, 60) "." ext                                ≤ 72 bytes
slug     := [a-z0-9]+ ("-" [a-z0-9]+)*
ext      := [a-z0-9]{1,8}
version  := <run_dir>/.versions/<stem>/v<N>.<ext>
```

- **Date first** so `ls` sorts chronologically; **UUID8 suffix** so a run dir is collision-free and greppable from a task id (task ids are UUIDv4, `dispatcher/lead_agent.rs:32`).
- **`NN-` sequence prefix** assigned at first creation, retained across versions, so `ls` shows production order. Two digits, widening to three past 99.
- **Head lives at the clean path** — what makes `open`, Reveal in Finder, `grep` and `git diff` work, and lets `POST /v1/files/{id}/open` skip its `$TMPDIR` staging copy (`routes/files_types.rs:92-118`), which exists only because content-addressed paths have no extension.
- **Slugification:** NFKD → drop combining marks → transliterate to ASCII → **lowercase** (required: APFS is case-insensitive) → collapse non-`[a-z0-9]` runs to `-` → trim → truncate to 60 bytes on a char boundary → empty ⇒ `artifact` → Windows reserved device names (`con`, `prn`, `aux`, `nul`, `com1..9`, `lpt1..9`) get a `_` prefix.
- **Traversal safety falls out of the grammar** — separators cannot survive slugification. Belt and braces: confine the final path to the store root with the canonicalize-the-parent technique at `tools/builtins/helpers/mod.rs:65-117`.
- **Write protocol:** write `.<stem>.tmp` → `fsync` → rename current head into `.versions/<stem>/v<N-1>.<ext>` → rename tmp to head. A crash leaves either the old head or the new one, never a truncated file.

**Upload grammar (D2):** `uploads/<YYYY-MM-DD>/NN-<slug(original_name,60)>.<ext>` — same slugifier, same confinement, same write protocol. Dedup is the existing owner-scoped sha256 *query*, not the path.

**Marker precedence** (`memory/workspace.rs:43-56`): `.openalpaca` first, `.git` second, `.alpaca` **deleted** (P3). Deliberate side effect, documented: writing the first artifact makes that directory a workspace root for memory scoping — desirable (the artifact write is exactly the moment the directory becomes an OpenAlpaca project), and intentional.

### 4.3 The DB holds addresses

Principle: **`project_root` + `rel_path` are the address of record; `storage_path` stays the resolved absolute path** so `routes/files.rs:315`, `files_types.rs:142-145` and `background.rs:339` need zero changes — kept valid across the root move by `rebase_asset_paths` (§2.2.6).

**One table, not two.** `file_assets` is extended; there is **no** parallel `artifacts` table:

- One content route, one id space, one client `Artifact` type — no client-side merge of uploads and produced files in the Library.
- It unblocks GAP-23 for free: `conversation_message_attachments.file_id REFERENCES file_assets(id)` (`migrations/028_message_attachments.sql`) carries artifact links via its existing `role TEXT NOT NULL DEFAULT 'attachment'` column.
- `origin` (`'upload' | 'produced'`) selects the placement strategy; everything else is shared.

**The real blob-in-DB offender is `TaskWorkspace`**: `WorkspaceEntry { content: String, .. }` capped at 32 768 × 50 entries (`orchestrator/task_state/workspace.rs:20-47`), persisted as one `state_json` TEXT column rewritten under optimistic locking on every mutation — up to 1.6 MB per task. §4.6's spill is the directive's core win. (§5 deliberately adds nothing to `state_json`.)

### 4.4 Migration 036 — `036_artifact_store.sql`

```sql
-- Migration 036: project-scoped artifact store.
-- Extends file_assets into the unified artifact record; adds version history.

-- Ownership and attribution (GAP-04)
ALTER TABLE file_assets ADD COLUMN origin TEXT NOT NULL DEFAULT 'upload';  -- 'upload' | 'produced'
ALTER TABLE file_assets ADD COLUMN kind TEXT;                -- ArtifactKind; NULL for legacy uploads
ALTER TABLE file_assets ADD COLUMN task_id TEXT REFERENCES task(id) ON DELETE SET NULL;
ALTER TABLE file_assets ADD COLUMN agent_id TEXT;            -- runtime instance id, "review_agent::a1b2c3d4"
ALTER TABLE file_assets ADD COLUMN agent_template_id TEXT;   -- "review_agent"

-- The address (the directive)
ALTER TABLE file_assets ADD COLUMN project_root TEXT;        -- NULL => home store
ALTER TABLE file_assets ADD COLUMN rel_path TEXT;            -- HEAD path relative to <store>/artifacts (or /uploads)

-- Versions (GAP-05)
ALTER TABLE file_assets ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE file_assets ADD COLUMN version_count INTEGER NOT NULL DEFAULT 1;

-- UI affordances
ALTER TABLE file_assets ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;   -- GAP-12
ALTER TABLE file_assets ADD COLUMN summary TEXT;             -- "+41 −6" / "exit 0 · 1.4s" / "3 rows"
ALTER TABLE file_assets ADD COLUMN missing_since TEXT;

CREATE INDEX IF NOT EXISTS idx_file_assets_task    ON file_assets(task_id);
CREATE INDEX IF NOT EXISTS idx_file_assets_origin  ON file_assets(origin, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_file_assets_project ON file_assets(project_root);
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_assets_addr
    ON file_assets(COALESCE(project_root, ''), rel_path) WHERE rel_path IS NOT NULL;

CREATE TABLE IF NOT EXISTS artifact_versions (
    artifact_id     TEXT    NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    rel_path        TEXT    NOT NULL,   -- '.versions/<stem>/v1.md', or = head rel_path for the head
    sha256          TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL,
    note            TEXT,               -- model-authored "why this version"
    author_agent_id TEXT,               -- NULL => a human edited the file by hand
    added_lines     INTEGER,            -- NULL on v1
    removed_lines   INTEGER,            -- NULL on v1
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (artifact_id, version)
);
CREATE INDEX IF NOT EXISTS idx_artifact_versions_artifact ON artifact_versions(artifact_id, version DESC);

-- The project a run belonged to; makes `rerun` faithful and lets the Library
-- filter by project without a join through file_assets.
ALTER TABLE task ADD COLUMN workspace_id TEXT;
CREATE INDEX IF NOT EXISTS idx_task_workspace ON task(workspace_id);

UPDATE schema_version SET version = 36 WHERE version = 35;
```

SQLite legality, checked: `ADD COLUMN` with `REFERENCES` is legal only with a NULL default (which `task_id` has; `foreign_keys = ON` at `database/mod.rs:62` enforces it); `NOT NULL DEFAULT` is legal for the rest; partial unique indexes need ≥ 3.8.0.

### 4.5 Two mandatory same-PR bug fixes

Migration 036 creates two live defects the moment it lands. **Both must be in the same commit as the migration.**

```sql
-- repository/file_asset/mod.rs:95-112 — orphan sweep (background.rs:308-357, every 6 h, 24 h grace).
-- WITHOUT THIS the sweep deletes every produced artifact AND its file — a produced
-- artifact is never linked to a conversation message.
   WHERE a.id IS NULL
     AND f.origin = 'upload'      -- ← ADD
     AND f.pinned = 0             -- ← ADD
     AND f.created_at < datetime('now', ?1)
```

```sql
-- repository/file_asset/mod.rs:76-85 — quota, read at routes/files.rs:84-97 against a 500 MB cap.
-- WITHOUT THIS agent output written into the user's own project counts against the
-- upload quota and starts rejecting uploads.
SELECT COALESCE(SUM(size_bytes), 0) FROM file_assets WHERE origin = 'upload'
```

Produced artifacts are **never** garbage-collected — they are the user's files. If retention is ever wanted it goes behind an explicit config key, defaulted off. The sweep's file-deletion path (`background.rs:341`) deletes under `state/assets/` (interim) or `uploads/` (post re-home) through the same `storage_path` mechanism — no extra change.

### 4.6 The producer

Today nothing creates a `FileAsset` for agent output; the only producers are the upload route (`routes/files.rs:220-247`) and the connector attachment path. Two additions:

1. **`artifact_write(name, kind, content, note?, summary?, metadata?) -> { artifact_id, path, version }`** — a builtin next to `file_write`, capability `artifact_write`. **Critical:** it resolves its scope from `ToolContext.workspace_id` via `StoreScope`/`content_dir`, **not** the startup-captured `workspace_root` that `file_write` uses (`services/tools.rs:37-39` → `tools/builtins/mod.rs:249-251`) — the first per-request file writer in the codebase. Attribution from `ToolContext.task_id`/`agent_id`.
2. **`workspace_write(entry_type="artifact")` spills.** Content goes to the store; the entry keeps `file_asset_id` plus a 512-char preview for prompt assembly (`format_for_prompt` truncates at 2000 anyway — `task_state/workspace.rs:157`).

Payoff chain: `ArtifactPointer.file_asset_id` (`task_state/outcome.rs:148-161`) resolves for the first time → `task.artifact_count` becomes meaningful → `GET /v1/tasks/{id}` gains real artifact references → **`deliver_artifacts` (`apps/openalpacad/src/notification/artifacts.rs:55-110`) starts working** — already-written, currently-dead feature (it silently `continue`s because the id is always `None`), switched on for free.

New caps in `[execution.artifacts]`: `max_artifact_bytes` default **10 MB** (matching `file_write`'s cap at `tools/builtins/file_ops.rs:102-110`, not the 50 MB upload cap — this content comes out of a context window), `max_versions_per_artifact` default 20 (prune the oldest `.versions/` file + row; head never pruned).

### 4.7 Where the project comes from (prerequisite)

**No client sends a workspace today.** `POST /v1/chat` reads `x-workspace-path` (`routes/chat.rs:65-68`) and `POST /v1/command` reads `workspace_path` (`routes/command.rs:78-80`), but no call site sets either (grep of `apps/openalpaca-gui/src` and `apps/openalpaca/src`: only the definition at `lib/chat-stream.ts:349-366`). So `handlers.rs:90-97` always takes the else branch. Ship, in order:

1. Home-root fallback for placement; never the daemon CWD. *(store-side, required)*
2. **The GUI sends `x-workspace-path`** on `POST /v1/chat` when a project is chosen — plumbing exists, only the caller is missing. The CLI sends `workspace_path` on `/v1/command` (Phase 7 CLI work completes this for both clients).
3. `task.workspace_id` persisted at dispatch (036) so a run remembers its project across restart and rerun.
4. `GET /v1/status` reports `home_root`, `state_dir`, `db_path`, and the resolved project dir.
5. **(D2)** `POST /v1/files/upload` reads `x-workspace-path` like `/v1/chat` does (the route currently reads no header — `routes/files.rs:24-27`) and writes via `upload_dir(scope, …)`; connector uploads have no project signal and take `StoreScope::Home`. **Collapse the connector's duplicated sha/dedup write path (`connectors/src/common/mod.rs:213-262`) into the same writer first** — it is two writers today, and D2 makes that a bug factory.

A fuller project concept (`GET /v1/projects`, activation) stays **out of scope** (§10). Whoever adds it must reconcile with `SkillCatalog::scan_multi_scope`/`SkillScope` (`orchestrator/skill/catalog/mod.rs:154`) — and point project skills at `.openalpaca/skills/` (§1.2), not a second resolution.

### 4.8 Integrity and lifecycle

| Concern | Design |
|---|---|
| File missing | `resolve_content` stats before serving; on absence sets `missing_since`, returns **410** `{error:{code:"ARTIFACT_GONE",…}}`. List rows carry `missing:true`; `?include_missing=` (default `false`). |
| Project moved | `rel_path` + `project_root` make re-basing one statement — and **one transaction** (P-12): `ArtifactStore::rebase_project(old,new)` also updates `session.workspace_id` (039), `task.workspace_id` (036) and the memory scope key (`memory/workspace.rs:58-64`), all of which carry the same canonical-path string. Exposed as `PATCH /v1/workspaces {old_path,new_path}` plus a CLI verb (Phase 8 item 11), not only via the project picker. Claude Code's encoded-cwd directories are the cautionary example — a rename strands transcripts, memory and history under the old name; a path-derived identity is cheap only if re-basing is one transaction. Optionally `project_id=<uuid>` on line 2 of the project `.layout` so a moved project is recognisable when the path changes but the id matches. |
| Home root moved | Impossible by construction — the home root *is* the address baseline (`project_root = NULL`); `OPENALPACA_HOME_STORE` changes are a config event, not data. |
| Project deleted | Rows survive as `missing`. No auto-deletion. |
| User edits a file by hand | Explicitly supported — the point. `sha256`/`size_bytes` go stale; next `put` (or `verify`) records the edit as a version with `author_agent_id = NULL` (Phase 8). |
| Concurrent writes | Serialize on `ArtifactStore::put`: version rotate inside one `with_connection` transaction, tmp+rename for the bytes. Last writer is head; both survive in `.versions/`. |
| Size accounting | Two numbers, never one: `upload_bytes` (quota-bearing regardless of placement) and `produced_bytes` (informational, per project). Both on `GET /v1/status`. |
| Two daemons on one project | Not designed for; the unique index is per-DB. The singleton lock prevents two per machine. Documented limitation. |

`ArtifactStore` (in `crates/openalpaca_storage/src/artifacts/mod.rs`) is **the one writer** for produced rows: `put / get / list / resolve_content / versions / diff / set_pinned / rebase_project / verify` (signatures: rev 1 §3.4, unchanged). `FileAssetRepository` keeps owning uploads with `origin` defaulted; the two coexist on one table.

### 4.9 HTTP surface

```http
GET  /v1/artifacts?task_id=&kind=&origin=&project_root=&pinned=&q=&include_missing=&limit=&offset=
     200 { "artifacts": [Artifact], "total": 1234 }
GET  /v1/artifacts/{id}                                    200 Artifact | 404
GET  /v1/artifacts/{id}/content?token=<bearer>[&version=N] 200 bytes | 410 ARTIFACT_GONE
GET  /v1/artifacts/{id}/versions                           200 { "versions": [ArtifactVersion] }
GET  /v1/artifacts/{id}/versions/{n}/content?token=        200 bytes
GET  /v1/artifacts/{id}/diff?from=1&to=2                   200 ArtifactDiff | 409 NOT_DIFFABLE
PUT  /v1/artifacts/{id}/pin  {"pinned":true}               200 { "id", "pinned" }
GET  /v1/files/{id}/content?token=<bearer>                 ← same ?token= change on the existing route
```

`Artifact` is a **superset** of `unbacked.ts:39-56` (client type compiles unchanged); additive fields `origin`, `pinned`, `missing`, and the directive's payoff — `path`, `project_root`, `rel_path`. `ArtifactVersion`/`ArtifactDiff` match `unbacked.ts:62-77` field-for-field (route coalesces `note: NULL → ""`). Diffs are text-only: `kind ∈ {image, binary}` → 409 `NOT_DIFFABLE`. `added_lines`/`removed_lines` computed at **write** time and stored. `ArtifactKind`'s snake_case spellings match `unbacked.ts:28-38` exactly; extension precedence: allow-listed extension on the model-supplied name → `kind` map → `mime_type` map → `.bin`.

**New workspace dependency: `similar`** (MIT, pure Rust, no build script) in `openalpaca_storage` — the workspace has no diff crate. Alternative: ~120-line hand-rolled LCS unified diff.

**New event** `ServerEvent::ArtifactWritten { artifact_id, task_id, agent_id, name, kind, version, path, ts, instance_id }` — carrying `ts`/`instance_id` from the start. Feeds the chat inline artifact card, the per-run event log (GAP-10), and the session log's `artifact_written` record (§5.5).

**GAP-11 mechanics:** move **only** the content routes (`/v1/files/{id}/content`, `/v1/artifacts/{id}/content`) out of `protected_routes` into a third merged sub-router beside `chat_sse` (`router.rs:268-280`), validating `?token=` inline exactly as `chat_stream_handler` does (`routes/chat.rs:104-113`). Authorization is not lost — `routes/files.rs:287-313` already 404s on `owner_id` mismatch; only authentication moves. Accept **both** header and query so `lib/api/files.ts:23-30` is unaffected. `/v1/files/{id}` (metadata) and `/open` stay header-authenticated.

### 4.10 What changes on disk (honest version of rev 1 §1.11)

1. Existing `file_assets` rows take column defaults (`origin='upload'`, `rel_path=NULL`, …); `storage_path` values are **rewritten once** by `rebase_asset_paths` after the root move, then again per-row by the Phase 8 uploads re-home.
2. **Files move — once, at boot, atomically, resumably** (§2.2). Rollback is not automated: the old root's layout is reconstructible by reversing the entry ledger, but pre-release + single user, nobody builds that.
3. `/v1/files/*` is unchanged in shape; `/v1/files/{id}/content` gains `?token=`.
4. `/v1/artifacts` lists uploads too, `kind` inferred from `mime_type` at read time for legacy rows. No backfill.
5. **Rollback to schema 35** loses the columns and `artifact_versions`; produced artifact *files* survive on disk as a folder of readably-named documents. The store degrades to "a folder", not to nothing.

---

## 5. Session persistence — the new pillar

Full evidence and rejected alternatives: appendix S. This section is the executable design.

### 5.1 The concept

> **A session is one conversation transcript: an epoch of a lane, bound to at most one workspace, with a lifecycle (`active` → `archived`) and a durable event log.**

The existing `conversations` table already *is* the session table minus three defects: a column-level `UNIQUE(lane_key)` (one conversation per lane, forever — `011_unified_conversations.sql:3-12`), no workspace column, no lifecycle. It is **rebuilt as `session`** (migration 039). Rejected: session = workflow run (that object is `task`); session = client connection (connector lanes are connectionless); a new table beside `conversations` (two overlapping transcript containers). Mapping to the Claude Code model the sketch pointed at: session ↔ `session` row; project ↔ `workspace_id` (= canonical root path, already the memory-scoping key); transcript ↔ `conversation_messages` (now session-keyed) + the session's JSONL for loop detail; run-inside-a-session ↔ `task` row + `subagent_span`; `--resume` ↔ `POST /v1/sessions/{id}/activate` + pickers.

**Runtime stays lane-keyed; persistence becomes session-keyed.** `SharedContext` (steering inboxes, cancellation tokens, `active_workflows_by_lane`, followup claiming) does not change; the gateway resolves `lane_key → active session id` once per turn at persist time. This keeps the change surface out of the loop's hot path and out of the steering rail entirely.

**Lifecycle.** Created explicitly (`POST /v1/sessions`, GUI "New chat") or implicitly on first message to a lane with no active session (`get_or_create_active_session`, evolving `get_or_create_conversation` — `repository/conversation/mod.rs:146`). Creation archives the lane's previously-active session; a **partial unique index makes one-active-per-lane a DB invariant**, not a convention. Workspace binding: set from the first `x-workspace-path` seen, `PATCH`-updatable; one workspace per session — changing project = new session; `NULL` = none. Archived sessions are fully readable and re-activatable. Connector lanes: one perpetual active session (N1; knob reserved, off).

### 5.2 Migration 039 — `039_sessions.sql`

```sql
-- Migration 039: sessions. Rebuilds `conversations` as `session`
-- (drops the column-level UNIQUE(lane_key); adds workspace + lifecycle),
-- keys messages/tasks/followups by session, and turns tool_execution_log
-- into the tool-call index for the session event log.

CREATE TABLE session (
    id             TEXT PRIMARY KEY,               -- UUIDv4 (same ids carried over)
    lane_key       TEXT NOT NULL,                  -- routing address, no longer UNIQUE
    source         TEXT NOT NULL,
    title          TEXT DEFAULT '',
    workspace_id   TEXT,                           -- canonical project root; NULL = none
    status         TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    message_count  INTEGER DEFAULT 0,
    last_message_at TEXT,
    summary        TEXT NOT NULL DEFAULT '',       -- carried from 014
    summary_version INTEGER NOT NULL DEFAULT 0,
    last_summarized_message_id INTEGER NOT NULL DEFAULT 0,
    summary_updated_at TEXT,
    ended_at       TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO session (id, lane_key, source, title, workspace_id, status, message_count,
                     last_message_at, summary, summary_version, last_summarized_message_id,
                     summary_updated_at, created_at, updated_at)
SELECT id, lane_key, source, title, NULL, 'active', message_count,
       last_message_at, summary, summary_version, last_summarized_message_id,
       summary_updated_at, created_at, updated_at
FROM conversations;
DROP TABLE conversations;

-- The invariant: at most one active session per lane.
CREATE UNIQUE INDEX idx_session_active_lane ON session(lane_key) WHERE status = 'active';
CREATE INDEX idx_session_workspace ON session(workspace_id, updated_at DESC);
CREATE INDEX idx_session_updated   ON session(updated_at DESC);
CREATE INDEX idx_session_source    ON session(source);

ALTER TABLE conversation_messages ADD COLUMN session_id TEXT;
UPDATE conversation_messages
   SET session_id = (SELECT s.id FROM session s WHERE s.lane_key = conversation_messages.lane_key);
CREATE INDEX idx_conv_msg_session ON conversation_messages(session_id, id);

ALTER TABLE task ADD COLUMN session_id TEXT;
CREATE INDEX idx_task_session ON task(session_id);

-- Follow-ups remember the conversation they came from (§5.3).
ALTER TABLE lane_followups ADD COLUMN session_id TEXT;

-- tool_execution_log → the tool-call index. Payloads live in the session
-- JSONL (or its results/ spill tier); rows hold previews and the pointer.
ALTER TABLE tool_execution_log ADD COLUMN session_id TEXT;
ALTER TABLE tool_execution_log ADD COLUMN task_id TEXT;
ALTER TABLE tool_execution_log ADD COLUMN log_seq INTEGER;      -- seq of the tool_call record
ALTER TABLE tool_execution_log ADD COLUMN args_preview TEXT;    -- ≤ 2048 chars
ALTER TABLE tool_execution_log ADD COLUMN result_preview TEXT;  -- ≤ 2048 chars
ALTER TABLE tool_execution_log ADD COLUMN result_ref TEXT;      -- 'log:<seq>' | 'file:results/<...>'
CREATE INDEX idx_tel_session ON tool_execution_log(session_id, id);
CREATE INDEX idx_tel_task    ON tool_execution_log(task_id, id);

UPDATE schema_version SET version = 39 WHERE version = 38;
```

Honestly stated:

- **The table rebuild is the unavoidable cost of breaking `UNIQUE(lane_key)`** — SQLite cannot drop a column constraint. Since the copy happens anyway, the rename to `session` costs only the SQL strings in `ConversationRepository` (one 376-line file) plus a handful of raw references in daemon routes. **Rust type names (`Conversation`, `ConversationRepository`, `ConversationMessage`) do not rename in this PR** — churn with zero behaviour change; opportunistic or never.
- **Struct churn is already paid**: migration 038's PR adds `#[derive(Default)]` to `ConversationMessage` (§6 Phase 6); `session_id` lands as `..Default::default()`-absorbed. **039 therefore sequences after 038.**
- **No FKs on the new columns**; session deletion is repo-level transactional (one transaction: session row, its messages, null `task.session_id`) — same posture rev 1's GAP-21 guidance found for 011's missing cascade.
- Backfill: one `active` session per existing lane — exactly today's semantics; nothing observable changes until a client creates a second session.
- **No `plans` table.** Post-planner-deletion a "plan" is `task.description` (the spec) + plan-kind artifacts (`ArtifactKind::Plan` → `.md`) + `task.outcome_json` and the completion-report message (linked via 038's `task_id`). A plans table would rebuild the deleted DAG planner's storage with no producer. A first-class step checklist, if ever wanted, is a `WorkspaceEntryType` addition — reserved.
- **No new `tool_calls`/`tool_results` tables.** The single writer already exists at the single right place — the sandboxed execute path every tool call funnels through (same sites Phase 4's GAP-10 instruments). A call has at most one result; two tables buy a join and nothing else. Payloads don't belong in rows: previews serve list views, `log_seq`/`result_ref` address the authoritative record, and GAP-18's `invocations_today` aggregation works unchanged.
- `agents`/`memories` from the sketch: already exist (`agent`/`agent_metrics`/`agent_task_history` + 037's `subagent_span`; `memory` with workspace scoping). Nothing added.

### 5.3 Follow-ups pin their session

A queued follow-up is a promise to continue *that* conversation: **the follow-up turn runs in its originating session** — `spawn_followup` re-activates `lane_followups.session_id` if archived (one `UPDATE … status='active'` + archive of the usurper, inside the claim transaction). Without this, a follow-up queued in conversation A silently appends to whatever conversation B the user opened since. Pre-039 rows (`session_id IS NULL`) fall back to the lane's active session. Likewise the **workflow completion report persists into the originating session** (`task.session_id`), not the lane's currently-active one (`dispatcher/lead_agent.rs:463-494` today writes by lane).

### 5.4 The JSONL event log

```
~/.openalpaca/sessions/<session-id>/     ← HOME root always, never the project dir: transcripts carry
  log.jsonl                                persona/memory/cross-project content and must not be
  log.<first_seq>-<last_seq>.jsonl         git-committable; the project link is session.workspace_id
  results/                               ← spilled tool results: <seq>-<tool-slug>.<ext>
  snapshots/                             ← reserved (Phase 8: file_write pre-edit images)
```

**Envelope** — one JSON object per line:

```json
{"v":1,"seq":184,"ts":"2026-09-01T10:22:03.114Z","type":"tool_result",
 "task_id":"3f2a1b7c-…","agent":"research_agent::a1b2c3d4","data":{…}}
```

`seq` — per-session, strictly monotonic, gap-free, assigned by the single writer task; the resume cursor for `/events` and the target of `tool_execution_log.log_seq`. `task_id` present on workflow-interior records, absent on main-loop/chat records. **`span_id` (P-20)** on every workflow-interior record (= the 037 `subagent_span` id, or the lead span) so `?agent=`/`?span_id=` on `/events` is a pure filter — one log, global order kept, subagents as a dimension; **never** split per agent (Claude Code splits because each subagent must be independently resumable via `SendMessage`; OpenAlpaca resumes the workflow and its global `seq` is the cursor). `data` hard-capped at 64 KB; larger payloads spill and `data` carries the **stub with a preview (P-15)**: `{"spill":{"rel":"results/000184-a1b2c3d4-web_fetch.json","bytes":…,"sha256":"…","mime":…}, "preview":"<first 2048 chars>"}` — the identical bytes that land in `tool_execution_log.result_preview`, written once by the same writer, so the expanded-turn view renders without touching `results/` and replay can inline the preview after a spill file is evicted. Spill files are named `results/<seq>-<span8>-<tool>.<ext>`.

**Spill, don't truncate — one threshold, two consumers (P-16, C-2).** Today the loop cuts every tool result to 32 KB head-only (`runner/agentic_loop/tool_helpers.rs:1-2` `MAX_TOOL_RESULT_SIZE`, applied at `agentic_loop/mod.rs:717-719` for `Ok` and `Err` alike) and tells the model only "[… truncated]" — the tail of a `cargo test` or `web_fetch` result is gone while the log keeps a copy the model cannot reach; `web_fetch`'s 8 192-char cut (`web_fetch.rs:94`) and `shell_execute`'s 512 KB cap (`shell_execute.rs:37`) are unrelated to each other and to the spill. Replace the constant with `[orchestrator.sessions] tool_result_inline_bytes` (default 32 KB). Above it the session writer spills once to `results/` and the **model-visible** `tool_result` becomes the stub — `"[result too large: <bytes> bytes; first 2 KB follow]\n<preview>\n[full result: result_ref=file:results/… — use read_result to page]"` — with `tool_execution_log.result_ref` pointing at the same file (no second write). A **scoped** builtin `read_result(result_ref, offset?, limit?)` resolves only inside the current session's `results/` (session id from `ToolContext`; `file_read` is **not** widened to the home root). `Err` results stay inline but switch to **head+tail** (compiler/test errors sit at the tail). Once the spill exists, `web_fetch`'s cut is raised to the shared threshold (or dropped) and `shell_execute`'s 512 KB passes through the spill path instead of being cut twice; the 64 KB JSONL `data` cap stays as the envelope bound since spilled results never sit inline. **How `read_result` enters every fail-closed allowlist is owner decision T15 (pending):** the lessons propose appending it constructor-side as an *ambient* capability — the mechanism that already appends `workspace_read/write` for subagents (`agent/template/mod.rs:562-566`) — so no `Only(empty)` policy from Phase 0 A0 can be left unable to page into its own spilled result; until decided, the stub is still emitted on every surface (the spill happens for the log regardless) and the builtin is registered, but its allowlist entry is not appended anywhere. Tests once T15 is decided: `only_empty_allowlist_can_still_read_result` (yes) or the per-template listing (no); `denied_read_result_is_refused` either way.

**Event catalog** (`type` → payload essentials): `session_start {daemon_version, boot_id, workspace_id, lane_key, source}` — **emitted again on every daemon boot that touches the session** (a boot boundary; P-13) — and `session_end`; `workspace_changed {from, to}` on `PATCH workspace_path` (P-13); `user_msg`/`assistant_msg` (msg_id, ≤512 preview, attachment file_ids, model — content stays in the DB); `delegation` (task_id, title); `round` (round #, model, tokens, stop reason, assistant text, **`tool_use` blocks verbatim** — id, name, full input JSON — plus **`context {window, system_prompt, tools, messages, free}`** from `ContextBudgetManager::section_breakdown()` (`context_budget/budget.rs:106`), so the GUI can draw a per-turn `/context`-style bar without a new endpoint; P-19); `tool_call`/`tool_result` (tool_use_id, full args/result or spill stub, duration, and — when `RegisteredTool::extension_id()` is `Some` after the design's C1 — **`ext {kind: "builtin"|"mcp"|"plugin", id, generation}`** plus the S4 refusal string when the call was refused; P-17 — this is what makes S4 auditable per session and gives design §13 Q1 a non-lying history to read from); `steering`/`steering_drained` (text/request_ids); `confirmation_req`/`confirmation_res`; `subagent_open`/`subagent_close` (span id = 037 span id, template, label, state; `plugin_id` for `AgentSource::Plugin` — P-17); `skill_invoked` (skill, source, `plugin_id` when plugin-contributed — P-17); `compaction {tier, trigger: auto|manual, pre_tokens, post_tokens, dropped_from_seq, dropped_to_seq, preserved_from_seq, dropped_messages, cumulative_dropped_tokens, duration_ms, summary_msg_id, preview≤512}` (P-14 — values from `SystemEvent::CompactionTriggered`; the summary text lives in `conversation_messages`); `artifact_written`; `followup_queued`; `log_trimmed`; `workflow_done`; `error`. `round` carrying `tool_use` verbatim is what makes replay-resume possible — both sides of the assistant(`tool_use`)/user(`tool_result`) alternation must be reconstructible bit-for-bit. **Do not stamp every line** with build/boot/cwd (Claude Code must, because its sessions move and its CLI upgrades mid-session); with one writer and a gap-free `seq`, the last boundary record is authoritative. **Replay rule after compaction (P-14):** start from the newest `compaction` record's summary and replay only rounds with `seq > preserved_from_seq`; §5.4's trimmed-head safety argument then also covers compacted spans, and SES-04 never re-feeds history the loop already dropped.

**One source of truth per event class — nothing is written twice:**

| Event class | Source of truth | Others carry |
|---|---|---|
| Chat message **content** | `conversation_messages` row | JSONL: msg_id + preview only |
| Task current state / outcome | `task` row | JSONL narrates transitions; WS streams them |
| Tool call/result **payloads** | **JSONL** (or `results/` spill) | `tool_execution_log`: previews + `log_seq` pointer |
| Loop narrative (rounds, steering drains, compaction, errors) | **JSONL** | WS streams live; `event_log` grows **no** new copies |
| Subagent span state | `subagent_span` (037) | JSONL open/close reference the span id — narrative, not state |
| System audit (connectors, keys, wake, commands) — **and the extension transitions** `extension_state_changed` / `extension_capability_withdrawn` (design §7.3) | `event_log` | unchanged; sessions don't touch it. A transition is written **once** (event_log); WS streams it live; any per-turn rendering of extension state to the model would be a *session-log record* (`context_block` — owner decision T4, pending), never a second channel or a second file (P-18) |
| Artifact content/addresses | filesystem + `file_assets` (§4) | JSONL references artifact_id |

Consequence for Phase 4 (GAP-10): **unchanged and unblocked** — `event_log.task_id` ships first and keeps serving `GET /v1/events/history?task_id=` from rows that already exist. Once the JSONL is live, payload-bearing per-run detail moves to `GET /v1/sessions/{id}/events`; `event_log` stays what it structurally is — a small-detail audit table. No writer added, none removed.

**Durability mechanics:** spill threshold 64 KB (tmp+rename, like the artifact store). Rotation at 64 MB → `log.<first>-<last>.jsonl`; readers list segments, sort by first seq, stream — most sessions never rotate. **fsync policy:** the writer task owns a `BufWriter`; flush (write syscall) after every record; `sync_data` only on `session_start`, `assistant_msg`, `workflow_done`, `confirmation_res`, `session_end`, and a 5 s timer while dirty. Crash exposure: at most the current round's tail — acceptable; the DB is WAL-synced independently and replay tolerates a truncated tail. **Never per token** — the token stream never touches the log. Torn tails: readers treat an unparseable final line as end-of-log; the writer truncates a torn tail before appending on reopen. `DELETE /v1/sessions/{id}` removes row (transactionally) then dir; a rowless leftover dir is swept opportunistically at boot. **`sessions/<id>/` is created by the writer on the first `emit`, never by `POST /v1/sessions` (P-22)** — Claude Code creates per-session dirs eagerly and had 334 empty ones on the owner's machine (shape-only local evidence); a daemon that runs for months must not accumulate that. Reconciled with §1.3 rule 3 explicitly: `sessions/<id>/` is store-created, so sweeping a rowless one is the store deleting what it *did* create; unknown names at the store root stay untouched.

**Size limits (decided 2026-09-01 — the log is bounded, not unbounded history).** Three caps, all config-driven under `[orchestrator.sessions]`:

| Key | Default | Effect |
|---|---|---|
| `log_max_session_bytes` | 256 MB | Per session, counting `log.jsonl` segments **plus** `results/`. On exceed, drop whole **oldest** segments and the spill files they reference (never the live segment), and write a `log_trimmed` record naming the dropped seq range. |
| `log_max_total_bytes` | 2 GB | Across all sessions. Evict oldest-touched **archived** sessions' logs first, LRU; an active session's log is never evicted. |
| `log_retention_days` | 0 (off) — **deliberately, pending owner decision T12**; the lessons recommend 90 as a privacy sweep ("age is for exposure, size is for disk" — size never expires a small old session that captured a secret in a tool result). Not an oversight. | Age-based sweep of archived session logs. **The sweep machinery ships regardless of the default (P-21, P-22):** a boot-time pass, run at most once per day via a `state/.last-sweep` stamp, covering archived session dirs (log segments + `results/` + `snapshots/`), rowless and empty session dirs, orphaned `results/` files whose referencing segment is gone, and `cache/`; never an active session; skipped when config cannot be parsed. `GET /v1/status` reports `last_sweep_at` (Phase 8 item 1). |

Trimming the oldest segments is safe because of §5.3's source-of-truth split: chat content lives in SQLite, so a trim loses loop-interior detail (rounds, tool payloads) — not the conversation. Replay resume (§5.6c) only ever reads the **tail**, so a trimmed head cannot break it; a session whose *live* segment is gone answers 409 and points at `rerun`, exactly as a gutted log already does. Enforcement runs in the writer task on rotation (cheap, no scan) and once at boot for the global cap.

### 5.5 The write path

1. **`SessionLogService`** (new, `crates/openalpaca_core/src/session_log/`): `DashMap<SessionId, SessionLogHandle>`; `handle_for(session_id)` lazily spawns the per-session writer task (mpsc → BufWriter; seq assignment, rotation, spill, fsync per §5.4); idle-close after N minutes. `SessionLogHandle::emit(Record)` is a non-blocking `try_send`; a full channel drops with a `tracing::warn!` counter — the log is an observability record; stalling the loop for it is the wrong trade.
2. **Gateway:** `GatewayPersistence` resolves the active session once per turn, writes `conversation_messages.session_id`, emits `user_msg`/`assistant_msg` (+ `delegation` beside where `result.delegation` is read, `gateway/router/mod.rs:285`).
3. **Loop:** `LoopConfig` gains `session_log: Option<SessionLogHandle>` next to the existing `steering` field — the identical pattern. Emit points in `run_agentic_loop_inner`: after each LLM response (`round`); around tool dispatch in the sandbox path (`tool_call`/`tool_result` — the same sites GAP-10 instruments); at the steering drain; on compaction; at every exit. The lead runner adds `subagent_open`/`subagent_close` beside the 037 span writes. Main-loop turns log identically with `task_id` absent.
4. **Steering:** `push_steering` (`runner/steering.rs:61-79`) additionally emits `steering` to the task's session log — one line, and crash recovery of interjections exists.
5. **Dispatch:** `dispatch_lead_agent` (`dispatcher/lead_agent.rs:23-32`) takes `session_id` (gateway-resolved) and persists it on the task row.

### 5.6 Recovery

**(a) Chat lanes** — already durable; recovery is an *identity* fix. After 039 the GUI lists sessions per workspace, reopens the active one, fetches by `session_id` instead of one endless per-lane history. The in-memory lane is caches-only (`lane/types/mod.rs:93-100`) and rebuilds lazily as today.

**(b) In-flight workflows — Phase 7b: honest interruption.** The boot sweep stops lying: non-terminal tasks become **`interrupted`** (new `TaskStatus` variant; code-only — `models/task.rs:9-16` has no CHECK constraint blocking it) instead of `failed` with a fabricated message. `interrupted` is terminal for lane bookkeeping but carries a `restartable` affordance: the GUI's "Restart" is Phase 5's `rerun` (new id, `source_task_id` link). The sweep additionally **reads each open session's JSONL tail**: `steering` records with no subsequent `steering_drained` containing their request_id are converted to `lane_followups(kind='unprocessed_steering', session_id=…)` — the exact rows the graceful path already writes (`dispatcher/lead_agent.rs:300-355`) — so a crash no longer silently eats interjections. Idempotent via a request_id uniqueness guard on insert; the before-ingress ordering guarantee (`bootstrap/migration.rs:13-18`) is preserved.

**(c) Phase 8: replay resume (opt-in).** For an `interrupted` task with a complete record chain, `POST /v1/tasks/{id}/action {"action":"resume"}` rebuilds the loop's messages from the log — compose persona layers fresh (they are regenerated every dispatch anyway); seed the objective from `task.description`; append each `round`'s assistant message (text + verbatim `tool_use`) and each `tool_result` as the corresponding user message (inlining spills); stop at the last *complete* round (all its results present — the model re-does at most one round); append a synthetic `<user_interjection>`: "the daemon restarted; you are resuming — verify the state of any side effects from your last round before repeating them" (side effects between the last durable record and the crash are unknowable; telling the model beats pretending); re-enter `run_agentic_loop_routed` under the **same task id** (D5's philosophy), `interrupted → running`, steering inbox re-registered fresh. Gated by `[orchestrator.sessions] resume_enabled` (default off until trusted); `rerun` remains the fallback; a gutted log is a clean 409 pointing at `rerun`.

**(d) Clients after restart** recover from the DB, not daemon memory: `GET /v1/sessions?workspace_id=` + `/{id}/messages` + `GET /v1/tasks?status=interrupted`. **The last-session pointer is server-side (P-23):** `last_session:{workspace_id}` (and `last_session:{lane_key}` for connector lanes) in the existing `preference` KV (`migrations/005_preference.sql`, `repository/preference/`), written on every session activation; `GET /v1/sessions?workspace_id=` returns `last_active_session_id` in the envelope. The GUI's localStorage id is a cache, not the source (two clients and a DB — a client-side pointer diverges between them; Claude Code keeps `lastSessionId` per project for the same reason); the CLI's `chat --resume`/`--session` read the same pointer.

### 5.7 HTTP surface (replaces `/v1/conversations` — P19)

```http
GET    /v1/sessions?workspace_id=&source=&status=&q=&limit=&offset=   200 { "sessions": [SessionView], "total": n }
POST   /v1/sessions {source?, workspace_path?, title?}                201 SessionView   (archives the lane's previous active)
GET    /v1/sessions/{id}                                              200 SessionView | 404
GET    /v1/sessions/{id}/messages?limit=&before_id=                   200 { "messages": […], "total": n }
GET    /v1/sessions/{id}/events?after_seq=&types=&limit=              200 { "events": [envelope…], "next_after_seq": n }
POST   /v1/sessions/{id}/activate                                     200   (archives the current active on that lane)
POST   /v1/sessions/{id}/archive                                      200
PATCH  /v1/sessions/{id} {title?, workspace_path?}                    200
DELETE /v1/sessions/{id}                                              204 | 409 SESSION_HAS_ACTIVE_WORKFLOWS
```

- `SessionView`: id, lane_key, source, title, workspace_id, status, message_count, last_message_at, created_at + derived `active_task_count` (from `SharedContext::active_workflows_by_lane`) and `interrupted_task_count` (one grouped query). Envelopes and errors follow §7's rules.
- **`POST /v1/chat` gains optional `session_id`** — appends to that session, auto-activating it (409 if it belongs to a different lane than the principal's). Absent ⇒ the lane's active session, created on demand — today's behaviour exactly.
- **`GET /v1/chat/history` retargets** to the lane's active session — same shape, correct scoping. The GUI transcript view moves to `/v1/sessions/{id}/messages`; the everyday path needs **no JSONL** — `/events` powers only the expanded turn view.
- **Rev 1's GAP-21 re-homes here**: rename = `PATCH`, delete = `DELETE` (transactional; 409 on active workflows). Built once, on sessions; `/v1/conversations` handlers deleted, not aliased (P19), GUI/CLI callers updated same PR.
- New `ServerEvent::SessionChanged { session_id, lane_key, status, ts, instance_id }` keeps a second window honest.
- **CLI:** `openalpaca sessions [--workspace <path>] [--all]`; `openalpaca chat --resume` (activate + continue the cwd workspace's most recent session), `--session <id>`; both print a transcript tail first. The CLI starts sending `workspace_path` on `/v1/command`, completing §4.7's client story.
- **GUI:** session sidebar per workspace; "Interrupted" badge + Restart on `status=interrupted`; expanded-turn tool timeline from `/events`.

**Filesystem tiers from the sketch, resolved:** `attachments/` **is D2's uploads area** — no per-session copies (owner-scoped sha256 dedup at `routes/files.rs:172-185` is load-bearing; a copy breaks it and multiplies bytes); sessions link attachments via `conversation_message_attachments.file_id`, and `user_msg` records carry the file ids. `large-tool-results/` is the session's `results/` dir. `snapshots/` is reserved; the one real thing specified (Phase 8): pre-edit images of user files `file_write` overwrites — copy target to `snapshots/<seq>-<slug>` + a `file_snapshot` record; task-state checkpoints (redundant with `state_json`) and whole-workspace snapshots (git's job) are deliberately not designed.

---

## 6. Phases

Order rationale: root move + purge first (everything else builds on the new paths and the deletions shrink every later diff); artifact store next (the Library is the biggest blocked surface); run observability and message links next (sessions **reuse** their instrumentation — the sandbox `ctx.task_id` passthrough and the `#[derive(Default)]` churn payment — so sessions sequence after them, per appendix S §8); sessions; then the long tail.

### Phase 0 — Bugs and one-liners *(no migration; ship first)*

Items 1–7 unchanged from rev 1; **A0–A4 added in rev 3** (lessons §4 and §8 Stream 1 — standalone, no design dependency, each its own commit, A0–A3 reviewed as security changes); **A5 added 2026-09-02** (the second half of lessons §8 Stream 1 item 5 — the C-3/P-3 fix the §0 N4 row records as adopted, which rev 3 left without a slot). Every item independently mergeable.

- **A0 — empty allowlist fails closed (bug A; P-24).** `check_agent_capability` (`security/capabilities/mod.rs:106-113`) treats an *empty* allow list as unconstrained, and `invoke_plugin_skill` builds that list from whatever the skill's `requires_capabilities` resolve to (`orchestrator/skill/invocation.rs:952-976`) — or from `fm.tools.allow` when that is empty — so a plugin skill whose providing extension is absent or off may call **any** tool: disabling an extension *widens* reach. Fix the callee, in the type: `SandboxPolicy.allowed_capabilities: Vec<String>` → `enum Allowlist { Unrestricted, Only(Vec<String>) }` where `Only(vec![])` yields `CapabilityNotAllowed` for every non-ambient capability and `Unrestricted` must be spelled by the (currently zero) callers that mean it; minimum acceptable alternative `Option<Vec<String>>` with `Some(empty)` = deny-all plus an audit of the seven policy sites (`simple_query_handler.rs:229`, `invocation.rs:299`, `invocation.rs:976`, `invoke_executor.rs:377`, `lead_agent/mod.rs:314-321`, `SandboxPolicy::from_constraints`, the lead's append guard). Keep deny-first. Caller side: `invoke_plugin_skill` refuses up front with the S4 wording when `requires_capabilities` is non-empty but resolves to nothing, and passes `Only(resolved)` otherwise; the `fm.tools.allow` fallback passes `Only(allow)` too. **Ambient set:** `Only(v)` is evaluated after the constructor-side set that already exists — `{workspace_read, workspace_write}` for subagents (`agent/template/mod.rs:562-566`); A0 ships with exactly that set. Whether the set is acceptable at all, and whether `read_result` joins it, is **owner decision T15 (pending)** — A0 does not extend it. Tests: `empty_allowlist_denies_every_non_ambient_capability`, `deny_beats_allow` (a name in both lists is denied), `plugin_skill_total_loss_cannot_call_unrelated_builtin`, `plugin_skill_with_no_lists_cannot_call_any_tool`. XS; the design's C5 then completes availability filtering on top of it.
- **A1 — `deny_plugin` unloads (bug B).** `manager.rs:601-618` sets `status = Disabled`, emits `PluginDisabled{reason:"denied by user"}` and returns — the child keeps running, tools/skills/templates stay registered. **Write the denial first** (W-deny, design §3.2 — `approved = Some(false)`, `approved_at`, `capabilities = []`, `enabled` untouched; a failed write returns `500` and changes nothing), **then** run the teardown `disable_plugin` (`:645-690`) already contains (`unload_plugin` → kill → re-insert) and commit `Unapproved{Denied}`; report a consent word (`denied`), never `disabled`. Write-first so a crash between teardown and the write cannot lose a consent decision (design §6.2 #8). Guard test: after deny, `list_plugins()` reports zero tools/skills/agents for the plugin and `process.try_wait()` reports exit. The status-word/bit semantics finish in the design's C3 (leave the `enabled` bit untouched once the tri-state entry exists).
- **A2 — redundant `enable_plugin` stops leaking (bug C).** `manager.rs:621-642` has no status guard: enabling a running plugin re-enters `try_load_plugin`, which inserts a fresh `PluginState { capability_provider_handle: None, .. }` (`:262-278`) and orphans the handle registered at `:500` that only `unload_plugin` (`:522`) releases. First line: if already `Running`, return `Ok` with the current state (the design's E0 "CAS fail → 200, never a reload"); make the map insert at `:262` refuse to replace an entry whose state is not `Disabled/Failed/Unapproved` so the scan path cannot leak either. `enable` stops calling `approve()` in C3. Test: redundant enable registers no second capability provider.
- **A3 — the disabled MCP client cannot resurrect its child (bug D, additive half in `openalpaca_mcp`).** `TransportClosed` is retriable (`error.rs:58-66`); `call_tool` reconnects on any retriable error (`client.rs:284-310`); `reconnect()` (`:180-195`) checks only the attempt counter, never whether the client was deliberately closed — any held clone respawns a stdio child the owner just disabled. Land: a new non-retriable `McpError::Closed`; a `closed: AtomicBool` seal set in `disconnect` (`:165`) **before** it takes the lock and checked first in `reconnect` **and at `do_handshake`'s install point under the service lock** (`:137` — closing the just-spawned `RunningService` when sealed; implementing only `reconnect`'s entry ships the S2 hole the design's residue names); `reconnect` also refuses on `Disconnected`/`Failed{..}`; `pub fn connection_state() -> ConnectionSnapshot`. No timed cooldown or auto-retry. The 401/403 → `NeedsAuthorization` classification waits for the design's C2 registry arms; **stdio first-failure-is-terminal vs transparent respawn is design §13 Q8 (T6(a)) — pending, not applied here.**
- **A4 — byte-based tools token estimate (C-1).** `budget.register_section("tools", tools.len() * 200)` at `runner/lead_agent/mod.rs:479`, `simple_query_handler.rs:519`, `invocation.rs:528` prices every tool at a flat 200 tokens; `runner/agentic_loop/mod.rs:222-231` already computes `(description + parameters + input_examples bytes) / 4`. Compute it once in the surface builder and pass it to `register_section`; delete the constant. A flat 200 under-counts large schemas so `should_compact`/`is_fixed_zone_oversized` fire late — and it is the measurement any T3 (deferred loading) decision needs; the 10 % rule is meaningless against a guess.
- **A5 — main-loop cost lockout, option 1 (C-3 / P-3; `tasks/bug-main-loop-cost-lockout.md`).** The agentic loop's per-turn budget is compared against a cumulative measure: `LoopState::new()` initialises `last_cost: 0.0` (`runner/agentic_loop/mod.rs:68`; constructed at `:203`, before the `loop` at `:254`), and the round-boundary cost check (`:312-317`) computes `cost_delta = round_cost - state.last_cost` where `round_cost = backend.agent_cost(..)` (`backend.rs:167-189`) reads the **agent-scoped** bucket — for the main loop, everything ever spent under `agent_id = "orchestrator"`. So on round 0 of every turn the delta is the whole cumulative spend, `cost_acc` absorbs it, and once the bucket passes `agent_defaults.max_cost` ($1) every fresh chat turn exits `CostExceeded` before its first LLM call (ADR-028: confirmed a bug, not a daily budget). Fix — **option 1 of the bug note, adopted in §0 N4:** capture `backend.agent_cost(0, 0).await` **once** before round 0 and initialise `LoopState.last_cost` with it (`LoopState::new()` gains the baseline as a parameter, or `state.last_cost` is assigned immediately after `:203`), so only *this turn's* spend accumulates into `cost_acc`; the delta arithmetic at `:313-317` is unchanged. **No daily budget** (that would be a new enforcement point in the router — out of scope, N4), **no attribution-row change** (`llm_usage` rows and the Settings usage view keep `orchestrator` as the agent id; option 2 of the bug note is rejected for exactly that churn). Applies to every loop caller — lead agent and subagents start fresh accumulators today so the baseline is ~0 for them and nothing changes. Tests: `main_loop_turn_with_prior_agent_spend_does_not_exit_cost_exceeded` (seed the tracker's `orchestrator` bucket above `max_cost` before one turn; assert at least one LLM call and a finish reason other than `CostExceeded`) and `per_turn_cap_still_trips_within_one_turn` (a turn whose own rounds exceed `max_cost` still exits `CostExceeded`). XS.

1. **GAP-01 — `approval_scope`.** `ConfirmationBody` (`routes/chat_types.rs:90-93`) gains `#[serde(default)] pub approval_scope: Option<ApprovalScope>`; `chat.rs:462` passes it. `ApprovalScope` exists (`security/confirmation.rs:87-97`) and the sandbox honours it (`security/sandbox/mod.rs:248-256`) — only the HTTP hop drops the field; "Always allow" silently approves once today. **Ship as written** — `ApprovalCache` is session-scoped and cleared on restart (`confirmation.rs:99-100`), and the sandbox defaults an omitted scope to `TheseArgs` (`sandbox/mod.rs:250-252`), the safest value, so there is no regression. **Two follow-ups recorded, not scheduled (P-25):** (a) if a persistent "always allow" is ever wanted, it is written as a human-readable rule under `~/.openalpaca/config/` (D1's human half — e.g. `[security.approvals] always_allow = ["<tool>"]`), never a DB row in `state/` and never an `enabled` bit — the exact shape Claude Code uses when it saves "Yes, and don't ask again" as an `allow` rule in `.claude/settings.local.json` at the git root; (b) GUI copy for `EntireTool` on a `destructive_hint` tool should name what "always" covers, and the option should be withheld when the prompt cannot show the full scope. Precedent to check against when GAP-01 is next touched (direction only): headless approval in Claude Code is delegated (`--permission-prompt-tool` / `PermissionRequest` hook) or closed (`dontAsk`), and a `requiresUserInteraction` tool is approvable by no programmatic path.
2. **GAP-07 — empty `title`/`name`.** Add `title: String` to the three task variants and `name: String` to `AgentStatusChanged` in `events.rs`. **Not** `SharedContext` into the bridge (spawns at `main.rs:245`, before `SharedContext` exists at `:306`). Five producer sites all have the value (`task_ops.rs:153-159`, `dispatcher/lead_agent.rs:237-243`, `dispatcher/outcome.rs:268-275,:294-299`, `dispatcher/mod.rs:171-176`). Post-restart DB-only tasks fall back to `""` — today's behaviour, never worse.
3. **GAP-08a** — replace `settings.rs:314`'s `0.0` with `query_daily_usage` for today (§0).
4. **GAP-08b** — `task_id` on `LlmUsageQuery` (`settings_types.rs:19-24`), branch first in `get_llm_usage` (`get_task_usage` exists, indexed — `repository/llm_usage/mod.rs:95-110`); **also `cost_for_tasks(&[String])`** (one grouped query) enriching `GET /v1/tasks` — the Work list needs per-row cost.
5. **GAP-16 — `GET /v1/me`.** `AppState` has `local_user_id`/`default_lane_key` (`state.rs:32-33`); `sources[]` = distinct `conversations.source` (post-039: `session.source`) via `list_conversations_for_owner`. Reading chosen and stated.
6. **GAP-22 — `ts`/`instance_id` on the six plugin events** (`openalpaca_api/src/events/mod.rs:250-280`) + `PluginManager::with_instance_id` builder chained at `main.rs:334-343`; `..` on the four test match arms. Rejected: stamping in the daemon sink (leaves the crate emitting invalid events for other embedders). `PluginCrashed`'s emit path **is** dead (confirmed — there is no crash-detection loop; the design's §3.6 reaper is what creates one). **Pending owner decision T14 (P-26):** the extension design's C7 deletes these six variants together with their GUI mappings and its `Extension*` family carries `ts`/`instance_id` from birth — so this item is dead work if the design lands before Phase 8. Left as written until the owner says drop or keep; do not start it before that answer.
7. **Consistency groundwork.** Shared `pub(crate) fn api_error(status, code, message)` in `routes/mod.rs` (collapses the byte-identical duplicate at `chat_types.rs:101-111`/`files_types.rs:174-184`); every new route uses it. Fix the plain-text 401 in `middleware.rs` to JSON. Do **not** retrofit the ~30 `{error:"string"}` sites (§7). Do the cheap half of the task-shape normalisation: typed `TaskSummaryResponse` replacing `list_tasks_handler`'s `as_object_mut()` post-injection.

*Verify:* A0's four tests above plus a deny-first regression; A1's exited-child/zero-contributions guard; A2's single-provider assertion; A3 — after `disconnect`, `call_tool` on a held clone returns `McpError::Closed` and no child is spawned (assert on `try_wait`/process count), and a `do_handshake` racing `disconnect` never installs a live service; A4 — `register_section("tools", …)` equals the loop's byte estimate for the same surface; A5 — after >$1 cumulative `orchestrator` spend (seed the `CostTracker` agent bucket, or chat until `llm_usage` for today under `agent_id = 'orchestrator'` exceeds $1.00 against a live daemon), a fresh chat turn does not exit `CostExceeded` before its first LLM call, and the per-turn $1 cap still trips within one turn; `ConfirmationBody` deserialization for all three bodies (`ChatView.test.tsx:288-298` flips to a real contract test); bridge test (`event_bridge.rs:540-560`) asserts non-empty title; full-workspace rebuild (GAP-22 touches `openalpaca_api`).

### Phase 1 — Root move + legacy purge *(migration 035)*

**Depends on:** nothing. **Blocks:** everything after it (paths).

1. `store/mod.rs` + `store/migrate.rs` (§1.4, §2): `home_root`, `state_dir`, `backups_dir`, `content_dir`/`ContentKind`, `ensure_store` seeding README (with the retention-class column)/.gitignore/.layout (line 2 `install_id=<uuid>` on the home root — pre-check (c)), the mover, `rebase_asset_paths`. `paths.rs` deleted (P1); consumer fan-out fixed mechanically (compiler-enumerated; inventory in appendix R §0). **Atomic three-binary rebuild in one commit** (§2.3).
2. Marker change: `.openalpaca` → `.git`, `.alpaca` deleted (P3), including the redundant-condition cleanup.
3. Purge items P2, P4, P5, P6, P10, P11, P12, P13, P14 — plus **migration 035** `035_drop_planner_telemetry.sql` (P7: `DROP COLUMN planner_ms/dispatch_ms` on `orchestrator_latency`, `planner_requested_mode` on `dispatch_decisions`; optional P15 timestamp normalise). Pre-checks §0(a)/(b) run first.
4. Documentation sweep (§3, last block).

*Verify:* mover unit tests — fresh install no-op; idempotent resume after a simulated kill between any two ledger entries; live-daemon guard aborts; per-child config merge preserves a GUI-pre-created `config/`; WAL/SHM+DB reunite before `Database::open`; `rebase_asset_paths` matches zero rows on second boot. Full workspace build + `cargo test --workspace` green with `paths.rs` gone. A dev-run from the repo still resolves `./config`.

### Phase 2 — Artifact store foundations *(migration 036)*

**Depends on:** Phase 1 (store module). **Blocks:** Phase 3, and the artifact half of Phase 6.

1. Migration 036 (§4.4) + **the two §4.5 fixes in the same commit** + `ArtifactKind`/`ArtifactOrigin` in `models/artifact.rs`.
2. Artifact grammar functions in `store/` (§1.4) — pure, heavily unit-tested.
3. `ArtifactStore` (§4.8) — the one writer for produced rows.
4. `artifact_write` tool + `workspace_write` spill bridge (§4.6), `[execution.artifacts]` config.
5. `task.workspace_id` written at dispatch; **the GUI sends `x-workspace-path`** (§4.7 items 1-4).
6. **D2 upload placement:** connector writer collapse first, then `POST /v1/files/upload` reads `x-workspace-path` and writes via `upload_dir` (§4.7 item 5). New uploads only; existing blobs stay at `state/assets/` until Phase 8.

*Verify:* `slugify` property tests (traversal, NFKD, case-folding, reserved names, truncation, empty); `confine_to_root` rejects symlinked-parent escapes; `put`→`put` leaves head clean + v1 in `.versions/`; **crash-injection between rename steps leaves head fully old or fully new**; a `produced` row survives 25 simulated sweep-hours; produced bytes don't count against quota; an upload with `x-workspace-path` lands in `<project>/.openalpaca/uploads/<date>/`; end-to-end lead-agent run lands an artifact under `<project>/.openalpaca/artifacts/`.

### Phase 3 — Artifact API, previews, versions *(unblocks the Library)*

**Depends on:** Phase 2. Unchanged from rev 1:

1. §4.9 routes; content routes on the third merged sub-router with inline `?token=` (**GAP-11**).
2. `ServerEvent::ArtifactWritten` through all four layers.
3. **GAP-05** versions + diff; add `similar`.
4. **GAP-12** pins (column + `PUT …/pin`; client `localStorage` stays authoritative until it opts in).
5. **CSP** — `tauri.conf.json:22` → `img-src 'self' data: blob: http://127.0.0.1:* http://localhost:*;` (both halves needed: `blob:` for object-URLs, loopback for direct `<img src=…?token=>`; CSP doesn't inherit from `connect-src`). Land **with** the preview, never before.
6. `POST /v1/files/{id}/open` skips `$TMPDIR` staging for produced artifacts (real extensions now).

*Verify:* `?token=` and `Authorization` both accepted; wrong token 401; other-owner artifact 404; 410 sets `missing_since`; `?task_id=` returns the run's files; Library renders end-to-end with GAP-04/05/11/12 deleted from `unavailable.ts`. **Deferred:** HTML artifact previews (`frame-src`/sandbox is a security review, not a CSP tweak).

### Phase 4 — Run observability *(migration 037)*

**Depends on:** Phase 0 (GAP-07 bridge work). GAP-09 and GAP-10 share the sandbox `task_id` passthrough — do them together. Phase 7 reuses these exact instrumentation sites.

Migration 037 — `037_run_observability.sql`: `subagent_span` table (id = spawn's node_id; task_id FK CASCADE; template_id, agent_instance_id, label unique-per-task, objective, `state CHECK IN ('running','done','failed','blocked','cancelled')`, detail, started_at, ended_at, duration_ms, output_preview; two indexes) · `ALTER TABLE event_log ADD COLUMN task_id TEXT` + index · `ALTER TABLE task ADD COLUMN source_task_id TEXT` + index · `UPDATE schema_version SET version = 37 WHERE version = 36`. (Full SQL: rev 1 §5, unchanged but renumbered.)

**GAP-09 — subagent timeline.** Do **not** extend `agent_task_history` (FK-bound to `agent(id)`, timezone-less `completed_at`, backs the legacy payload). Correction that changes the work: **no row exists until completion** — `record_agent_history` (`dispatcher/usage.rs:76-101`) is called only after the subagent's loop returns (`runner/lead_agent/tools.rs:548-556`, `:642-650`); an in-flight span is *absent*, not underivable. Write sites, all in `runner/lead_agent/tools.rs` (`self.db` is a field at `:48`): **open** after the `DagNodeStarted` publish (`:232-240`), reusing `node_id` as span id; **close** at both completion sites (`:524-543` plugin, `:610-628` LLM — map `Cancelled` explicitly; `agent_success` at `:598-604` folds it into `false` today and the UI has separate copy); **close** the cancelled-before-start early return (`:477-486`). Labels unique per run (`<short template>·<ordinal>`; `ParallelWork.tsx:129,170` keys by label). **Lead span** in `spawn_lead_agent_execution` (`dispatcher/lead_agent.rs:224-246`), closed at `finalize_task_with_outcome` (`:507-512`). `blocked` is **derived**: `ConfirmationBroker.pending` becomes `DashMap<String,(ConfirmationRequest, Sender)>` with `pending_requests()`; `ConfirmationRequest` gains `task_id`/`agent_instance_id` (built at `sandbox/mod.rs:212-220`). **Add `agent_instance_id` to `ToolContext`; do not repurpose `ToolContext.agent_id`** (it is the template id, reported in capability violations). Route `GET /v1/tasks/{id}/timeline` matches `TimelineLane` (`unbacked.ts:121-141`) field-for-field; terminal tasks report stale `running` spans as `cancelled`/`"interrupted"`; `close_orphans` runs once at boot. New `SubagentSpan` event through all four layers. **Keep `DagNodeStatus` emitting alongside** (GUI consumes it — `run-events.ts:63-71`); delete in Phase 8 (P9).

**GAP-10 — per-run event log.** `ctx.task_id` is already at every emit site inside `SandboxManager::execute_tool` (`security/sandbox/mod.rs:134-139`). Add `task_id: Option<String>` to `ToolExecuted`, `SecurityViolation`, `ToolConfirmationRequested`, `CircuitBreakerTripped`, `LlmCallCompleted`; `emit_security_violation`/`emit_tool_executed` each take one `Option<&str>`. Retro-fit the six already-persisted variants that carry the id buried in `detail` JSON (`events/persistence.rs:59-80,260-275,308-341,343-354`) to `log_for_task`. Route: `GET /v1/events/history?task_id=&event_type=&before=&limit=` → `{events, next_before}` — **always the envelope** (P20; the CLI call site at `commands/tasks.rs:310` updates in the same PR). Paginate on autoincrement `id`, not the dual-format timestamp.

**Exit criterion (P8):** the `assignments`/`assigned_agents` payload, its serde rename, and the GUI/CLI parsers are deleted in the timeline PR; GAP-20's run counts re-point at `subagent_span`.

*Verify:* two-subagent run → two uniquely-labelled spans + a lead span, correct start/end; cancelled subagent reports `cancelled`; a pending confirmation flips exactly one lane to `blocked`; daemon kill mid-run → `close_orphans` reports `interrupted`; `?task_id=` filters; the envelope is returned with and without filters (CLI updated).

### Phase 5 — Run control *(no migration)*

**Depends on:** Phase 4 (`source_task_id`), Phase 2 (`workspace_id`). Unchanged from rev 1:

1. **GAP-02 — `POST /v1/tasks/{id}/steer {message}`.** Pure reuse: `push_steering` (`runner/steering.rs:60-77`) is task-addressed and already emits `WorkflowSteered`. Lane from `task.source_lane` — the GUI addresses a run, not a lane. `200 {task_id, accepted, inbox_depth, lane_key}`; 409 `STEERING_INBOX_FULL` / `TASK_NOT_STEERABLE`; 503 `STEERING_DISABLED`. Leave the `/steer ` chat prefix alone (CLI/Telegram's only channel). Optional `workspace_path` defaulted from `task.workspace_id` so an `unprocessed_steering` leftover re-enters scoped to the same project.
2. **GAP-03 — follow-up routes.** `GET/POST /v1/lanes/{lane_key}/followups`, `DELETE …/{id}`. Never serialize `FollowupRecord` (carries `principal_json` — `repository/followup/mod.rs:19-34`); return `FollowupView` matching `unbacked.ts:225-235`. Add `cancel_if_queued` (CAS on status — unconditional `mark_cancelled` races the autostart at `dispatcher/lead_agent.rs:527-542`). Clients never mint `unprocessed_steering` (`claim_next` never claims it; the row would sit forever). Publish `FollowupQueued` on POST, `FollowupCancelled` on DELETE. Validate `lane_key`.
3. **GAP-06 — `rerun` and `start`.** `Orchestrator::rerun_task(task_id)` (not `TaskDispatcher` on `AppState`): read row → `dispatch_lead_agent` with `description`/`title`/`created_by`/`source_lane`/`workspace_id` → `set_source_task(new, old)`; 404/409/422; **201 with a new id** — deliberate asymmetry with `start`. `start` (D5, same id): intercepted in the route before `apply_task_action` (`task_ops.rs:107` would swallow it); `dispatch_lead_agent_with_id` + idempotent `upsert_queued`; 409 if `cancellation_tokens` holds the id (already live). Update the `UnknownAction` message (`routes/tasks.rs:287`).

*Verify:* steering a running task injects at the next round boundary; steering a finished task is 409; DELETE after `claim_next` is 409 and the turn runs; `rerun` id survives restart; `start` on a dispatched row is 409.

### Phase 6 — Message → run links *(migration 038)*

**Depends on:** Phases 2 and 4.

```sql
-- 038_message_run_links.sql
ALTER TABLE conversation_messages ADD COLUMN task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_conv_msg_task ON conversation_messages(task_id);
UPDATE schema_version SET version = 38 WHERE version = 37;
```

**GAP-23 needs no link table** — `conversation_message_attachments.role` (default `'attachment'`, `028:7`) carries `role='artifact'` via `link_to_message_with_role`. **Two links, because a turn cannot know its own artifacts:** the delegating message gets `task_id` (`persist_assistant_message` at `gateway/router/mod.rs:260`, `result.delegation` read at `:285` — same match arm, add one param); the completion-report message gets `task_id` **plus** `role='artifact'` links (by then `file_assets` has rows for the task — this is the message `RunReportCard` renders). `ConversationMessage` gains a field = wide struct-literal change: **add `#[derive(Default)]` and `..Default::default()` in the same commit** (`gateway/persistence.rs:34-46,:106-119,:156-168` + repo mappers) — this is the churn payment Phase 7 reuses. History query joins the link table so the client gets `artifact_ids: string[]` in one round trip.

*Verify:* a delegating turn persists `task_id` across reload; the completion message carries resolvable `artifact_ids`; `role='attachment'` rows unaffected.

### Phase 7 — Sessions *(migration 039)*

**Depends on:** Phase 6 (the `Default` churn payment), Phase 4 (sandbox passthrough + span ids, which the log references), Phase 1 (the `sessions/` dir lives under the moved root). Design: §5.

**7a — schema + surface (S0):** migration 039 (§5.2); `get_or_create_active_session`; `/v1/sessions` family + `POST /v1/chat session_id` + `chat/history` retarget (§5.7); `task.session_id` at dispatch; completion report into the originating session; follow-up session pinning (§5.3); **`/v1/conversations` deleted (P19)** and GAP-21 built here once; GUI session sidebar; CLI `sessions`/`--resume` + `workspace_path` on `/v1/command`. Effort **L**.

**7b — the log (S1):** `SessionLogService` + writer; emit points (gateway, loop, lead runner, steering — §5.5); `tool_execution_log` columns used; `results/` spill; `GET …/events`; **`interrupted` status + sweep rewrite** (mark interrupted + JSONL steering recovery — §5.6b); `SessionChanged` event. Effort **L**.

*Verify:* 7a — two sessions on one lane produce two clean transcripts; the partial unique index rejects a second `active`; a completion report lands in its originating (archived) session; follow-up autostart re-activates its session. 7b — a two-subagent run yields a well-formed log (seq gap-free; every `tool_call` matched by `tool_result` or the loop exit); a >64 KB result spills and `result_ref` resolves; `kill -9` mid-round leaves a parseable log (torn tail truncated on reopen), the sweep marks the task `interrupted` and recovers an undrained steering message into `lane_followups`.

### Phase 8 — Config, catalog, long tail

Independently orderable; each its own commit. Ordered by value per hour.

1. **GAP-14 — `GET /v1/status`, Phase A.** `started_at` at the top of `run()`, `uptime_secs`, `schema_version` from `Database::schema_version()` (the DB is the truth, not `MIGRATIONS.len()`), **`home_root`/`state_dir`/`db_path`** + resolved project dir (§4.7.4), `upload_bytes`/`produced_bytes` (§4.8). Protected route (leaks paths); `/v1/health` untouched. `log_path`: per **N2 (resolved: serve it)** — `state/logs/daemon.log` when the CLI-managed log exists, else `null`. **Bound it in the same change:** `apps/openalpaca/src/manager.rs:38` opens the file in plain append mode with no rotation, so it grows forever. Before opening, rotate when it exceeds `16 MB` (`daemon.log` → `daemon.log.1`, keeping 3), which is ~15 lines at the call site and needs no new dependency. Phase B (a real in-daemon appender + un-discarding the sidecar's stdout, `src-tauri/src/lib.rs:128,152`) stays a separate day+ task and inherits the same limits. **Sessions and retention numbers (P-27):** `sessions: {count, active, bytes, cap_bytes, last_sweep_at, evicted_total}` and `retention: {log_max_session_bytes, log_max_total_bytes, log_retention_days}`, so the Settings page can show "sessions use 1.2 GB of 2 GB; last sweep 3 h ago" *before* the first eviction surprises the owner — visibility is the cheap half of bounding.
2. **GAP-18 — `GET /v1/tools` SHIPPED in the extension design's C6 (rev 3.2); `/v1/skills` is what remains.** The tools half landed exactly in the §8 shape described below — read-only, `origin`-carrying, no `denied`, no per-tool write — so this item is now the skill-listing half alone. The paragraph is kept as the record of that contract.
   **(original item, for the contract:)** **`GET /v1/tools`, `/v1/skills` — owned by the extension design's C6; read-only until then.** The `AppState` plumbing is unchanged (clone the tool-registry `Arc` before its move into `Orchestrator::new` at `main.rs:373`; two `AppState` fields — whichever of C6 and this item lands first owns that edit, the other rebases; the design recommends landing C6 first because it defines the shape). The **shape is the design's §8 contract, not rev 2's:** a bare array of `{name, description, source: "builtin"|"mcp"|"plugin"|"config", origin: {kind, id, enabled, state} | null, provides_capabilities, requires_confirmation, invocations_today, version, author}`. `origin` is **`null` for builtins and `config/tools/*.toml` tools** — a builtin row carries no enable field at all — and it **supersedes** `ToolCatalogEntry.denied: boolean` **and folds `provider: string | null` into `origin.id`** (`apps/openalpaca-gui/src/lib/api/unbacked.ts:288-296`, the ADR-029 shape; C7 deletes both fields rather than leaving `provider` dangling). **`global_tool_deny` is no longer the source of anything here** (P-28; the design's C8 purges the key) — rev 2's "`global_tool_deny` → `denied`" read is withdrawn. There is no per-tool enable state anywhere in the system; availability is *derived* — (the agent's capabilities) ∩ (its extension being enabled) — never asserted per tool; **no `PUT`, no per-tool toggle (S1)**. `invocations_today` = `COUNT(*)` over `tool_execution_log` since **local midnight converted to UTC** (`timestamp` is UTC text — migration 030 line 37; the index `idx_tel_tool_ts` serves the predicate; the count lags a call by one bus hop because the daemon's event persistence writes the row, not the sandbox). `author` exists; `destructive_hint` → `requires_confirmation` (`sandbox/mod.rs:398-417`). Sort by name (`DashMap` iteration jitters). `/v1/skills` is unchanged from rev 2 (bare array, §7 rule). If the owner ever adopts a per-tool deny **rule** (T1 — pending), the only addition is a read-only `denied_by: "<rule>" | null` — never an enable bit.
3. **D2 uploads re-home (final).** Existing `state/assets/` blobs → `uploads/<created-date>/NN-<name>.<ext>`: per-row move + `storage_path`/`rel_path` UPDATE, resumable, then delete `interim_assets_dir()`. Same mover discipline as §2.2.
4. **GAP-15 — provider enable/disable.** Only the write route is missing (`enabled` exists end-to-end: `router_config.rs:65-66`, `router_builder.rs:98-102`, served at `settings_service.rs:169-178`). `deregister_provider` for hot disable (strips models — enable must re-register **and** `refresh_models()`); extract `persist_only(mutate)` from `persist_and_reload` **and route it through `config_io::atomic_write_toml` / `atomic_write_with_backup`** (§1.4, P-11) so `llm.toml` gets the same rotation and unparseable-copy behaviour as the design's `mcp.toml` and `.permissions.toml` writers; **409 when disabling the default model's provider**.
5. **GAP-20 — template run counts** (counts only): `agent_task_history ⨝ agent GROUP BY template_id` — after P8, prefer `subagent_span`. Document that counts are *completed* runs. `?window=7d`, 400 on unknown windows. The `enabled` toggle stays deferred (enforcement in the spawn path is the real cost). **Shape for when part 2 is picked up (P-31):** model "template disabled" as a **deny-class rule on the spawn capability** — `spawn_subagent` refuses template X with an attributed error; `resolve_agent_tools` and the lead's template listing omit it — reusing the S4 refusal wording and the design's log/event plumbing, **not** a third toggle axis with its own persistence and reconciliation (Claude Code's `permissions.deny: ["Agent(name)"]` is the precedent). Builtins/templates are governed by policy, extensions by toggle; keep it that way.
6. **GAP-17 — connector detail.** `source`, `registered`, **`messages_7d`** (§0). The "unwired" badge needs nothing server-side (`hooks/useConnectors.ts:33-44`). Fix the hardcoded name match (`connectors.rs:34-38` — Discord falls through to the raw id).
7. **GAP-08c — `GET /v1/usage/summary?window=today`.** Totals from `query_daily_usage`; nested `caps` object; `by_provider` from `llm_call_log` for today (not the lifetime `all_provider_usage()`); echo the authoritative UTC `date` (client's `todayIsoDate()` is local — they disagree up to 12 h/day). Per **N4**: `caps` is `{workflow_max_cost_usd, agent_max_cost_usd}` — the per-workflow and per-turn `max_cost` values, named as such. **No `daily_*` key and no daily cap**; today's total ships as an unbounded figure (no progress bar, no denominator). The GUI copy at `views/settings/ConnectionSection.tsx:11,95` changes from "the cap is not served" to "spend is not capped daily by design; caps are per workflow" in the same commit — the current omission of the bar is already correct, only the reason is wrong.
8. **GAP-13 — per-chat model override.** `HandleRequest` struct (not a 9th positional arg on `MessageHandler::handle` — `gateway/router/mod.rs:63-73`; mechanical across `gateway_bridge.rs:52,81`, `followup.rs:87`, `scheduled_skills.rs:324`, two stubs) → `LoopOverrides::MainLoop` seam → `LoopConfig.model`. Validate via `model_registry().get_model_info()`, 400 `UNKNOWN_MODEL` (a bogus id silently degrades the trimming budget — `simple_query_handler.rs:256-260`). Request-scoped only; lane persistence later via the `preference` KV (`lane_model:{lane_key}`).
9. **GAP-24 (was GAP-19) — extension install/update/uninstall; plugin `source:"path"` only.** Disposition adopted verbatim from design §12.1: GAP-19 is **renamed GAP-24 — not subsumed, not deleted** — and **widened to cover MCP-server add/remove**; it stays scheduled here with its **mechanism unchanged** and is **not** part of the extension design (which owns the T/E sequences the verbs call and the identity rule — the plugin key is the *directory name*, design §2.2; this plan owns the on-disk ordering). Under the design a freshly installed plugin lands `enabled = true` (serde default) + consent `NeverSeen`, so **approving is the single action that starts it** — install grants nothing; the approve gate governs. The design's C7 replaces `"GAP-19"` with `"GAP-24"` in `apps/openalpaca-gui/src/lib/unavailable.ts` (union at `:18-41`, entry at `:37`, descriptor at `:233`) so the registry and this plan agree on landing; this item and the §8 row are relabelled now. **Specification (lessons P-29; the design points here — X-34):** **install** — parse `plugin.toml` *before* copying; return the manifest summary (`capabilities.provides`, `capabilities.virtual.provides`, `types` flags, `entry`, required config keys) as the approval preview beside the resulting `unapproved/never_seen` row; add a dry-run `POST /v1/extensions/plugin/validate {path}` that parses and reports without copying; record `installed_from`/`installed_at` in the `.permissions.toml` entry; 409 on collisions, reject paths inside `plugin_dir`, refuse escaping symlinks. **update** — `disable` (design T0–T5) → copy to `plugins/.staging/<name>` and rename over → `enable` (E0–E5, whose E1 drift check runs against the entry on disk **before** switch-in so the preview shows "Now also asks for: …"); 409 while the state is `Enabled/Enabling/Disabling`. The child runs with `current_dir(plugin_dir)` (`process_pool.rs:38`), so an in-place replace of a live plugin is never allowed — the staged rename is the point, and the design's `generation` stamp handles the in-process half; `plugins/.data/<name>/` (§1.1, P-7) is what survives the swap. **uninstall** — T0–T5 if anything is loaded → remove the permissions entry through the same atomic writer → move the directory to `plugins/.trash/<name>-<ts>/` (never `rm -rf` a user-dropped directory — §1.3 rule 3) → `keep_data` (default true) decides `plugins/.data/<name>/` → 200 with no row. **MCP add/remove** — a declaration block is written into `mcp.toml` through the same writer and reconciled by the design's watcher path; remove requires `Disabled` first. "Load in place from an arbitrary path" and **`source:"url"` stay declined** — the latter is its own security review.
10. **Delete `DagNodeStatus`** once the GUI renders `SubagentSpan` (P9).
11. **Artifact phase 2:** user-edit detection as a version with `author_agent_id = NULL`; `rebase_project` as the **one transaction** of §4.8 (file_assets + `session.workspace_id` + `task.workspace_id` + memory scope key) exposed as `PATCH /v1/workspaces {old_path,new_path}` and a CLI verb, and wired to the project picker (P-12).
12. **S2 — replay resume** (§5.6c): the algorithm, `action:"resume"`, `resume_enabled` config, synthetic resume interjection. The only speculative piece in the plan; everything before it is useful without it. Day+.
13. **S3 — snapshots** (§5.7 tail): `file_write` pre-edit images. M, deferrable indefinitely.
14. **`openalpaca plugin config get` replaces the file hint (P-30).** Rev 2's premise was wrong: the config really lives at `<root>/plugins/.config/<name>.toml` (`main.rs:331`, `permission_gate.rs:23,37`), so the `.config` suffix in `plugin.rs:225,230` / `CLI_Manual.md:222` is correct and must stay. The actual problem is that a doc telling the owner to *read* that file is safe only while it is guaranteed secret-free — once the design's `sensitive` config fields land (X-29; default store is owner decision T9, pending) it holds references to secrets. Replace the hint with a real `openalpaca plugin config get <name>` backed by the design's redacting `GET` on the config route (C6 already lists both); keep the path mention only as "where non-sensitive values are stored".
15. **`openalpaca store purge <project>|--all [--dry-run]` (P-5, XS).** Prints the deletion plan in the README's retention-class terms — session dirs, DB rows, uploads — and names the entries it will *not* touch (`artifacts/`, `memory/`, `skills/`, unknown names); `--dry-run` is the default until `-y`. Claude Code's `claude project purge --dry-run` is the shape.

---

## 7. Cross-cutting: envelopes, list shapes, task shape

Three error envelopes plus a plain-text 401 coexist: `{error:{code,message}}` (`chat_types.rs:101-111` = `files_types.rs:174-184`), `{error:{code,status,message}}` (`settings_types.rs:6-17`), `{error:"string"}` (~30 ad-hoc sites), `(401, "Invalid token")` (`middleware.rs`).

- **New routes use Phase 0's shared `api_error()`.** Drop `status` from the envelope — it duplicates the HTTP status.
- **One exception, already landed: the `/v1/extensions*` family (R20).** Those routes use the extension design's §8 flat `{"error":"<word>"}` envelope, where the word (`not_loaded`, `store_unreadable`, `unsupported_for_kind`, `orphaned`, `not_orphaned`) is the error's own `Display` and both clients match on it; §8 chose it deliberately as *"not a third envelope"*. `not_orphaned` alone adds a `message` field explaining that an orphan is only re-scanned at the next daemon start. `GET /v1/tools` is a bare array, exactly as the list-shape rule below already says.
- **Do not retrofit the `{error:"string"}` sites now.** Both clients absorb all shapes (`lib/http.ts:72-107`, `client.rs:147-153`); the retrofit is churn with a real regression surface — its own commit after the GUI work. (Deliberately *not* part of the §3 purge: this is churn-avoidance, not legacy-compat.)
- **List shapes follow the existing legible rule** — paginated ⇒ `{items,total}`; unbounded ⇒ bare array. Codified, nothing changed. `/v1/tools` and `/v1/skills` are bare arrays; `/v1/sessions*` and `/v1/events/history` are envelopes (the latter *always*, per P20).
- **`GET /v1/tasks` vs `/{id}` normalisation** happens in Phase 4 with P8, where the agent-run representation is being replaced anyway. Phase 0 does the cheap half (typed `TaskSummaryResponse`).

---

## 8. Gap disposition — all 23, plus the session pillar

| Gap | Phase | Needs | Effort |
|---|---|---|---|
| **GAP-01** approval_scope | 0 | Two lines; enforcement path already complete. | XS |
| **GAP-07** empty title/name | 0 | `title`/`name` on 4 event variants; 5 producer sites have the value. | XS |
| **GAP-08a** daily_cost | 0 | `query_daily_usage`, not the since-boot tracker. | XS |
| **GAP-08b** cost by task | 0 | `task_id` on `LlmUsageQuery` + `cost_for_tasks()` on `GET /v1/tasks`. | S |
| **GAP-16** `/v1/me` | 0 | Fields exist on `AppState`; `sources[]` = distinct session source. | XS |
| **GAP-22** plugin event ts/id | 0 (**pending T14** — drop or keep) | 6 variants + `with_instance_id`; full-workspace rebuild. The extension design's C7 deletes these variants; see §0 pending table. | S |
| **C-3 / P-3** main-loop cost lockout (not a GUI gap; `tasks/bug-main-loop-cost-lockout.md`) | 0 (**A5**) | Baseline `LoopState.last_cost` from `agent_cost()` before round 0 (`runner/agentic_loop/mod.rs:68`, delta at `:313-317`); option 1, no daily budget, no attribution change. | XS |
| **GAP-04** artifact API | 2 → 3 | Migration 036, `ArtifactStore`, `artifact_write`, `/v1/artifacts*`. The Library's whole blocker. | L |
| **GAP-05** versions & diff | 3 | `artifact_versions` (036) + `similar` + `/versions`, `/diff`. | M |
| **GAP-11** `?token=` content | 3 | Third merged sub-router; inline token check; CSP with the preview. | S |
| **GAP-12** server-side pins | 3 | `pinned` (036) + `PUT …/pin`. | XS |
| **GAP-09** subagent timeline | 4 | Migration 037 `subagent_span`; ~6 edit sites; lead span; broker metadata; `ToolContext.agent_instance_id`; route + event. Largest single item after the store. | L |
| **GAP-10** per-run event log | 4 | `event_log.task_id` (037) + `ctx.task_id` passthrough + retro-fit 6 variants. Envelope always (P20). | M |
| **GAP-02** steer endpoint | 5 | Pure reuse of `push_steering`; lane from `task.source_lane`. | S |
| **GAP-03** follow-up routes | 5 | `list_by_lane` + `cancel_if_queued` (CAS) + 3 handlers + `FollowupView`. | M |
| **GAP-06** rerun / start | 5 | `rerun_task`; `source_task_id` (037); `dispatch_lead_agent_with_id` for `start` (D5). | M |
| **GAP-23** message→run links | 6 | Migration 038; `persist_assistant_message` param; `role='artifact'` on the completion message. | M |
| **GAP-21** conversation CRUD | **7a** | **Re-homed onto sessions** — `PATCH`/`DELETE /v1/sessions/{id}`; built once; `/v1/conversations` deleted (P19). | S (inside 7a) |
| **GAP-14** `/v1/status` | 8 | Phase A; single-root fields; `log_path` per N2. | S / day+ |
| **GAP-18** `/v1/tools`, `/v1/skills` | 8 → **extension design C6** | Clone the registry `Arc` before the move; 2 `AppState` fields. Shape = design §8 (`origin` replaces `denied` + `provider`); read-only; no per-tool toggle. | M (in C6) |
| **GAP-15** provider toggle | 8 | Write route + `deregister_provider` round trip + 409 on the default model's provider. | M |
| **GAP-17** connector detail | 8 | `source`/`registered`/`messages_7d`. | M |
| **GAP-20** template metrics | 8 | Counts only; source flips to `subagent_span` post-P8. `enabled` deferred. | M / day+ |
| **GAP-13** per-chat model | 8 | `HandleRequest` refactor + `LoopOverrides` seam. Request-scoped only. | day+ |
| **GAP-24** (was GAP-19) extension install / update / uninstall | 8 | Plugin `source:"path"` only, preview + `validate`, staged update, trash-not-delete uninstall with `keep_data`; MCP add/remove via the declaration writer; URL declined. Renamed per design §12.1; mechanism unchanged. | day+ |
| **SES-01** session identity + surface | 7a | Migration 039; `/v1/sessions` family; chat `session_id`; follow-up pinning; report into originating session. | L |
| **SES-02** session event log | 7b | `SessionLogService`; loop/gateway/steering emit points; spill; `/events`. | L |
| **SES-03** honest interruption + steering recovery | 7b | `interrupted` status; sweep rewrite reading JSONL tails. | M (inside 7b) |
| **SES-04** replay resume | 8.12 | §5.6c; `resume_enabled` gated, off by default. | day+ |
| **SES-05** file_write snapshots | 8.13 | Pre-edit images + `file_snapshot` records. | M, deferrable |

---

## 9. Risks and breaking changes

| Risk | Reality | Mitigation |
|---|---|---|
| **The live-DB root move** (top risk) | The mover renames a WAL-mode SQLite DB, the master key, user-approved plugin permissions, and the encrypted-keys `config/` — at boot, on the only real install. | Five-part mitigation, all in §2.2: (1) **live-daemon guard** — non-blocking lock probe on the old root aborts rather than racing a running process; (2) **per-entry atomic rename** on the same volume, `EXDEV` aborts rather than degrading to copy; (3) **idempotent resume** — skip-if-destination-exists, abort-on-first-failure, next boot continues where it stopped; (4) **WAL/SHM sidecars move before the `.db`**, and the mover always completes before `Database::open` in the same boot, so SQLite never pairs a split trio; (5) **`rebase_asset_paths`** repairs stored absolute paths idempotently after open. Worst case if all else fails: the DB trio is intact at one root or the other, never half-open. |
| **Atomic three-binary rebuild** | An old GUI/CLI binary reads discovery from the old root and concludes no daemon runs (the GUI would spawn-loop). | One commit rebuilds daemon + CLI + GUI; no compatibility window by design (directive 2). Verified consumer inventory (appendix R §0) shows discovery consumers need no code changes, only the rebuild. |
| **`.alpaca` roots go quiet** (P3) | A directory that was a workspace root only via hand-made `.alpaca` stops resolving; its memories go quiet. | One-time `mv .alpaca .openalpaca` by the user; stated in the commit message. |
| **Orphan sweep eats every produced artifact** | Real the moment 036 lands (`background.rs:308-357` deletes row **and file** after 24 h). | §4.5 fix in the same commit; test: a produced row survives 25 simulated hours. |
| **Upload quota starts rejecting uploads** | `total_storage_bytes` sums all rows against 500 MB. | §4.5 fix in the same commit. |
| **`conversations` → `session` table rebuild** (039) | A data-copying migration over the primary chat table on the live DB. | Runs inside the migration transaction like every numbered migration; the copy is column-for-column with verified defaults; backfill preserves today's semantics exactly (one active session per lane). Rollback to 38 loses only the session columns — messages are untouched (`conversation_messages` is only `ALTER`ed). |
| **Session log overhead in the loop** | A new write on every round/tool call. | Non-blocking `try_send` to a per-session writer; drops (counted) over stalls; `sync_data` only at declared boundaries + 5 s timer; never per token (§5.4). |
| **`.openalpaca/` silently changes memory scoping** | The first artifact write creates a workspace-root marker where none existed. | Deliberate and desirable; documented; the marker change and the store land together. |
| **Everything lands in the home root** | Certain until a client sends `x-workspace-path`; none does today. | §4.7 items are Phase 2/3 scope, not "later". |
| **CSP loosening** | `blob:` + loopback origins widen the webview's image surface. | Land with the preview, never before; HTML previews are a separate review. |
| **`?token=` in a URL** | Long-lived token in webview history / `Referer`. | Pre-existing posture (`/v1/chat/stream`); add `Referrer-Policy: no-referrer` on content responses; short-lived per-asset tokens deferred. |
| **`start` id-injection** (D5) | The messiest code in the plan: create-or-update in the dispatcher's persist step. | Isolated in `TaskRepository::upsert_queued`, called out in review. |
| **New `similar` dependency** | Workspace has no diff crate. | MIT, pure Rust, no build script, `openalpaca_storage` only; alternative: hand-rolled LCS. |
| **`DagNodeStatus` double emission** | Two event families describe the same spawns for a phase. | Bounded: deleted in Phase 8 once the client switches (P9). |
| **A0 flips the meaning of an empty allowlist** (Phase 0) | Any surface that today runs with a legitimately empty policy and relies on "empty = everything" starts denying every non-ambient tool. | The type change forces every policy site to spell `Unrestricted` or `Only(..)`; the seven sites are enumerated in A0 and the compiler finds the rest. The file-based and nested skill paths already dodge the hole via `policy_opt = None`, so the observable change is confined to plugin skills with unresolved requirements — exactly the escalation being closed. Review as a security change. |

---

## 10. Explicitly out of scope

- **A full project concept** (`GET /v1/projects`, activation, a switcher). §4.7's items suffice for the Library and sessions. Whoever adds it reconciles with `SkillCatalog::scan_multi_scope`/`SkillScope` and points project skills at `.openalpaca/skills/` (§1.2).
- **Daemon file logging** (GAP-14 Phase B): `tracing-appender` + rotation + retention + un-discarding the GUI sidecar's stdout.
- **Tool/extension enable writes** are **no longer out of scope of the project — they are out of scope of *this plan*:** N5 is resolved and `tasks/extension-enable-design.md` rev 15 (verified 2026-09-02) owns them end to end (C1–C8), including `GET /v1/tools` (Phase 8 item 2 defers to its C6) and the `global_tool_deny` purge. **Agent-template `enabled`** (GAP-20 part 2) stays out: it is the ALLOW axis's own question and needs enforcement in the spawn path; when picked up it takes the deny-class-rule shape of Phase 8 item 5 (P-31), not a third toggle axis.
- **Extension-tool surface loading policy** (a LOADED axis — defer/threshold/always-load per install unit) — **owner decision T3, pending** (§0). Phase 0 A4 lands the byte-based measurement either way; nothing is built until the 10 % threshold is observed crossed on the owner's real configuration (today `config/mcp.toml` declares zero enabled servers, so the cost is 0).
- **Remote plugin install** (`source:"url"`) — its own security review.
- **HTML artifact previews** — `frame-src`/sandbox is a security decision.
- **Retrofitting the ~30 `{error:"string"}` sites** and **normalising list shapes** (§7) — churn with a regression surface.
- **The `AgentConfigFile` template-vs-instance redesign** (P17) — flagged OWN-TASK; the "legacy" shape is the live GUI contract.
- **Connector idle-auto-archive / in-chat `/new`** (N1) — **resolved**: perpetual session, knob reserved and off. Revisit only if connector lanes grow unwieldy. Reserving the `/new` command *name* (a deterministic-tier op mapped to `POST /v1/sessions` for the lane) is **owner decision T11 — pending**, not applied; N1 itself is not reopened by it.
- **Full-content `assistant_msg` JSONL records** (log-only session export) — decided *no*: the DB is authoritative for content; revisit only if export-a-session-as-one-file becomes a feature.
- **Session `results/` retention beyond the §5.4 caps** — `results/` is evicted *with* its session's log under `log_max_total_bytes` (a spill file is part of the log it belongs to, and orphaning one from its `log.jsonl` would leave an unreadable pointer). A finer-grained policy — evicting spill payloads while keeping the records that reference them — is deliberately not designed; revisit only if spill dominates the root. **Precedent recorded (P-32):** Claude Code keeps `<session>/tool-results/` beside the transcript and sweeps it with the transcript and `subagents/` — the same shape, independently arrived at, which also corroborates D1's single root for session state; a future "move spills next to the project" proposal is foreclosed.
- **Whole-workspace snapshots / task-state checkpoints** — a VCS's job / redundant with `state_json`.
- **A `resync_needed` WS signal** (`routes/events.rs` drops on `Lagged`) — worth doing, not blocking any surface.
- **Two daemons over one shared project directory** — documented limitation.
- **Automated reverse-mover** (rollback of §2.2) — the entry ledger is reversible by hand; pre-release, single user.

---

## 11. Migration ledger

One ledger, no conflicts. Each file ends with its own `UPDATE schema_version`; `database/tests.rs:11` asserts the head version and updates with each.

| # | File | Phase | Contents |
|---|---|---|---|
| 034 | *(head today — verified)* | — | `drop_context_compaction_log` |
| **035** | `035_drop_planner_telemetry.sql` | 1 | P7: `DROP COLUMN` `orchestrator_latency.planner_ms`/`dispatch_ms`, `dispatch_decisions.planner_requested_mode` (pre-check: SQLite ≥ 3.35, else 024-style rebuild) · optional P15 `event_log` timestamp normalise |
| **036** | `036_artifact_store.sql` | 2 | 11 `file_assets` columns · 4 indexes · `artifact_versions` · `task.workspace_id` |
| **037** | `037_run_observability.sql` | 4 | `subagent_span` · `event_log.task_id` · `task.source_task_id` |
| **038** | `038_message_run_links.sql` | 6 | `conversation_messages.task_id` |
| **039** | `039_sessions.sql` | 7a | `conversations` → `session` rebuild (drops `UNIQUE(lane_key)`; workspace + lifecycle; partial unique active index) · `session_id` on `conversation_messages`/`task`/`lane_followups` · `tool_execution_log` index columns |

Two **unnumbered boot-time fixups**, listed for completeness but explicitly not schema migrations: `move_app_root()` (filesystem, §2.2 — idempotent, resumable) and `rebase_asset_paths()` (runtime-prefix UPDATE, §2.2.6 — idempotent, zero rows after first boot).

Sequencing constraints encoded above: 035 before everything path-dependent lands beside the mover; 039 **after** 038 (the `#[derive(Default)]` churn payment) and after 037 (the span ids and sandbox passthrough the session log references). Rev 1's numbering (artifacts=035…links=037) shifts up one because the purge migration takes the first slot; Lens R's "038_drop_planner_telemetry" and Lens S's "038_sessions" collided — resolved by phase order: the purge migrates first, sessions last.
