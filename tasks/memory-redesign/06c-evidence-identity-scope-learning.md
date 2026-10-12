> Evidence appendix to [05-review.md](05-review.md), rev 2 review of 2026-09-23. Read-only map of owners, workspaces, audience and automatic learning at HEAD `0d490847`, written to check the assumptions of [03-design.md](03-design.md) rev 1.

# Identity, scope and learning today — map for checking `03-design.md` §2, §3, §6

Snapshot: branch `chore/deps-upgrade-and-simplify`, HEAD `0d490847`, 2026-09-23. Read-only source reading plus one throwaway in-memory SQLite probe (scratchpad, no repo or `~/.openalpaca` access). Line numbers are today's.

Path shorthand: `core/` = `crates/openalpaca_core/src/`, `storage/` = `crates/openalpaca_storage/src/`, `conn/` = `crates/openalpaca_connectors/src/`, `d/` = `apps/openalpacad/src/`.

---

## 0. Findings that contradict or weaken a design assumption (read first)

| # | Design assumption | What the code does | Evidence |
|---|---|---|---|
| A1 | §3 rule 5: "shared/group connector lanes" can be denied recall | **No group lane exists.** A connector turn's lane is `{owner}:{platform}` — one lane (and one active session) per owner per platform, mixing every DM and every group of that platform. The gateway reduces `EventSource` to a platform string; the chat id survives only as `Scope::Conversation{id}`, which nothing in core reads. Core cannot tell a group turn from a DM turn. A lane-level "no recall in group lanes" rule would switch recall off for all Telegram/Discord/iMessage DMs too. | `core/gateway/router/mod.rs:261-279,500-509`; `core/security/policy.rs:23` (only definition); `core/orchestrator/context_builder.rs:30-63` |
| A2 | §3 rule 5: "identity equality is not sufficient permission" (implies identity = one human) | Identity already covers several humans. **iMessage:** every processed message runs as the local owner — any group member who types `/ask`/`@openalpaca`, and any non-owner DM that uses the prefix. **Telegram/Discord:** every account linked with a code from `/v1/auth/link` becomes the owner. Those humans can recall, `memory_store` and `memory_forget` the owner's memories, and their statements feed the owner's `USER.md` extraction. | `conn/imessage/handler.rs:98,219-244`; `conn/imessage/routing.rs:95-160`; `d/routes/auth.rs:18-31`; `core/tools/builtins/main_loop.rs:158-184`; `core/orchestrator/handlers.rs:337` |
| A3 | §3 rule 5: blocking recall keeps private facts out of groups | Recall is one of five paths. In a group reply today the model also gets the `USER.md` system block, the shared lane's transcript and summary (which include the owner's DMs on that platform), and the task list. Separately, run completion/failure/progress notices go to the "last chat" of each connector, which may be a group. That includes GUI- and CLI-origin runs through the cross-channel fan-out, and Telegram also gets the run's artifact files. | `core/prompt_ctx/sources/user_profile.rs:25-61`; `context_builder.rs:52-63`; `d/notification/mod.rs:162-193,228-232,311-317,358-381` |
| A4 | §3 "factory reset … destroyed identity mappings must not cause the next user to inherit" | The owner id is `system_config['identity.local_user_id']`, which lives in the DB. **A non-factory `openalpaca config reset` (and the TUI's "Reset All Config") deletes it** (`DELETE FROM system_config`) while promising "agents and data preserved". The next boot then mints a new UUID, and every existing memory, lane, session, task and artifact row belongs to an owner nobody is any more. A factory reset also mints a new id. In both cases `config/orchestrator/USER.md` survives and is injected for the new owner. So the next user already inherits the learned profile. The realistic failure is that the rightful owner is orphaned, not that a stranger inherits. | `d/bootstrap/migration.rs:38-64`; `storage/repository/config.rs:84-90`; `apps/openalpaca/src/commands/config_handlers.rs:466-476`; `config_tui.rs:84-96`; `config_factory_reset.rs:166-212` |
| A5 | §3 "use safe internal owner keys" | This is needed. `POST /v1/command {"command":"link_generate"}` creates a **new** `global_user` (a second owner) when `args.user_id` is absent, and accepts an arbitrary string as the owner id when it is present. | `d/routes/command.rs:138-196` |
| A6 | §5/(a): losing automatic recall of extracted notes is a big change | Automatic per-turn recall is already weak. `MemorySource` is FTS-only, and its query is the whole user message with every token quoted, which FTS5 treats as an implicit **AND**. The tokenizer is default `unicode61` with no stemming, and a run of CJK characters is one token. The probe matched 0 rows for "what editor do I prefer?" against "User prefers vim for editing", 0 for "简洁" against "用户喜欢简洁的回答", and 1 only for keyword queries. Recall that actually works comes from the model calling `memory_search` and from the lead's proactive injection, both vector-assisted with the seeded local E5 model. The strongest learning effect today is the background `USER.md` writer (b), not (a). | `storage/repository/memory/search.rs:13-24`; `core/prompt_ctx/sources/memory.rs:40-47`; baseline `001_baseline.sql:301-310`; `scripts/release/templates/config/llm.toml:46-50` |
| A7 | §3 "workspace moves … under the existing move lock" | **There is no move lock.** `PATCH /v1/workspaces` is a preflight (it refuses in-flight runs and foreign owners), then one DB transaction that re-keys `memory.scope_id` among other things, then one `rename(2)`. | `storage/store/project_move.rs:35-95` (no lock); `storage/artifacts/mod.rs:1502-1552` |
| A8 | §3 rule 1: "retire daemon-CWD fallback … must not select a project for connector/scheduled requests" | In shipped launches this fallback already does nothing. Both launchers start the daemon with CWD `~/.openalpaca`. The marker walk resolves that to `$HOME`, which folds to the home store, so the result is `None`. It fires only for a hand-started daemon, such as `cargo run -p openalpacad` from a checkout, where every connector and scheduled turn then scopes memory to the repo. Separately, **the CLI always sends its own CWD as `x-workspace-path`**, so a CLI turn is implicitly project-scoped by where it runs. The GUI's project is an explicit picker value. | `core/memory/scope_context.rs:100-175`; `apps/openalpaca/src/manager.rs:218`; `apps/openalpaca-gui/src-tauri/src/lib.rs:233,269`; `apps/openalpaca/src/chat_stream/mod.rs:127-190` |
| A9 | §6 "keep [USER.md's] explicit editor" | After onboarding, no explicit editor exists. `update_persona` is added to the surface only while bootstrapping, `file_ops` refuses writes to USER.md, and the GUI and CLI have no USER.md editor. Removing the background writer (b) leaves hand-editing the file as the only way to change it. Bootstrap completion requires USER.md content, and today the background writer can supply it. | `core/orchestrator/query_handler/simple_query_handler.rs:124-130`; `core/tools/builtins/file_ops.rs:260-266`; `core/orchestrator/bootstrap.rs:18-53` |
| A10 | §2 "Existing seeded text … update it"; ignore owner-memory in project `.gitignore` | The project `README.md` and `.gitignore` are written **only when absent**. Only the *home* README has a superseded-text refresh. Existing project stores keep "memory/ deliberately not git-ignored" and a `.gitignore` without a memory rule unless a new append or refresh mechanism is built. | `storage/store/mod.rs:478-515,873-880,925,940-975` |
| A11 | §2 "the existing session log is the evidence archive" | Session-log records hold a message id and a **preview**, not the text; the content stays in the DB. A factory reset deletes the DB and leaves `sessions/` orphaned. Sessions are swept at 2 GB total, oldest archived first. | `core/gateway/router/mod.rs:344-362`; `core/daemon_config/orchestrator.rs:69-74`; `core/session_log/sweep.rs:93-184` |
| A12 | §8 cost controls "behind existing router budgets" | The "daily" caps are counters accumulated since the daemon started, seeded at boot from today's usage rows. Nothing resets them at midnight. They also never bind on a local model, whose cost is $0. | `crates/openalpaca_llm/src/routing/cost_tracker/mod.rs:238-241,284-292`; `d/services/llm.rs:260-290` |
| A13 | §8 "reuse `notify`, UUIDs" in storage/core | Only `openalpaca_wake` depends on `notify`, and core may not depend on wake. The wake watcher also has **no add-path API** after start: paths are fixed, and missing ones are skipped. `uuid` is v4 only (no v7). In `openalpaca_storage`, `tempfile` is a dev-dependency only. | `crates/openalpaca_wake/Cargo.toml:12`; `crates/openalpaca_wake/src/watcher/filesystem/mod.rs:134-178`; root `Cargo.toml:44`; `crates/openalpaca_storage/Cargo.toml:31` |

---

## 1. Owners

### What an `owner_id` is

- **One local owner per install.** `resolve_local_user_id` reads `system_config['identity.local_user_id']`. If the key is missing, it mints a UUID v4, stores it, and creates a `global_user` row whose `display_name` is `$USER`/`$USERNAME` (`d/bootstrap/migration.rs:38-64`). The daemon resolves it once at boot (`d/main.rs:238-240`) and holds it as `AppState.local_user_id` (`d/state.rs:49`).
- In the orchestrator, `owner_id` is `Some(global_id)` only for `Principal::User` (`core/orchestrator/handlers.rs:75-79`). `Principal::System` and `Principal::External` produce `None`, which means no recall, no extraction, and memory tools refuse with "Cannot store memory without an owner_id" (`core/orchestrator/memory_ops/mod.rs:25`). `principal_id()` renders External as `provider:id` for task and lane attribution only (`core/orchestrator/mod.rs:568-574`).

### Derivation per surface

| Surface | Principal / owner | Lane | Evidence |
|---|---|---|---|
| GUI (`POST /v1/chat`) | `Principal::User{local_user_id}`; no per-user auth (the discovery bearer token *is* the owner) | `{local}:gui` | `d/routes/chat.rs:393`; `core/chat/service.rs:137,193-199` |
| CLI chat | same route, same owner | `{local}:gui` (shared with every GUI window — T18) | `apps/openalpaca/src/chat_stream/mod.rs:127-190` |
| `POST /v1/command process` | `Principal::System` → owner `None` | `{request_id}:api` (a new lane per call) | `d/routes/command.rs:73-110`; `router/mod.rs:506` |
| Telegram | `resolve_principal("telegram", from.id)`. Linked gives `User{global_id}`; unlinked gives `External`, and TrustGate **denies `chat.respond`** to External, so only linked accounts ever reach the handler | `{global_id}:telegram` (all chats of all linked accounts) | `conn/telegram/handler.rs:93-147,164-167,234-250`; `conn/common/mod.rs:13-34`; `core/security/policy.rs:49-66` |
| Discord | Same as Telegram, per Discord user id. Guild messages need an @mention; DMs always pass; optional `discord.guild_id` restriction | `{global_id}:discord` | `conn/discord/connector.rs:333-374,387-393,466-472,553` |
| iMessage | **Always** `User{local_user_id}` through `resolve_owner()`: the constructor id, then `system_config`, then as a last resort a fresh UUID global user *per message* | `{local}:imessage` (all chats) | `conn/imessage/handler.rs:98-103,213-244`; `conn/lib.rs:263-274` |
| Scheduled skills | `User{local_user_id}`, `EventSource::Internal`, `unattended: true` | `lane_override = {local}:scheduled` | `d/scheduled_skills.rs:17-20,195-225` |
| Follow-ups | The principal persisted on the row (`principal_json`), replayed with the stored `workspace_path`; route-queued ones use the local owner | `lane_override` = the originating lane | `d/followup.rs:36-50`; `d/routes/followups.rs:244-258`; `core/runner/lead_agent/tools.rs:1406-1481` |
| Lead agent | `task.created_by` (from `ToolContext::created_by()`: principal global id, else owner id, else `"system"`) | the task's `source_lane` | `core/tools/registry/mod.rs:149-158`; `core/runner/lead_agent/mod.rs:256-275` |
| Subagents | `owner_id = created_by`, `principal: None`, lane-detached, same workspace pair as the lead | none | `core/runner/lead_agent/tools.rs:338-357` |

Linking: `POST /v1/auth/link` issues an 8-character code for `local_user_id` (`d/routes/auth.rs:18-38`). `/link <code>` in a chat consumes it and runs `migrate_lane_on_link`. That moves messages, tasks, conversation_map, the `conversation_summary` pref, sessions and follow-ups from `{provider_uid}:{p}` to `{global}:{p}`. It does **not** move `memory.owner_id` or `file_assets.owner_id` (`storage/repository/identity/mod.rs:380-477`). Neither the GUI nor the CLI exposes link-code generation; the only way is a raw API call (the only doc mention is `docs/Daemon_Manual.md:560`). `/unlink` drops the account back to External, which cannot chat.

### Several owners on one install

- **Normally there is one.** All owner-scoped HTTP reads — `/v1/me`, artifacts, tasks, sessions, files, workspaces — use `state.local_user_id` (for example `d/routes/auth.rs:67-70`, `d/routes/artifacts.rs:525`).
- **Extra owners can appear three ways.** (1) `link_generate` with no `user_id` mints a new `global_user` and a code for it (`d/routes/command.rs:138-196`). A chat account linked with that code then runs as a second owner with its own memory partition and `{id}:{platform}` lanes, invisible to every HTTP surface. (2) `link_generate` with `args.user_id` adopts any string as an owner id. (3) The iMessage last-resort path mints a new owner per message if `identity.local_user_id` is gone while the connector runs (`handler.rs:235-244`).
- **Several humans, one owner.** This is the common case: iMessage group members and non-owner prefixed DMs, and every linked Telegram/Discord account.

### Where identity is stored

All of it is in the DB (`001_baseline.sql`):

- `system_config` holds `identity.local_user_id`.
- `global_user` (`:15-20`).
- `external_identity(provider, provider_user_id → global_user_id)` (`:22-33`).
- `link_token` (`:51-59`).
- `conversation_map(provider, chat id → lane_key)`: many chats map to one lane (`:38-47`).
- `preference(user_id, key)`, which holds `telegram.last_chat_id`, `imessage.last_reply_target`, `imessage.last_is_group` and similar (`:70-79`).
- The owner is embedded in `task.created_by`, `task.source_lane`, `session.lane_key`, `conversation_messages.lane_key`, `lane_followups.{lane_key,principal_json}`, `memory.owner_id` and `file_assets.owner_id`.
- Outside the DB: `.layout` on the home root carries `install_id=<uuid>`, written once and never rewritten (`storage/store/mod.rs:12,470,667,834`). It survives every reset.

### What resets do

- **`openalpaca config reset --factory`** deletes the DB trio. The next boot mints a new `local_user_id`. `USER.md`, `SOUL.md` and `IDENTITY.md` survive under `config/orchestrator/`, as do `artifacts/`, `uploads/`, `sessions/` and `.layout`. The preview says "every memory" is gone (`apps/openalpaca/src/commands/config_factory_reset.rs:166-212`).
- **`openalpaca config reset`** (no key) and **the TUI "Reset All Config"** call `ConfigRepository::clear_all()`, which runs `DELETE FROM system_config` and so deletes `identity.local_user_id` while keeping every data row (`storage/repository/config.rs:84-90`; `config_handlers.rs:466-476`; `config_tui.rs:84-96`). At the next boot all data is orphaned under the old id. See A4.

---

## 2. Workspaces

### The resolver

`MemoryScopeContext::for_request(workspace_path)` (`core/memory/scope_context.rs:117`) produces two fields (`:153-180`):

- **A path was sent:** it walks up to the nearest `.openalpaca` or `.git` (`core/memory/workspace.rs:64-77`), canonicalizes, and folds `$HOME` or the home store to `None` (`resolves_to_the_home_store`, `:43-66`). Both `workspace_id` (memory) and `request_workspace_root` (content and placement) are set to the result.
- **No path was sent:** `workspace_id` comes from the daemon CWD walk, with the same fold. `request_workspace_root` is `None`, so this branch never places files.
- Cascade reads are `[Global, Workspace?]`, iterated most specific first (`:211-218`; `storage/repository/memory/search.rs:212-256`).
- `default_write_scope()` (`:198`) has **no production caller**.

### Callers

- **Gateway:** binds the session to `request_workspace_root` (`core/gateway/router/mod.rs:304-316`).
- **Handler:** builds `scope_ctx` once per turn (`core/orchestrator/handlers.rs:85`) and threads it into the main loop, the skill tier and the ContextManager.
- **Tools:** receive `workspace_id` and `request_workspace_root` on `ToolContext` (`core/tools/registry/mod.rs:20-50`). `start_workflow` stores it through `from_tool_context` (`core/tools/builtins/start_workflow.rs:130`).
- **Relaunch or resume:** runs `for_request(task.workspace_id)` again, so a task stored with no workspace gets the CWD fallback once more (`core/orchestrator/task_launch.rs:153`).

### Project store

`<project>/.openalpaca/` holds content only (`storage/store/mod.rs:448-462`). `content_dir(Project, kind)` first seeds the store, including its `.gitignore` (`:586-596`).

### What each surface sends

| Surface | What it sends |
|---|---|
| GUI | `x-workspace-path` from a per-machine `localStorage` project picker. No value means no header (`apps/openalpaca-gui/src/stores/project.ts:1-60`). |
| CLI | Always sends its canonicalized CWD (`chat_stream/mod.rs:127-133`). |
| A named `session_id` | The session's own binding wins over the header (R49, `d/routes/chat.rs:158-184`). |
| Connectors (Telegram, Discord, iMessage) | `workspace_path: None` (`telegram/handler.rs:245`, `discord/connector.rs:553`, `imessage/handler.rs:165`). |
| Scheduled skills | `None` (`d/scheduled_skills.rs:208`). There is no persisted workspace binding for a scheduled skill. |
| Follow-ups | The stored `workspace_path`. |
| `/v1/command process` | `args.workspace_path`. |

### The CWD fallback in practice

- Launched daemons run with CWD = the home store: `manager.rs:218` (the CLI's `runtime_dir`) and the Tauri sidecar at `src-tauri/src/lib.rs:233,269`. The walk resolves to `$HOME` and folds to `None`, so connector and scheduled turns get **global-only** memory.
- `cargo run -p openalpacad` from a checkout resolves to the repo, so connector and scheduled turns read and write that repo's workspace memory.

### Lanes

| Lane | Who shares it |
|---|---|
| `{owner}:gui` | Every GUI window and the CLI |
| `{owner}:telegram` | All Telegram chats (DMs and groups) of every linked account |
| `{owner}:discord` | All Discord DMs and guild channels |
| `{owner}:imessage` | All iMessage chats and senders |
| `{owner}:scheduled` | All scheduled skills |
| `{request_id}:api` | One lane per `/v1/command process` call |

Follow-ups re-enter their origin lane. Lane key construction is at `core/gateway/router/mod.rs:269-279`. There is one active session per lane (`idx_session_active_lane`), and a context build replays that session (`core/orchestrator/context_builder.rs:52-63`).

---

## 3. Group chats and audience

- **Telegram:** the handler never inspects chat type (no group or private branch in `conn/telegram/*.rs`). A linked user's message in a group is handled exactly like a DM, on the same lane and session, and the reply goes to the group. Unlinked group members get a denial that advertises `/link` (`common/mod.rs:38-40`).
- **Discord:** guild channels need an @mention and DMs always pass (`connector.rs:333-358`). Same lane for both.
- **iMessage:** knows `is_group`, but only for the prefix rule and the reply target. By default groups require a prefix, then **any sender** is processed as the owner (`routing.rs:111-128`; `handler.rs:98`).
- **Core:** `HandleRequest.source` is a string such as "telegram" and `Scope::Conversation{id}` is unused, so no audience signal reaches the orchestrator, `ContextRequest` or `ToolContext`.
- **Confirmations:** `/yes` and `/no` are keyed by chat or channel, not by sender, so any group member can approve a pending tool (`conn/common/mod.rs:120-160`; `telegram/handler.rs:68-79`; `discord/connector.rs:312-330`; `imessage/handler.rs:44-56`).

**Can owner X's memory reach a group today? Yes.**

- Every main-loop and skill-tier turn with a User principal runs `MemorySource` (active on every path, `core/prompt_ctx/sources/mod.rs:35-37`; registered at `core/orchestrator/mod.rs:305-326`) and offers `memory_search`, `memory_store` and `memory_forget` (`core/tools/builtins/main_loop.rs:158-184`).
- In an iMessage group this applies to any member who types `/ask`. In Telegram and Discord groups, to the linked user.
- In the same reply: `USER.md` is a system message (`user_profile.rs:25-61`), and the lane's recent messages and summary include the owner's DMs on that platform.
- **Pushes:**
  - A run started from a connector reports to that platform's *last* chat. For Telegram that is the `telegram.last_chat_id` pref, then `conversation_map`; for iMessage, `last_reply_target` plus `last_is_group` (`d/notification/mod.rs:162-181,323-336,358-381`; `d/notification/imessage.rs:9-35`).
  - A GUI-, CLI- or scheduled-origin run's completion is fanned out to all three connectors' last chats, and Telegram also gets artifact files (`notification/mod.rs:182-192`). Failures (`:228-232`) and extension notices (`:311-317`) fan out the same way.

---

## 4. Learning today, end to end

### Background writers

All three run on the router's default model unless overridden, and all are fire-and-forget `tokio::spawn` with no queue and no drain.

**1. Trait extraction** (`core/orchestrator/extraction.rs:26-374`), spawned after each turn (`handlers.rs:330-392`) when `owner_id.is_some() && result.is_ok()`, on any lane including groups.

- **Gates:**
  - message ≥ `extract_min_content_len` = 20 bytes and not starting with `/` (`:44-49`)
  - every `extract_every_n_turns` = 5 qualifying turns *per lane*, counted in an in-memory map that restarts at boot (`:52-62`)
  - agent cost `orchestrator_user_extract` > `extract_max_daily_cost_usd` = $0.25 skips it (`:65-73`)
- **Input:** only **the current turn**, with the user message cut to 800 chars and the answer to 400 (`:77-78`). `max_tokens` 256, no reasoning, `extraction_model` defaults to `None` (the router default).
- **"memory" targets:** confidence ≥ `memory_confidence_threshold` = **0.65** (the prompt says 0.5; `core/daemon_config/orchestrator.rs:242`). Stored as a **Global Preference**, source Conversation, importance 0.6, confidence 0.7, no evidence ids. Similarity supersession applies (`extraction.rs:323-351`). Immediately recallable.
- **"profile" targets:** confidence ≥ 0.8 (set: fills only unset sections) or ≥ 0.9 (update: replaces). Fields: `identity.*`, `communication_style`, `expertise`, `projects`, `preferences`, `notes` (`:257-305`). Written to USER.md by read, parse, patch and render, then a fixed `USER.md.tmp` and rename. No fsync, no lock, no compare-and-swap (`:374-548`). It then swaps the in-memory doc, bumps `persona_version` and publishes `UserProfileUpdated{actor:"extraction"}` (`:550-592`). S6 made a seeded USER.md parse as empty so this writer can fill it; V5 keeps unowned lines.
- **What the user sees:** nothing in the GUI (it has no memory or profile surface). There is a daemon log line (`d/event_bridge.rs:262-276`, target `user_audit`) and usage rows under agent `orchestrator_user_extract` (`/v1/llm/usage`). From the next turn on, the answer behaves differently because USER.md is a system message on the main loop and skill tier. The lead's prompt passes `user_document: None` (`core/runner/lead_agent/prompt.rs:213`).

**2. Task-output extraction** (`core/memory/task_extraction/mod.rs:52-315`), spawned only from lead-agent completion (`core/orchestrator/dispatcher/lead_agent.rs:1096-1111` → `dispatcher/memory.rs:13-59`).

- **Gates:** success only; `task_extract_enabled` = true; output ≥ 100; cost > `task_extract_max_daily_cost_usd` = $0.50 skips it (`:60-85`).
- **Input and filtering:** description cut to 500, output to 2000; `max_tokens` 512; at most 5 items with confidence ≥ 0.5.
- **Storage:** kind fact or preference, importance as the LLM gives it, source `Tool`, metadata `{task_id, source_path}`. Scope is the **Workspace** if `workspace.workspace_id` is set (CWD fallback included), otherwise Global (`:243-298`).
- **Supersession:** similarity-based, and it can downgrade explicit memories (audit F7).
- **Visible:** a log line only.

**3. Lane summary** (`core/orchestrator/summary.rs:14-40`): ≥ 12 new older messages, $0.50 cap, stored on the session row. It is not memory, but it is replayed into every turn on that lane.

**Caps:** "daily" means cumulative since boot (A12), and $0 on local models.

### Recall paths

| Path | Where | Search | Scope |
|---|---|---|---|
| Automatic per turn (main loop, skill tier, forced/bootstrap) | `core/prompt_ctx/sources/memory.rs:25-74` | FTS only, whole message, implicit AND, 10 results, injected as an untrusted user message | cascade `[Workspace?, Global]` |
| Lead proactive injection at workflow start | `core/runner/lead_agent/mod.rs:415-487` | hybrid FTS + vector (embeds the task description), 5 results, 300 chars each | cascade if the workspace resolves; **otherwise every scope of the owner** (audit F1) |
| `memory_search` tool | `core/tools/builtins/memory_search.rs:20-110` | hybrid with the embedder | cascade on `ctx.workspace_id` (audit F2 bypass when `None`) |

Subagents receive an **empty** parent context bundle (`core/runner/lead_agent/mod.rs:218-220`), so they get no automatic recall.

### Who holds memory tools

- **Main loop** (per request, when a DB exists): `memory_store` and `memory_forget` as per-request instances, plus the global `memory_search` (`core/tools/builtins/main_loop.rs:158-184`).
  - `memory_store` writes a Preference, importance 0.9, confidence 1.0. The default scope is **Global**; a literal `--workspace` in the content opts into Workspace (`core/orchestrator/memory_ops/mod.rs:17-75,126-142`).
  - `memory_forget` deletes the best FTS Preference hit (`:81-114`).
  - Bootstrap and forced turns (`ModelOnly`) get keyword-suggested global tools only, so no store or forget (`simple_query_handler.rs:118-134`).
- **Lead:** `memory_search` is added **unconditionally** (`core/runner/lead_agent/mod.rs:157-160`); its template also lists `memory_read`. No store or forget.
- **Subagent templates** (`config/agents/*.md`): `memory_read` (which provides `memory_search`, `memory_search.rs:166`) is granted to code, explore, general, planning and review agents. It is **not** granted to research, system or writing agents. No template can write memory, because store and forget are not globally registered.
- **Skills:** only through `requires_capabilities` or `tools.allow` (`core/orchestrator/skill/invocation.rs:188-270`). None of the four shipped skills asks for memory. Automatic recall still applies on the skill tier.
- **Persona:** `update_persona` provides `persona_write` (`update_persona/mod.rs:251`). It is on the surface only during bootstrap, no template grants it, and `general_agent` denies it.
- **Dead code:** the KB importer `memory/kb.rs::ingest_directory` has no production caller.

---

## 5. What a user would notice under the proposal

### (a) Automatic extractions become `candidate`, excluded from recall until reviewed

- **Today:**
  - On a 5th qualifying turn per lane, a situational preference from *that turn* becomes a global memory at once. The model can find it on a later turn through `memory_search`, and so can a later workflow's lead.
  - Every successful workflow can add up to 5 workspace facts, such as "Project X uses PostgreSQL 15". The next workflow in that project sees them in its `RETRIEVED MEMORY` block without anyone acting.
  - Per-turn automatic recall of either rarely fires (A6).
- **Under (a):** both kinds stop reaching any agent until someone approves them. No review surface exists today: no route, no CLI verb, no GUI view. Until the Memory view ships, automatic learning is inert while still spending extraction tokens.
- **Explicit "remember X"** still works immediately, because it is a synchronous tool write.
- **Side effect:** extraction can no longer supersede or downgrade explicit memories (F7).

### (b) Background USER.md writer removed

- **Today:** saying "I'm Alex, I'm in Tokyo" (on a counted turn) fills `identity.Name` and `Timezone` with confidence ≥ 0.8. From the next turn the fact sits in the **system prompt of every main-loop and skill-tier turn on every lane**, including groups, and it survives both reset kinds.
- **Under (b):**
  - Nothing edits USER.md after onboarding except the user opening the file. `update_persona` is bootstrap-only and `file_ops` blocks the model.
  - Learned traits would go to notes, which (a) makes candidates, so "call me Alex" said in passing is not applied at all until reviewed. Once approved, the note is recalled only when search matches, not unconditionally on every turn.
  - Onboarding can finish only through the model's `update_persona(user)` call, or a hand edit. Today extraction can also complete it.
- **Gain:** removes today's contamination path, where group members' or linked other humans' self-descriptions are written into the owner's profile.

### (c) No recall in shared/group connector lanes

- **Today:** a linked Telegram user asking in a group, or any iMessage group member asking with `/ask`, gets owner-memory recall and full `memory_store`/`memory_forget`.
- **Under (c):**
  - A per-turn audience rule means group turns stop recalling. That needs new plumbing: chat type into `GatewayRequest`, then `HandleRequest`, then `ToolContext` and `ContextRequest`, plus Telegram chat-type detection.
  - A per-lane rule means every connector DM also loses recall, because DMs and groups share one lane.
- **Not fixed by (c) as written:** the USER.md block, the shared-lane transcript and summary replay, "last chat" notification pushes (including artifact files), non-owner iMessage senders acting as the owner, and memory writes or forgets from groups (the design is silent on writes).

---

## 6. Infrastructure the design could reuse

### Atomic writes

| Helper | Crate / location | What it does | Gaps for notes |
|---|---|---|---|
| `store::write_atomic` | storage, `store/mod.rs:754-771`, private | fixed `<file>.tmp`, `sync_all`, rename | no directory fsync, no lock |
| `config_io::atomic_write_toml` / `atomic_write_with_backup` | core, `config_io.rs:82,142,162-196` | `<path>.lock` via the `file-lock` crate (`:241-256`), unique `tempfile` with pid prefix, `sync_all`, rotates the old version into `state/backups/` (5 kept, `:260`), `persist` | backups would copy private note bodies into `state/backups`; no directory fsync |
| `update_persona/common.rs:38-60` | core | async, fixed tmp name, fsync then rename, backup dir plus pruning | no lock |
| Extraction's USER.md write | `extraction.rs:537-548` | fixed tmp name, rename | no fsync, no lock, no CAS (not reusable) |
| Artifact store `write_bytes` | storage, `artifacts/mod.rs:~2659-2760` | closest analog to revisions plus `.history`: `confine_to_root`, `.<stem>.tmp`, fsync, `crash_point(WriteStep)` injection hooks for tests, rotates the previous version into `.versions/`, rename, `content_io::fsync_dir` (`content_io.rs:24-34`), `sha256_hex` (`:13-17`), a documented recovery table | — |
| `UploadStore` | `uploads/mod.rs:~480-500` | the same pattern | — |
| `confine_to_root` | `store/artifact.rs:215-270` | rejects `..`, canonicalizes the nearest existing ancestor | canonicalize-then-open (the race the design names); no `O_NOFOLLOW` |

### Locks

- The daemon singleton lock `state/openalpacad.lock` (`storage/daemon_lifecycle.rs`). The factory reset claims it (`config_factory_reset.rs:125`).
- Per-file `<path>.lock` (`config_io`; the llm config write lock).
- **No workspace move lock** (A7).

### Watching and hot reload

- **The watcher:** `openalpaca_wake::FilesystemWatcher` is a `notify::PollWatcher` polling every 200 ms. It watches directories recursively and debounces 100 ms per path. It uses `try_send` and **drops events when the channel is full**. Paths are fixed at `start`; a nonexistent path is skipped with a warning, and the handle can only *unwatch* (`crates/openalpaca_wake/src/watcher/filesystem/mod.rs:15-21,48-100,134-178`; `manager/mod.rs:43-58,109`).
- **What the daemon watches:** config files, the persona docs and `config/skills/` (`d/main.rs:315-344`).
- **Dispatch:** by path in `d/hot_reload.rs:131-220`.
- **Patterns to copy:**
  - Self-write suppression: the sha256 `RecentHashes` ring, 8 deep (`hot_reload.rs:117-121,280,739-760`).
  - Directory-of-markdown reconcile: skills reload per folder (`:217,554-570`). This is the closest analog for a notes directory.
  - Lock-free config: `ArcSwap`.
  - Persona documents: `RwLock` plus a `persona_version` memo bump (`extraction.rs:550-568`).
- **No project-store watcher exists.**

### Store

- `ContentKind::Memory` → `memory` already exists (`storage/store/mod.rs:409-445`), is in `ALL` and in `unknown_entries`, and `content_dir` creates it on use.
- Home README row: "`memory/`, `skills/` reserved; not created until used — yours, never swept" (`:904`).
- Project README row: "…deliberately *not* git-ignored" (`:925`).
- `GITIGNORE_TEXT` has no memory line (`:873-880`). Both project files are written only when absent (A10).
- Workspace purge keeps memory (`d/routes/workspaces.rs:628-636`). Rebase re-keys `memory.scope_id` in the same transaction as the other three members (`storage/artifacts/mod.rs:1535-1546`).

### Dependencies

Workspace `Cargo.toml`: `uuid` 1.20 with features `v4`, `serde` (`:44`); `notify` 8.0 (`:71`); `file-lock` 2.1 (`:99`); `tempfile` 3.24 (`:103`); `sha2` (`:143`).

| Crate | `notify` | `uuid` | `tempfile` | `file-lock` | `sha2` |
|---|---|---|---|---|---|
| `openalpaca_storage` | **no** | yes (`:21`) | dev only (`:31`) | yes (`:15`) | yes (`:22`) |
| `openalpaca_core` | **no** (and must not depend on wake) | yes (`:24`) | yes (`:33`) | yes (`:32`) | yes (`:39`) |
| `openalpaca_wake` | yes (`:12`) | yes (`:17`) | dev | — | — |
| `openalpacad` | via `openalpaca_wake` | yes (`:15`) | dev (`:58`) | — | yes (`:49`) |
