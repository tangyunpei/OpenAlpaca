# OpenAlpaca Daemon Manual (`openalpacad`)

`openalpacad` is the local control plane for OpenAlpaca, serving HTTP APIs, streaming events, orchestration runtime, connectors, and storage.

Related docs:
- [Daemon API Reference](api/apps/openalpacad.md)
- [CLI Manual](CLI_Manual.md)
- [GUI Manual](GUI_Manual.md)
- [Database Schema](api/database/schema.md)
- [Database Migrations](api/database/migrations.md)

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

## Startup and Lifecycle

1. Initialize tracing/logging.
2. Migrate legacy app directory naming (if needed).
3. Acquire single-instance lock (`openalpacad.lock`).
4. Resolve config directory and ensure master key.
5. Install signal handlers.
6. Bind to `127.0.0.1:0` (OS-selected port).
7. Write discovery metadata (`discovery.json`).
8. Open SQLite database and apply migrations.
9. Start orchestrator, wake manager, connectors, hot reload, and HTTP router.

Shutdown can be initiated by signal handling or daemon command endpoint.

## Config Resolution

Config base directory precedence:

1. `OPENALPACA_CONFIG_DIR` env override (if path exists)
2. Upward search from current executable for `config/llm.toml`
3. Upward search from current working directory for `config/llm.toml`
4. Fallback: `<cwd>/config`

Important runtime files:

- `config/llm.toml`
- `config/daemon.toml`
- `config/agents/*.toml`
- `config/tools/*.toml`
- orchestrator profile docs under `config/orchestrator/`

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

See complete matrix in [Daemon API Reference](api/apps/openalpacad.md).

Major groups:

- Core: health, command, events history
- Tasks: list/create/status/action, DAG view
- Agents: CRUD/action/config plus templates and runtime instances
- Chat: send/history/conversations/stream
- Connectors + auth link token
- LLM/settings/models/pricing/usage
- Preferences
- Memory + KB ingest + index/reindex status
- Orchestrator metrics (latency and decisions)
- Daemon provider config endpoints

## Streaming Surfaces

### WebSocket Events

- Endpoint: `GET /v1/events?token=...`
- Payload: `openalpaca_api::events::ServerEvent`
- Includes operational, task, agent, security, and orchestration events.

### SSE Chat Stream

- Create stream: `POST /v1/chat`
- Consume stream: `GET /v1/chat/stream/{stream_id}?token=...`
- SSE event types: `thinking`, `delta`, `done`, `error`

## Event Taxonomy (High Level)

Representative server event types include:

- `heartbeat`, `log`, `command_received`, `wake`
- `task_status`, `agent_status`, `agent_config_changed`
- `connector_status`, `key_status_changed`
- `chat_stream_started`, `chat_stream_ended`
- `orchestrator_config_changed`, `daemon_config_changed`
- `dag_node_status`, `task_replanned`
- `security_violation`, `circuit_breaker_tripped`, `tool_executed`
- `llm_call_completed`, `skill_catalog_updated`, `soul_updated`

## Storage Model

- SQLite location is resolved by `openalpaca_storage::paths::database_path()`.
- Migrations are embedded and applied from `openalpaca_storage::migrations::MIGRATIONS`.
- Current migration chain includes `001` through `026`.

Use these references for details:

- [Schema](api/database/schema.md)
- [Migrations](api/database/migrations.md)

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
