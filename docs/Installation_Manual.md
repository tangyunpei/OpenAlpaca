# OpenAlpaca Installation Manual (macOS, No Cargo Required on Target)

This manual describes the production-style package flow for OpenAlpaca on macOS:

- Build machine: creates a distributable archive.
- Target machine: installs and runs without Cargo.

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
[Other Platforms](#other-platforms) below.

## Build Machine Requirements

On the machine that builds release artifacts, install:

- `cargo` / `rustc`
- `bun` / `bunx`
- `tar`, `shasum`, `git`
- Standard Unix tools: `awk`, `sed`, `date` (present on any macOS machine)

## Build Release Artifact

From repository root:

```bash
./scripts/release/package-macos.sh
```

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

Exactly one of `--file` or `--url` must be given. Optional arguments:

- `--prefix <dir>` (default: `~/.local/openalpaca`)
- `--app-dir <dir>` (default: `~/Applications`; macOS only, ignored on Linux)
- `--yes` (non-interactive overwrite)
- `-h` / `--help` (print usage)

Installer behaviors:

- Validates host architecture vs package target (from `manifest.json`).
- Verifies SHA256 when available.
- Stops running daemon if found (via the pid in `discovery.json`).
- Preserves user data/config and only replaces app/program files. Config
  templates are copied into the runtime config dir only for files that do not
  already exist — existing config is never overwritten.
- Appends PATH block to `~/.zshrc` and `~/.bashrc` (idempotent).
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

When the CLI launches the daemon, it sets `OPENALPACA_CONFIG_DIR` to the
runtime config directory above — that is the config the installed daemon
actually reads (not any repo checkout).

Override the whole runtime root with `OPENALPACA_HOME_STORE=/abs/path`. It
must be an absolute path — an empty or relative value is rejected and the
daemon refuses to start.

## Migrating From the Old Data Directory

Older installs kept everything under `~/Library/Application Support/OpenAlpaca`
(macOS) / `~/.local/share/openalpaca` (Linux). The rebuilt **daemon** moves
that directory's contents into the new `~/.openalpaca` layout on its first
boot, before it takes the singleton lock. On the CLI side exactly one command
runs the same move itself — `openalpaca config` (in every form: `set`, `get`,
`list`, `reset`, and the bare interactive editor), because it is the only one
that opens the database directly instead of asking the daemon. Every other
`openalpaca` subcommand talks to the running daemon over HTTP, so for those the
move is whatever the daemon already did. It is one move either way:

- The move is **idempotent and resumable** (a process killed mid-move
  finishes on the next boot) but **not reversible** — back up the old
  directory before upgrading if you want to keep a fallback.
- A **still-running old daemon blocks the move**: stop it first (`openalpaca
  daemon stop` against the old install, or kill the process holding
  `openalpacad.lock` in the old directory).
- If **both** the old directory and `~/.openalpaca/state` end up holding an
  `openalpaca.db`, the mover refuses to choose between them and aborts before
  it renames anything: the daemon exits instead of starting, and `openalpaca
  config` exits instead of reading the database. The error names both paths;
  move one aside and start again. Every other CLI command is unaffected in
  itself — it opens no database — but it needs a daemon that will not start
  until the two are one.
- Anything the mover doesn't recognize left behind in the old directory
  produces a boot warning (check the daemon log) rather than being deleted
  silently.

## Run and Verify

```bash
openalpaca --help
openalpaca daemon start --daemon-only
openalpaca daemon status
openalpaca gui start
```

A first boot also writes the content the daemon carries in its own binary into
`~/.openalpaca/config`: `llm.toml`, `daemon.toml`, `mcp.toml`, the nine agent
templates (`agents/`), the skills (`skills/`) and the tool config (`tools/`).
The rule is per directory: a directory that already exists is left
alone entirely, even one you emptied on purpose, and inside a directory being
filled an existing file is never overwritten. Without the templates the first
workflow request has no lead agent to run and says so.

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
`http://localhost:11434/v1`. Change `[providers.ollama] base_url` in
`~/.openalpaca/config/llm.toml` if yours listens elsewhere.

### 2. Turn the provider on

Enabling it is the only action required — the seeded `llm.toml` ships
`[providers.ollama] enabled = false` and everything else already set.

- **GUI**: Settings → Models & keys, the `ollama` row's switch. The row reads
  `no key needed` instead of offering a key editor, and the toast reports how
  many models the daemon found.
- **By hand**: set `enabled = true` under `[providers.ollama]` in
  `~/.openalpaca/config/llm.toml` and save. The config watcher registers any
  provider the file enables that the router is not already holding — one that
  was disabled or missing at boot — discovery included, so no restart is
  needed. (There is no CLI verb for the provider switch today.)

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
- **Streaming.** A local reply arrives token by token, and the token counts
  come back on the stream itself.
- **The configured model may not be yours.** Every shipped agent template, and
  the seeded `[orchestrator] model`, names a Claude id. Those are right when
  Anthropic is configured and fall through a fallback ladder when it is not:
  the request ends up on the first routable model, preferring one that can use
  tools. The substitution is never silent — `openalpaca llm status` reads
  `configured: X — not available, using Y`, Settings → Models & keys shows the
  same sentence, and the daemon logs one `WARN` per pair. Set `[orchestrator] model`
  to one of your local tags to stop substituting. With nothing routable at all,
  one error names the fix instead of "Unknown model".

### 5. Knobs worth knowing

All in `~/.openalpaca/config/llm.toml`:

| Key | Default (seeded) | What it does |
|---|---|---|
| `[providers.ollama] default_model` | `""` | Empty means "whatever is installed" — the router picks a discovered model, preferring a tools-capable one. Name a tag to pin it. |
| `[providers.ollama] default_max_tokens` | `8192` | The output ceiling for one answer (the cloud default is 4096). A request that names its own `max_tokens` still wins. |
| `[providers.ollama] request_timeout_secs` | `600` | Wall clock for one **non-streaming** call to this provider. |
| `[timeouts] llm_request_timeout_secs` | `120` | The same budget for any provider that sets no override. |

A **streamed** reply is not bound by those: the HTTP layer bounds the connect
and the gap between chunks, not the total, so a long generation is never cut
mid-answer. What gives up on a stalled stream is the loop's idle bound — 90
seconds with no chunk.

### 6. The embedding model

Memory search embeds locally by default (`[embeddings] provider = "local"`),
and the first boot downloads about 1 GB of model before the daemon reports
ready. It is cached at `~/.openalpaca/state/cache/fastembed` — inside the
store, regenerable, safe to delete at the cost of one re-download. Set
`[embeddings] enabled = false` if you would rather skip it.

## Runtime Overrides

- Override daemon binary path:
  - `OPENALPACA_DAEMON_BIN=/abs/path/openalpacad`
- Override GUI app path:
  - `OPENALPACA_GUI_APP=/abs/path/openalpaca-gui.app`

## Upgrade

Re-run installer with a newer artifact:

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<new-version>.tar.gz --yes
```

Upgrade keeps:

- `~/.openalpaca` data and config (see [Migrating From the Old Data
  Directory](#migrating-from-the-old-data-directory) if you're upgrading from
  a pre-root-move install)

Upgrade replaces:

- CLI/daemon binaries and GUI app bundle

## Uninstall

Every package ships `uninstall.sh` in the archive root (also available at
`scripts/release/uninstall.sh`):

```bash
./uninstall.sh [--prefix <dir>] [--app-dir <dir>] [--yes]
```

It stops a running daemon, then removes:

- the install prefix (default `~/.local/openalpaca`)
- the GUI app (`~/Applications/openalpaca-gui.app` on macOS)
- the `~/.local/bin/openalpaca` symlink
- the PATH block from `~/.zshrc` and `~/.bashrc`

User data at `~/.openalpaca` is **not** removed; delete it manually for a
complete cleanup.

## Other Platforms

- **Linux**: `install.sh` / `uninstall.sh` work as-is. Differences from macOS:
  the GUI is an AppImage installed to `<prefix>/gui/openalpaca-gui.AppImage`
  (with a desktop entry and icon under `~/.local/share/`) and `--app-dir` is
  ignored; the data dir is the same `~/.openalpaca` as macOS (the store root
  is `<home>/.openalpaca` on every platform — it does not follow the
  platform's data-directory convention). A pre-root-move install's legacy
  data lived at `~/.local/share/openalpaca` and is moved on first boot of the
  rebuilt binaries; see [Migrating From the Old Data
  Directory](#migrating-from-the-old-data-directory). Build artifacts with
  `./scripts/release/package-linux.sh` on a Linux machine
  (`x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu`).
- **Windows**: use the PowerShell scripts `scripts/release/package-windows.ps1`,
  `install-windows.ps1`, and `uninstall-windows.ps1`.

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
  - Check `~/.openalpaca/state/logs/daemon.log`.
- `two databases: ... has not moved yet and ... already exists` at startup
  - Both the old and new data directories hold an `openalpaca.db`. Keep the
    one you want (the legacy file is the older install's data), move or
    remove the other, and restart. See [Migrating From the Old Data
    Directory](#migrating-from-the-old-data-directory).
