# OpenAlpaca Daemon Manual (`openalpacad`)

`openalpacad` is the local control plane for OpenAlpaca, serving HTTP APIs, streaming events, orchestration runtime, connectors, and storage.

Related docs:
- [API Docs index](api/README.md) (generated from source by `python3 scripts/gen_api_docs.py`)
- [CLI Manual](CLI_Manual.md)
- [GUI Manual](GUI_Manual.md)

## Run

From repository root:

```bash
cargo run -p openalpacad
```

Release:

```bash
cargo build -p openalpacad --release
./target/release/openalpacad
```

## macOS Package Install (No Cargo on target machine)

Build package (builder machine):

```bash
./scripts/release/package-macos.sh
```

Install package (target machine):

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<version>.tar.gz
```

`install.sh` also supports `--url <https://...>` (instead of `--file`), `--prefix <dir>` (default `~/.local/openalpaca`), `--app-dir <dir>` (macOS app bundle location, default `~/Applications`), and `--yes` (non-interactive). Linux (`package-linux.sh`) and Windows (`package-windows.ps1` / `install-windows.ps1`) equivalents exist alongside the macOS scripts.

Runtime paths after installer-based startup. Everything lives under one root,
`~/.openalpaca` (`OPENALPACA_HOME_STORE` overrides it; absolute paths only, an
empty or relative value is rejected and the daemon refuses to start). The
organising rule is that `state/` is the machine's and everything else at the
root is the human's:
- config base dir: `~/.openalpaca/config`
- discovery: `~/.openalpaca/state/discovery.json`
- database: `~/.openalpaca/state/openalpaca.db` (plus its `-wal` / `-shm`)
- single-instance lock: `~/.openalpaca/state/openalpacad.lock`
- master key: `~/.openalpaca/state/.master_key`
- rotated copies of hand-edited config: `~/.openalpaca/state/backups/`
- plugins: `~/.openalpaca/plugins/` (plus `.permissions.toml`, `.config/<name>.toml`, and `.trash/` for uninstalled ones)
- content, home scope: one directory per content kind under the root, created on first use — `artifacts/`, `uploads/`, `sessions/`, `memory/`, `skills/`, `scratch/`, `cache/`
- a project's own store: `<project>/.openalpaca/` — the same content shape, so an artifact of a project lives beside the project
- daemon log (CLI-managed startup): `~/.openalpaca/state/logs/daemon.log` —
  appended across restarts and rotated by `openalpaca daemon start` when it is
  past 16 MB (`daemon.log.1` … `.3`, oldest dropped), so it costs at most four
  files. `GET /v1/status` reports the path when this file exists; a daemon
  started any other way (`cargo run`, the GUI sidecar) writes none and reports
  `null`.

## Startup and Lifecycle

1. Initialize tracing/logging.
2. Seed the `~/.openalpaca` home store, then move a legacy app directory into it
   — once, before the lock is taken, because the lock file itself moves
   (`store::ensure_store` + `store::migrate::move_app_root`; see
   [Installation Manual](Installation_Manual.md#migrating-from-the-old-data-directory)).
3. Acquire single-instance lock (`openalpacad.lock`).
4. Resolve config directory, seed missing default configs, and ensure master key.
5. Install signal handlers.
6. Bind to `127.0.0.1:0` (OS-selected port).
7. Write discovery metadata (`discovery.json`).
8. Open SQLite database and apply migrations.
9. Bootstrap persona documents (SOUL/USER/IDENTITY/BOOTSTRAP) if missing.
10. Start orchestrator, wake manager, plugin manager, MCP clients, connectors, hot reload, background workers, and HTTP router.

Shutdown can be initiated by signal handling or daemon command endpoint. A watchdog force-exits the process (exit code 1) if graceful shutdown takes longer than 10 seconds; after a forced exit, a stale `discovery.json` may be left behind.

## Config Resolution

Config base directory precedence:

1. `OPENALPACA_CONFIG_DIR` env override (if path exists)
2. Upward search from current executable for `config/llm.toml`
3. Upward search from current working directory for `config/llm.toml`
4. Fallback: `<cwd>/config`

Important runtime files:

- `config/llm.toml`
- `config/daemon.toml`
- `config/mcp.toml` (MCP server declarations)
- `config/agents/*.md` (Markdown with YAML frontmatter; legacy `.toml` agent files still load with a deprecation warning)
- `config/skills/*/SKILL.md`
- `config/tools/*.toml`
- orchestrator persona docs under `config/orchestrator/`

## Secrets and First Run

- On first startup the daemon seeds missing `llm.toml` and `daemon.toml` from templates embedded in the binary (sourced from `scripts/release/templates/config/`).
- The AES-256-GCM master key lives at `~/.openalpaca/state/.master_key` (`store::master_key_dir()`); a key left in a legacy app directory is moved there by the boot-time mover. The daemon exports it as `OPENALPACA_MASTER_KEY` for its own process; startup fails hard if the key cannot be ensured.
- Persona documents (`SOUL.md`, `USER.md`, `IDENTITY.md`, and conditionally `BOOTSTRAP.md`) are written into `<config>/orchestrator/` from templates if absent.

## Hot Reload

A file watcher reloads configuration without restart:

- `config/orchestrator/SOUL.md`, `USER.md`, `IDENTITY.md`, `BOOTSTRAP.md` (parse failures keep the last valid version)
- `config/llm.toml` and `config/daemon.toml`
- the `config/skills/` and `config/agents/` directories
- `config/mcp.toml` — that file **is** the MCP declaration and toggle store, so a hand edit is authoritative: the supervisor diffs desired against actual on presence, the `enabled` bit and a config fingerprint, and loads or unloads only what changed. An unparseable save keeps the last good set rather than tearing servers down, and the daemon's own writes are swallowed by a hash ring so a toggle does not reconcile twice.

## MCP Servers

Servers declared in `config/mcp.toml` are connected at boot; per-server failures are logged, never fatal. Each remote tool registers in the tool registry as `<server>__<tool>` and provides a capability equal to that namespaced name. Installed MCP and plugin tools are available by default to the assistant's main conversational loop (under both `tool_selection` modes) and to the lead agent orchestrating background workflows. Subagents remain template-scoped — they see only the capabilities their template declares. To expose an MCP tool to an agent, list the namespaced name in the agent template's `capabilities` frontmatter (for skills: `requires_capabilities`). MCP resources and prompts are not implemented, and serving MCP is a non-goal.

There is **no per-tool switch**: a whole server (or plugin) is turned off with `openalpaca ext disable mcp <server>` — see Extensions below. A disabled server's tools leave every surface and the gate refuses them.

## Extensions (MCP servers + plugins)

The ENABLE axis is one toggle per install unit: per MCP server (`config/mcp.toml`'s `enabled`) and per plugin (`enabled` in the plugins root's `.permissions.toml`). Builtins are never toggled, and there is no per-tool toggle or deny list. Disabled means unloaded: the plugin child is killed, the MCP connection dropped, and no reconnect is attempted. A tool whose extension is disabled is refused at the gate, with a warning in the log and — where a surface asked for it — in the run.

Verbs (`openalpaca ext …`, and `POST /v1/extensions/{kind}/{id}/{verb}`):

| Verb | Effect |
|---|---|
| `enable` | Writes the bit, then loads. A plugin that has not been approved records the bit and stays `Unapproved` — consent pre-empts the switch. |
| `disable` | Writes the bit, drains in-flight calls (`[extensions] drain_timeout_secs`, default 10 s), then unloads. |
| `reload` | Re-applies an edited declaration or a rotated credential: unload, then load from what is on disk. |
| `approve` / `deny` | Plugins only — records consent, or refuses and unloads. `approve` alone does not turn a plugin on. |
| `remove` (`DELETE`) | Drops the permissions entry of an orphaned plugin (directory gone). Untouched by the flag below: the bare `DELETE` never touches a directory. |

Install, update and uninstall are their own routes rather than verbs:

| Route | Effect |
|---|---|
| `POST /v1/extensions/plugin` | Copies the directory at `{"source":"path","path":"/abs/dir"}` into `~/.openalpaca/plugins/`. It grants nothing — the row lands unapproved and `approve` is the one action that starts it. `source: "url"` is declined by name. |
| `POST /v1/extensions/mcp` | Writes a `[servers.<name>]` block into `config/mcp.toml`. A declaration handing the daemon a literal secret is refused `422` (`secret_literal_refused`) — use the `env_from` / `bearer_env` / `extra_headers_from` indirections. |
| `POST /v1/extensions/plugin/validate` | Reads a candidate directory's `plugin.toml` and answers what it would install. Copies nothing. |
| `PUT /v1/extensions/plugin/{id}` | Replaces an installed plugin's tree with the one at a given path, through the same drain-and-unload a disable uses. |
| `DELETE /v1/extensions/{kind}/{id}?uninstall=true` | The real removal. A plugin's directory is **moved to `~/.openalpaca/plugins/.trash/`**, never deleted, and the response says where; `&keep_data=false` sends its `.data/<name>/` to the trash too. An MCP server must be `Disabled` first, and its `[servers.<name>]` block is what goes. Nothing auto-purges `.trash/`. |

`openalpaca ext list` / `info` render the same rows the GUI's Extensions view uses; `openalpaca ext list --include-orphaned` adds plugins whose directory has disappeared. `openalpaca ext install|update|uninstall` and `openalpaca ext mcp add|remove` cover the routes above. `openalpaca plugin …` remains as the plugin-shaped shortcut over the same routes, including `plugin config get|set` (values stored as secret references read back `<redacted>`). `GET /v1/tools` lists the tool catalog read-only — each row carries its `origin` (`kind`, `id`, `enabled`, `state`) for extension tools and `null` for builtins; there is no per-tool write.

## Discovery and Auth Model

Daemon writes discovery object including:

- instance id
- process id
- listen host/port
- auth token with expiry
- build metadata

Auth behavior:

- Public: `/`, `/v1/health`
- Bearer token: most `/v1/*` endpoints
- Query token: `/v1/events`, `/v1/chat/stream/{stream_id}`
- Either, checked inside the handler: the three content routes
  (`/v1/files/{id}/content`, `/v1/artifacts/{id}/content`,
  `/v1/artifacts/{id}/versions/{n}/content`) accept `?token=` **or** the bearer
  header, because a webview `<img src>` cannot send one. Only authentication
  moved off the middleware — the owner check is unchanged, and the metadata and
  `/open` routes stay header-only.

## API Route Groups

Route table source of truth: `apps/openalpacad/src/router.rs` (see also the [API docs index](api/README.md)).

Major groups:

- Core: health, `/v1/command`, `/v1/events/history`, `GET /v1/me` (user id and default lane), `GET /v1/status` (uptime, schema version, store roots and sizes, `log_path` when the CLI manages the log)
- Tasks: list/create/status/action, plus `GET /v1/tasks/{id}/timeline` — one lane per spawned subagent (label, template, state, start/end), which is where a run's agents are reported; the legacy `assigned_agents` (list) / `assignments` (detail) arrays were deleted, and a list row keeps only their count as `subagent_count`. `POST /v1/tasks/{id}/steer` pushes into the run's steering inbox (owner-scoped, `404` on a run that is not yours) and `POST /v1/tasks/{id}/rerun` answers `201` with a **new** id copied from a finished run's goal
- Lane follow-ups: `GET|POST /v1/lanes/{lane_key}/followups`, `DELETE /v1/lanes/{lane_key}/followups/{id}` (a cancel that lost the race to autostart answers `409`)
- Agents: CRUD/action/config plus template CRUD (`/v1/agent-templates`) and a read-only instance list (`GET /v1/agent-instances`)
- Chat: send/history/stream, message feedback (`PUT|GET|DELETE /v1/chat/messages/{message_id}/feedback`), tool confirmations (`POST /v1/chat/confirmations/{request_id}`)
- Sessions: `GET|POST /v1/sessions`, `GET|PATCH|DELETE /v1/sessions/{id}`, `GET /v1/sessions/{id}/messages`, `GET /v1/sessions/{id}/events`, `POST /v1/sessions/{id}/activate|archive`. A lane holds many sessions with exactly one `active`; the old `/v1/conversations` family was deleted
- Files: `POST /v1/files/upload` (body limit 100 MiB), `GET /v1/files/{id}`, `GET /v1/files/{id}/content`, `POST /v1/files/{id}/open`
- Artifacts: `GET /v1/artifacts` (filters and paging), `GET /v1/artifacts/{id}`, `…/versions`, `…/diff?from=&to=`, `PUT …/pin`, and the content routes `…/content` and `…/versions/{n}/content`
- Workspaces: `GET|PATCH /v1/workspaces` (describe a project root; re-base everything addressed under it) and `POST /v1/workspaces/purge` (`dry_run` defaults to true)
- Connectors + auth link token (`POST /v1/auth/link`)
- LLM settings/models/usage, key management (delete/reorder/priority/validate/status), credential discovery (`GET /v1/settings/llm/credentials`, `POST /v1/settings/llm/credentials/rescan`), CLI backends (`GET /v1/settings/llm/cli-backends`), the provider ENABLE bit (`PUT /v1/settings/llm/providers/{provider}/enabled`), and `GET /v1/usage/summary` (today's spend, its per-provider breakdown, and the two caps that bound it)
- Orchestrator: metrics (latency and decisions) and config (`GET|PUT /v1/orchestrator/config`)
- Daemon provider config endpoints (`GET /v1/daemon/config/providers`, `PUT /v1/daemon/config/providers/web-search`)
- Skills: `GET /v1/skills` (read-only catalog), `GET /v1/skills/health`
- Extensions: `GET /v1/extensions`; `POST /v1/extensions/{kind}/{id}/{verb}` (`enable|disable|reload|approve|deny`); `GET|POST /v1/extensions/{kind}/{id}/config` (plugins only); `POST /v1/extensions/{kind}` (install/declare), `POST /v1/extensions/plugin/validate`, `PUT /v1/extensions/plugin/{id}` (replace a tree), `DELETE /v1/extensions/{kind}/{id}` (orphan row; `?uninstall=true` for the real removal). Plugins are loaded from `~/.openalpaca/plugins`; the plugin system is early-stage. The former `/v1/plugins*` routes were removed.
- Tools: `GET /v1/tools` (read-only catalog; no per-tool toggle)

## Message Routing (Orchestrator)

Every chat message (any source: GUI, CLI, connectors) is routed by the orchestrator in tiers:

1. **Deterministic commands** (no LLM call):
   - `/status` / `/tasks` — task summary; `/status <task_id>` for one task.
   - `/cancel`, `/pause`, `/resume` — task control. Bare forms (no id) resolve against the lane's active workflows; an explicit id (`/cancel <id>`) targets that task.
   - `/steer <text>` — inject a steering message into the lane's running workflow (guaranteed delivery, bypasses the model; requires `orchestrator.routing.steering_enabled`, default on).
   - `/<skill> [args]` — invoke a skill by slash command, alias, or skill ID (directory name); skills can also be selected by the weighted skill router.
2. **Social fast path** — trivial acknowledgements ("ok", "thanks") answered with an ultra-light prompt.
3. **Main loop** — everything else, including messages sent while workflows run. The model answers directly or calls routing tools: `start_workflow` (background workflow), `steer_workflow` / `queue_followup` (offered while the lane has active workflows), `task_status`, and memory tools. Workflows run in the background under a lead agent that can spawn subagents; concurrency is capped per lane (`max_workflows_per_lane`, default 3). On completion the workflow posts a model-authored completion report to the lane, and queued follow-ups auto-start (`followup_autostart`, default on).

Tunables live under `[orchestrator.routing]` in `config/daemon.toml` (steering, per-lane workflow cap, follow-up autostart, main-loop round/tool budgets, tool-surface selection, scheduled-skills kill switch).

### Scheduled skills

Skills whose frontmatter sets `invoke.cron` (see the Skill Template Reference) are registered with the wake-module cron scheduler at boot and re-synced on skill hot-reload. Each fire injects the skill's slash command as a fresh turn — through the same gateway as user messages — on the local user's `<user>:scheduled` lane, running as that user so memory and preferences apply and results appear in chat history. `scheduled_skills_enabled = false` under `[orchestrator.routing]` disables all skill cron jobs (hot-reloadable).

## Streaming Surfaces

### WebSocket Events

- Endpoint: `GET /v1/events?token=...`
- Payload: `openalpaca_api::events::ServerEvent`
- Includes operational, task, agent, security, and orchestration events.

### SSE Chat Stream

- Create stream: `POST /v1/chat`
- Consume stream: `GET /v1/chat/stream/{stream_id}?token=...`
- SSE event types: `thinking`, `delta`, `done`, `error`, `confirmation_requested`
- When the reply started a background workflow, the `done` event carries an optional `delegation` object (`{"task_id": ..., "title": ...}`) so clients can track the created task without parsing prose.

When a tool run requires approval, the stream emits `confirmation_requested`; the client resolves it via `POST /v1/chat/confirmations/{request_id}`.

Confirmation prompts also reach connector channels: when the originating lane belongs to Telegram, iMessage, or Discord, the connector sends the prompt into the conversation and the user replies `/yes` (or `/y`) to approve, `/no` (or `/n`) to deny. Multiple pending confirmations in one conversation are answered in FIFO order. If no interface answers within the timeout (default 300s, `execution.agent_defaults.confirmation_timeout_secs` in daemon.toml), the tool is denied (fail-closed).

## Event Taxonomy

The wire names below are the `type` tag of `ServerEvent`
(`crates/openalpaca_api/src/events/mod.rs`, `rename_all = "snake_case"`); the
list is the whole union. Every frame also carries `ts` and `instance_id`.

- `heartbeat`, `command_received`, `wake`
- `task_status`, `agent_status`, `agent_config_changed`
- `workflow_started`, `workflow_progress`, `workflow_steered`
- `subagent_span` — one subagent lane of a run opening or closing: the
  `subagent_span` row itself, so the socket and `GET /v1/tasks/{id}/timeline`
  cannot disagree. `state` is never `blocked` (a blocked lane is derived at read
  time from the confirmation broker)
- `followup_queued`, `followup_cancelled`
- `session_changed` (a session's lifecycle transition, or the `interrupted`
  frame, which names the run it belongs to)
- `artifact_written` (`artifact_id`, `task_id`, `agent_id`, `name`, `kind`,
  `version`, `path`)
- `connector_status`, `key_status_changed`
- `chat_stream_started`, `chat_stream_ended`
- `orchestrator_config_changed`, `daemon_config_changed`
- `security_violation`, `circuit_breaker_tripped`, `tool_executed`,
  `tool_confirmation_requested`
- `llm_call_completed`, `skill_catalog_updated`, `soul_updated`
- `skill_invocation_started`, `skill_completed`, `skill_failed`
- `extension_state_changed` (an MCP server or plugin changed state), `extension_capability_withheld` (a surface asked for a tool a disabled extension owns), `extension_capability_withdrawn` (a disable/crash took tools away, with the affected templates, skills and cron skills)

`dag_node_status` was **deleted**: `subagent_span` describes the same spawns,
and emitting both meant two frames for one transition. Rows an older daemon
already wrote keep their `event_type` and still list from
`GET /v1/events/history`, so a stored log stays readable — nothing new carries
that name.

Persisted rows use the same words with two exceptions: `agent_status` is
logged as `agent_status_change`, and `heartbeat` is not persisted at all
(`apps/openalpacad/src/events/persistence.rs`).

## Background Tasks

The daemon runs periodic workers, all cancelled together on shutdown (intervals are read from `daemon.toml` and hot-reloadable unless noted):

- heartbeat event emitter
- embedding indexer (only when an embedder is configured)
- memory importance decay
- file-processing worker and asset cleanup (upload governance)
- telemetry cleanup (fixed daily interval)
- chat-stream cleanup (stale SSE streams)

## Storage Model

- SQLite location is resolved by `openalpaca_storage::store::database_path()` — `~/.openalpaca/state/openalpaca.db`. Every path in the layout comes from that module; no crate joins a literal directory name onto a store root.
- Migrations are embedded and applied from `openalpaca_storage::migrations::MIGRATIONS`. The authoritative list is `crates/openalpaca_storage/src/migrations/` (currently `001` through `040`); `GET /v1/status` reports the version the open database is actually at.

## Logging and Operations

Control logging with `RUST_LOG`, for example:

```bash
RUST_LOG=info cargo run -p openalpacad
RUST_LOG=openalpacad=debug cargo run -p openalpacad
```

Graceful shutdown endpoint:

```http
POST /v1/command
{"command":"shutdown"}
```

## Troubleshooting

- Daemon already running: check lock/discovery and stop existing instance cleanly.
- Discovery expired: restart daemon to rotate token and rewrite discovery.
- DB lock/contention: ensure single daemon instance and avoid conflicting external writers.
- Config not loading: verify resolved config directory and presence of expected files.
- GUI/CLI auth failures: ensure they read the current discovery token and instance.
