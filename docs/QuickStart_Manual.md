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
