# Daemon API Requirements — surfaced by the GUI rework

**Date:** 2026-09-01 · **Branch:** feat/ui-rework · **Source of truth in code:** `apps/openalpaca-gui/src/lib/unavailable.ts` (23-entry gap registry; every unbacked surface in the UI renders its gap id + proposed endpoint) · **Full detail:** `apps/openalpaca-gui/API_MAP.md` §3.

The new UI is fully built. It ships **no mock data**: every surface the daemon cannot serve renders its real chrome plus an honest note naming the missing API. This is the list of what to build to light those surfaces up, ordered by cost-to-value.

---

## Tier 1 — Bugs & one-liners (do these first)

| # | What's wrong | Fix | Unlocks |
|---|---|---|---|
| **GAP-01** | `ConfirmationBody` is `{approved}` and passes `approval_scope: None`, though `ApprovalScope::{TheseArgs,EntireTool}` exists in core and `ApprovalCache` honours it | Accept `approval_scope` on `POST /v1/chat/confirmations/{id}` | The **"Always allow" button** (UI already sends `entire_tool`; today it silently approves once) |
| **GAP-07** | `event_bridge.rs` passes `""` for `task_status.title` (all but TaskCreated) and `agent_status.name` (always) | Pass the real values — `task_registry` already holds them | Removes an N+1 refetch per status tick; run titles appear instantly |
| **GAP-08a** | `get_orchestrator_config` hardcodes `daily_cost_usd = 0.0` with a stale "async" comment; the handler is already async and `cost_tracker` is used 3 fns later | `await` the tracker | Today's spend in Settings + the composer budget line |
| **GAP-11** | `/v1/files/{id}/content` is header-auth only, so `<img>`/`<iframe>` can't load it | Accept `?token=` inline (as `/v1/chat/stream` and `/v1/events` already do) | Inline artifact previews without a blob-URL dance |
| **GAP-22** | Six `ServerEvent::Plugin*` variants omit `ts`/`instance_id` | Add both fields | Plugin events can be ordered/deduped like every other event |

## Tier 2 — New routes over data that already exists

| # | Need | Proposed API |
|---|---|---|
| **GAP-02** | Steering is reachable only by POSTing the literal `"/steer "` prefix to `/v1/chat`, which targets the lane, not a task | `POST /v1/tasks/{id}/steer {message}` |
| **GAP-03** | `lane_followups` table + `FollowupQueued` event exist with **zero routes** — only the model can write one | `GET/POST /v1/lanes/{lane_key}/followups`, `DELETE .../{id}` |
| **GAP-06** | Task actions are exactly `cancel\|pause\|resume` | Add `rerun` and `start` verbs (note `POST /v1/tasks` persists a row but never dispatches, so it can't serve as re-run) |
| **GAP-08b** | `LlmUsageRepository::get_task_usage(task_id)` exists; `LlmUsageQuery` only accepts `agent_id`/`key_id` | Add `task_id` param; optionally `GET /v1/usage/summary?window=today` |
| **GAP-10** | No way to scope the event log to one run | `GET /v1/events/history?task_id=…&event_type=…` |
| **GAP-13** | Per-chat model choice mutates global config | Accept an optional `model` on `POST /v1/chat` |
| **GAP-14** | No uptime, schema version, or log path | `GET /v1/status` (authenticated sibling of `/v1/health`) |
| **GAP-16** | The default lane key isn't discoverable by the client | `GET /v1/me` → `{user_id, default_lane_key}` |
| **GAP-18** | No tool or skill listing (only `/v1/skills/health`, which has no names) | `GET /v1/tools`, `GET /v1/skills` |
| **GAP-21** | Conversations can't be renamed or deleted | `PATCH`/`DELETE /v1/conversations/{id}` |

## Tier 3 — Needs a schema change (the big ones)

- **GAP-04 — Artifact API. Blocks the entire Library view.** `FileAsset` has no `task_id`/`agent_id`, and `FileAssetRepository` exposes only `get_by_id`/`list_orphaned`/`list_by_status`; there is no list route. Needs a migration (ownership columns) plus `GET /v1/artifacts?task_id=&kind=&pinned=`, `GET /v1/artifacts/{id}`, `/content`.
- **GAP-05 — Artifact versions & diff.** No history table exists in any of the 34 migrations, so the design's `v1→v2`, History and Diff tabs are unimplementable. Needs a versions table + `GET /v1/artifacts/{id}/versions`, `/versions/{n}/content`, `/diff?from=&to=`.
- **GAP-09 — Subagent timeline. Blocks the Work view's "Parallel work" swimlanes.** `agent_task_history` has no `started_at` (only `runtime_seconds` + `completed_at`, so in-flight spans can't be derived). `DagNodeStarted`/`DagNodeCompleted` **are** still emitted — by the lead agent's subagent spawn path (`runner/lead_agent/tools.rs:232,529,614`), not the deleted DAG executor (corrected 2026-09-01; an earlier revision of this doc wrongly called them dead). They are point-in-time pings, though: no row exists until completion, since `record_agent_history` is only called post-loop. Needs a purpose-built `subagent_span` table written at spawn, `GET /v1/tasks/{id}/timeline`, and a `SubagentSpan` event — see `tasks/api-fix-plan.md` Phase 3.
- **GAP-12 — Server-side pins** for artifacts (currently client-only would be per-machine).
- **GAP-23 — Message→run links**: chat messages carry no reference to the runs/artifacts they produced, so the transcript can't deep-link into Work/Library.

## Tier 4 — Product decisions, not blockers

**GAP-15** provider enable/disable · **GAP-17** connector call counts + "unwired" signal + add flow · **GAP-19** plugin install route · **GAP-20** agent-template run counts and enabled state.

---

## Notes for whoever implements these

1. **Two error envelopes and two list shapes coexist** — chat/settings use `{error:{code,message}}`, tasks/agents/plugins use `{error:"string"}`; some routes return bare arrays, others envelopes. New routes should pick the envelope style and stay consistent (the client handles both today).
2. `GET /v1/tasks` and `GET /v1/tasks/{id}` return genuinely different shapes (summary `assigned_agents` vs full `AgentTaskHistory[]` under the legacy `assignments` key). Worth unifying while adding the timeline.
3. **CSP**: when GAP-04/11 land and previews render blob URLs, `apps/openalpaca-gui/src-tauri/tauri.conf.json` needs `img-src 'self' data: blob:`. Deliberately not loosened now.
4. The WS is best-effort — `routes/events.rs` logs and continues on `RecvError::Lagged(n)`, so clients silently lose events. The UI treats it as additive only and refetches; a `resync_needed` signal would let it be precise.
