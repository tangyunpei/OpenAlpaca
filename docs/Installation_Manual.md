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
(macOS) / `~/.local/share/openalpaca` (Linux). The first boot of a rebuilt
daemon or CLI moves that directory's contents into the new `~/.openalpaca`
layout automatically:

- The move is **idempotent and resumable** (a process killed mid-move
  finishes on the next boot) but **not reversible** — back up the old
  directory before upgrading if you want to keep a fallback.
- A **still-running old daemon blocks the move**: stop it first (`openalpaca
  daemon stop` against the old install, or kill the process holding
  `openalpacad.lock` in the old directory).
- If **both** the old directory and `~/.openalpaca/state` end up holding an
  `openalpaca.db`, the daemon (and any CLI command that opens the database
  directly, e.g. `openalpaca config ...`) refuses to start until you move one
  aside — the error names both paths.
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
