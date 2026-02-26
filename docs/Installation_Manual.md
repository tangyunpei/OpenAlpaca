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

## Build Machine Requirements

On the machine that builds release artifacts, install:
- `cargo` / `rustc`
- `bun` / `bunx`
- `tar`, `shasum`

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
- `config/` (safe templates only)
- `install.sh`
- `manifest.json`

## Install on Target Machine

### Option A: Install from local file

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<version>.tar.gz
```

### Option B: Install from URL

```bash
./scripts/release/install.sh --url https://example.com/openalpaca-macos-<target>-v<version>.tar.gz
```

Optional arguments:
- `--prefix <dir>` (default: `~/.local/openalpaca`)
- `--app-dir <dir>` (default: `~/Applications`)
- `--yes` (non-interactive overwrite)

Installer behaviors:
- Validates host architecture vs package target.
- Verifies SHA256 when available.
- Stops running daemon if found.
- Preserves user data/config and only replaces app/program files.
- Appends PATH block to `~/.zshrc` and `~/.bashrc` (idempotent).

## Installed Paths

Defaults after install:
- CLI binary: `~/.local/openalpaca/bin/openalpaca`
- Daemon binary: `~/.local/openalpaca/libexec/openalpacad`
- CLI symlink: `~/.local/bin/openalpaca`
- GUI app: `~/Applications/openalpaca-gui.app`
- Runtime root: `~/Library/Application Support/OpenAlpaca`
- Runtime config: `~/Library/Application Support/OpenAlpaca/config`
- Runtime DB: `~/Library/Application Support/OpenAlpaca/openalpaca.db`

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
- `~/Library/Application Support/OpenAlpaca` data and config

Upgrade replaces:
- CLI/daemon binaries and GUI app bundle

## Troubleshooting

- `openalpaca: command not found`
  - Restart shell, or run `export PATH="$HOME/.local/bin:$PATH"`.
- `Target mismatch` during install
  - Use artifact matching your machine architecture.
- GUI blocked by macOS quarantine
  - Installer already runs best-effort quarantine removal; if needed:
    - `xattr -dr com.apple.quarantine ~/Applications/openalpaca-gui.app`
- Daemon not starting
  - Check `~/Library/Application Support/OpenAlpaca/daemon.log`.
