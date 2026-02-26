# OpenAlpaca CLI Manual

`openalpaca` is the command-line interface for controlling a local `openalpacad` instance.

Related docs:
- [Daemon Manual](Daemon_Manual.md)
- [GUI Manual](GUI_Manual.md)
- [Daemon HTTP API](api/apps/openalpacad.md)
- [Database Schema](api/database/schema.md)

## Installation and Run

From repository root (development):

```bash
cargo run -p openalpaca -- <subcommand> [args]
```

Build release binary:

```bash
cargo build -p openalpaca --release
./target/release/openalpaca --help
```

### macOS Package Install (No Cargo on target machine)

Build a distributable package on a macOS build machine:

```bash
./scripts/release/package-macos.sh
```

Install on a target macOS machine from local artifact or URL:

```bash
# local file
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<version>.tar.gz

# remote URL
./scripts/release/install.sh --url https://example.com/openalpaca-macos-<target>-v<version>.tar.gz
```

Defaults:
- binaries: `~/.local/openalpaca`
- GUI app: `~/Applications/openalpaca-gui.app`
- PATH link: `~/.local/bin/openalpaca`
- runtime config/data: `~/Library/Application Support/OpenAlpaca/`

## Connection and Auth Model

- The daemon writes discovery metadata to `discovery.json` under the OpenAlpaca app data directory.
- CLI reads base URL and token from discovery.
- Protected endpoints use `Authorization: Bearer <token>`.
- Streaming endpoints may use query-token auth (handled by CLI internals).

If discovery is missing or expired, daemon-backed commands fail until daemon is started/restarted.

## Quick Start

```bash
# Start daemon (and GUI by default)
openalpaca daemon start

# Check daemon health
openalpaca daemon status

# List active tasks
openalpaca tasks list --status active

# Open interactive chat
openalpaca chat
```

## Top-Level Commands

### `daemon`

Manage daemon process lifecycle.

```bash
openalpaca daemon status
openalpaca daemon tail [--count N]
openalpaca daemon start [--daemon-only]
openalpaca daemon stop
openalpaca daemon restart
```

Notes:
- `start` launches daemon and then GUI unless `--daemon-only` is set.
- `restart` restarts daemon only.
- `tail` streams live daemon events (not historical query output).
- Optional daemon binary override: `OPENALPACA_DAEMON_BIN=/abs/path/openalpacad`.
- Daemon startup sets `OPENALPACA_CONFIG_DIR` to `~/Library/Application Support/OpenAlpaca/config`.

### `config`

Manage system and runtime configuration.

```bash
openalpaca config
openalpaca config set <key> <value>
openalpaca config get <key>
openalpaca config list [--all] [--format table|json] [--verbose]
openalpaca config reset [<key>] [--factory]
```

Backends:
- DB-backed settings (`system_config`, preferences, etc.)
- `config/llm.toml`
- `config/daemon.toml`

`--factory` performs full storage reset.

### `gui`

Manage GUI process.

```bash
openalpaca gui start
openalpaca gui stop
```

Optional GUI app override:
- `OPENALPACA_GUI_APP=/abs/path/openalpaca-gui.app`

### `connector`

Manage platform connectors.

```bash
openalpaca connector list
openalpaca connector enable <name>
openalpaca connector disable <name>
openalpaca connector delete <name>
```

### `tasks`

Task lifecycle commands.

```bash
openalpaca tasks list [--status <status>] [--limit <n>] [--format table|json]
openalpaca tasks status <task_id> [--format table|json]
openalpaca tasks log <task_id> [--limit <n>]
openalpaca tasks create [description] [--priority <n>]
openalpaca tasks cancel <task_id>
openalpaca tasks pause <task_id>
openalpaca tasks resume <task_id>
```

### `agents`

Sub-agent and template-backed runtime control.

```bash
openalpaca agents list [--status <status>] [--format table|json]
openalpaca agents status <agent_id> [--format table|json]
openalpaca agents config <agent_id> [--format table|json]
openalpaca agents pause <agent_id>
openalpaca agents resume <agent_id>
openalpaca agents set <agent_id> <dotted.path> <value>
openalpaca agents create [--from-file <path>] [--interactive] [--from-chat <desc>]
openalpaca agents remove <agent_id>
```

`openalpaca agents` with no subcommand enters interactive creation mode.

### `llm`

LLM keys, usage, model metadata, and routing control.

```bash
openalpaca llm status [--format table|json]
openalpaca llm keys list [--format table|json]
openalpaca llm keys add [--provider <name>] [--secret <key>] [--priority primary|fallback] [--source <src>] [--notes <text>]
openalpaca llm keys remove <provider> <key_id>
openalpaca llm keys validate --provider <name> --secret <key>
openalpaca llm keys set-primary <provider> <key_id>
openalpaca llm keys reorder <key_id>...
openalpaca llm usage [--agent <id>] [--date YYYY-MM-DD] [--key <key_id>] [--daily] [--format table|json]
openalpaca llm models [--format table|json]
openalpaca llm strategy --provider <name> <strategy>
openalpaca llm credentials [--format table|json]
openalpaca llm backends [--format table|json]
openalpaca llm provider-usage [--format table|json]
```

### `chat`

Interactive or one-shot chat through daemon orchestrator.

```bash
openalpaca chat
openalpaca chat --message "hello"
echo "hello" | openalpaca chat
```

## Troubleshooting

- Discovery missing/expired: start or restart daemon.
- Auth errors: ensure CLI and daemon use the same current discovery file.
- `daemon status` unhealthy: inspect daemon logs and `RUST_LOG` settings.
- Chat/stream failures: verify daemon is reachable on `127.0.0.1` and token is valid.
