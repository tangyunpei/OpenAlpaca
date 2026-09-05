# OpenAlpaca CLI Manual

`openalpaca` is the command-line interface for controlling a local `openalpacad` instance.

Related docs:
- [Daemon Manual](Daemon_Manual.md)
- [GUI Manual](GUI_Manual.md)
- [API Docs](api/README.md)

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
- runtime config/data: `~/.openalpaca/` (`OPENALPACA_HOME_STORE` overrides — absolute paths only)

Linux and Windows packaging/install scripts also exist under `scripts/release/` (`package-linux.sh`, `package-windows.ps1`, `install-windows.ps1`, `uninstall.sh`, `uninstall-windows.ps1`).

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
openalpaca daemon tail [-c|--count N]
openalpaca daemon start [--daemon-only]
openalpaca daemon stop
openalpaca daemon restart
```

Notes:
- `start` launches daemon and then GUI unless `--daemon-only` is set.
- `stop` stops both daemon and GUI.
- `restart` restarts daemon only.
- `tail` streams live daemon events (not historical query output); `--count` limits the number of events shown, default `0` = unlimited (Ctrl+C to stop).
- Optional daemon binary override: `OPENALPACA_DAEMON_BIN=/abs/path/openalpacad`.
- Daemon startup sets `OPENALPACA_CONFIG_DIR` to `~/.openalpaca/config`.

### `config`

Manage system and runtime configuration.

```bash
openalpaca config
openalpaca config set <key> <value>
openalpaca config get <key>
openalpaca config list [--all] [--format table|json] [-v|--verbose]
openalpaca config reset [<key>] [--factory]
```

Notes:
- Bare `openalpaca config` (no subcommand) opens an interactive configuration TUI.
- `config` operates directly on the local database and TOML files — no running daemon required (the TUI's agent-management screen is the exception; it talks to the daemon).
- `--all` includes unset keys with their defaults; `-v/--verbose` adds a source column (db / llm.toml / daemon.toml).
- `set` validates keys against the config schema; unknown keys get "did you mean" suggestions.

Backends:
- DB-backed settings (`system_config` table)
- `config/llm.toml`
- `config/daemon.toml`

`reset` without `--factory` resets configuration only (agents preserved); `--factory` performs a full storage reset (wipes agents, memories, everything) after confirmation.

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

Notes:
- `--status` accepts `queued`, `running`, `completed`, `failed`, `cancelled`, `paused`, `active`.
- `--limit` defaults to 50 (for both `list` and `log`).
- `create` prompts for a title if the description argument is omitted; `--priority` defaults to 0.

### `agents`

Sub-agent and template-backed runtime control.

```bash
openalpaca agents list [--status <status>] [--format table|json]
openalpaca agents status <agent_id> [--format table|json]
openalpaca agents config <agent_id> [--format table|json]
openalpaca agents pause <agent_id>
openalpaca agents resume <agent_id>
openalpaca agents set <agent_id> <dotted.path> <value>
openalpaca agents create [--from-file <path>] [--interactive]
openalpaca agents remove <agent_id>
```

Notes:
- `openalpaca agents` with no subcommand enters interactive creation mode.

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
openalpaca llm usage [--agent <id>] [--key <key_id>] [--daily [--date YYYY-MM-DD]] [--format table|json]
openalpaca llm models [--format table|json]
openalpaca llm strategy --provider <name> <strategy>
openalpaca llm credentials [--format table|json]
openalpaca llm backends [--format table|json]
openalpaca llm provider-usage [--format table|json]
```

### `plugin`

Manage daemon plugins (approval, lifecycle, configuration).

```bash
openalpaca plugin list [--format table|json]
openalpaca plugin approve <name>
openalpaca plugin deny <name>
openalpaca plugin enable <name>
openalpaca plugin disable <name>
openalpaca plugin info <name>
openalpaca plugin config <name> set <key> <value>
openalpaca plugin config <name> get [<key>]
```

Notes:
- `approve` allows a plugin to load; `deny` prevents loading; `enable`/`disable` toggle an approved plugin.
- `info` shows name, version, status, and tools for one plugin.
- `config set` writes a key through the daemon (values are parsed as number/bool/string).
- `config get` does not fetch anything: it only prints a pointer to the plugin's config file (`~/.openalpaca/plugins/.config/<name>.toml`).

### `chat`

Interactive or one-shot chat through daemon orchestrator.

```bash
openalpaca chat
openalpaca chat --message "hello"
openalpaca chat --message "summarize these" --file a.txt --file b.png
echo "hello" | openalpaca chat
```

Notes:
- `--file <PATH>` is repeatable and uploads the files as message attachments; it requires `--message` (attachments are not supported in interactive or pipe mode).
- With no `--message` and a TTY on stdin, an interactive REPL opens: streaming replies, tab completion, and client-side slash commands (`/help`, `/model`, `/models`, `/agents`, `/keys`, `/usage`, `/clear`, `/verbose`). Exit with `exit`, `quit`, or Ctrl-D.
- If stdin is piped, the CLI reads all of stdin, sends it as one message, and streams the reply.
- Routing is decided by the daemon: the model answers directly or starts a background workflow via a tool call. When a reply delegates work to a workflow, the daemon returns structured delegation metadata (task id + title) and the CLI polls that task by id, printing the result when it completes (Ctrl-C stops waiting; the task keeps running — check it later with `openalpaca tasks status <task_id>`).
- The daemon also recognizes chat-level commands with no LLM call: `/status [task_id]`, `/tasks`, `/cancel`/`/pause`/`/resume` (bare forms target the lane's active workflows, or pass an explicit task id), `/steer <text>` (inject a correction into the running workflow), and `/<skill>` invocations. In the interactive REPL, only the client-side commands listed above are handled locally; every other slash line — the daemon commands here and anything unrecognized (which may be a skill command) — is forwarded to the daemon as a chat message, so `/steer focus on the tests` works directly at the prompt. One-shot mode works too: `openalpaca chat --message "/steer focus on the tests"`.

## Troubleshooting

- Discovery missing/expired: start or restart daemon.
- Auth errors: ensure CLI and daemon use the same current discovery file.
- `daemon status` unhealthy: inspect daemon logs and `RUST_LOG` settings.
- Chat/stream failures: verify daemon is reachable on `127.0.0.1` and token is valid.
