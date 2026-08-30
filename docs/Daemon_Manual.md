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

Runtime paths after installer-based startup:
- config base dir: `~/Library/Application Support/OpenAlpaca/config`
- discovery: `~/Library/Application Support/OpenAlpaca/discovery.json`
- database: `~/Library/Application Support/OpenAlpaca/openalpaca.db`
- daemon log (CLI-managed startup): `~/Library/Application Support/OpenAlpaca/daemon.log`

## Startup and Lifecycle

1. Initialize tracing/logging.
2. Migrate legacy app directory naming (if needed).
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
- The AES-256-GCM master key lives at `<app_dir>/.master_key` (a legacy `<config>/.master_key` is migrated automatically). The daemon exports it as `OPENALPACA_MASTER_KEY` for its own process; startup fails hard if the key cannot be ensured.
- Persona documents (`SOUL.md`, `USER.md`, `IDENTITY.md`, and conditionally `BOOTSTRAP.md`) are written into `<config>/orchestrator/` from templates if absent.

## Hot Reload

A file watcher reloads configuration without restart:

- `config/orchestrator/SOUL.md`, `USER.md`, `IDENTITY.md`, `BOOTSTRAP.md` (parse failures keep the last valid version)
- `config/llm.toml` and `config/daemon.toml`
- the `config/skills/` and `config/agents/` directories

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

## API Route Groups

Route table source of truth: `apps/openalpacad/src/router.rs` (see also the [API docs index](api/README.md)).

Major groups:

- Core: health, `/v1/command`, `/v1/events/history`
- Tasks: list/create/status/action
- Agents: CRUD/action/config plus templates (`/v1/agent-templates`) and runtime instances (`/v1/agent-instances`)
- Chat: send/history/conversations/stream, message feedback (`PUT|GET|DELETE /v1/chat/messages/{message_id}/feedback`), tool confirmations (`POST /v1/chat/confirmations/{request_id}`)
- Files: `POST /v1/files/upload` (body limit 100 MiB), `GET /v1/files/{id}`, `GET /v1/files/{id}/content`, `POST /v1/files/{id}/open`
- Connectors + auth link token (`POST /v1/auth/link`)
- LLM settings/models/pricing/usage, key management (delete/reorder/priority/validate/status), credential discovery (`GET /v1/settings/llm/credentials`, `POST /v1/settings/llm/credentials/rescan`), CLI backends (`GET /v1/settings/llm/cli-backends`)
- Preferences
- Memory + KB ingest + index/reindex status
- Orchestrator: metrics (latency and decisions) and config (`GET|PUT /v1/orchestrator/config`)
- Daemon provider config endpoints (`GET /v1/daemon/config/providers`, `PUT /v1/daemon/config/providers/web-search`)
- Skills: `GET /v1/skills/health`
- Plugins: `GET /v1/plugins`; `POST /v1/plugins/{name}/approve|deny|enable|disable|config` (plugins are loaded from `<app_dir>/plugins`; the plugin system is early-stage)

## Message Routing (Orchestrator)

Every chat message (any source: GUI, CLI, connectors) is routed by the orchestrator in tiers:

1. **Deterministic commands** (no LLM call):
   - `/status` / `/tasks` — task summary; `/status <task_id>` for one task.
   - `/cancel`, `/pause`, `/resume` — task control. Bare forms (no id) resolve against the lane's active workflows; an explicit id (`/cancel <id>`) targets that task.
   - `/steer <text>` — inject a steering message into the lane's running workflow (guaranteed delivery, bypasses the model; requires `orchestrator.routing.steering_enabled`, default on).
   - `/<skill> [args]` — invoke a skill by slash command; skills can also be selected by the weighted skill router.
2. **Social fast path** — trivial acknowledgements ("ok", "thanks") answered with an ultra-light prompt.
3. **Main loop** — everything else, including messages sent while workflows run. The model answers directly or calls routing tools: `start_workflow` (background workflow), `steer_workflow` / `queue_followup` (offered while the lane has active workflows), `task_status`, and memory tools. Workflows run in the background under a lead agent that can spawn subagents; concurrency is capped per lane (`max_workflows_per_lane`, default 3). On completion the workflow posts a model-authored completion report to the lane, and queued follow-ups auto-start (`followup_autostart`, default on).

Tunables live under `[orchestrator.routing]` in `config/daemon.toml` (steering, per-lane workflow cap, follow-up autostart, main-loop round/tool budgets, tool-surface selection).

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

## Event Taxonomy (High Level)

Representative server event types include:

- `heartbeat`, `log`, `command_received`, `wake`
- `task_status`, `agent_status`, `agent_config_changed`
- `connector_status`, `key_status_changed`
- `chat_stream_started`, `chat_stream_ended`
- `orchestrator_config_changed`, `daemon_config_changed`
- `dag_node_status` (subagent start/finish within lead-agent workflows)
- `security_violation`, `circuit_breaker_tripped`, `tool_executed`
- `llm_call_completed`, `skill_catalog_updated`, `soul_updated`
- `skill_invocation_started`, `skill_completed`, `skill_failed`
- `plugin_loaded`, `plugin_crashed`, `plugin_pending_approval`

## Background Tasks

The daemon runs periodic workers, all cancelled together on shutdown (intervals are read from `daemon.toml` and hot-reloadable unless noted):

- heartbeat event emitter
- embedding indexer (only when an embedder is configured)
- memory importance decay
- file-processing worker and asset cleanup (upload governance)
- telemetry cleanup (fixed daily interval)
- chat-stream cleanup (stale SSE streams)

## Storage Model

- SQLite location is resolved by `openalpaca_storage::paths::database_path()`.
- Migrations are embedded and applied from `openalpaca_storage::migrations::MIGRATIONS`. The authoritative list is `crates/openalpaca_storage/src/migrations/` (currently `001` through `033`).

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
