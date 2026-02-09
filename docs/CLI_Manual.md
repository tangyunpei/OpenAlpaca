# OpenAlpaca CLI Manual

`openalpaca` is the command-line interface for OpenAlpaca. It talks to the local daemon (`openalpacad`) over HTTP using the token published in `discovery.json`.

Related docs:
- Daemon manual: `Daemon_Manual.md`
- Daemon HTTP API: `api/apps/openalpacad.md`
- Database schema: `api/database/schema.md`

## Installation

Development (from repo root):

```bash
cargo run -p openalpaca -- <subcommand> [args]
```

Release binary:

```bash
cargo build -p openalpaca --release
./target/release/openalpaca --help
```

## How The CLI Connects

- The daemon binds to a random local port and writes `discovery.json` in the OpenAlpaca app data directory.
- The CLI reads that file and sends requests with `Authorization: Bearer <token>`.
- If the daemon is not running, most commands will fail (except parts of `config` that operate on local files).

## Commands

### `daemon` (start/stop/status/tail)

- `openalpaca daemon status`: reads discovery + calls `GET /v1/health`.
- `openalpaca daemon tail [--count N]`: connects to WebSocket `GET /v1/events?token=...` and prints events.
- `openalpaca daemon start [--daemon-only]`: spawns daemon (dev convenience, logs to `daemon.log`).
- `openalpaca daemon stop`: sends SIGTERM to daemon PID from discovery.
- `openalpaca daemon restart`: stop then start.

Notes:
`openalpaca daemon tail` is a live view of the daemon’s server events. It does not query history; for history use `GET /v1/events/history` (see `api/apps/openalpacad.md`).

### `config` (system + AI config)

Config is split across two backends:
- SQLite system config (daemon/CLI shared DB).
- `config/llm.toml` (LLM router config, stored on disk in the repo working directory, matching daemon behavior).

Interactive mode:

```bash
openalpaca config
```

Non-interactive:

```bash
openalpaca config list [--all] [--format table|json]
openalpaca config get <key>
openalpaca config set <key> <value>
openalpaca config reset [<key>] [--factory]
```

`--factory` wipes the database content (agents, tasks, identities, memories, logs, usage).

### `connector` (platform connectors)

```bash
openalpaca connector list
openalpaca connector enable <name>
openalpaca connector disable <name>
openalpaca connector delete <name>
```

`delete` clears connector config in the DB and stops the connector. It can also sever identity/linking data for that platform (depending on connector implementation and how unlinking is handled).

### `tasks` (task lifecycle)

```bash
openalpaca tasks list [--status <status>] [--limit <n>] [--format table|json]
openalpaca tasks status <task_id> [--format table|json]
openalpaca tasks log <task_id> [--limit <n>]
openalpaca tasks create [description] [--priority <n>]
openalpaca tasks cancel <task_id>
openalpaca tasks pause <task_id>
openalpaca tasks resume <task_id>
```

Supported status filters include `queued`, `running`, `paused`, `completed`, `failed`, `cancelled`, and the special view `active`.

### `agents` (sub-agents)

```bash
openalpaca agents list [--status <status>] [--format table|json]
openalpaca agents status <agent_id> [--format table|json]
openalpaca agents config <agent_id> [--format table|json]
openalpaca agents pause <agent_id>
openalpaca agents resume <agent_id>
openalpaca agents set <agent_id> <dotted.key.path> <value>
openalpaca agents create [--from-file <path>] [--interactive] [--from-chat <desc>]
openalpaca agents remove <agent_id>
```

If you run `openalpaca agents` with no subcommand, it defaults to interactive creation.

### `llm` (keys, models, usage)

```bash
openalpaca llm status [--format table|json]
openalpaca llm keys list [--format table|json]
openalpaca llm keys add [--provider <name>] [--secret <key>] [--priority primary|fallback] [--source <src>] [--notes <text>]
openalpaca llm keys remove <provider> <key_id>
openalpaca llm keys validate --provider <name> --secret <key>
openalpaca llm keys set-primary <provider> <key_id>
openalpaca llm keys reorder <key_id>...
openalpaca llm models [--format table|json]
openalpaca llm usage [--agent <id>] [--date YYYY-MM-DD] [--key <key_id>] [--daily] [--format table|json]
openalpaca llm credentials [--format table|json]
openalpaca llm backends [--format table|json]
openalpaca llm provider-usage [--format table|json]
```

### `chat` (interactive or one-shot)

- Interactive REPL (when stdin is a TTY):

```bash
openalpaca chat
```

- One-shot:

```bash
openalpaca chat --message "hello"
```

- Pipe mode:

```bash
echo "hello" | openalpaca chat
```

## Troubleshooting

- “Daemon is not running (no discovery file)”: start the daemon (GUI, `openalpaca daemon start`, or `cargo run -p openalpacad`).
- “Discovery expired”: restart the daemon to regenerate its token.
- WebSocket can’t connect: ensure you are using the query token form (`/v1/events?token=...`) and that the daemon is bound to `127.0.0.1`.

## GUI

If you’re using the desktop app, see `GUI_Manual.md`.
