# OpenAlpaca QuickStart (macOS)

From a source checkout to a first chat in five steps.

There is no hosted download. You build the package once (step 1) and install it
on any Mac of the same architecture; the machine you install on needs no Rust
toolchain.

On Linux, build with `package-linux.sh`, install the
`openalpaca-linux-*.tar.gz` it writes, and start with `--daemon-only`
([why](Installation_Manual.md#linux)). For Windows, and for every option not
shown here, see the [Installation Manual](Installation_Manual.md).

## 1. Build the package

On a Mac with `cargo`, `bun` and `git`, from the repository root:

```bash
./scripts/release/package-macos.sh
```

It builds the release binaries and the desktop app, and writes:

- `dist/openalpaca-macos-<target>-v<version>.tar.gz`
- `dist/openalpaca-macos-<target>-v<version>.tar.gz.sha256`

## 2. Install it

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-*.tar.gz
```

If `dist/` holds more than one archive, name the one you want instead of the
`*`. Nothing needs `sudo`. Installing on a different Mac? The archive carries
its own `install.sh`; see
[On a machine without the repository](Installation_Manual.md#on-a-machine-without-the-repository).

Then make `openalpaca` visible in the current shell (new shells get it from the
PATH block the installer adds to `~/.zshrc` and `~/.bashrc`):

```bash
export PATH="$HOME/.local/bin:$PATH"
openalpaca --help
```

## 3. Start it

```bash
openalpaca daemon start      # starts the daemon and opens the desktop app
openalpaca daemon status
```

Terminal only? Use `openalpaca daemon start --daemon-only`.

The first start takes longer than later ones. It writes the default config,
agent templates and skills into `~/.openalpaca/config`, and downloads the local
embedding model (about 1 GB) into `~/.openalpaca/state/cache/fastembed`. The
daemon answers no request until that download is done, so wait for
`openalpaca daemon status` to report `Daemon is running` before step 4.

## 4. Connect a model

A fresh install has every provider switched off. Pick one.

**A — a local model with [Ollama](https://ollama.com) (no API key)**

```bash
openalpaca config set ai.ollama.enabled true   # the only setup step
ollama pull <model>                            # any chat model you want to run
openalpaca llm models --refresh                # your installed tags, priced 0
```

**B — a cloud provider (`anthropic` or `openai`)**

```bash
openalpaca config set ai.anthropic.enabled true
openalpaca llm keys add --provider anthropic   # prompts for the key, a source and a note
```

Keep this order: switch the provider on, then add the key. Both take effect
without a restart. The desktop app does the same two steps on one screen —
Settings → Models & keys, where `Add key` refuses to save until the provider
is switched on and offers the switch in the refusal.

Check the result:

```bash
openalpaca llm status
```

With only Ollama enabled, the `Model:` line reads
`<configured> — not available, using <your model>`. That is expected: the
default config names a Claude model, and the router falls back to what you have
and says so. [Local Models (Ollama)](Installation_Manual.md#local-models-ollama)
explains how to pin your own model.

## 5. Say hello

```bash
openalpaca chat --message "say hello in five words"
```

`openalpaca chat` with no arguments opens an interactive session, and the
desktop app has the same chat.

**The first conversation is onboarding.** Until it knows who you are, the
assistant asks about you instead of running tasks, and it cannot start
workflows or use memory, MCP servers or plugins. Answer its questions and the
full tool surface comes back on the next turn. To skip onboarding, delete
`~/.openalpaca/config/orchestrator/BOOTSTRAP.md`. The next daemon start puts
it back for as long as `USER.md` or `IDENTITY.md` beside it is still empty.

## Where things are

| What | Path |
|---|---|
| CLI (symlink) | `~/.local/bin/openalpaca` |
| Binaries | `~/.local/openalpaca` (`bin/openalpaca`, `libexec/openalpacad`) |
| Desktop app | `~/Applications/openalpaca-gui.app` |
| Config | `~/.openalpaca/config` |
| Data, database, logs | `~/.openalpaca` (`state/` is the machine's; the rest is yours) |

## Stop, upgrade, uninstall

```bash
openalpaca daemon stop         # stops the daemon and the desktop app
openalpaca daemon restart
openalpaca daemon tail         # follow daemon events; -c N stops after N
```

- **Upgrade:** build a newer package, run `install.sh` again with `--yes`, then
  `openalpaca daemon start`. The installer stops the running daemon; your data
  and config are kept.
- **Uninstall:** `./scripts/release/uninstall.sh`. It leaves `~/.openalpaca`
  in place; delete that directory yourself for a complete cleanup.
- **Have a development build's data** under
  `~/Library/Application Support/OpenAlpaca`? Nothing is moved for you. If that
  directory holds a database, a `.master_key` or a `config/` and `~/.openalpaca`
  has no database yet, the daemon refuses to start rather than come up on an
  empty database beside it; otherwise it starts, and warns once per boot while that
  directory still holds a database, `.master_key`, `config/`, `plugins/` or `assets/`.
  Read [If You Have Data From an Older
  Build](Installation_Manual.md#if-you-have-data-from-an-older-build) first.

## Next

- [Installation Manual](Installation_Manual.md) — installer flags, install from
  a URL, Linux and Windows, environment overrides, troubleshooting.
- [CLI Manual](CLI_Manual.md) · [GUI Manual](GUI_Manual.md) ·
  [Daemon Manual](Daemon_Manual.md)
