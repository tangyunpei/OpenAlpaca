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

# List stored conversations, then continue one
openalpaca sessions
openalpaca chat --resume
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
- `--status` accepts `queued`, `running`, `completed`, `failed`, `cancelled`, `paused`,
  `interrupted`, `active`. `interrupted` is what the daemon writes at boot for a run it
  was driving when it went away — terminal, but not a failure, and re-runnable.
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

### `ext`

Manage extensions — MCP servers and plugins — on the one ENABLE axis. There is
no per-tool switch: a tool is turned off by turning off the server or plugin
that provides it.

```bash
openalpaca ext list [--include-orphaned] [--format table|json]
openalpaca ext info <kind> <id>          # kind = mcp | plugin
openalpaca ext enable <kind> <id>
openalpaca ext disable <kind> <id>
openalpaca ext reload <kind> <id>
openalpaca ext approve <plugin-id>
openalpaca ext deny <plugin-id>
openalpaca ext remove <plugin-id>

openalpaca ext install <path> [--dry-run]
openalpaca ext update <plugin-id> <path>
openalpaca ext uninstall <kind> <id> [--purge-data]
openalpaca ext mcp add <name> [--transport stdio|http] ...
openalpaca ext mcp remove <name>
```

Notes:
- `enable` writes the toggle and then loads; `disable` writes it, drains
  in-flight calls (`[extensions] drain_timeout_secs`, default 10 s) and unloads
  — the plugin child is killed, the MCP connection dropped, no reconnect.
- `enable` on a plugin that has not been approved records the bit and leaves it
  `unapproved`: consent pre-empts the switch. `approve` records consent and
  does **not** turn the plugin on; `deny` refuses it and unloads it.
- `reload` re-applies an edited declaration or a rotated credential.
- `remove` drops the permissions entry of an *orphaned* plugin — one whose
  directory is gone. `list --include-orphaned` is how you see those.
- Rows report `kind`, `id`, `enabled`, `state` (`enabled`, `disabled`,
  `unapproved`, `failed`, `orphaned`, and the in-flight `enabling`/`disabling`)
  and what the extension contributes.

#### Installing and removing extensions

`install` copies a plugin directory into the plugins root under **its own
directory name**, which becomes the plugin's id. The path must be absolute and
outside the plugins root, and its `plugin.toml` must name the same directory —
a manifest that renames itself is refused rather than landed, because such a
directory can never load.

**Installing grants nothing.** The plugin arrives with its toggle at the
default (on) and *no consent decision*, so it sits `unapproved`/`never_seen`
and nothing runs. `ext approve <id>` is the single action that starts it. The
command prints the manifest first — what it contributes, what capabilities it
asks for, which config keys it needs — because that is the decision approving
makes.

```bash
openalpaca ext install ~/src/openalpaca-notion --dry-run   # parse and report only
openalpaca ext install ~/src/openalpaca-notion
openalpaca ext approve openalpaca-notion                   # now it runs
```

`update <id> <path>` replaces an installed plugin's tree: the plugin is torn
down first (its child runs with its directory as the working directory, so an
in-place replace is never allowed), the incumbent tree is moved to
`plugins/.trash/`, the replacement is renamed into place, and the load path
runs again. Consent survives an update whose declared capabilities are
unchanged; if they changed, consent goes back to pending and the command says
what the new version also asks for.

`uninstall <kind> <id>` is the real removal, and it **deletes nothing**: the
plugin is unloaded, its `.permissions.toml` entry removed, and its directory
*moved* to `plugins/.trash/<id>-<timestamp>/` — the command prints where.
`plugins/.data/<id>/` is kept unless you pass `--purge-data`, which moves it to
the trash as well. For `kind = mcp` this removes the `[servers.<name>]` block
from `config/mcp.toml`; the server must be turned off first (`ext disable mcp
<name>`), otherwise the command refuses with `not_disabled`.

`ext mcp add` writes a `[servers.<name>]` block into `config/mcp.toml` through
the daemon's atomic, comment-preserving writer — your comments, defaults and
other servers come back unchanged — and then connects it. Writing a server into
your own config *is* the consent, so there is no approve step; `--disabled`
declares it turned off instead.

**The daemon does not write secrets.** `--env KEY=VALUE` carries literal values
for ordinary settings, but a key that names a credential (anything containing
`token`, `key`, `secret`, `password` or `credential`) is refused with
`secret_literal_refused`: the value would sit in `config/mcp.toml` in the clear
and in every rotated copy under `state/backups/`, which is not somewhere you
would think to look when rotating a leaked token. Use `--env-from KEY=HOST_VAR`
instead — the block records the *name* of a variable, and the daemon reads that
variable from its own environment each time it starts the server, exactly as
`--bearer-env` does for HTTP. A variable that is not set is a start failure
naming it, never an empty value handed to the server.

The same rule covers HTTP headers, which is where a remote server's credential
actually travels. A header named `Authorization`, `Proxy-Authorization`,
`Cookie` or `X-Api-Key`, or one whose name or value looks like a credential, is
refused with `secret_literal_refused`; `--header-from HEADER=HOST_VAR` records
the variable's *name* and the daemon reads it from its own environment each time
it connects.

```bash
export GITHUB_TOKEN=ghp_xxx            # in the daemon's environment
openalpaca ext mcp add github --command npx \
  --arg -y --arg @modelcontextprotocol/server-github \
  --env-from GITHUB_TOKEN=GITHUB_TOKEN
openalpaca ext mcp add remote --transport http \
  --url https://example.com/mcp --bearer-env REMOTE_TOKEN
openalpaca ext mcp add tracked --transport http \
  --url https://example.com/mcp \
  --header-from Authorization=REMOTE_TOKEN
openalpaca ext disable mcp github && openalpaca ext mcp remove github
```

Installing from a URL is **not** supported: `source: "url"` is declined until it
has had its own security review. Only a local directory can be installed.

### `plugin`

The plugin-shaped shortcut over the same routes (`/v1/extensions`), kept for
plugin-only work such as configuration.

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
- The verbs keep their names but carry the `ext` meanings above: `enable` no
  longer records consent, and `deny` performs a full unload.
- `info` shows the same row `ext info` shows, for one plugin.
- `config set` writes a key through the daemon (values are parsed as
  number/bool/string).
- `config get` reads the plugin's configuration back through the daemon; a
  value the manifest declares as a secret reads `<redacted>`, and nothing
  prints it in the clear.

### `sessions`

List the conversations a lane holds. A lane keeps many of them and exactly one
is live at a time; these are the ids `chat --session` takes.

```bash
openalpaca sessions
openalpaca sessions --workspace .          # only this project's conversations
openalpaca sessions --workspace /repo/one
openalpaca sessions --all                  # every lane, connectors included
openalpaca sessions --limit 10 --format json
```

Notes:
- The table is `ID`, `STATUS` (`active` / `archived`), `TITLE`, `WORKSPACE` and `UPDATED`, newest first. A conversation that has never been renamed prints `(untitled)`; one bound to no project prints `-`, which is the normal state for a connector lane.
- With no flags the list is the lane the CLI and the GUI share (`{user}:gui`) — the lane a `openalpaca chat` turn lands on, read from `GET /v1/me`. `--all` widens to every lane the daemon holds.
- `--workspace <path>` narrows to one project. The path is resolved the same way a turn's project is — up to the nearest `.openalpaca`/`.git` — so `--workspace .` works from anywhere inside a repository. A directory under no such marker is an error, not a filter that matches nothing.
- An empty result says which kind of empty it is: no conversations at all, or none in the project that was filtered for.

### `chat`

Interactive or one-shot chat through daemon orchestrator.

```bash
openalpaca chat
openalpaca chat --message "hello"
openalpaca chat --message "summarize these" --file a.txt --file b.png
echo "hello" | openalpaca chat

# Continue a stored conversation
openalpaca chat --resume
openalpaca chat --session 0f2c9a41-3b7d-4e58-9a10-6c1f2d3e4b55
```

Notes:
- `--file <PATH>` is repeatable and uploads the files as message attachments; it requires `--message` (attachments are not supported in interactive or pipe mode).
- Every turn carries the CLI's working directory as its project (`x-workspace-path`), the same way the GUI sends the project chosen in its window. The daemon resolves it up to the nearest `.openalpaca`/`.git` marker; that root is what a run records as its `workspace_id` and where the files it writes land. A directory the CLI cannot canonicalize sends no project at all rather than a path the daemon would resolve against its own directory.
- `--resume` continues a stored conversation instead of the lane's current one. With a terminal it opens a picker over this lane's conversations, newest first; with stdin piped it takes the most recent — the row the picker would have opened on — because a prompt written into a pipe is a hang, not a question. `--session <id>` names one directly; `openalpaca sessions` lists the ids. The two are mutually exclusive.
- Resuming **re-opens** the conversation (`POST /v1/sessions/{id}/activate`) before anything is sent. A lane holds exactly one live conversation, so resuming an archived one archives whatever was live; the CLI prints the conversation it resumed, and its project, so that is visible rather than discovered later. The last few turns are printed before the prompt opens.
- A resumed conversation's own project governs the turn, overriding the working directory: one conversation belongs to one project. A conversation that has no project yet takes the working directory's and is bound by it.
- With no `--message` and a TTY on stdin, an interactive REPL opens: streaming replies, tab completion, and client-side slash commands (`/help`, `/model`, `/models`, `/agents`, `/keys`, `/usage`, `/clear`, `/verbose`). Exit with `exit`, `quit`, or Ctrl-D.
- If stdin is piped, the CLI reads all of stdin, sends it as one message, and streams the reply.
- Routing is decided by the daemon: the model answers directly or starts a background workflow via a tool call. When a reply delegates work to a workflow, the daemon returns structured delegation metadata (task id + title) and the CLI polls that task by id, printing the result when it completes (Ctrl-C stops waiting; the task keeps running — check it later with `openalpaca tasks status <task_id>`).
- The daemon also recognizes chat-level commands with no LLM call: `/status [task_id]`, `/tasks`, `/cancel`/`/pause`/`/resume` (bare forms target the lane's active workflows, or pass an explicit task id), `/steer <text>` (inject a correction into the running workflow), and `/<skill>` invocations. In the interactive REPL, only the client-side commands listed above are handled locally; every other slash line — the daemon commands here and anything unrecognized (which may be a skill command) — is forwarded to the daemon as a chat message, so `/steer focus on the tests` works directly at the prompt. One-shot mode works too: `openalpaca chat --message "/steer focus on the tests"`.

## Troubleshooting

- Discovery missing/expired: start or restart daemon.
- Auth errors: ensure CLI and daemon use the same current discovery file.
- `daemon status` unhealthy: inspect daemon logs and `RUST_LOG` settings.
- Chat/stream failures: verify daemon is reachable on `127.0.0.1` and token is valid.
