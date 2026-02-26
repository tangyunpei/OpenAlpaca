# OpenAlpaca QuickStart (macOS)

Use this for the fastest path to package and install OpenAlpaca without Cargo on the target machine.

## 1) Build Package (builder machine)

```bash
./scripts/release/package-macos.sh
```

Artifact output:
- `dist/openalpaca-macos-<target>-v<version>.tar.gz`

## 2) Install Package (target machine)

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<version>.tar.gz
```

Or install from URL:

```bash
./scripts/release/install.sh --url https://example.com/openalpaca-macos-<target>-v<version>.tar.gz
```

## 3) Verify

```bash
openalpaca --help
openalpaca daemon start --daemon-only
openalpaca daemon status
openalpaca gui start
```

## Default Install Locations

- CLI: `~/.local/bin/openalpaca` (symlink)
- Binaries: `~/.local/openalpaca`
- GUI: `~/Applications/openalpaca-gui.app`
- Data/config: `~/Library/Application Support/OpenAlpaca`

## Optional Overrides

- `OPENALPACA_DAEMON_BIN=/abs/path/openalpacad`
- `OPENALPACA_GUI_APP=/abs/path/openalpaca-gui.app`

For full details, see [Installation Manual](Installation_Manual.md).
