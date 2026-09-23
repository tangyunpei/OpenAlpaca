# OpenAlpaca CLI Manual

`openalpaca` is the command-line interface for controlling a local `openalpacad` instance.

Related docs:
- [Quick Start](QuickStart_Manual.md)
- [Installation Manual](Installation_Manual.md)
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

Every command and subcommand takes `--help`. `openalpaca` has no `--version` flag; `openalpaca daemon status` prints the running daemon's version.

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

- The daemon writes discovery metadata to `~/.openalpaca/state/discovery.json` when it starts (under `OPENALPACA_HOME_STORE` instead, when that is set).
- CLI reads base URL and token from discovery.
- Protected endpoints use `Authorization: Bearer <token>`.
- Streaming endpoints may use query-token auth (handled by CLI internals).
- The token is valid for 24 hours from daemon start. After that the CLI refuses it with `Discovery token has expired`; `openalpaca daemon restart` writes a fresh one.

If discovery is missing or expired, daemon-backed commands fail until daemon is started/restarted. `config` is the exception — it works on local files and needs no daemon.

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

No API key? A local [Ollama](Installation_Manual.md#local-models-ollama) needs none. Start the daemon once first, so its config files exist:

```bash
ollama pull <model>                          # any chat model
openalpaca config set ai.ollama.enabled true # the whole setup; no restart needed
openalpaca llm models --refresh              # the pulled model is listed
openalpaca llm status                        # shows which model will answer
```

## Top-Level Commands

| Command | What it manages |
|---|---|
| [`daemon`](#daemon) | The daemon process: status, live events, start, stop, restart |
| [`config`](#config) | Settings in the database, `llm.toml` and `daemon.toml` |
| [`gui`](#gui) | The desktop app process |
| [`connector`](#connector) | Chat-platform connectors |
| [`tasks`](#tasks) | Runs (workflows) and their approval prompts |
| [`agents`](#agents) | Agents and their configuration |
| [`llm`](#llm) | Keys, models, usage |
| [`ext`](#ext) | MCP servers and plugins |
| [`plugin`](#plugin) | Plugin-only shortcut over `ext` |
| [`sessions`](#sessions) | Stored conversations |
| [`store`](#store) | Project history: re-base or purge |
| [`chat`](#chat) | Talk to the orchestrator |

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
- `stop` asks the daemon to stop and waits until its process has exited and its single-instance lock is free — at most 15 s — then stops the GUI. A daemon still there after 15 s is reported with its PID, the daemon log to read and the `kill -9` that finishes the job by hand, and `stop` exits with status 2.
- `restart` restarts the daemon only. It stops it the same way and starts the new one only once the old process has exited **and** the lock is free: a daemon keeps the lock for up to 10 s after its port closes, and a new one started into it would exit at once. If the old daemon is not gone within 15 s, nothing is started — `restart` prints why, and the commands that finish the job, and exits with status 2.
- `status` reads the discovery file and calls the daemon's health endpoint. It prints status, version, PID, instance id and URL.
- `tail` streams live daemon events (not historical query output); `--count` limits the number of events shown, default `0` = unlimited (Ctrl+C to stop). When the daemon shuts down, `tail` prints its `daemon_shutting_down` frame as an unknown event and then `Connection closed by server`.
- The CLI and the GUI app manage one daemon, not one each. A `stop` or `restart` from here shows in an open app window as `stopped elsewhere`; the app does not start a daemon again until someone chooses `Start daemon` there, which after a `restart` finds the new one. This verb sends the daemon SIGTERM and the app sends `POST /v1/command {"command":"shutdown"}`; the daemon treats the two as one shutdown, and both then wait the same way — for the process, then for the lock.
- `start` finds `openalpacad` in this order: `OPENALPACA_DAEMON_BIN=/abs/path/openalpacad`, next to the `openalpaca` binary (symlinks followed), `../libexec/`, then `PATH`. From a repository checkout it falls back to `cargo run -p openalpacad`.
- Daemon startup sets `OPENALPACA_CONFIG_DIR` to `~/.openalpaca/config` and runs the daemon with `~/.openalpaca` as its working directory.
- The daemon's output is appended to `~/.openalpaca/state/logs/daemon.log`; `start` prints the path. A log past 16 MB is rotated at start (`daemon.log.1` … `.3`).

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
- `config` operates directly on the local database and TOML files — no running daemon required. Two exceptions: the TUI's agent-management screen talks to the daemon, and `reset --factory` refuses while one is running. A file-backed key (`ai.*`, `daemon.*`) does not open the database at all, so `set`, `get` and a keyed `reset` on those keys work even when the database is refused; `list`, a keyless `reset` and the interactive editor all need it.
- `--all` includes unset keys with their defaults; `-v/--verbose` adds a source column (db / llm.toml / daemon.toml).
- `set` validates keys against the config schema; unknown keys get "did you mean" suggestions. A sensitive value (a token, an API key) is masked wherever it is printed.
- `get` prints the stored value, or the schema default followed by `(default)` when nothing is stored.

Each key belongs to one backend, chosen by its prefix:

| Key prefix | Backend | Examples |
|---|---|---|
| `telegram.*`, `imessage.*`, `discord.*` | database (`system_config` table) | `telegram.token`, `discord.enabled` |
| `ai.*` | `llm.toml` | `ai.default_model`, `ai.fallback_models`, `ai.ollama.enabled`, `ai.anthropic.api_key` |
| `daemon.*` | `daemon.toml` | `daemon.execution.max_rounds`, `daemon.orchestrator.routing.steering_enabled` |

`openalpaca config list --all` prints every key with its default and description.

```bash
openalpaca config set ai.ollama.enabled true
openalpaca config set ai.default_model qwen3:8b
openalpaca config get ai.default_model
openalpaca config list --all -v
```

Which file is edited: the one in `OPENALPACA_CONFIG_DIR` when that names a directory; otherwise `~/.openalpaca/config/<file>` when it exists; otherwise `./config/<file>` under the current directory (a repository checkout). A daemon running on the same config directory watches `llm.toml` and `daemon.toml` and picks an edit up without a restart.

`reset <key>` deletes that one key, from whichever backend owns it — the database, `llm.toml` or `daemon.toml`. `reset` with no key clears all configuration after a confirmation (agents and data are preserved). `<key>` and `--factory` are mutually exclusive.

`reset --factory` is the rescue verb. It deletes the database file `~/.openalpaca/state/openalpaca.db` together with its `-wal` and `-shm` siblings, then clears `config/llm.toml` (provider settings and every API key in it, including the keychain entries those keys point at) and resets `config/daemon.toml` to its defaults. The next daemon start builds an empty database at the current schema version. It is the way out of `Unsupported legacy schema version …`, because it is the one form of `config` that opens no database at all — every other form that needs the database opens it, and dies on the same guard.

- It **refuses while a daemon is running.** Stop it with `openalpaca daemon stop` first. Deleting a file the daemon has open would leave it writing into an unlinked inode while the next start created a second database beside it.
- It prints the absolute store root it is about to wipe — `OPENALPACA_HOME_STORE` may point anywhere — and requires you to type `factory-reset`. `y` is not enough, there is no `--yes` flag, and the prompt cannot be answered by a pipe: run it at a terminal.
- **No backup is taken and there is no undo.**
- **Not touched:** `state/cache/` (the local embedding model, ~1 GB), `state/.master_key`, `state/logs/`, `state/backups/`, `config/mcp.toml`, `config/agents/`, `config/skills/`, `config/orchestrator/`, and `~/.openalpaca/plugins/`.
- **Left on disk, now unreferenced:** everything under `artifacts/`, `uploads/` and `sessions/`. The rows that indexed those files are gone; the bytes are not. Delete the directories yourself if you want the space back.

### `gui`

Manage GUI process.

```bash
openalpaca gui start
openalpaca gui stop
```

`start` looks for the app bundle in this order: `OPENALPACA_GUI_APP=/abs/path/openalpaca-gui.app`, `~/Applications/openalpaca-gui.app`, `/Applications/openalpaca-gui.app`. From a repository checkout it falls back to the Tauri dev build.

### `connector`

Manage platform connectors.

```bash
openalpaca connector list
openalpaca connector enable <name>
openalpaca connector disable <name>
openalpaca connector delete <name>
```

Notes:
- Connector names are `telegram`, `imessage` (macOS only) and `discord`.
- These commands talk to the running daemon. A connector's credentials and options are config keys, set with `config`:

```bash
openalpaca config set telegram.token <bot-token>
openalpaca connector enable telegram
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
openalpaca tasks confirmations list [--format table|json]
openalpaca tasks confirmations watch
openalpaca tasks confirmations approve <request_id> [--entire-tool]
openalpaca tasks confirmations deny <request_id>
```

Notes:
- `--status` accepts `queued`, `running`, `completed`, `failed`, `cancelled`, `paused`,
  `interrupted`, `active`. `interrupted` is what the daemon writes at boot for a run it
  was driving when it went away — terminal, but not a failure, and re-runnable.
- `--limit` defaults to 50 (for both `list` and `log`).
- `list` prints `ID` (first 8 characters), `TITLE`, `STATUS`, `AGENTS` (how many subagents the run spawned, `-` for none) and `CREATED`.
- `status` prints the task and, under `Lanes:`, one line per subagent it spawned. With `--format json` the subagents are the `lanes` array; `lanes_error` is present when the timeline could not be read, so an empty array is never mistaken for "spawned nothing".
- Every command that takes a `<task_id>` needs the **full** id; the daemon does not match a prefix. The `list` table shortens it, so copy it from `openalpaca tasks list --format json`.

#### Creating a task

- `create` prompts for a title if the description argument is omitted; `--priority` defaults to 0.
- `create` **parks** a queued run. It does not start it, and the CLI has no `start` verb: start it from the GUI (`Start now` on the queued run), or with `POST /v1/tasks/{id}/action` and the body `{"action":"start"}`. To start work straight away from a terminal, ask for it in `openalpaca chat` instead.
- The parked row belongs to **your own lane** — the one this CLI and the GUI share, read from `GET /v1/me` the same way a `chat` turn reads it. The run's completion report lands in that conversation once it is started. If the daemon cannot be reached for that read, the create fails before anything is parked.
- A create with no terminal on stdin or stdout (a script, a cron line) sends `unattended: true`, and the daemon **stores it on the parked row**. Whoever starts the run later inherits that declaration unless it declares for itself, so a scripted run's confirm-listed tools are refused at once instead of waiting out the timeout. Nothing is pre-approved by it. See [Approval prompts](#approval-prompts) under `chat` for the rule.

#### Resuming a task

`resume` is one word over two verbs:

- On a **paused** run it is the plain transition back to running.
- On an **interrupted** run it is *replay resume* — **experimental, and off by default**. The daemon rebuilds the run's loop history from its session log (its rounds and their tool results) and continues it under the same id, telling the model not to repeat side-effecting calls it already made. Nothing recorded is re-executed.
- Turn replay resume on with `resume_enabled = true` under `[orchestrator.routing]` in `daemon.toml`. Until then an interrupted run answers `RESUME_DISABLED`.
- The other way to redo an interrupted run is a re-run: the GUI's `Re-run` button, or `POST /v1/tasks/{id}/rerun` directly. The CLI has no `rerun` verb yet.
- A resume that finds no usable transcript (the sweep took the log) answers `RESUME_LOG_MISSING` and leaves the row exactly as it was.
- When it succeeds the command names how much came back: `replayed 3 rounds from session <id>`.

#### Answering approval prompts (`confirmations`)

A tool on the confirm list suspends its run and asks. In the GUI the card is there; from a terminal these are the way:

- `watch` sits on the daemon's event socket and prompts for each one as it is raised: `Allow? [y]es / [a]lways this tool / [N]o`. `y` allows this call, `a` allows every later call of that tool for the rest of the session, anything else (Enter included) denies.
  - This is what answers a **background workflow's** prompt from a terminal: that run raises it long after the interactive `openalpaca chat` turn that started it has finished, so the chat itself is no longer listening.
  - Prompts that were already waiting when `watch` started are not replayed; `list` shows those.
  - Ctrl+C leaves every unanswered prompt exactly as it was.
- `approve` / `deny` answer one by id. `approve --entire-tool` allows every later call of that tool for the rest of the session. An id the daemon is not holding — it was answered already, it timed out, or the daemon restarted — is said plainly and changes nothing.
- `list` shows the prompts a run is **waiting on right now**, oldest first, read from `GET /v1/chat/confirmations`. The table is `REQUEST_ID`, `TOOL`, `RUN` (`-` for a prompt raised by a chat turn rather than a run) and `RAISED`.
  - It is a snapshot, not a subscription: a prompt answered or timed out since the read is refused by id, plainly, and nothing is changed.
  - It is **unscoped**: it lists what the whole daemon is waiting on, not only your own runs. Each row carries the tool's arguments — `--format json` hands them on; the table does not print them. On a single-owner machine that is the same list either way.

A prompt is never auto-approved, and `--entire-tool` (or `a` in `watch`) is the only thing that widens one. An unanswered prompt times out on the daemon as a refusal (`confirmation_timeout_secs` under `[execution.agent_defaults]` in `daemon.toml`, default 300 s). A one-shot or piped `chat` turn declares that it *cannot* answer, and its confirm-listed tools are refused immediately rather than waiting — see [Approval prompts](#approval-prompts).

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
- `openalpaca agents` with no subcommand enters interactive creation mode. So does `agents create` with no flags.
- `--from-file <path>` creates the agent from a TOML file.
- `set` takes a dotted config path, for example `openalpaca agents set <agent_id> llm.model <model-id>`. Every section on the way to the last key must already exist in the agent's config (`agents config <agent_id>` shows it). The value is read as a number, then `true`/`false`, then a string.
- `remove` archives the agent.

### `llm`

LLM keys, usage, model metadata, and routing control.

```bash
openalpaca llm status [--format table|json]
openalpaca llm keys list [--format table|json]
openalpaca llm keys add [--provider <name>] [--secret <key>] [--priority primary|fallback] [--source <src>] [--notes <text>]
openalpaca llm keys remove <provider> <key_id>
openalpaca llm keys validate --provider <name> [--secret <key>]
openalpaca llm keys set-primary <provider> <key_id>
openalpaca llm keys reorder <key_id>...
openalpaca llm usage [--agent <id>] [--key <key_id>] [--daily [--date YYYY-MM-DD]] [--format table|json]
openalpaca llm models [--refresh] [--format table|json]
openalpaca llm strategy --provider <name> <strategy>
openalpaca llm credentials [--format table|json]
openalpaca llm backends [--format table|json]
openalpaca llm provider-usage [--format table|json]
```

#### Turning a provider on

There is no `llm enable` verb. The provider ENABLE bit is a config key:

```bash
openalpaca config set ai.ollama.enabled true      # also ai.anthropic.enabled, ai.openai.enabled
```

- It lives under `config`, not `llm`, because it writes `llm.toml` through the config schema like every other `ai.*` key.
- Ollama needs no API key, so enabling it asks for none.
- The same switch is in the interactive `openalpaca config` TUI under API-Keys → `<provider>`, in the GUI (Settings → Models & keys), and in `llm.toml` itself (`[providers.<name>] enabled`). All of them write the same field.
- Whichever writes it, the daemon's file watcher registers any provider the file enables that the router is not already holding, model discovery included. No restart is needed.
- See [Installation Manual → Local Models (Ollama)](Installation_Manual.md#local-models-ollama).

#### Models and status

- `llm models --refresh` asks every loaded provider what it can serve before listing (`POST /v1/models/refresh`). Providers that need no key are asked exactly like the ones that do, so a model you just installed with `ollama pull` appears without restarting the daemon or editing `llm.toml`. Without the flag the catalogue is read as it stands.
- The models table carries a `TOOLS` column. A `-` there means the daemon did not say, not "no".
- An empty catalogue names the fix instead of printing `No items found.`
- `llm status` reads the daemon's own verdict on the default model. Its `Model:` line reads:
  - the configured model alone, when it is routable;
  - `X — not available, using Y` when the configured model is not routable and the fallback ladder answers with another;
  - the fix (`openalpaca config set ai.ollama.enabled true`, …) when nothing is routable at all.

#### Keys

- `llm keys add` asks interactively for whatever was not passed: provider, secret, source and notes. Pass all of `--provider`, `--secret`, `--source` and `--notes` to run it without a prompt. The key is checked first and the verdict printed (`valid` / `invalid`), but it is added either way.
- `llm keys remove` asks for confirmation before it deletes.
- A provider that **needs no API key** (Ollama) reads `· <provider> — no key needed` under `Key Health` in `llm status`, and gets a row saying the same in `llm keys list`. A keyed provider with an empty pool reads `✗ <provider> — no key configured`.
- `llm keys validate --provider <keyless provider>` answers that no key is needed and posts nothing. For a keyed provider, `--secret` is required.

#### Other notes

- `--format json` echoes the daemon's own field names. For `llm models` those are `id`, `input_price_per_million`, `output_price_per_million` and `supports_tools`. For `llm keys list` the row carries the daemon's `priority` string and a `keyless` flag.
- `llm usage --date` only applies together with `--daily`.
- **`llm strategy` has no effect at present.** The command prints a success line, but the daemon route it calls (`PUT /v1/orchestrator/config`) reads only `model` and `fallback_models` and ignores the strategy. Key selection is read from `llm.toml`: `strategy = "round_robin"` (the default), `"lru"` or `"primary_fallback"` under `[providers.<name>]`.

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
openalpaca ext mcp add <name> [options]    # options: see "Declaring an MCP server"
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
- `list` rows report `kind`, `id`, `enabled`, `state` (`enabled`, `disabled`,
  `unapproved`, `failed`, `orphaned`, and the in-flight `enabling`/`disabling`),
  the reason for that state, and how many tools the extension contributes.
  `info` shows one extension in full.

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

Installing from a URL is **not** supported: `source: "url"` is declined until it
has had its own security review. Only a local directory can be installed.

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

#### Declaring an MCP server

`ext mcp add` writes a `[servers.<name>]` block into `config/mcp.toml` through
the daemon's atomic, comment-preserving writer — your comments, defaults and
other servers come back unchanged — and then connects it. Writing a server into
your own config *is* the consent, so there is no approve step; `--disabled`
declares it turned off instead. `<name>` becomes the server's extension id, and
its tools register as `<name>__<tool>`.

| Option | Transport | Meaning |
|---|---|---|
| `--transport <TRANSPORT>` | both | `stdio` (default) or `http` |
| `--command <COMMAND>` | stdio | The program to run |
| `--arg <ARG>` | stdio | One argument; repeatable; a leading dash is fine |
| `--env KEY=VALUE` | stdio | One environment entry; repeatable; never a secret |
| `--env-from KEY=HOST_VAR` | stdio | Read the value from the daemon's own `HOST_VAR`; repeatable |
| `--cwd <DIR>` | stdio | Working directory for the child |
| `--url <URL>` | http | The endpoint to connect to |
| `--bearer-env <VAR>` | http | Environment variable holding the bearer token |
| `--api-key-header <HEADER>` | http | The header an API key is sent in |
| `--api-key-env <VAR>` | http | Environment variable holding that API key |
| `--header-from HEADER=HOST_VAR` | http | Read a header's value from `HOST_VAR`; repeatable |
| `--connect-timeout-secs <N>` | both | Seconds to wait for the connection |
| `--request-timeout-secs <N>` | both | Seconds to wait for one request |
| `--disabled` | both | Declare it turned off, so nothing is spawned |

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

It also covers the two places a credential hides outside `--env` and the
headers, because `config/mcp.toml` cannot tell them apart:

- **`--arg`.** An argument that *names* a credential — `--api-key sk-…`,
  `--token=…`, `PASSWORD=…` — is refused with `secret_literal_refused` naming
  the argument. The test is on the flag's name, and on both halves of a
  `NAME=VALUE` pair; a bare positional value is never tested, because it has no
  name to judge — `--port 8080` and a path like `/srv/keys/server.js` go in
  unchanged. Use `--env-from` and let the server read the value from the
  daemon's environment.
- **`--url`.** A url whose authority carries userinfo
  (`https://user:token@host`) or whose query carries a credential-shaped
  parameter (`?api_key=…`, `&token=…`) is refused the same way, naming which
  part tripped. `--bearer-env REMOTE_TOKEN` is the indirection that already
  exists for it.

Both refusals govern only what the daemon *writes*. A `config/mcp.toml` you
edited by hand keeps whatever it says — the parser reads it unchanged; the rule
is that this command never becomes the thing that wrote a secret down.

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
- The verbs carry the `ext` meanings above: `enable` writes the toggle and does
  not record consent, `approve` records consent and does not turn the plugin
  on, and `deny` performs a full unload.
- `info` shows the same row `ext info` shows, for one plugin.
- `config set` writes a key through the daemon (values are parsed as
  number/bool/string).
- `config get` reads the plugin's configuration back through the daemon; a
  value the manifest declares as a secret reads `<redacted>`, and nothing
  prints it in the clear.
- Non-sensitive values are stored at `~/.openalpaca/plugins/.config/<name>.toml`.
  That file is the place to look for them, not for secrets: `config set` refuses
  a key the manifest marks `sensitive`, so nothing secret is ever written there.

### `sessions`

List the conversations a lane holds. A lane keeps many of them and exactly one
is live at a time; these are the ids `chat --session` takes.

```bash
openalpaca sessions
openalpaca sessions --workspace .          # only this project's conversations
openalpaca sessions --workspace /repo/one
openalpaca sessions --all                  # every lane, connectors included
openalpaca sessions --limit 10 --format json

openalpaca sessions delete <id>            # rows and transcript, both
```

Notes:
- The table is `ID`, `STATUS` (`active` / `archived`), `TITLE`, `WORKSPACE` and `UPDATED`, newest first. A conversation that has never been renamed prints `(untitled)`; one bound to no project prints `-`, which is the normal state for a connector lane.
- With no flags the list is the lane the CLI and the GUI share (`{user}:gui`) — the lane a `openalpaca chat` turn lands on, read from `GET /v1/me`. `--all` widens to every lane the daemon holds.
- `--workspace <path>` narrows to one project. The path is resolved the same way a turn's project is — up to the nearest `.openalpaca`/`.git` — so `--workspace .` works from anywhere inside a repository. A directory under no such marker is an error, not a filter that matches nothing.
- An empty result says which kind of empty it is: no conversations at all, or none in the project that was filtered for.
- `delete <id>` deletes one conversation through `DELETE /v1/sessions/{id}`: its messages and the conversation itself, then its log directory under `~/.openalpaca/sessions/` — the transcript goes with the rows, which is what deleting a conversation means. What the conversation only *pointed at* survives, unpinned from it: its runs stay in `openalpaca tasks`, a queued follow-up stays on the lane, and the tool-call audit rows keep their history with only the session index cleared. It prints what went — title, id, message count and project. This is the counterpart to the GUI sidebar's delete, and the way to remove a conversation with no project, which `store purge` deliberately never touches.
- There is no `-y` on it: the one thing a confirmation could protect — the transcript a run is still writing into — the daemon already refuses (`409 SESSION_HAS_ACTIVE_WORKFLOWS`, cancel the run first), and another owner's conversation answers `404`.

### `store`

Manage the content store: re-base a project you moved on disk, or purge one you
are done with.

```bash
openalpaca store rebase /old/path/my-project /new/path/my-project --dry-run
openalpaca store rebase /old/path/my-project /new/path/my-project

openalpaca store purge /path/my-project          # prints the plan, deletes nothing
openalpaca store purge /path/my-project -y       # prints the same plan, then carries it out
openalpaca store purge --all --dry-run           # every project root on record
```

Notes:
- A project's path is its identity in four places — its artifacts, its conversations, its runs and its workspace memories — so moving the directory strands all four at once. `rebase` moves them together in one database transaction: either every member lands on the new path or none does.
- `--dry-run` changes nothing and reports what is recorded under each root, whether a store directory stands at either, and which refusal the real call would hit. It names runs that are *queued* under the old root separately from those still running or paused: a re-base takes a queued run's row with it, while a `purge` of that same root would be refused until it finishes, so the two numbers are not interchangeable.
- The daemon refuses rather than guesses: nothing of yours recorded under the old path is a `404`, and so is a root holding rows that belong to another owner (a re-base rewrites only rows you can see); a run under it that is still running or paused is refused until it finishes (rewriting the row would not move the process); a new path that already has a store recorded against it is refused rather than merged with the old one; two store directories — one at each root — are refused rather than chosen between; either path resolving to the home store (`~/.openalpaca`, or `$HOME` itself) is a `409` `WORKSPACE_IS_HOME`, because the home store is not a project and cannot become one; and a new path that sits *inside* another project's root is a `422` `WORKSPACE_NOT_A_ROOT` naming that root, rather than a silent re-base onto it.
- If `<old>/.openalpaca` still exists, the directory is moved too, after the transaction. In the usual case you moved the whole project with `mv` already, so only the rows are behind and nothing on disk is touched.
- The old path is resolved the way a chat turn's project is (up to the nearest `.openalpaca`/`.git`), falling back to the path itself when the old directory is gone. The new path is taken literally after canonicalization — a destination is where you say it is, or the call is refused. Absolute paths only.
- The GUI offers the same re-base from Settings → Connection when the project you choose has a store that records a different path.

`purge` notes:
- **`--dry-run` is what happens anyway.** Without `-y` the command prints the plan and deletes nothing; `-y` prints the same plan and then carries it out. `<project>` and `--all` are mutually exclusive, and one of them is required — a destructive verb never guesses which project you meant. The daemon defaults the same way, so a direct `POST /v1/workspaces/purge` with no `dry_run` field is also a dry run.
- **`<project>` is taken almost literally.** Rows are the proof of a root: it is canonicalized, and if any of your conversations, runs, uploads or memories still name that exact path, it is purgeable outright — no further check, even when the path sits under another project's `.git`/`.openalpaca` (a project moved out from under a monorepo, its own marker gone with it, purges by the root its rows still name). Only a path nothing names is checked against the same `.openalpaca`/`.git` walk `rebase`'s *old* path takes — but unlike `rebase`, purge never silently follows that walk. If it resolves to an **ancestor** different from what you typed, the whole call is refused with a `422` `WORKSPACE_NOT_A_ROOT` naming that root: `openalpaca store purge /repo/src` deletes nothing and tells you to name `/repo` (or give `/repo/src` a project marker of its own first), rather than purging the whole project for a path that named one file of it. A path that resolves to itself, or to no marker at all, is taken as given — and from there the ordinary `404` follows if nothing turns out to be recorded under it.
- **What goes:** this project's conversations (`session` rows, their messages, tool-call index rows and queued follow-ups) together with each conversation's log directory under `~/.openalpaca/sessions/`; its runs (`task` rows with their subagent spans, run events, dispatch decisions and LLM call log); and the uploads copied into `<project>/.openalpaca/uploads/`, rows and bytes. Rows go in one transaction per project, then the files.
- **What stays, and is named in the plan rather than left out of it:** `artifacts/` and every produced file record (never garbage-collected), your workspace memories, `skills/` and `config/`, the store's own `.layout` / `README.md` / `.gitignore`, and any directory inside `<project>/.openalpaca/` that OpenAlpaca did not create. Conversations with no project belong to the home store and are never touched — `--all` says how many there are, names the home store's `state/` as kept too (a factory reset is a separate, deliberate act), and leaves the conversations to `openalpaca sessions delete`.
- The plan is printed in the retention classes of the README seeded into every store root, so the reason for each verdict is on the line beside it:

```text
Would purge /Users/me/code/my-project
  delete  sessions/                       3 conversations, 41 messages, 12 tool calls, 0 follow-ups — their logs live in the home store's sessions/
                                          (size-capped, optional age sweep)
  delete  runs (database)                 2 runs, 5 subagent spans, 18 run events
                                          (never swept — removed only when you ask)
  delete  uploads/                        4 uploads copied into this project
                                          (swept: an upload attached to no message is deleted once past the grace period)
    keep  artifacts/                      9 files produced by runs in this project
                                          (never garbage-collected)
    keep  memory/                         3 workspace memories, and the reserved memory/ directory
                                          (yours — never swept)
    keep  skills/, config/                reserved; not created until used
                                          (yours — never swept)
    keep  .layout, README.md, .gitignore  this store's own markers
                                          (store metadata — never swept)
    keep  notes-of-my-own                 not created by OpenAlpaca
                                          (the store never deletes what it did not create)

Nothing was deleted. Re-run with -y to carry this out.
```

- The daemon refuses rather than guesses, the same way the re-base does: nothing of yours recorded under the path is a `404`, and so is a root holding rows that belong to another owner; a run there that is queued, running or paused — or a conversation with a run in flight — is a `409` `WORKSPACE_BUSY` (a queued run has not started yet, but it already named this root and is about to resolve the store the moment it does), and with `--all` one busy root refuses the whole call rather than purging the others; a path resolving to the home store is a `409` `WORKSPACE_IS_HOME` (the home store is not a project, and a factory reset is `openalpaca config reset --factory`, a separate deliberate act); a relative path is a `400`.
- A purge is not reversible and there is no undo. Back up `<project>/.openalpaca` first if you might want the transcripts again.

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

#### Three ways to run it

| Invocation | What happens |
|---|---|
| `openalpaca chat` on a terminal | An interactive REPL opens. |
| `openalpaca chat --message "<text>"` | One-shot: the turn is sent, the reply printed, the process exits. |
| stdin piped, no `--message` | All of stdin is read and sent as one message; the reply is the output. |

- The REPL has streaming replies, tab completion and line history (kept in `~/.openalpaca/state/repl_history`). Exit with `exit`, `quit`, or Ctrl-D. If the daemon was restarted in the meantime, the REPL reconnects once and retries.
- The REPL handles these slash commands itself: `/help`, `/model`, `/models`, `/agents`, `/keys`, `/usage`, `/clear`, `/verbose`. Every other slash line goes to the daemon — see [Slash commands](#slash-commands).
- Piped stdin that is empty (or only whitespace) sends nothing and exits **1** with `No input on stdin` — a pipe that produced nothing is a mistake upstream, not a request to send an empty turn.

#### Attachments

- `--file <PATH>` is repeatable and requires `--message`. Attachments are not supported in the REPL or in pipe mode.
- Each file is uploaded before the turn is sent. The CLI prints `Uploaded: <name> (<file id>)` on stderr for each one.
- The daemon's default limits are 10 files per message and 50 MB per file (`max_files_per_message` and `max_file_size_bytes` under `[upload]` in `daemon.toml`). A message with too many files is refused with `TOO_MANY_ATTACHMENTS`.
- What reaches the model depends on the model that answers. An image needs a model with image input. A document still travels as its extracted text to a model with no native document input; it is skipped only when no text could be extracted. An audio clip needs a model with audio input: nothing transcribes it, so any other model skips it.
- **A file that does not reach the model is never dropped silently.** After the answer, the CLI prints one line per skipped file on stderr, with the daemon's reason:

```text
Attachment <file id> did not reach the model: the answering model does not support image input
```

- Some turns cannot carry files at all, and report every attachment as skipped the same way: a task command such as `/status`, a `/steer`, or a skill that comes from a plugin (it receives the question as plain text).

#### Project and conversation

- Every turn carries the CLI's working directory as its project (`x-workspace-path`), the same way the GUI sends the project chosen in its window. The daemon resolves it up to the nearest `.openalpaca`/`.git` marker; that root is what a run records as its `workspace_id` and where the files it writes land. A directory the CLI cannot canonicalize sends no project at all rather than a path the daemon would resolve against its own directory.
- `--resume` continues a stored conversation instead of the lane's current one. `--session <id>` names one directly; `openalpaca sessions` lists the ids. The two are mutually exclusive.
- `--resume` is scoped to **this project**: the conversations bound to the working directory's own root, newest first. The lane is shared with the GUI and with every other checkout, so a lane-wide list would offer another project's conversations.
  - With a terminal it opens a picker over them.
  - With stdin piped it takes the most recent — the row the picker would have opened on — because a prompt written into a pipe is a hang, not a question.
  - A project with no conversations yet says so and names itself, rather than reaching for another project's.
  - From a directory under no project marker the list stays lane-wide, which is the same scope such a turn itself has.
- `--session <id>` only continues a conversation on the lane the CLI talks on. One on another lane (a connector's) is refused here, before anything is sent, naming both lanes: activating it would archive that lane's own live conversation and the turn would still be refused (`409 SESSION_LANE_MISMATCH`). `openalpaca sessions --all` is what lists those.
- Resuming **re-opens** the conversation (`POST /v1/sessions/{id}/activate`) before anything is sent. A lane holds exactly one live conversation, so resuming an archived one archives whatever was live. The CLI prints the conversation it resumed, and its project, so that is visible rather than discovered later. The last 8 messages are printed before the prompt opens.
- A resumed conversation's own project governs the turn, overriding the working directory: one conversation belongs to one project. A conversation that has no project yet takes the working directory's and is bound by it.

#### What is printed

On a **terminal**:

- The reply is labelled `Alpaca: ` and streams as the model writes it — the daemon forwards the provider's own tokens.
- **A thinking model's reasoning is shown dimmed**, as it arrives, and the answer then starts on its own line. A local reasoning model can spend ten seconds or more thinking before its first token; this is what fills that silence. Reasoning is never part of the answer: nothing stores it, and `openalpaca chat --resume` will not replay it.
- **The turn ends on the authoritative answer.** The streamed pieces do not always add up to the final answer: a turn that calls a tool streams the text written before the call, and a stream that breaks mid-way is answered by a non-streaming fallback. When the turn is done the CLI reconciles — it appends what is missing, or, when the stream diverged, prints the final answer on a fresh line — so the terminal always ends with the answer, exactly once.
- A usage line follows the answer: `[<model> | <tokens> in | <tokens> out | <ms>ms]`. The model named is the one that answered, which can differ from the configured one when the fallback ladder stepped in.

On a **pipe or a redirect** (`openalpaca chat --message q > answer.txt`, `openalpaca chat < question.txt > answer.txt`, `... | jq`):

- No `Alpaca: ` label, no reasoning, no partial text. Nothing is written before the turn is done; then the final answer is written once.
- The usage line is still written to stdout after the answer. When the turn delegated to a workflow, so is the workflow's outcome (`[Task completed in 42s]`, then `Result: …`). A script that wants the answer alone must drop those lines.
- `Uploaded: …`, the skipped-attachment lines and `Waiting for task to complete...` go to stderr, so they never mix with the answer.
- A failed turn writes no partial answer.

Both:

- **A one-shot exits when the turn is over.** The process returns on the stream's last frame (`done`, or an `error`) rather than waiting for the daemon to close the connection, which the daemon keeps open a few seconds longer for late readers.
- **A turn that reaches no answer says why.** A turn that runs out of tool rounds, hits its cost cap or is truncated with nothing written answers with one line naming the reason and, where there is one, the last tool error — "I stopped after 8 tool rounds without reaching an answer. The last tool error was: …". It is the turn's answer like any other: stored in the conversation, printed on a terminal, written to a pipe.
- **A reply never claims a run it did not start.** If an answer states a task id that matches none of your runs, and no workflow was started in that turn, the daemon appends one line beneath it: `Note from OpenAlpaca: no workflow was started in this turn, and no task with id <id> exists. Ask again to start one.`

#### Failures and exit codes

- **A failed turn fails the command.** When the daemon reports an error (no routable model, a provider that cannot be reached, a broken stream), the message goes to **stderr** and the process exits non-zero — in the `--message` path and the piped path alike.
- The interactive REPL prints the failure on stderr and keeps the prompt open.
- If the failure is "no routable model", `openalpaca llm status` names the fix.

#### Approval prompts

A tool on the confirm list suspends the turn and asks before it runs.

- **With a terminal on both stdin and stdout** — the REPL, or a `--message` typed at a prompt — the confirmation arrives on the stream the CLI is reading and it asks inline: `Allow execution? [y/N]`. `y` or `yes` approves; anything else denies.
- **Otherwise the turn declares `unattended: true`** on `POST /v1/chat`: `openalpaca chat --message q > answer.txt`, `... | jq`, `openalpaca chat < question.txt`, a cron line, a CI step. Nobody could answer there, so the daemon **refuses** the confirm-listed tool at once and tells the model where it can be approved, instead of waiting out `confirmation_timeout_secs` (default 300 s) *per tool call*.
- It is a declaration, not an approval: nothing is auto-allowed.
- Nothing is approved by leaving, either: a prompt left unanswered times out on the daemon as a refusal.
- The declaration travels with the work: a workflow started by an unattended turn is unattended too, so no prompt is ever raised for it and `tasks confirmations watch` has nothing to answer. Work that needs an approval has to start from somewhere that can give one — the GUI, or an interactive `openalpaca chat`.
- A **background workflow** started from an interactive chat raises its prompts after the turn that started it has ended, so the inline `[y/N]` never sees them. Answer those from the GUI, or keep `openalpaca tasks confirmations watch` open in another terminal — see [`tasks`](#answering-approval-prompts-confirmations).

#### Workflows

- Routing is decided by the daemon: the model answers directly or starts a background workflow via a tool call.
- When a reply delegates work to a workflow, the daemon returns structured delegation metadata (task id + title) and the CLI polls that task by id every 2 seconds, printing the result when it completes.
- Ctrl-C stops waiting; the task keeps running. The CLI also stops waiting by itself after 5 minutes. Either way, check the run later with `openalpaca tasks status <task_id>`.
- A run the daemon was driving when it restarted reads `interrupted`. The CLI says so and stops waiting. See [Resuming a task](#resuming-a-task) for the ways to redo it.

#### Slash commands

- The daemon answers these chat-level commands itself, without asking a model: `/status [task_id]`, `/tasks`, `/cancel`/`/pause`/`/resume` (bare forms target the lane's active workflows, or pass an explicit task id), and `/steer <text>` (inject a correction into the running workflow).
- `/<skill>` runs that skill directly, without the main loop choosing it.
- In the REPL, only the client-side commands (`/help`, `/model`, `/models`, `/agents`, `/keys`, `/usage`, `/clear`, `/verbose`) are handled locally. Every other slash line — the daemon commands here and anything unrecognized (which may be a skill command) — is forwarded to the daemon as a chat message, so `/steer focus on the tests` works directly at the prompt.
- One-shot mode works too: `openalpaca chat --message "/steer focus on the tests"`.
- A skill's answer streams like any other turn. A skill contributed by a plugin cannot stream: its answer arrives in one piece.

## Troubleshooting

- Discovery missing/expired: start or restart daemon. `Discovery token has expired` means the daemon has been up for more than 24 hours — `openalpaca daemon restart`.
- Auth errors: ensure CLI and daemon use the same current discovery file (the same `OPENALPACA_HOME_STORE`, if you set one).
- `daemon status` unhealthy: inspect the daemon log (`~/.openalpaca/state/logs/daemon.log` for a daemon started with `openalpaca daemon start` or by the GUI app) and `RUST_LOG` settings.
- `daemon stop` or `restart` exits with status 2: the old daemon was not gone 15 s after it was asked to stop. The message says whether the process is still running (it prints the `kill -9 <pid>` that finishes the job) or something still holds the single-instance lock (check `openalpaca daemon status`). `restart` starts nothing in either case.
- Chat/stream failures: verify daemon is reachable on `127.0.0.1` and token is valid.
- A chat turn fails with no routable model: run `openalpaca llm status` — its `Model:` line names the fix. With a local Ollama that is `openalpaca config set ai.ollama.enabled true`, then `openalpaca llm models --refresh`.
- A scripted or piped turn reports that a tool needs an approval it cannot ask for: the turn was unattended, so the tool was refused without a prompt. Run that work from the GUI or from an interactive `openalpaca chat`, and approve it there.
- A workflow sits in `running` and nothing happens: it may be waiting on an approval. `openalpaca tasks confirmations list` shows what is waiting; `approve`/`deny <request_id>` answers it.
- A command says a task was not found although `tasks list` shows it: the table shortens ids. Use the full id from `openalpaca tasks list --format json`.
