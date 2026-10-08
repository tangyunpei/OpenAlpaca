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
| [`store`](#store) | Projects: list them, re-point a moved one, delete one's data |
| [`memory`](#memory) | Memory notes: list, search, add, edit, confirm, undo, forget, sync, doctor, adopt |
| [`persona`](#persona) | `USER.md`: show it, edit it |
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
- `stop` asks the daemon to stop and waits until its process has exited and its single-instance lock is free — at most 15 s — and stops nothing else: an open app window stays open, keeps its unsent draft and shows `stopped elsewhere` (below). To quit the app too, run `openalpaca gui stop`. A daemon still there after 15 s is reported with its PID, the daemon log to read and the `kill -9` that finishes the job by hand, and `stop` exits with status 2.
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

`reset --factory` is the rescue verb. It deletes the database file `~/.openalpaca/state/openalpaca.db` together with its `-wal` and `-shm` siblings, then clears `config/llm.toml` (provider settings and every API key in it, including the keychain entries those keys point at) and resets `config/daemon.toml` to its defaults. The next daemon start builds an empty database at the current schema version. It is the way out of ``Database `<path>` is at schema version <n>, which this build does not know`` for a database from before the schema was re-baselined at version 1 (2026-09-28), and the only form of `config` that clears a refused database. It reaches one because it opens no database — it deletes the files and asks the schema nothing. Every other form that needs the database opens it and dies on the same guard; a file-backed `ai.*` or `daemon.*` key (above) opens none either, so it still works, but it leaves the database as refused as it was. The same refusal also meets a database a **newer** OpenAlpaca created, and its text names both cases because it cannot tell them apart: that database is fine and this build is the wrong one — use the build that created it, and do not reset.

- It **refuses while a daemon is running.** Stop it with `openalpaca daemon stop` first. Deleting a file the daemon has open would leave it writing into an unlinked inode while the next start created a second database beside it. The check is the daemon's own singleton lock, taken before the prompt and held until the files are gone, so a daemon that is still booting also refuses it, and a daemon started while the prompt is open — the app, or `daemon start` in another terminal — exits on the lock instead of opening the database. When the lock is held but no running daemon can be found — one still booting, or a lock file you cannot write (a daemon once run under `sudo` leaves `state/openalpacad.lock` owned by root) — it refuses with a message naming that file instead of telling you to stop a daemon that `openalpaca daemon stop` will not find.
- It prints the absolute store root it is about to wipe — `OPENALPACA_HOME_STORE` may point anywhere — and the absolute paths of the `llm.toml` and `daemon.toml` it will clear, found the way `config set` finds them (above), so a run from a repository checkout names the checkout's own `./config/` files when that is where they resolve; a file that does not exist is named as none. It then requires you to type `factory-reset`. `y` is not enough, there is no `--yes` flag, and the prompt cannot be answered by a pipe: run it at a terminal.
- **No backup is taken and there is no undo.**
- **Not touched:** `state/memory-control/` (each memory partition's control snapshot — revisions, origins and the tombstones of forgotten notes), `state/cache/` (the memory search index, `memory-index-v1.sqlite`, which the next start opens as it is — a disposable cache, rebuilt in the background when it is missing, damaged, of another version or grown past twice its size after its last rebuild), `state/.master_key`, `state/logs/`, `state/backups/`, `config/mcp.toml`, `config/agents/`, `config/skills/`, `config/orchestrator/` (so `USER.md` stays as it is), and `~/.openalpaca/plugins/`.
- **Left on disk, orphaned until adopted:** your memory notes under `memory/` and in each project's folder under `projects/`. The next start mints a new owner id, so `openalpaca memory doctor` lists the old partitions as orphaned; nothing reassigns them for you — `openalpaca memory adopt <owner-key>` re-binds them, finding each project's by the project's id.
- **Left on disk, now unreferenced:** everything under `artifacts/`, `uploads/` and `sessions/`, and each project's `projects/<name>/artifacts/` and `projects/<name>/uploads/`. The rows that indexed those files are gone; the bytes are not. Delete those directories yourself if you want the space back.
- **A project's folder under `projects/` is not unreferenced.** Its `project.json` — the project's id and path — still registers it, so `store list` lists it and the next turn in that project uses the same folder, and its `memory/` holds the project's notes, which `openalpaca memory adopt` re-binds (above). Deleting the folder drops those notes.

### `gui`

Manage GUI process.

```bash
openalpaca gui start
openalpaca gui stop
```

`start` looks for the app bundle in this order: `OPENALPACA_GUI_APP=/abs/path/openalpaca-gui.app`, `~/Applications/openalpaca-gui.app`, `/Applications/openalpaca-gui.app`. From a repository checkout it falls back to the Tauri dev build. `stop` sends SIGTERM to the app's process — on macOS the bundle's `openalpaca_gui` — and leaves the daemon running; an unsent draft in the app is lost.

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
- `log` prints one line per event of the run: time, event, `[agent]` and the event's detail. An approval prompt's rows (`tool_confirmation_requested`, `tool_confirmation_resolved`) name a tool and an agent that can come from a third party, so the agent is shown with every hidden character as `<U+XXXX>` and a detail printed as JSON writes them with JSON's own `\u` escapes — never raw.
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
- A **per-call** prompt — `memory_confirm`, the assistant asking whether a suggestion should replace a note you saved (see [`memory`](#memory)) — is good for that one call only: `approve --entire-tool` is refused before anything is sent, and `watch` asks `Allow this one call? [y]es / [N]o`, where `a` denies like any other word. `deny` dismisses the suggestion for good, as `openalpaca memory undo` does.
- `list` shows the prompts a run is **waiting on right now**, oldest first, read from `GET /v1/chat/confirmations`. The table is `REQUEST_ID`, `TOOL`, `RUN` (`-` for a prompt raised by a chat turn rather than a run) and `RAISED`.
  - It is a snapshot, not a subscription: a prompt answered or timed out since the read is refused by id, plainly, and nothing is changed.
  - It is **unscoped**: it lists what the whole daemon is waiting on, not only your own runs. Each row carries the tool's arguments — `--format json` hands them on; the table does not print them. On a single-owner machine that is the same list either way.
  - A prompt whose card carries a daemon-authored **preview** (`memory_confirm`'s: which note, your text, the new text, why, and when you said it) is printed under the table as `<request_id> — from OpenAlpaca:` followed by the preview; `--format json` carries it as `preview`, beside `per_call`. `watch` prints it in the prompt block, before the arguments.
- **Hidden characters.** Every hidden character in a prompt's tool name, agent id and arguments — a bidi override, a zero-width or control character, a variation selector, any space but a plain one (a no-break, ideographic or braille blank), an object replacement character (U+FFFC, which macOS draws as blank space), a code point Unicode has not assigned (which has no agreed appearance: a macOS terminal draws some as blank space), a private-use code point (which looks like whatever a font puts there: Apple's system fonts draw some as nothing, and a macOS terminal some as blank space), and in the name and the agent id also a line break or a tab — is shown as `<U+XXXX>`, never printed raw. What is hidden is decided by Unicode's classes — control, format, default-ignorable, unassigned and private-use characters, and every blank but the plain space — not by what your font draws: a letter that a particular font happens to draw as an empty cell is printed as it is and not counted (JetBrains Mono, the default font of several terminals, draws the Bamum letter U+16910 so). A run of ordinary spaces is printed as it is, and the terminal wraps it like any text, so an argument padded with them can push its end lines below its start: read an argument whole before answering.
  - `watch` prints a line under the arguments counting them: `Note: this request contains 2 hidden characters, shown as <U+…>.` Only real escapes are counted, so text that merely looks like `<U+202E>` is not.
  - The `list` table escapes the `TOOL` column, and under the table prints the same line for each prompt that hides anything, as `<request_id> — Note: …`. Its count includes the agent id and the arguments, which the table does not print, and one line under the notices says so and points at `--format json`, which prints them — worth reading before `approve <id>`.
  - `--format json` writes those characters with JSON's own `\u` escapes instead: a JSON reader gets exactly the daemon's values, and the terminal never receives one raw.
  - The answer line after `watch` and the refusal of `--entire-tool` show the name the same way. The preview is the daemon's own text and is printed as it comes: a summary cut short for reading, from which the daemon leaves out every hidden character of the titles, texts and reason it quotes, and when it left any out, its last line says how many (`This preview leaves out 2 hidden characters; the Memory view shows them as <U+…>.`). A suggestion's exact text is what `openalpaca memory show <id>` prints, each hidden character as `<U+XXXX>`.
  - On a terminal, `watch` resets the terminal's character set, text attributes, insert mode and line wrap before each prompt, and the `list` table does the same before its first line, so an earlier program cannot cut, garble or conceal what they print; `--format json` gets no reset. What the reset leaves alone is under [Approval prompts](#approval-prompts).

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
makes. Every hidden character the plugin's author put in it — in the name,
version, description, entry, capabilities or keys: a bidi override, a zero-width
or control character, a variation selector, any space but a plain one, an object
replacement character, a code point Unicode has not assigned or leaves to
private use, a line break or a tab — is shown as `<U+XXXX>`, and a line under
the manifest counts them (`Note: this request contains 2 hidden characters,
shown as <U+…>.`). On a terminal the manifest starts with the same reset as an
approval prompt (see [Approval prompts](#approval-prompts)). `install --dry-run`
and `update` print it the same way. The lines under the manifest — `ok installed
<id>`, the `ext approve <id>` it tells you to run, and `update`'s `ok updated
<id>` and `now also asks for:` — show the id and each capability the same way,
and the count under the manifest covers them: they are the manifest's own name
and capabilities. A warning `update` prints shows its control characters as
`<U+XXXX>` and is not counted.

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

Deleting a conversation also forgets its **episode notes** — the memory notes
that summarise it (see `memory` below).

Notes:
- The table is `ID`, `STATUS` (`active` / `archived`), `TITLE`, `WORKSPACE` and `UPDATED`, newest first. A conversation nothing has named yet — neither the daemon, which names one from its first sentence, nor a rename — prints `(untitled)`; one bound to no project prints `-`, which is the normal state for a connector lane.
- With no flags the list is the lane the CLI and the GUI share (`{user}:gui`) — the lane a `openalpaca chat` turn lands on, read from `GET /v1/me`. `--all` widens to every lane the daemon holds.
- `--workspace <path>` narrows to one project. The path is resolved the same way a turn's project is — to the nearest folder at or above it that is a recorded project's root or holds `.git` — so `--workspace .` works from anywhere inside a repository. An absolute path that is no longer there — a missing project's root, or a path inside it — still names that project; a relative one that is not there is an error. A directory in no project is an error, not a filter that matches nothing.
- An empty result says which kind of empty it is: no conversations at all, or none in the project that was filtered for.
- `delete <id>` deletes one conversation through `DELETE /v1/sessions/{id}`: its messages and the conversation itself, then its log directory under `~/.openalpaca/sessions/` — the transcript goes with the rows, which is what deleting a conversation means. What the conversation only *pointed at* survives, unpinned from it: its runs stay in `openalpaca tasks`, a queued follow-up stays on the lane, and the tool-call audit rows keep their history with only the session index cleared. It prints what went — title, id, message count and project. This is the counterpart to the GUI sidebar's delete, and the way to remove a conversation with no project, which `store purge` deliberately never touches.
- There is no `-y` on it: the one thing a confirmation could protect — the transcript a run is still writing into — the daemon already refuses (`409 SESSION_HAS_ACTIVE_WORKFLOWS`, cancel the run first), and another owner's conversation answers `404`.
- A conversation's **working state** — where its work stands: topic, goal, what is done, decided and open, the next step — has no verb here; `GET /v1/sessions/{id}` serves it (`working_state`), and the GUI draws it beside the chat.

### `store`

Your projects: list them, re-point one whose folder you moved, or delete
everything OpenAlpaca holds for one.

```bash
openalpaca store list
openalpaca store rebase /old/path/my-project /new/path/my-project --dry-run
openalpaca store rebase /old/path/my-project /new/path/my-project

openalpaca store purge /path/my-project          # prints the plan, deletes nothing
openalpaca store purge /path/my-project -y       # prints the same plan, then carries it out
```

A project is the git repository a conversation runs in — its top folder,
wherever inside it you are — or a folder you chose (`openalpaca chat --project
<dir>`, or `Use` in the app); a path belongs to the nearest of the two at or
above it. Each has a folder in the home store, `~/.openalpaca/projects/<name>/`,
named after its path: `project.json` (the project's permanent id and its path)
and the project's artifacts, uploads and memory notes. Nothing is written
inside the project's own folder by a daemon `openalpaca daemon start` or the
app launched, which start it in the home store; a daemon started somewhere
else still writes `file_write`'s plain files into the directory it was started
in (Daemon Manual → *Where an agent's files land*). A project is recorded the
first time the daemon accepts work for it, and is listed from then on.

`list` notes:
- Each project prints its path, then its folder under `projects/` and its id. A project whose path is no longer a directory is marked `missing — its folder is gone: re-point it, or delete its data`; its data is kept and its conversations still open. With no projects yet, it says how one is made.

`rebase` notes:
- Moving or renaming a project's folder on disk leaves it `missing` at its old path; nothing follows it on its own. `rebase <old> <new>` re-points it: the project's folder under `projects/` is renamed for the new path, and every row that names the old path — conversations, messages, runs, follow-ups and file records — is rewritten in one database transaction. The project's id does not change, so its memory notes come with it untouched. A re-point a crash interrupted is finished at the next daemon start.
- `<old>` is first matched, as written, against the roots the projects recorded, so the root `store list` prints names its own project even when it no longer leads to its folder — a folder renamed by case only, or moved with a link left at the old path that now reaches another project. Otherwise it is resolved the way a turn's path is, so a path inside the project names the whole project, and an old folder that is gone is matched as written against the recorded projects. `<new>` must be an existing directory. A relative path is taken from the shell's directory.
- `--dry-run` changes nothing. It reads both paths and prints what is recorded under the old one (artifacts, conversations, runs, messages, follow-ups), the folder that would be renamed, and the refusals it can foresee. It names runs queued there apart from those running or paused; the daemon refuses the re-point while either kind is there, as it refuses deleting that project's data.
- The daemon refuses rather than guesses, changing nothing: an old path that is no project of yours is a `404` `WORKSPACE_NOT_FOUND`; a run there that is queued, running or paused — counted again as the re-point starts, so one that starts while it waits refuses it too — or a memory write that holds the project for more than a few seconds is a `409` `WORKSPACE_BUSY` (retry once it finishes), and so is a project re-pointed or deleted by someone else while this one waited (list the projects again) or one whose own re-point a crash interrupted (the next daemon start finishes it; until then it can be neither re-pointed nor purged); a new path that is not a directory is a `422` `WORKSPACE_NOT_A_DIRECTORY` (move the folder first); a new path that is already a project of its own, that rows still name, or that is either path of another project's unfinished re-point, is a `409` `WORKSPACE_EXISTS` — delete that project's data first, or re-point onto another path, because two projects are never merged (for a path of an unfinished re-point the answer names instead the next daemon start, which finishes that re-point: its data cannot be deleted until then); a new path that is the home store, lies inside it or contains it is a `409` `WORKSPACE_IS_HOME` (an old path there is no project of yours: the `404` above); and the path the project already has is a `400` `INVALID_PATH`.
- On success it prints what moved and the project's new folder under `projects/`, then one line for the app: a window that still names the old path in Settings → Connection takes the new root with `Use`. A window that knows the project's id — it chose the project with `Use`, or re-pointed it, in this build — follows on its own and says so; one an earlier build set does not.
- The app's Settings → Connection → Projects → `Re-point…` does the same.

`purge` notes:
- **Without `-y` nothing is deleted.** The command prints the plan; `-y` prints the same plan and then carries it out. A direct `POST /v1/workspaces/purge` with no `dry_run` field is a dry run too.
- **`<project>` is a project's root, exactly as `store list` prints it** — matched as written, so it names its own project even when it now leads to another project's folder — or its folder as `store list` prints it, `projects/<name>` (`openalpaca store purge projects/-Users-me-code-my-project`). A path inside a project — `openalpaca store purge /repo/src` — is refused, and the answer names the project's root: one mistyped path never deletes a whole project's conversations and notes. A relative path is taken from the shell's directory. A project whose folder is missing, or that has nothing recorded yet, is deleted the same way.
- **What goes:** the project's conversations — messages, tool-call rows, queued follow-ups, and each one's log directory under `~/.openalpaca/sessions/` — and any message another conversation recorded while it ran in the project; its runs with their subagent spans and run events; its uploads and its by-products with every version; the memory notes of **every** owner in the project, with their control snapshots in `state/memory-control/`; the summaries of the deleted conversations in your global memory; and its folder under `projects/`. A follow-up queued from another project to run in this one goes too. The rows go in one transaction, then the files.
- **What is kept:** a file of the project that a message in a surviving conversation still shows — an upload, or a by-product as its newest version — is moved into the home store, under `uploads/kept/` or `artifacts/kept/`, and stays readable there; the plan counts them. The project's own folder is never touched: OpenAlpaca deletes only what it holds for the project.
- The plan names each entry with its retention class, the way the home store's `README.md` does:

```text
Would delete the data of /Users/me/code/my-project
  projects/-Users-me-code-my-project · id 3f6c2a1e-8b4d-4c7a-9e2f-5d1b0a9c8e7f
  delete  sessions/                                      3 conversations, 41 messages, 12 tool calls, 0 follow-ups — their logs live in the home store's sessions/
                                                         (size-capped, optional age sweep)
  delete  runs (database)                                2 runs, 5 subagent spans, 18 run events
                                                         (never swept — removed only when you ask)
  delete  projects/-Users-me-code-my-project/uploads/    4 uploads handed to this project's conversations
                                                         (swept: an upload attached to no message is deleted once past the grace period)
  delete  projects/-Users-me-code-my-project/artifacts/  9 by-products of this project's runs, with every version
                                                         (never garbage-collected — removed only when you ask)
  delete  projects/-Users-me-code-my-project/memory/     6 memory notes of every owner, in 1 partitions, with their history
                                                         (yours — never swept)
  delete  projects/-Users-me-code-my-project/            project.json — the project's id and path; OpenAlpaca forgets the project
                                                         (store metadata)
    keep  uploads/ (root store)                          1 uploads another conversation still shows, moved here under kept/
                                                         (swept: an upload attached to no message is deleted once past the grace period)
    keep  artifacts/ (root store)                        0 by-products another conversation still shows, moved here under kept/ as their newest version
                                                         (never garbage-collected)
    keep  /Users/me/code/my-project                      the project's own folder — OpenAlpaca never touches it
                                                         (yours)
  kept: 1 uploads and 0 by-products another conversation still uses, moved to the root store

Nothing was deleted. Re-run with -y to carry this out.
```

- With `-y` the same plan is followed by what was done — `deleted:` with every count, zeroes included, and `removed:` with the session-log directories, the memory partitions and the folder under `projects/`. If not every memory partition could be dropped, or removing the folder stopped part-way, the rows are deleted all the same but the folder under `projects/` stays with its `project.json`, the project is still listed, and `deleted:` counts only the notes that went; `removed:` says ``not the folder projects/<name> — run `openalpaca store purge <root> -y` again to finish (the daemon log says why)``. The same `store purge … -y` again finds no row left, drops the rest and removes the folder. The same line also answers a folder that was already gone: the daemon's answer does not tell the cases apart, and then `store list` no longer lists the project.
- The daemon refuses rather than guesses, deleting nothing: a path where no project of yours is recorded — one in no project, in the home store, or a git repository no conversation has used yet — is a `404` `WORKSPACE_NOT_FOUND`; a path inside a project is a `422` `WORKSPACE_NOT_A_ROOT` that names the project's root; a run there that is queued, running or paused — or a conversation with a run in flight — is a `409` `WORKSPACE_BUSY` (a queued run has not started yet, but it already named this project), and so is a project whose own re-point a crash interrupted (the next daemon start finishes it), and, with `-y`, a memory write that holds the project for more than a few seconds, a run that started while the delete waited for it, or the project re-pointed or deleted by someone else meanwhile. A conversation with no project is never touched: `openalpaca sessions delete` is the verb for one.
- A purge is not reversible and there is no undo.

### `memory`

The memory notes — the Markdown files under `~/.openalpaca/memory/owners/<owner-key>/notes/`
and, for a project, `~/.openalpaca/projects/<name>/memory/owners/<owner-key>/notes/` — from
the command line. Every verb but the two local ones goes through the daemon's
`/v1/memories*` routes, so it sees exactly what the app and the model see.

```bash
openalpaca memory list    [--kind K] [--status S] [--scope global|project] [--history] [--candidates] [--limit N] [--format json]
openalpaca memory search  <query> [--kind K] [--scope S] [--limit N] [--historical] [--as-of RFC3339]
                          [--since DATE] [--until DATE] [--order relevance|newest|oldest] [--format json]
openalpaca memory timeline <id> | --query Q | --date DATE  [--before N] [--after N] [--format json]
openalpaca memory show    <id> [--revision <rev>] [--raw]
openalpaca memory add     --kind K --title T [--scope S] [--alias A]... [--tag T]... [--fact-key KEY] [--supersedes <id>] [BODY | -]
openalpaca memory edit    <id>
openalpaca memory suggestions [--limit N] [--format json]
openalpaca memory confirm <id>
openalpaca memory undo    <id>
openalpaca memory forget  <id>... [--yes]
openalpaca memory sync
openalpaca memory doctor  [--repair]
openalpaca memory adopt   <owner-key>
```

**Which project.** Every daemon-backed verb sends the CLI's working directory as
the project, exactly as `chat` does, and the daemon resolves it as it resolves a
turn's — the nearest folder at or above it that is a recorded project's root or
holds `.git`; a write records the project, a read never does — so `openalpaca
memory list` inside a repository shows
that project's notes beside your global ones, and outside any project shows the
global ones alone. `--global` sends no project (the global partition only);
`--project <path>` names one instead of the working directory — the root a
project records, as `store list` prints it, still names that project when its
folder is gone, so a missing project's notes stay reachable; any other path
must exist, so a mistyped one is refused rather than sending a note meant for a
project to the global partition.

**Ids.** A note's id is a UUID; every verb that takes one accepts a unique
prefix (at least four hex characters). `show`, `list` and `search` print the
first eight.

**From a chat.** `/remember`, `/forget` and `/memories`, typed in `openalpaca
chat`, do the same three things without leaving the conversation — see
[Memory commands](#memory-commands).

Notes:
- `list` is the active notes by default. `--status candidate|archived` lists those
  instead; `--history` adds notes a newer note replaced; `--candidates` adds the
  extractor's unconfirmed inferences. The table's STATUS column is the *effective*
  status (a note listed as `superseded` still says `active` in its own file — being
  replaced is derived from the replacement, never written into the old note).
- `search` is literal: a term is matched as a whole word or as the beginning of
  one, so `prefer` finds `prefers` (the result says `prefix`); inside a word it is
  matched only in a title, an alias or a tag, or in Chinese, Japanese, Korean,
  Thai, Hindi, Tamil and similar scripts, so `presso` does not find a body's `espresso`;
  a one- or two-character query (a short Chinese term) is a substring scan. Results are cited — the heading and line
  range the excerpt came from, why it matched, and its freshness — and a result
  flagged `conflict` or `same-key loser` is one `doctor` also names. Candidates are
  never returned. `--historical` includes superseded, archived and ended-window
  notes; `--as-of` evaluates validity at that instant. `--limit` is 1–50, default 10.
  `--since` and `--until` keep notes *dated* in that range, both ends inclusive — a
  note's date is when what it records was observed (a learned note carries the
  time of the message it came from), else when it was first saved; each takes an
  RFC 3339 instant or a date `YYYY-MM-DD`, which is that whole UTC day. `--order
  newest|oldest` sorts by that date instead of by relevance.
- `timeline` shows what surrounds a note in time — the notes dated just before and
  just after it, oldest first — around exactly one of a note (its id or a unique
  prefix), the best match of `--query`, or `--date` (an instant, or a `YYYY-MM-DD`
  from its start). `--before` and `--after` take 0–20 (default 5). Unlike `search`
  it lists notes that no longer reach a chat — superseded, archived, or whose
  validity window has ended — with that status in the STATUS column (in yellow);
  the anchor is in bold. Candidates are never listed, and a timeline is not drawn
  around one. It is the model's `memory_timeline` over `GET
  /v1/memories/timeline`. While the daemon is rebuilding its search index the
  timeline is refused at once (`503 MEMORY_INDEX_REBUILDING`) — `search`
  answers meanwhile; try the timeline again in a minute.
- `show` prints the note with its provenance — its key, aliases, tags and validity
  bounds among its facts; `--raw` prints the note *file* as the
  daemon holds it, frontmatter and all; `--revision <rev>` reads one of the last
  five revisions the daemon wrote (a hand edit is not one of them) — `--raw` of a
  revision is that revision's file.
- What a note holds is printed so it cannot drive your terminal. Every title,
  heading, alias and tag — in `list`, `search`, `timeline`, `show`, the lines
  `confirm`, `undo` and `forget` print, and an ambiguous id's list — is one
  line with no control character (an escape sequence), no Unicode format
  character (a bidi override, a zero-width character) and nothing else that
  draws as nothing or as blank space (a variation selector, a braille blank, an
  object replacement character, a code point Unicode has not assigned or leaves
  to private use). `search`'s excerpt lines drop the control and format
  characters too but keep their own spacing and tabs; `show`'s body drops
  control characters only and keeps its line breaks, tabs and format characters,
  since it is the note's whole text. `--raw` prints the file unchanged.
- Where you decide, nothing is dropped: `suggestions` and `show` of a
  **candidate** (a note waiting for `confirm`) show every hidden character — a
  bidi override, a zero-width or control character, a variation selector, any
  space but a plain one, an object replacement character, a code point Unicode
  has not assigned or leaves to private use, and in a title, alias, tag, fact
  key, reason or changed value also a line break or a tab — as `<U+XXXX>`, and a line counts
  them (`Note: this request contains 2 hidden characters, shown as <U+…>.`):
  under each suggestion that holds any, and under a candidate's text, where it
  counts the title, aliases, tags, fact key and text — as the GUI's candidate
  detail does, which also counts the title of the note it would replace. A
  suggestion's count includes both notes' texts, which its line does not print,
  and one line under the list says so; `show <id>` prints the suggestion's own,
  and `--format json` prints both, each hidden character as a JSON `\u` escape.
  On a terminal both start with the same reset as an approval prompt (see
  [Approval prompts](#approval-prompts)).
- `add` saves a note **as you**: its origin is `user`, the highest authority, and a
  tool call can never edit, archive or replace it (the model can only *forget* it,
  under the ordinary confirmation card). The body is the last argument, `-` for
  stdin, or — with neither — an editor (`$VISUAL`, else `$EDITOR`, else `vi`) opened
  on a blank body; an empty body saves nothing. `--scope` defaults by kind: a
  `preference` is global, everything else goes to the project when there is one.
  `--supersedes <id>` replaces a note: the target's current revision is fetched
  first and sent with the new note, so a target that changed meanwhile is a `409`,
  never an overwrite; the old note is untouched and reads as superseded from then
  on. An exact duplicate (same kind, title, body, fact key, aliases and tags) is
  answered with the existing note's id, not saved twice.
- `edit` opens the note file in the editor — the same bytes `show --raw` prints —
  and sends back only the fields that changed: the title, the body, `aliases`,
  `tags`, `fact_key`, `valid_from`, `valid_to`, `review_after`. Changing anything
  else (`kind`, `status`, the timestamps, `pinned`, `sources`, `supersedes`, or a key
  the note did not have) is refused by name before anything is sent, and your text
  is kept. The save carries the revision that was fetched: if the note changed
  while the editor was open — the app, a chat turn, the extractor — the daemon
  answers `409`, the command prints both revisions and **keeps your text in a
  named temp file** (`openalpaca-memory-<id8>-….md`); run `show`, merge, and edit
  again. An editor session that changes nothing sends nothing. An edit through
  this verb re-records the note as yours (`user`), whatever wrote it before.
  A note file holds at most 32 KiB, frontmatter included, so a read always
  carries the whole body: a file you have made longer by hand does not parse,
  `doctor` lists it as malformed (`oversized`), and every other verb treats its
  id as unknown until you shorten the file. Should a read ever come back cut
  (`truncated`), `edit` refuses before the editor opens — saving would replace
  the whole note with its first 32 KiB — prints
  ``This note is longer than 32 KiB, so `openalpaca memory edit` would save only its first 32 KiB. Edit the file itself: <path>``
  (the note's file, relative to its partition's directory) and exits 1, having
  sent nothing.
- `suggestions` lists what learning is waiting on you for. When something you say
  changes a note **you saved** (or edited by hand), the extractor does not overwrite
  it: it writes a *suggestion* — a candidate naming the note it would replace, left
  out of answers until you decide. One line each, newest first, one per note:
  `<id8>  replace “X” (saved by you) → “B”  from your words, 2026-09-24 — <reason>`,
  with `(+N older)` when several restatements named the same note, and `inferred
  from the conversation` for one read out of an answer or a tool's output rather
  than your own words. The assistant's own change to a note you saved — which
  it may not apply itself — is a suggestion too, and reads `suggested by the
  assistant, <date>`, with no reason (its text may copy your note's, whose reason
  is that note's). A suggestion that changes more than the title and the text
  says what on a line under its own — `changes: tags none → tools; valid until
  none → 2026-12-31T00:00:00Z` (the key, aliases, tags and validity bounds, your
  note's values first) — so a change of those alone is not two identical texts.
  A suggestion whose own text changed after it was written
  says who wrote it now: `edited by you, suggested <date>` (or `edited outside
  OpenAlpaca`, `imported`). A change to a note that was itself *learned* needs no
  question: it replaces the old note at once, and `undo` brings the old one back.
- `confirm` makes a candidate an active note of yours. For a suggestion it also
  reads the note the suggestion would replace, at that moment, and sends that
  revision with the confirm — so it replaces the note **as it stands when you run
  it**, including an edit you made after the suggestion was written (`show` the
  suggestion and the note it names first to read both); only a change landing between
  that read and the confirm is a `409`. One already replaced or archived some
  other way is refused, since nothing is left to replace (`undo` the suggestion
  instead); then it prints what the suggestion replaced — kept
  as history, readable with `show` — and any other suggestions for the same note,
  which the confirm dismissed. It is refused for a note that is not a candidate.
- `undo` takes back what learning did, and forgets nothing else. On a *recently
  learned* note — one the extractor wrote from your own words (origin
  `extracted_user`), which the app lists under "Learned from what you said" — it
  removes that note, and if the note replaced an older one, the older one is back.
  On a suggestion (or any candidate) it dismisses it: *keep mine*, and your note
  stays exactly as it was. Either way it is a withdrawal, not a forget: the message
  the note came from is not suppressed, so anything else learned from that message
  stays learned. Once you have edited or confirmed a note it is yours, `undo` is
  refused (`409 MEMORY_NOT_UNDOABLE`), and `forget` is the verb.
- `forget` prints the title and revision of every note named, asks once (off a
  terminal it refuses and says to pass `--yes`), then forgets them with
  `POST /v1/memories/forget` — in requests of up to 20 ids, the pending
  suggestions among them sent first — each with the revision it printed, and
  prints each one's outcome as its request returns. A forgotten note's files, history copies and index rows are
  gone and its sources are never re-extracted (a pending suggestion's or
  candidate's, but for the messages and runs a note you saved, or the note it
  would replace, cites — below); the transcript messages it cited
  stay in the conversation, and the command says so. Any suggestion that would
  replace a forgotten note is dismissed with it — but one you name in the same
  command is forgotten as asked, whatever the order (the daemon, too, takes
  the pending suggestions of a request first). Forgetting a pending suggestion
  or candidate leaves the note it would replace in force, whoever saved it,
  and every note you saved — such as a candidate from the same run that you
  have since confirmed: it suppresses none of the messages or runs those notes
  cite. If the daemon refuses a request, the command stops with that refusal,
  but first says which of that request's ids were already forgotten — the
  daemon forgets them
  in turn, the pending suggestions first and each of them even after one fails,
  so those and the ids before the one that failed may be gone — and how many
  ids after it were never sent. There is no undo. An
  **episode note** is the exception on sources: forgetting one forgets that note
  alone, and what its messages taught stays learned.
- **Episode notes.** After a conversation has been quiet for 30 minutes, when you
  switch conversations and before the daemon stops, a conversation with at least
  4 new turns of yours gets an `episode` note, `Conversation on <date>: <topic>`
  — what was discussed, decided and left open, written by the assistant (origin
  `extracted_other`) and active. `list --kind episode`, `search`, `timeline` and
  the model's memory search find it; it is never recalled into a chat on its
  own. It is written by a model call of its own on `[orchestrator.costs]
  summary_model`, under `summary_max_daily_cost_usd`, after the conversation's
  working state is brought up to date.
- `sync` runs one reconciliation scan now: the daemon re-hashes the note files,
  records any edited, added or removed behind its back as `external`, and prints
  what changed. The daemon also scans every 30 s on its own, and before every read,
  so `sync` is for when you want the answer now.
- `doctor` reports every partition the store knows: yours (global and the current
  project), every other owner key's as an **orphan**, health (`ready`, `absent`, or
  `quarantined` with why), note, unverified and tombstone counts, malformed files,
  duplicate ids, conflicts (two active notes with the same fact key, or a
  replacement link the replacing note lacks the authority for), and each record an
  earlier build kept for a project with no project id — never opened, adopted or
  deleted, and harmless (see the Installation Manual's switch-over note). With a
  daemon running it asks the daemon; with none it reads the store directly (and
  opens the database for your owner id). Either way it changes nothing.
- `doctor --repair` is **local and takes the store lock**: it refuses while a daemon
  runs (`openalpaca daemon stop` first). It rebuilds a missing or invalid control
  snapshot from the note files of your global partition and the current project's,
  lifting the quarantine — every note is recorded at its current bytes as
  `external`, tombstones are empty, and a duplicate id refuses the repair naming
  both files. It never rewrites,
  moves or deletes a note file. Another owner key's partition is an orphan and not
  repaired; `adopt` is the verb for that.
- `adopt <owner-key>` is likewise local and takes the lock. After a factory reset
  the next daemon start mints a new owner id, and every partition of the old key —
  the global one and each project's — is left as an orphan with its notes,
  tombstones and revisions intact. `adopt` re-binds all of them to the current
  owner in one command, renaming each `owners/<old-key>` directory and its control
  snapshot; if the current owner already has a partition where one would land, the
  whole command is refused naming the collision, and nothing is partially adopted
  (merging two owners' notes is not an operation). A project's partition is found
  by the project's id, so one re-pointed since is adopted at its new folder; a
  record an earlier build kept with no project id is left as it is. The daemon
  rebuilds its search index for the adopted partitions at its next start.

Refusals come from the daemon in its own words: `404 MEMORY_NOT_FOUND`,
`409 MEMORY_REVISION_CONFLICT` (the note changed since it was read; the current
revision is in the message), `409 MEMORY_QUARANTINED` (its partition's snapshot is
missing or invalid — `doctor --repair`), `409 MEMORY_NOT_UNDOABLE`,
`400 MEMORY_INVALID_INPUT`, `409 MEMORY_BUSY` (a write on the partition is in
flight — retry), `503 MEMORY_INDEX_REBUILDING` (`timeline` only, while the search
index is being rebuilt — retry in a minute), `500 MEMORY_INTERNAL` (the daemon's
store or database refused; the daemon log has the cause).

### `persona`

`USER.md` — what every conversation is told about you — from the command line,
through the daemon's `GET|PUT /v1/persona/user`.

```bash
openalpaca persona show user
openalpaca persona edit user
```

Notes:
- `USER.md` is yours to write. The daemon's background learner no longer adds
  what it picks up from your conversations to it — that becomes a memory note
  (see [`memory`](#memory)) — so the file holds only what the onboarding
  conversation put there and what you write. The lines an older build's learner
  wrote are still in it until you delete them.
- `show` prints the file as the daemon holds it.
- `edit` opens it in `$VISUAL`, else `$EDITOR`, else `vi`, and saves it back
  with the revision it fetched. If the file changed while the editor was open —
  the onboarding conversation's `update_persona`, or the app's Settings →
  Persona — the daemon answers `409 PERSONA_REVISION_CONFLICT`, the command
  prints both revisions and **keeps your text in a named temp file**; merge and
  edit again. An editor session that changes nothing sends nothing. A document
  `update_persona` would refuse is `400 PERSONA_INVALID`.
- `user` is the only document with an editor; `soul` and `identity` are refused
  by the argument parser.

### `chat`

Interactive or one-shot chat through daemon orchestrator.

```bash
openalpaca chat
openalpaca chat --message "hello"
openalpaca chat --message "summarize these" --file a.txt --file b.png
echo "hello" | openalpaca chat

# Work in a folder with no git, or name the project explicitly
openalpaca chat --project ~/notes

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
- The REPL handles these slash commands itself: `/help`, `/model`, `/models`, `/agents`, `/keys`, `/usage`, `/clear`, `/verbose`. Every other slash line goes to the daemon — see [Slash commands](#slash-commands) and [Memory commands](#memory-commands).
- Piped stdin that is empty (or only whitespace) sends nothing and exits **1** with `No input on stdin` — a pipe that produced nothing is a mistake upstream, not a request to send an empty turn.

#### Attachments

- `--file <PATH>` is repeatable and requires `--message`. Attachments are not supported in the REPL or in pipe mode.
- Each file is uploaded before the turn is sent. The CLI prints `Uploaded: <name> (<file id>)` on stderr for each one.
- A file is uploaded into the turn's project: the working directory's, or `--project`'s — or, with `--resume` or `--session`, the resumed conversation's own project when it has one, since its turns run there (below).
- The daemon's default limits are 10 files per message and 50 MB per file (`max_files_per_message` and `max_file_size_bytes` under `[upload]` in `daemon.toml`). A message with too many files is refused with `TOO_MANY_ATTACHMENTS`.
- What reaches the model depends on the model that answers. An image needs a model with image input. A document still travels as its extracted text to a model with no native document input; it is skipped only when no text could be extracted. An audio clip needs a model with audio input: nothing transcribes it, so any other model skips it.
- **A file that does not reach the model is never dropped silently.** After the answer, the CLI prints one line per skipped file on stderr, with the daemon's reason:

```text
Attachment <file id> did not reach the model: the answering model does not support image input
```

- Some turns cannot carry files at all, and report every attachment as skipped the same way: a task command such as `/status`, a `/steer`, a memory command such as `/remember` (it saves only the text you typed), or a skill that comes from a plugin (it receives the question as plain text).

#### Project and conversation

- Every turn carries the CLI's working directory as its project (`x-workspace-path`), the same way the GUI sends the project chosen in its window. The daemon resolves it — to the nearest folder at or above it that is a recorded project's root or holds `.git` — and records the project on that turn; that root is what a run records as its `workspace_id`, and the files the turn writes land in the project's folder under `~/.openalpaca/projects/`, never in the project itself — with a daemon `openalpaca daemon start` or the app launched, which start it in the home store. A daemon started somewhere else (`cargo run -p openalpacad` in a checkout) still writes `file_write`'s plain files into the directory it was started in, as the Daemon Manual's *Where an agent's files land* explains. A directory the CLI cannot canonicalize sends no project at all rather than a path the daemon would resolve against its own directory.
- `--project <dir>` chooses the project instead of the working directory: the CLI first asks the daemon to record `<dir>` (`POST /v1/workspaces`) — the nearest recorded project or git repository at or above it, else the folder itself, which is how a folder with no git becomes a project — and every turn of the chat carries the root the daemon answered. A refusal (the home store, a path that is not a directory) is printed and nothing is sent.
- `--resume` continues a stored conversation instead of the lane's current one. `--session <id>` names one directly; `openalpaca sessions` lists the ids. The two are mutually exclusive.
- `--resume` is scoped to **this project**: the conversations bound to the working directory's own root (or the root the daemon answered for `--project`, taken as answered), newest first. The lane is shared with the GUI and with every other checkout, so a lane-wide list would offer another project's conversations.
  - With a terminal it opens a picker over them.
  - With stdin piped it takes the most recent — the row the picker would have opened on — because a prompt written into a pipe is a hang, not a question.
  - A project with no conversations yet says so and names itself, rather than reaching for another project's.
  - From a directory in no project the list stays lane-wide, which is the same scope such a turn itself has.
- `--session <id>` only continues a conversation on the lane the CLI talks on. One on another lane (a connector's) is refused here, before anything is sent, naming both lanes: activating it would archive that lane's own live conversation and the turn would still be refused (`409 SESSION_LANE_MISMATCH`). `openalpaca sessions --all` is what lists those.
- Resuming **re-opens** the conversation (`POST /v1/sessions/{id}/activate`) before anything is sent. A lane holds exactly one live conversation, so resuming an archived one archives whatever was live. The CLI prints the conversation it resumed, and its project, so that is visible rather than discovered later. The last 8 messages are printed before the prompt opens.
- A resumed conversation's own project governs the turn, and its `--file` uploads, overriding the working directory: one conversation belongs to one project. A conversation that has no project yet takes the working directory's and is bound by it.

#### What is printed

On a **terminal**:

- The reply is labelled `Alpaca: ` and streams as the model writes it — the daemon forwards the provider's own tokens.
- **A thinking model's reasoning is shown dimmed**, as it arrives, and the answer then starts on its own line. A local reasoning model can spend ten seconds or more thinking before its first token; this is what fills that silence. Reasoning is never part of the answer: nothing stores it, and `openalpaca chat --resume` will not replay it.
- **The turn ends on the authoritative answer.** The streamed pieces do not always add up to the final answer: a turn that calls a tool streams the text written before the call, and a stream that breaks mid-way is answered by a non-streaming fallback. When the turn is done the CLI reconciles — it appends what is missing, or, when the stream diverged, prints the final answer on a fresh line — so the terminal always ends with the answer, exactly once.
- **A conversation that carries the previous one says so.** On a new conversation's first turns (three by default), the daemon gives the model the previous conversation on this project — its working state and last exchange — and the CLI prints one dim line under the answer: `continuing from: <topic>` (`continuing from: the previous conversation` when it left no state). The topic is the summariser's text, so a hidden character in it is shown as `<U+XXXX>`. Nothing is printed into a pipe. See the Daemon Manual, *Conversation continuity*.
- A usage line follows the answer: `[<model> | <tokens> in | <tokens> out | <ms>ms]`. The model named is the one that answered, which can differ from the configured one when the fallback ladder stepped in.
- **The model's text cannot drive the terminal.** Every control character except a line break and a tab — an escape sequence, a carriage return — is shown as `<U+XXXX>` instead of being acted on, in the answer and the reasoning, in a workflow's `Result:`, `Error:` or `Detail:` line, in the turns `--resume` or `--session` prints, in a failed turn's error message (which can quote a tool's own error text), and in the model's name on the usage line and in the REPL's `/models` (a provider's catalogue names it: a remote Ollama or a gateway lists whatever it likes). An escape sequence could otherwise leave the terminal in a state the next approval prompt inherits: line wrap turned off, for example, cuts an argument at the right margin, and a scroll region or a changed palette colour can hide the prompt's first lines. On a pipe the text is written exactly as the daemon sent it; the error message follows the same rule for stderr.

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

- **With a terminal on both stdin and stdout** — the REPL, or a `--message` typed at a prompt — the confirmation arrives on the stream the CLI is reading and it asks inline: `Allow execution? [y/N]`. `y` or `yes` approves; anything else denies. When the daemon wrote a preview for the card, it is printed first, under `From OpenAlpaca:`, and the JSON arguments after it. Every hidden character in the tool name and the arguments (a bidi override, a zero-width or control character, a variation selector, any space but a plain one, an object replacement character, a code point Unicode has not assigned or leaves to private use, and in the name also a line break or a tab) is shown as `<U+XXXX>`, and a line under the arguments counts them (`Note: this request contains 2 hidden characters, shown as <U+…>.`). Those are Unicode's classes, not what your font draws: a letter that a particular font happens to draw as an empty cell, such as the Bamum letter U+16910 in JetBrains Mono, is printed as it is and not counted. Before the prompt the CLI resets the terminal's character set, text attributes (colour, bold, conceal, reverse video), insert mode and line wrap, which an earlier program could have left set; `tasks confirmations watch` does the same before each prompt, and `tasks confirmations list` before its table. A scroll region, the cursor position, palette colours or a switch out of UTF-8 made by an earlier program in that terminal are not reset (resetting the palette would also undo a colour theme you set at run time). None of these can come from what `openalpaca chat` prints of the model's or a tool's text (see [What is printed](#what-is-printed)); `openalpaca tasks log` and `tasks status` print a run's title and result as stored, and can be that earlier program.
- **A suggestion raised in chat.** On such a turn the assistant may ask whether a suggestion that cites a message you typed — one learning took from your own words, or one the assistant saved itself in chat — should replace a note you saved — the tool `memory_confirm`, at most one per turn. The preview says which note, your text, the new text and where it came from: "from what you said on <date>", or "suggested by the assistant on <date>" (the day it saved it). `y` makes the new note yours, exactly as `openalpaca memory confirm` does; anything else denies, and a denial dismisses the suggestion for good, as `openalpaca memory undo` does. Nothing but your answer confirms it: the model's own say-so, or `[security] auto_approve_confirmations`, never does. A one-shot or piped turn is never offered the tool.
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
- In the REPL, only the client-side commands (`/help`, `/model`, `/models`, `/agents`, `/keys`, `/usage`, `/clear`, `/verbose`) are handled locally. Every other slash line — the daemon commands here, the [memory commands](#memory-commands), and anything unrecognized (which may be a skill command) — is forwarded to the daemon as a chat message, so `/steer focus on the tests` works directly at the prompt. `/help` lists them and Tab completes them.
- One-shot mode works too: `openalpaca chat --message "/steer focus on the tests"`.
- A skill's answer streams like any other turn. A skill contributed by a plugin cannot stream: its answer arrives in one piece.

#### Memory commands

Three chat commands change and read your memory notes **with no model involved**: the daemon answers them itself, like `/status`. Because you typed the text, `/remember` saves it as a note **you wrote** (origin `user`) — what `openalpaca memory add` and the app's Memory view save, not a note learned or written by the assistant.

| Command | What it does |
|---|---|
| `/remember [--kind <kind>] [--global \| --project] <text>` | Saves `<text>` as a note. The title is its first line, cut at 80 characters; the body is the whole text, as typed. `--kind` takes `fact` (the default), `preference`, `decision`, `procedure`, `reference` or `episode`. The scope follows the kind: a `preference` is global, the other kinds go to the project of your working directory (the global notes when there is none); `--global` or `--project` says which instead, and `--project` outside any project is refused. `--` ends the options, for a text that starts with dashes. The same text twice answers the note already saved. |
| `/forget <id>` | Forgets that note — a full id or a unique prefix of at least 8 hex characters, as `/memories` prints them. A prefix several notes share lists them, with their full ids, and forgets nothing. |
| `/forget <words>` | Forgets **nothing**: lists up to 5 notes that match, one line each, so you can send `/forget <id>`. |
| `/memories` | Your 10 most recently updated notes, and how many suggestions wait in the Memory view (`openalpaca memory suggestions`). |
| `/memories <words>` | Up to 8 notes that match, with why each matched. |

```text
you: /remember --kind preference I review diffs in a split view
Alpaca: Saved as a note you wrote: “I review diffs in a split view” (3f9a1c2d, global). It is used from your next message.
you: /memories diffs
Alpaca: 3f9a1c2d  I review diffs in a split view  [preference/global]  (matched: term)
you: /forget 3f9a1c2d
Alpaca: Forgot “I review diffs in a split view” (3f9a1c2d). The note files, their history and their index rows are gone, …
```

- A line is `<id>  <title>  [<kind>/<scope>]`, at most 160 characters, and at most 10 notes are listed. No note's body is printed: `openalpaca memory show <id>` reads one.
- A forget here is a forget like `openalpaca memory forget`, with no second prompt — typing the id is the say-so: the note file, its history and its index rows go, and the answer says what it leaves behind (conversation messages and session logs are separate data).
- **Only where you can read the answer.** The commands work in the REPL and in a `--message` typed at a terminal. A piped or redirected `openalpaca chat` is unattended (see [Approval prompts](#approval-prompts)), and the daemon answers ``Memory commands work only in the OpenAlpaca app's chat, or in `openalpaca chat` run at a terminal — not with its input or output piped.`` and changes nothing — as it does on a connector (Telegram, Discord, iMessage) and for a scheduled skill.
- A memory command's answer is not learned from: learning leaves out every `/`-line and its answer.
- The three names are reserved: a skill whose slash command is `/remember`, `/forget` or `/memories` is not reached by it (the daemon logs one warning naming the skill when it loads it).

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
