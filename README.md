# OpenAlpaca

OpenAlpaca is a local-first, daemon-based personal AI agent orchestrator written as a Rust workspace. A single background daemon (`openalpacad`) owns the SQLite database, the orchestrator/agent runtime, a multi-provider LLM router, chat connectors, and an HTTP/WebSocket/SSE API on localhost; a clap CLI and a Tauri desktop GUI are thin clients that find it via `discovery.json`. Incoming messages pass a small deterministic tier (slash commands, skills) and then enter an agentic main loop where the model itself decides — via tool calls — whether to just chat, start a background workflow run by a lead agent with subagents, or steer one already running; all of this sits on persistent hybrid-search memory, a progressive skill system, MCP tool integration, an out-of-process plugin system, and connectors for Telegram, iMessage, and Discord.

This is an evolving personal project. Core paths work; several subsystems are explicitly experimental or partially wired (noted honestly below).

## Features

**Working:**

- **Multi-provider LLM routing** — Anthropic, OpenAI, and Ollama providers behind one router with key pools, per-key rate limiting (RPM/TPM token buckets), circuit breaking, model fallback chains, and a last-resort fallback to the `claude` / `codex` CLI binaries if present on PATH.
- **Tool-call routing + background workflows** — a deterministic tier handles slash commands with no LLM call (`/status`, `/tasks`, `/cancel`/`/pause`/`/resume` — bare or with a task id — `/steer <text>`, and `/<skill>` invocations); everything else goes to a main loop where the model chooses between answering directly, starting a background workflow (`start_workflow`), steering or queuing follow-ups on a running one, checking task status, and updating memory. Workflows are executed by a lead agent that spawns and coordinates subagents (singly or in batches), accepts mid-run steering, posts a model-authored completion report to the chat lane, and auto-starts queued follow-ups when it finishes.
- **SQLite memory** — single-file DB (rusqlite + sqlite-vec, WAL mode, 33 embedded migrations) with hybrid memory search: FTS5 full-text plus 768-dim vector KNN, scope cascade (workspace → global), importance decay, and dedup/supersession.
- **Skills** — markdown `SKILL.md` definitions with YAML frontmatter, slash commands, a weighted skill router, bundled executable scripts, and project-over-user scope overrides.
- **MCP client (tools only)** — connects out to external MCP servers (stdio or streamable HTTP) declared in `config/mcp.toml`; discovered tools register as `<server>__<tool>` with reconnect/retry handling.
- **Plugin system (tools, skills, agents)** — out-of-process plugins speaking JSON-RPC 2.0 over stdio, with a manifest schema, first-load approval gate, and per-plugin config. Managed via `openalpaca plugin ...` or `/v1/plugins` routes.
- **Chat connectors** — Telegram (long-polling, interactive tool confirmations via `/yes` `/no`), iMessage (macOS-only, chat.db polling + AppleScript send), and Discord (twilight gateway). Identity linking (`/link`), attachment ingestion, and per-chat rate limiting.
- **Security gate** — capability/trust checks per principal, input sanitization, sandboxed tool execution with timeouts, interactive tool confirmations, and per-tool circuit breakers.
- **Cost tracking & telemetry** — per-agent/task/provider usage with pricing from the model registry, daily budget caps for background LLM work, and persisted dispatch-decision/latency records.
- **Hot-reloadable config** — a polling filesystem watcher reloads `daemon.toml`, `llm.toml`, agent templates, skills, and persona documents without a restart.
- **Secrets management** — API keys via env var, OS keychain, or local AES-256-GCM encryption; OAuth credential auto-discovery from existing Claude Code / Codex CLI installs.

**Experimental / partial (honest status):**

- **Plugin connectors and plugin LLM providers** — declared in the plugin manifest and discovered, but not yet registered with the connector manager or LLM router. Not functional.
- **MCP resources and prompts** — stubbed; only MCP *tools* work. Server mode (exposing OpenAlpaca over MCP) is an explicit non-goal.
- **`openalpaca_platform` / `openalpaca_platform_macos`** — empty placeholder crates; nothing depends on them.
- **Assorted stubs** — agent creation from chat (returns "planned"), `plugin config get` via CLI, and without any configured LLM provider the daemon degrades to an echo stub.

## Architecture

### Workspace layout

10 library crates + 3 apps (see `Cargo.toml`):

| Crate | Role |
|---|---|
| `crates/openalpaca_core` | The brain: orchestrator, message routing, task dispatcher, agent registry, agentic loop, tool registry, skills, security gate, event bus |
| `crates/openalpaca_llm` | LLM router, providers (Anthropic/OpenAI/Ollama), key pools, rate limiting, cost tracking, embeddings, secret storage, CLI-backend fallback |
| `crates/openalpaca_storage` | SQLite database, migrations, typed repositories, hybrid memory search, `discovery.json` + app paths + single-instance lock |
| `crates/openalpaca_api` | Shared event types (`WakeEvent`, `ServerEvent`) and plugin executor traits — dependency-free leaf |
| `crates/openalpaca_wake` | Cron scheduler + polling filesystem watcher producing wake events (powers config hot-reload) |
| `crates/openalpaca_connectors` | Telegram / iMessage / Discord chat adapters over the core gateway |
| `crates/openalpaca_mcp` | MCP client SDK wrapper (rmcp): stdio + streamable-HTTP transports, reconnect/retry |
| `crates/openalpaca_plugins` | Out-of-process plugin manager: JSON-RPC over stdio, manifests, approval gate |
| `crates/openalpaca_platform` | Placeholder (empty scaffold) |
| `crates/openalpaca_platform_macos` | Placeholder (empty scaffold) |

| App | Role |
|---|---|
| `apps/openalpacad` | The daemon: owns everything, serves HTTP + WebSocket + SSE on a dynamic localhost port, bearer-token auth |
| `apps/openalpaca` | CLI: daemon lifecycle, chat REPL, tasks/agents/keys/plugins management, offline config editor |
| `apps/openalpaca-gui` | Tauri v2 desktop app (React 19 + Tailwind v4); bundles the daemon as a sidecar |

### How the pieces connect

The daemon binds `127.0.0.1:<random port>` and writes `discovery.json` (port + 24h bearer token) into the app data directory, guarded by an advisory lock so only one instance runs. The CLI and GUI read `discovery.json` to connect; both can also spawn the daemon if it isn't running. Chat responses stream over SSE; system events (task/agent status, tool executions, costs) stream over WebSocket.

## Quick start

Prerequisites: Rust 1.93.0 (pinned via `rust-toolchain.toml`); [Bun](https://bun.sh) for the GUI.

```bash
# Build the whole workspace
cargo build

# Run the daemon
cargo run -p openalpacad

# Use the CLI (separate terminal)
cargo run -p openalpaca -- daemon status
cargo run -p openalpaca -- chat --message "hello"
cargo run -p openalpaca -- chat            # interactive REPL
cargo run -p openalpaca -- config         # interactive config TUI (works offline)

# GUI development (starts frontend hot-reload + builds the daemon sidecar)
cd apps/openalpaca-gui
bun install
bun run tauri dev
```

Other useful commands: `cargo test`, `cargo clippy`, `cargo build --release`. For end-user installation from a release tarball, see `scripts/release/install.sh` and [docs/Installation_Manual.md](docs/Installation_Manual.md).

To actually talk to a model, add a provider key: `cargo run -p openalpaca -- llm keys add`, or edit `config/llm.toml`, or set `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`. If Claude Code or Codex CLI is installed, their credentials are auto-discovered as fallbacks.

## Configuration

Config directory resolution: `OPENALPACA_CONFIG_DIR` env var → walk up from the executable/CWD looking for `config/llm.toml` → fallback `./config`.

| File | Purpose |
|---|---|
| `config/daemon.toml` | Execution limits, routing/steering (`[orchestrator.routing]`), cost caps, security, server capacities, upload governance. The repo copy is the full reference; a fresh install seeds a minimal version and relies on compiled-in defaults. |
| `config/llm.toml` | Provider keys, model registry/pricing, fallback chains, embeddings, web search. **Gitignored (contains secrets)** — generated on first run from an embedded template. |
| `config/mcp.toml` | External MCP server declarations (all examples commented out by default). |
| `config/agents/*.md` | Agent templates: YAML frontmatter (model, capabilities, cost limits) + markdown persona. Nine ship in-repo. |
| `config/skills/*/SKILL.md` | Skill definitions (four ship in-repo: code-review, commit-message, create-skill, explain-code). |
| `config/orchestrator/` | Live persona documents `SOUL.md` / `USER.md` / `IDENTITY.md` — generated on first run from `config/orchestrator/templates/`; not checked in. |

Runtime data lives in `~/.openalpaca/` (same path on every platform; `OPENALPACA_HOME_STORE` overrides it, absolute paths only): `state/` holds `openalpaca.db`, `discovery.json`, `openalpacad.lock`, `.master_key`, and logs; `config/` and `plugins/` sit beside it.

## Documentation

| Document | What it covers |
|---|---|
| [docs/QuickStart_Manual.md](docs/QuickStart_Manual.md) | Getting up and running quickly |
| [docs/Installation_Manual.md](docs/Installation_Manual.md) | Installing from release packages |
| [docs/CLI_Manual.md](docs/CLI_Manual.md) | The `openalpaca` CLI: commands, REPL, config editing |
| [docs/Daemon_Manual.md](docs/Daemon_Manual.md) | The `openalpacad` daemon: lifecycle, API, configuration |
| [docs/GUI_Manual.md](docs/GUI_Manual.md) | The Tauri desktop app |
| [docs/agent-loop.md](docs/agent-loop.md) | Reference for the agentic execution loop |
| [docs/Skill_Template_Reference.md](docs/Skill_Template_Reference.md) | Full `SKILL.md` frontmatter schema |
| [docs/tools/DESIGN.md](docs/tools/DESIGN.md) | Tool system design |
| [docs/tools/TECHNICAL.md](docs/tools/TECHNICAL.md) | Tool system technical details |
| [docs/api/README.md](docs/api/README.md) | Generated API docs (via `python3 scripts/gen_api_docs.py`) |

## Toolchain & project status

- Rust **1.93.0** (edition 2024, resolver v3), pinned in `rust-toolchain.toml`; `rustfmt` and `clippy` components included.
- GUI: Bun + Vite 7 + React 19 + TypeScript 5.9 (strict) + Tailwind v4, Tauri v2.
- CI (`.github/workflows/ci.yml`) builds/tests/lints the Rust workspace and type-checks, tests and builds the GUI frontend.

OpenAlpaca is an evolving personal project, not a polished product. Interfaces, config schemas, and the database schema change frequently, and some subsystems (plugins beyond tools/skills/agents, MCP resources/prompts, the platform crates) are scaffolding or disabled by default. Read the status notes above before depending on a feature.
