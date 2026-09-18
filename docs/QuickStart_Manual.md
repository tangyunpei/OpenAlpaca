# OpenAlpaca QuickStart (macOS)

Use this for the fastest path to package and install OpenAlpaca without Cargo on the target machine.

## 1) Build Package (builder machine)

```bash
./scripts/release/package-macos.sh
```

Requires `cargo`, `rustc`, `bun`, and `bunx` on the builder machine (the script builds the release binaries and the Tauri app bundle itself) and must run on macOS. The output is not codesigned; the installer removes the quarantine attribute automatically.

Artifact output:
- `dist/openalpaca-macos-<target>-v<version>.tar.gz` (plus a `.sha256` checksum)

For Linux/Windows, use `package-linux.sh`, `package-windows.ps1`, and `install-windows.ps1` in the same directory.

## 2) Install Package (target machine)

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<version>.tar.gz
```

Or install from URL:

```bash
./scripts/release/install.sh --url https://example.com/openalpaca-macos-<target>-v<version>.tar.gz
```

Useful flags: `--prefix <dir>` (default `~/.local/openalpaca`), `--app-dir <dir>` (default `~/Applications`), and `--yes` to overwrite an existing install without prompting.

## 3) Verify

```bash
openalpaca --help
openalpaca daemon start --daemon-only
openalpaca daemon status
openalpaca gui start
```

On a first install, restart your shell (or run `export PATH="$HOME/.local/bin:$PATH"`) so `openalpaca` is found — the installer adds `~/.local/bin` to your PATH via `~/.zshrc` / `~/.bashrc`.

The first daemon start also seeds `~/.openalpaca/config` with the agent templates, skills and tool config it carries in its binary, alongside `llm.toml`, `daemon.toml` and `mcp.toml`. A directory that already exists is left alone.

## 4) Run on a Local Model (Ollama, no API key)

```bash
ollama pull <model>                       # whatever you want to run
openalpaca config set ai.ollama.enabled true   # turn the provider on
#   or the GUI:  Settings → Models & keys → the `ollama` switch
#   or by hand:  [providers.ollama] enabled = true in ~/.openalpaca/config/llm.toml
openalpaca llm models --refresh           # your installed tags, priced 0
openalpaca chat --message "say hello in five words"
```

Enabling the provider is the only action: the daemon then asks the running Ollama what is installed (its own `/api/tags` and `/api/show`) and registers every chat model it reports — **no API key, no `[models]` rows**, real context lengths, prices `0`, and replies that stream token by token. All three ways in write the same `enabled` field in `llm.toml`, and the daemon picks it up live — no restart. `openalpaca llm models --refresh` is what you run after a later `ollama pull`.

Two things to expect: the seeded default model and every shipped agent template name a Claude id, so on an Ollama-only machine the router substitutes and says so (`openalpaca llm status` reads `configured: X — not available, using Y`); and the first boot downloads about 1 GB of local embedding model into `~/.openalpaca/state/cache/fastembed` unless you set `[embeddings] enabled = false`.

Full details, including the output-ceiling and timeout knobs: [Installation Manual → Local Models (Ollama)](Installation_Manual.md#local-models-ollama).

## Default Install Locations

- CLI: `~/.local/bin/openalpaca` (symlink)
- Install prefix: `~/.local/openalpaca` (CLI under `bin/`, daemon under `libexec/`)
- GUI: `~/Applications/openalpaca-gui.app`
- Data/config: `~/.openalpaca`

On a machine that already ran an older install, first boot moves the previous
`~/Library/Application Support/OpenAlpaca` (macOS) contents into the new
location automatically (idempotent and resumable, but **not reversible**) —
back that directory up first. A still-running old daemon blocks the move;
start it there and stop it, or move the daemon binary aside, before
launching the rebuilt one. See [Installation Manual](Installation_Manual.md#migrating-from-the-old-data-directory)
for the fail-closed rules if both locations end up holding a database.

## Lifecycle & Uninstall

- Stop/restart: `openalpaca daemon stop`, `openalpaca daemon restart`, `openalpaca gui stop`
- Follow daemon events: `openalpaca daemon tail` (`-c N` to limit)
- Uninstall: `./scripts/release/uninstall.sh` (same `--prefix`, `--app-dir`, `--yes` flags)

## Optional Overrides

- `OPENALPACA_DAEMON_BIN=/abs/path/openalpacad`
- `OPENALPACA_GUI_APP=/abs/path/openalpaca-gui.app`
- `OPENALPACA_HOME_STORE=/abs/path` — moves the whole data/config root (default `~/.openalpaca`). Must be an absolute path; empty or relative values are rejected and the daemon refuses to start.

For full details, see [Installation Manual](Installation_Manual.md).
