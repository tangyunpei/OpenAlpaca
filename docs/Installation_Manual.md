# OpenAlpaca Installation Manual (macOS, No Cargo Required on Target)

This manual describes the package flow for OpenAlpaca on macOS:

- Build machine: creates a distributable archive.
- Target machine: installs and runs without Cargo.

There is no hosted download; you build the archive yourself. In a hurry? The
[QuickStart](QuickStart_Manual.md) is the same flow in five steps.

Related docs:

- [CLI Manual](CLI_Manual.md)
- [GUI Manual](GUI_Manual.md)
- [Daemon Manual](Daemon_Manual.md)

## Scope

- Platform: macOS (`aarch64-apple-darwin` and `x86_64-apple-darwin`)
- Components included:
  - `openalpaca` CLI
  - `openalpacad` daemon
  - `openalpaca-gui.app`
- Installer mode: user-level install (no `sudo` required)

Linux and Windows have equivalent scripts (`scripts/release/package-linux.sh`,
`package-windows.ps1`, `install-windows.ps1`, `uninstall-windows.ps1`), and
`install.sh` / `uninstall.sh` themselves run on both macOS and Linux. See
[Other Platforms](#other-platforms) below — Windows is experimental.

## Build Machine Requirements

On the machine that builds release artifacts, install:

- `cargo` / `rustc`
- `bun` / `bunx`
- `tar`, `shasum`, `git`
- Standard Unix tools: `awk`, `sed`, `date` (present on any macOS machine)

The script checks for each of these and stops with the missing command's name.

## Build Release Artifact

From repository root:

```bash
./scripts/release/package-macos.sh
```

It must run on macOS, and it packages for the host architecture only. It builds
the two release binaries and the Tauri app bundle itself.

Expected output:

- `dist/openalpaca-macos-<target>-v<version>.tar.gz`
- `dist/openalpaca-macos-<target>-v<version>.tar.gz.sha256`

Package contents:

- `bin/openalpaca`
- `libexec/openalpacad`
- `gui/openalpaca-gui.app`
- `config/` (safe templates only, staged from `scripts/release/templates/config/` — never the repo runtime config)
- `install.sh`
- `uninstall.sh`
- `manifest.json` — build metadata: `name`, `version`, `target`, `built_at_utc`, `git_sha`

Note: binaries and the `.app` bundle are **not codesigned**. macOS Gatekeeper
will show "unidentified developer" warnings; the installer works around this by
stripping the quarantine attribute (see Troubleshooting).

## Install on Target Machine

### Option A: Install from local file

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<version>.tar.gz
```

If a `<archive>.sha256` file sits next to the archive, it is used for checksum
verification.

### Option B: Install from URL

```bash
./scripts/release/install.sh --url https://example.com/openalpaca-macos-<target>-v<version>.tar.gz
```

Requires `curl`. The installer also tries to fetch `<url>.sha256` and, when
found, verifies the checksum before installing.

### On a machine without the repository

The archive carries its own `install.sh`, so the archive is all the target
machine needs:

```bash
tar -xzf openalpaca-macos-<target>-v<version>.tar.gz
./openalpaca-macos-<target>-v<version>/install.sh --file ./openalpaca-macos-<target>-v<version>.tar.gz
```

### Options

Exactly one of `--file` or `--url` must be given. Optional arguments:

- `--prefix <dir>` (default: `~/.local/openalpaca`)
- `--app-dir <dir>` (default: `~/Applications`; macOS only, ignored on Linux)
- `--yes` (non-interactive overwrite)
- `-h` / `--help` (print usage)

Installer behaviors:

- Validates host architecture vs package target (from `manifest.json`).
- Verifies SHA256 when available.
- Asks before overwriting an existing install, unless `--yes` is given.
- Stops running daemon if found (via the pid in `discovery.json`).
- Preserves user data/config and only replaces app/program files. Config
  templates are copied into the runtime config dir only for files that do not
  already exist — existing config is never overwritten.
- Appends PATH block to `~/.zshrc` and `~/.bashrc` (idempotent). With any other
  shell it tells you to add `~/.local/bin` to your PATH yourself.
- Uses `jq` for manifest parsing when available, with a grep/sed fallback —
  `jq` is optional.

## Installed Paths

Defaults after install:

- CLI binary: `~/.local/openalpaca/bin/openalpaca`
- Daemon binary: `~/.local/openalpaca/libexec/openalpacad`
- CLI symlink: `~/.local/bin/openalpaca`
- GUI app: `~/Applications/openalpaca-gui.app`
- Runtime root: `~/.openalpaca`
- Runtime config: `~/.openalpaca/config`
- Runtime DB: `~/.openalpaca/state/openalpaca.db`

Inside the runtime root, `state/` is the machine's (database, logs, lock,
master key, caches) and everything else is yours to read and edit. The root
describes itself: the daemon seeds a `README.md` there that explains every
entry.

When the CLI or the desktop app launches the daemon, it sets
`OPENALPACA_CONFIG_DIR` to the runtime config directory above — that is the
config the installed daemon actually reads (not any repo checkout).

The whole runtime root can be moved with `OPENALPACA_HOME_STORE`; see
[Runtime Overrides](#runtime-overrides).

## If You Have Data From an Older Build

Builds before this one kept everything under
`~/Library/Application Support/OpenAlpaca` (macOS),
`~/.local/share/openalpaca` (Linux) or `%APPDATA%\OpenAlpaca\data` (Windows),
and moved that directory into `~/.openalpaca` on their first boot. **This build
does not move anything.** OpenAlpaca was never released, so no installed copy
ever wrote to the old location; if you have one, it is from a development build.

What happens instead:

- If an old directory holds a database, a `.master_key` or a `config/`
  directory **and** `~/.openalpaca/state/openalpaca.db` does not exist yet, the
  daemon **refuses to start**. Starting would create an empty database and leave
  your conversations, memories, tasks and encrypted config with nothing pointing
  at them. The error names both directories and lists exactly what to move
  where. Nothing is changed and nothing is lost — it is a refusal, not a
  failure.
- If an old directory is still there but `~/.openalpaca` already has its own
  database, you get one warning per boot naming both paths. Nothing reads the
  old directory; move or delete it yourself and the warning stops.
- `openalpaca config` — the one CLI command that opens the database directly
  instead of asking the daemon — applies the same rule and exits with the same
  message. Every other `openalpaca` subcommand talks to the daemon over HTTP
  and is unaffected in itself, though it still needs a daemon that will start.

To carry old data over by hand, with no OpenAlpaca process running:

| From the old directory | To |
|---|---|
| `openalpaca.db`, `openalpaca.db-wal`, `openalpaca.db-shm`, `.master_key` | `~/.openalpaca/state/` |
| `config/`, `plugins/` | `~/.openalpaca/` (merge into what is there) |
| `assets/` | **leave it where it is** |

`assets/` stays put because uploaded files are recorded by absolute path:
moving that directory breaks every row that points into it. Everything else in
the old directory — logs, `discovery.json`, `openalpacad.lock`, `repl_history` —
is regenerated and can be discarded.

## Run and Verify

```bash
openalpaca --help
openalpaca daemon start --daemon-only
openalpaca daemon status
openalpaca gui start
```

`openalpaca daemon start` without `--daemon-only` starts the daemon and opens
the desktop app in one step. The desktop app works on its own too: opened
directly, it uses the running daemon, or starts the copy bundled inside the app
when none is running.

### What the first boot writes

A first boot writes the content the daemon carries in its own binary into
`~/.openalpaca/config`: `llm.toml`, `daemon.toml`, `mcp.toml`, the nine agent
templates (`agents/`), the skills (`skills/`) and the tool config (`tools/`).
The rule is per directory: a directory that already exists is left
alone entirely, even one you emptied on purpose, and inside a directory being
filled an existing file is never overwritten. Without the templates the first
workflow request has no lead agent to run and says so.

It also creates the persona documents in `~/.openalpaca/config/orchestrator/`
— `SOUL.md`, `USER.md`, `IDENTITY.md` and `BOOTSTRAP.md` — and downloads the
local embedding model (see [The embedding model](#6-the-embedding-model)).
That download is why the first start is the slow one: the daemon answers no
request until it is done, so give `openalpaca daemon status` a few minutes to
report `Daemon is running`.

### Connect a model

The seeded `llm.toml` has **every provider switched off**, so a fresh install
cannot answer until you enable one:

- **Local, no API key:** follow [Local Models (Ollama)](#local-models-ollama).
- **Cloud (`anthropic` or `openai`):** switch the provider on, then add a key — in that order.

  ```bash
  openalpaca config set ai.anthropic.enabled true
  openalpaca llm keys add --provider anthropic     # prompts for the key, a source and a note
  ```

  Both take effect without a restart. If you added the key first and
  `openalpaca llm status` shows no usable cloud model, run
  `openalpaca llm models --refresh` or restart the daemon. The desktop app has the on/off switch
  (Settings → Models & keys) but no key editor yet, so keys go in through the
  CLI. See [CLI Manual → `llm`](CLI_Manual.md#llm) for the other key commands.

`openalpaca llm status` shows the result, and names the fix when nothing is
routable.

### Onboarding comes first

While `~/.openalpaca/config/orchestrator/BOOTSTRAP.md` exists, the assistant is
getting to know you. During onboarding it is offered `update_persona` and the
few builtins your message's keywords suggest — not `start_workflow`, memory,
MCP servers or plugins. Ask it for a workflow at this point and it has no tool
to start one with.

- Answer its questions. Once `USER.md` and `IDENTITY.md` have content, the
  daemon deletes `BOOTSTRAP.md` and the ordinary tool surface is back on the
  next turn.
- Or delete `BOOTSTRAP.md` yourself; the running daemon picks that up without
  a restart. While either document is still empty, the next daemon start
  recreates the file.

## Local Models (Ollama)

OpenAlpaca can run entirely on models served by an [Ollama](https://ollama.com)
you run yourself. **No API key is involved anywhere**, and a local model is
priced at zero, so the cost caps never bite.

### 1. Install Ollama and pull a model

```bash
ollama pull <model>          # e.g. a tools-capable chat model
ollama list                  # the tags the daemon will discover
```

Ollama serves on `http://localhost:11434`; the daemon's default `base_url` is
that address plus the OpenAI-compatibility suffix,
`http://localhost:11434/v1`. If yours listens elsewhere, change
`[providers.ollama] base_url` in `~/.openalpaca/config/llm.toml`, or run
`openalpaca config set ai.ollama.base_url <url>`.

### 2. Turn the provider on

Enabling it is the only action required — the seeded `llm.toml` ships
`[providers.ollama] enabled = false` and everything else already set.

- **GUI**: Settings → Models & keys, the `ollama` row's switch. The row reads
  `no key needed` where a cloud provider counts its keys, and the toast reports
  how many models the daemon found.
- **CLI**: `openalpaca config set ai.ollama.enabled true`. It is the same
  switch, written through the config schema (`ai.<provider>.enabled`, backend
  `llm.toml`), so the key name and value are validated rather than hand-typed.
  No key is asked for. The interactive `openalpaca config` TUI has the same row
  under API-Keys → Ollama.
- **By hand**: set `enabled = true` under `[providers.ollama]` in
  `~/.openalpaca/config/llm.toml` and save.

All three write the same file, and the config watcher registers any provider
the file enables that the router is not already holding — one that was
disabled or missing at boot — discovery included, so no restart is needed.

### 3. What discovery does

When the provider is registered — at boot, on the enable, on a hot reload, on a
refresh — the daemon asks the running Ollama what is installed, using Ollama's
own API rather than the OpenAI-compatible one:

- `GET /api/tags` for the list of installed tags.
- `POST /api/show` per tag for its real context length and its capabilities.

Each **chat** model it reports is registered with input and output price `0`,
the context window `/api/show` gave (8192 when it does not say), image support
from the `vision` capability and tool support from `tools`. A model whose
capabilities omit `completion` — an embedding-only model — is deliberately not
registered: it is not something a turn could use. Nothing needs an API key and
nothing needs a `[models]` row; a `[models."<id>"]` row you write by hand still
overrides the discovered fields.

```bash
openalpaca llm models                 # the catalogue as it stands
openalpaca llm models --refresh       # ask every provider again, then list
```

`--refresh` is the command for "I just pulled a model and it is not in the
list": it reaches keyless providers too, so the new tag appears without
restarting the daemon or editing a line of config. The GUI's `Refresh models`
button in Settings → Models & keys does the same. A model you removed from
Ollama is withdrawn at the next refresh (a row you declared in `[models]` is
kept on disk — it simply stops being offered).

If Ollama is not running, the provider still registers, with zero models and
one `WARN`; the enable's answer carries the reason rather than a bare zero, so
an empty list is never mistaken for "nothing installed".

### 4. What you should see

```bash
openalpaca llm status     # Key Health: · ollama — no key needed
openalpaca llm models     # your tags, provider ollama, prices 0, TOOLS column
openalpaca chat --message "say hello in five words"
```

- **No key.** `llm status` prints `· ollama — no key needed` for a keyless
  provider, never a `✗`, and `llm keys list` gives it a row saying the same.
- **Cost 0.** Prices come from the router's live catalogue, so a discovered
  local model costs nothing: usage shows real token counts against `$0.00`.
- **Streaming.** A local reply arrives token by token — the provider's own
  deltas, forwarded as it produces them — and the token counts come back on the
  stream itself. A thinking model's reasoning comes with it, on its own
  `reasoning` frame: the GUI shows it in the thinking indicator and the CLI
  dims it at a terminal, so the wait before the first answer token does not
  look like a hang. Nothing stores reasoning; it is live or it is gone.
- **The configured model may not be yours.** Every shipped agent template, and
  the seeded `[orchestrator] model`, names a Claude id. Those are right when
  Anthropic is configured and fall through a fallback ladder when it is not:
  the request ends up on the first routable model, preferring one that can use
  tools. The substitution is never silent — `openalpaca llm status` reads
  `Model: X — not available, using Y`, Settings → Models & keys shows
  `configured: X — not available, using Y`, and the daemon logs one `WARN` per
  pair. To stop substituting, set `[orchestrator] model` to one of your local
  tags, by hand or with `openalpaca config set ai.default_model <tag>`. With
  nothing routable at all, one error names the fix instead of "Unknown model".

### 5. Knobs worth knowing

All in `~/.openalpaca/config/llm.toml`:

| Key | Default (seeded) | What it does |
|---|---|---|
| `[providers.ollama] default_model` | `""` | Empty means "whatever is installed" — the router picks a discovered model, preferring a tools-capable one. Name a tag to pin it. |
| `[providers.ollama] default_max_tokens` | `8192` | The output ceiling for one answer (the cloud default is 4096). A request that names its own `max_tokens` still wins. |
| `[providers.ollama] request_timeout_secs` | `600` | Wall clock for one **non-streaming** call to this provider. |
| `[timeouts] llm_request_timeout_secs` | `120` | The same budget for any provider that sets no override. |

You should not need a `[models]` row at all — discovery fills the catalogue —
but one you write by hand still overrides what was discovered, field by field:

| Key under `[models."<tag>"]` | What it overrides |
|---|---|
| `provider` | Which provider serves the tag. Required in the row. |
| `input_price` / `output_price` | Dollars per million tokens. Discovery says `0`; say otherwise if you are costing your own hardware. |
| `context` | The context window. Discovery uses `/api/show`, or `8192` when it does not say. |
| `supports_image` | Discovery reads it from the `vision` capability. |
| `supports_tools` | **Defaults to true when omitted**, here and in discovery, and is *recorded, not enforced*: nothing withholds tools from a call because of it. What it changes is the effective-model ladder, which prefers a tool-capable model when it has to choose one for you. Set `false` on a tag that cannot take tools so the ladder stops picking it. |
| `supports_audio` / `supports_document` / `supports_reasoning` | Declared capabilities for the same row. |

A **streamed** reply is not bound by the two timeouts above as a total. Three
other bounds sit over a stream, and whichever trips first wins:

| Bound | Value | Configurable |
|---|---|---|
| No chunk from the provider | 90 s | No — `STREAM_IDLE_TIMEOUT` in `crates/openalpaca_llm/src/streaming.rs` |
| Gap between HTTP reads | `request_timeout_secs` (600 s for the seeded Ollama) | Yes |
| One whole streamed round | 10 minutes | No — `max_stream_duration` in `crates/openalpaca_core/src/runner/agentic_loop/config.rs` |

The HTTP layer also bounds the connect phase at 30 s. When any of these trips
the turn is not lost: the loop logs it and retries the round **without**
streaming, where the per-provider total (600 s for the seeded Ollama) applies
from the start.

In practice the 90 s is the one that fires. A model that is still loading into
memory can exceed it before its first token. That is the case to watch: warm
the model once (`ollama run <tag>` and one prompt) before a long agent run.

### 6. The embedding model

Memory search embeds locally by default (`[embeddings] provider = "local"`),
and the first boot downloads about 1 GB of model before the daemon reports
ready. It is cached at `~/.openalpaca/state/cache/fastembed` — inside the
store, regenerable, safe to delete at the cost of one re-download. Set
`[embeddings] enabled = false` if you would rather skip it
(`openalpaca config set ai.embeddings.enabled false` writes the same field).

## Runtime Overrides

| Variable | Effect |
|---|---|
| `OPENALPACA_HOME_STORE=/abs/path` | Moves the whole runtime root (default `~/.openalpaca`). Must be an absolute path — an empty or relative value is rejected and the daemon refuses to start. |
| `OPENALPACA_CONFIG_DIR=/abs/path` | The config directory `openalpacad` reads when you run it directly. A path that does not exist is ignored with a warning. `openalpaca daemon start` and the desktop app always set it to `<root>/config` for the daemon they start. `openalpaca config` edits the `llm.toml` and `daemon.toml` in this directory when it is set. |
| `OPENALPACA_DAEMON_BIN=/abs/path/openalpacad` | The daemon binary `openalpaca daemon start` launches. Without it the CLI looks next to itself, then in `../libexec`, then on `PATH`. |
| `OPENALPACA_GUI_APP=/abs/path/openalpaca-gui.app` | The app bundle `openalpaca gui start` opens. Without it the CLI looks in `~/Applications`, then `/Applications`. |

`install.sh` and `uninstall.sh` do **not** read `OPENALPACA_HOME_STORE`: they
always stage config templates under `~/.openalpaca/config` and look for a
running daemon under `~/.openalpaca/state`. If you moved the root, stop the
daemon yourself before upgrading. (The Windows scripts do honor the variable.)

## Upgrade

Re-run installer with a newer artifact:

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<new-version>.tar.gz --yes
```

Upgrade keeps:

- `~/.openalpaca` data and config (if you have a development build's data
  elsewhere, see [If You Have Data From an Older
  Build](#if-you-have-data-from-an-older-build) — it is not moved for you)

Upgrade replaces:

- CLI/daemon binaries and GUI app bundle

The installer stops the running daemon but does not start the new one; run
`openalpaca daemon start` afterwards.

## Uninstall

Every package ships `uninstall.sh` in the archive root (also available at
`scripts/release/uninstall.sh`):

```bash
./uninstall.sh [--prefix <dir>] [--app-dir <dir>] [--yes]
```

It lists what it will remove and asks first (`--yes` skips the question). Then
it stops a running daemon and removes:

- the install prefix (default `~/.local/openalpaca`)
- the GUI app (`~/Applications/openalpaca-gui.app` on macOS; on Linux the
  AppImage goes with the prefix, plus the desktop entry and icon under
  `~/.local/share/`)
- the `~/.local/bin/openalpaca` symlink
- the PATH block from `~/.zshrc` and `~/.bashrc`

User data at `~/.openalpaca` is **not** removed; delete it manually for a
complete cleanup.

## Other Platforms

The data root is `<home>/.openalpaca` on every platform — it does not follow
the platform's data-directory convention.

### Linux

`install.sh` / `uninstall.sh` work as-is. Differences from macOS:

- **Build:** `./scripts/release/package-linux.sh`, on a Linux machine
  (`x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu`). It needs
  `sha256sum` where the macOS script needs `shasum`. Output:
  `dist/openalpaca-linux-<target>-v<version>.tar.gz` plus `.sha256`.
- **GUI:** an AppImage, installed to `<prefix>/gui/openalpaca-gui.AppImage`
  with a desktop entry (`OpenAlpaca`) and an icon under `~/.local/share/`.
  `--app-dir` is ignored.
- **Starting the GUI:** `openalpaca gui start` only knows how to open a macOS
  `.app` bundle, so on Linux it fails — and so does the GUI half of a plain
  `openalpaca daemon start`, after the daemon itself has started. Use
  `openalpaca daemon start --daemon-only`, and open the app from the desktop
  entry or by running the AppImage.
- **Older installs:** a development build's data may sit at
  `~/.local/share/openalpaca`. It is **not** moved for you; see [If You Have
  Data From an Older Build](#if-you-have-data-from-an-older-build).

### Windows

**Status: experimental.** CI tests the two installer scripts on Windows, but
never builds or runs the binaries there, and the CLI's daemon start/stop code
is written against Unix signals. Expect `package-windows.ps1` to fail at the
build step until that is ported.

The scripts, all PowerShell 5.1 or later, in `scripts/release/`:

| Script | Usage |
|---|---|
| `package-windows.ps1` | No arguments. Needs `cargo`, `rustc`, `bun`, `git`. Writes `dist\openalpaca-windows-<target>-v<version>.zip` plus `.sha256`, with the GUI as an MSI. |
| `install-windows.ps1` | `-File <archive.zip>` or `-Url <https://…/archive.zip>`, plus `-Prefix <dir>` (default `%LOCALAPPDATA%\OpenAlpaca`) and `-Yes`. |
| `uninstall-windows.ps1` | `-Prefix <dir>` and `-Yes`. |

What the installer does:

- Verifies the SHA256 when a `.sha256` sits beside the archive or URL, and
  checks the package target against the machine.
- Copies `bin\openalpaca.exe` and `libexec\openalpacad.exe` under the prefix,
  and installs the GUI MSI silently.
- Stages the config templates into `<root>\config`, missing files only.
- Adds `<prefix>\bin` to your user `PATH` and creates an `OpenAlpaca CLI` Start
  Menu shortcut. Restart the terminal afterwards.

The data root is `%USERPROFILE%\.openalpaca`, or `%OPENALPACA_HOME_STORE%` when
that is set. A relative value stops either script with an error.

Both scripts stop a running daemon through `<root>\state\discovery.json`, and
only when the pid in it belongs to a live `openalpacad` process — a stale file
never gets another program killed. If the daemon is still running after 10
seconds, the script stops with an error rather than overwrite or remove files
that are in use.

The uninstaller removes the GUI MSI, the prefix, the Start Menu folder and the
`PATH` entry. It leaves the data root in place, exactly as on macOS and Linux.

## Troubleshooting

- `openalpaca: command not found`
  - Restart shell, or run `export PATH="$HOME/.local/bin:$PATH"`.
- `Target mismatch` during install
  - Use artifact matching your machine architecture.
- GUI blocked by macOS quarantine
  - Release binaries are not codesigned, so Gatekeeper may warn or block.
    The installer already runs best-effort quarantine removal; if needed:
    - `xattr -dr com.apple.quarantine ~/Applications/openalpaca-gui.app`
- Daemon not starting
  - Check `~/.openalpaca/state/logs/daemon.log`. Only a daemon started with
    `openalpaca daemon start` writes it; the copy the desktop app starts on its
    own discards its output, so stop that one and start from the CLI to get a
    log. Each start rotates a file that has passed 16 MB and keeps three older
    generations (`daemon.log.1` to `.3`).
- `No model is available: no enabled provider offers one`
  - A fresh install has every provider switched off. Turn one on; see
    [Connect a model](#connect-a-model).
- `FATAL: an older OpenAlpaca install's data is still on this machine` at startup
  - A development build's data directory is still present and this install has
    no database yet. The message lists what to move where. See [If You Have Data
    From an Older Build](#if-you-have-data-from-an-older-build).
