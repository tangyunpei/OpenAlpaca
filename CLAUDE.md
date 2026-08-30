# CLAUDE.md — OpenAlpaca

## Workflow Orchestration

### 1. Plan Node Default
- Enter plan mode for ANY non-trivial task (3+ steps or architectural decisions)
- If something goes sideways, STOP and re-plan immediately - don't keep pushing
- Use plan mode for verification steps, not just building
- Write detailed specs upfront to reduce ambiguity

### 2. Subagent Strategy
- Use subagents liberally to keep main context window clean
- Offload research, exploration, and parallel analysis to subagents
- For complex problems, throw more compute at it via subagents
- One tack per subagent for focused execution

### 3. Self-Improvement Loop
- After ANY correction from the user: update `tasks/lessons.md` with the pattern
- Write rules for yourself that prevent the same mistake
- Ruthlessly iterate on these lessons until mistake rate drops
- Review lessons at session start for relevant project

### 4. Verification Before Done
- Never mark a task complete without proving it works
- Diff behavior between main and your changes when relevant
- Ask yourself: "Would a staff engineer approve this?"
- Run tests, check logs, demonstrate correctness

### 5. Demand Elegance (Balanced)
- For non-trivial changes: pause and ask "is there a more elegant way?"
- If a fix feels hacky: "Knowing everything I know now, implement the elegant solution"
- Skip this for simple, obvious fixes - don't over-engineer
- Challenge your own work before presenting it

### 6. Autonomous Bug Fixing
- When given a bug report: just fix it. Don't ask for hand-holding
- Point at logs, errors, failing tests - then resolve them
- Zero context switching required from the user
- Go fix failing CI tests without being told how

## Task Management
1. **Plan First**: Write plan to `tasks/todo.md` with checkable items
2. **Verify Plan**: Check in before starting implementation
3. **Track Progress**: Mark items complete as you go
4. **Explain Changes**: High-level summary at each step
5. **Document Results**: Add review section to `tasks/todo.md`
6. **Capture Lessons**: Update `tasks/lessons.md` after corrections

## Core Principles
- **Simplicity First**: Make every change as simple as possible. Impact minimal code.
- **No Laziness**: Find root causes. No temporary fixes. Senior developer standards.
- **Minimal Impact**: Changes should only touch what's necessary. Avoid introducing bugs. 

## Build & Development

```bash
# Full workspace
cargo build                      # debug build (library crates only — apps are excluded via default-members)
cargo build --workspace          # debug build including apps (daemon, CLI, GUI backend)
cargo build --release            # release build
cargo test                       # run library-crate tests
cargo test --workspace           # run all tests including apps
cargo clippy                     # lint

# Individual crates
cargo build -p openalpaca_core
cargo build -p openalpaca_llm
cargo test -p openalpaca_storage

# Apps
cargo run -p openalpacad         # start daemon
cargo run -p openalpaca          # CLI (use -- <subcommand> for args)

# GUI (Tauri + SvelteKit frontend)
cd apps/openalpaca-gui
bun install                      # install JS deps
bun run tauri dev                # dev mode (hot-reload frontend + Rust rebuild)
bun run dev                      # frontend-only dev server
bun run build                    # production frontend build
bun run prepare:sidecar:dev      # bundle the daemon as a Tauri sidecar (or :release)
```

Toolchain: Rust 1.93.0 (edition 2024, resolver v3). Pinned in `rust-toolchain.toml`.

## Architecture

### Workspace Layout

```
apps/
  openalpacad/          # Daemon binary — axum HTTP/WS server, manages all services
  openalpaca/           # CLI binary — clap, connects to daemon via discovery.json
  openalpaca-gui/       # Tauri v2 desktop app — SvelteKit + Tailwind frontend
crates/
  openalpaca_core/      # Orchestrator, agents, tools, runtime, bus, security, prompt composition
  openalpaca_llm/       # LLM router, providers (Anthropic/OpenAI/Ollama), key management
  openalpaca_storage/   # SQLite (rusqlite + sqlite-vec), repositories, migrations
  openalpaca_api/       # Shared event types (WakeEvent) + plugin executor traits
  openalpaca_wake/      # Cron scheduler + filesystem watcher
  openalpaca_connectors/# Chat platform adapters (Telegram, iMessage, Discord)
  openalpaca_mcp/       # MCP client (rmcp wrapper) — connects out to MCP servers, imports tools
  openalpaca_plugins/   # Out-of-process plugin system (JSON-RPC over stdio, approval gate)
  openalpaca_platform/  # Platform abstractions (placeholder)
  openalpaca_platform_macos/  # macOS-specific (placeholder)
config/
  daemon.toml           # Execution limits, server settings, memory/cost budgets
  mcp.toml              # MCP server declarations + connect/reconnect defaults
  agents/               # Agent template definitions (markdown with YAML frontmatter)
  orchestrator/         # templates/ only — live persona docs are generated at first run
  skills/               # Skill definitions (SKILL.md files)
  tools/                # Tool configuration
```

`config/llm.toml` is not checked in — the daemon seeds it on first start from `scripts/release/templates/config/llm.toml` (`apps/openalpacad/src/bootstrap/config.rs`, `seed_default_configs`). Likewise `config/orchestrator/SOUL.md`, `USER.md`, `IDENTITY.md`, and `BOOTSTRAP.md` are created at first run from `config/orchestrator/templates/*_temp.md`.

### Dependency Graph

```
openalpaca_api          ← leaf (no internal deps)
openalpaca_llm          ← leaf (no internal deps)
openalpaca_storage      ← leaf (no internal deps)
openalpaca_mcp          ← leaf (no internal deps)
openalpaca_wake         ← openalpaca_api
openalpaca_core         ← openalpaca_api, openalpaca_llm, openalpaca_mcp, openalpaca_storage
openalpaca_connectors   ← openalpaca_api, openalpaca_core, openalpaca_storage
openalpaca_plugins      ← openalpaca_api, openalpaca_connectors, openalpaca_core, openalpaca_llm
openalpacad             ← api, connectors, core, llm, mcp, plugins, storage, wake
openalpaca (CLI)        ← openalpaca_core, openalpaca_llm, openalpaca_storage
openalpaca_gui          ← openalpaca_api, openalpaca_storage
```

### Three Execution Modes

The orchestrator dispatches complex tasks via `TaskDispatcher` with three paths:

1. **Sequential Pipeline** (`dispatcher/pipeline.rs`) — agents run in order, each receiving the previous agent's output. One `tokio::spawn` per pipeline.
2. **DAG Execution** (`dispatcher/dag.rs` + `src/runner/dag_executor/`, the latter directly under the crate root, not under `orchestrator/`) — independent nodes run concurrently up to `max_concurrent_agents` (default 4). Optional replanning after N nodes.
3. **Lead Agent** (`dispatcher/lead_agent.rs`) — a single orchestrating agent handles delegation autonomously, spawned from an agent template with the `orchestration` capability (`config/agents/lead_agent.md`).

Selection is driven by the task planner's output: `plan.use_lead_agent` → lead agent, `plan.dag` present → DAG, otherwise → sequential pipeline.

### Intent Routing

The `Orchestrator` classifies incoming messages into intents before dispatch:
- `SimpleQuery` → direct LLM call
- `TaskQuery` → query task registry
- `ComplexTask` → dispatch to agents via `TaskDispatcher`
- `TaskControl` → manage task lifecycle (cancel/pause/resume)

### Extensibility: MCP + Plugins

Two mechanisms extend the tool surface:

- **MCP servers** (`config/mcp.toml`): the daemon connects out to declared MCP servers at boot (`openalpaca_mcp` crate, stdio or streamable-HTTP transports) and registers their tools in the tool registry as `<server>__<tool>`. Tools only — MCP resources/prompts are stubbed (not implemented), and serving MCP is a non-goal. Per-server failures are logged, never fatal.
- **Plugins** (`~/Library/Application Support/OpenAlpaca/plugins/`): out-of-process child programs speaking JSON-RPC 2.0 over stdio, declared by a `plugin.toml` manifest, gated by a first-load approval flow (`.permissions.toml`). Plugin tools register as `<plugin>::<tool>`; plugins can also contribute skills and agent templates. Connector and LLM-provider plugin bridges exist in code but are not yet wired into `ConnectorManager`/`LlmRouter` — treat those plugin types as non-functional. Managed via `GET/POST /v1/plugins/...` routes and `openalpaca plugin ...` CLI commands.

## Key Patterns

### Async / Concurrency

- **Runtime**: tokio multi-thread (`#[tokio::main]` in both daemon and CLI)
- **Event bus**: `tokio::sync::broadcast` channel (`EventBus`) for system-wide events
- **Cancellation**: `tokio_util::sync::CancellationToken` per task, registered in `SharedContext.cancellation_tokens`. Checked before each pipeline step.
- **Global LLM concurrency**: `tokio::sync::Semaphore` in `LlmRouter` caps in-flight API calls
- **Shutdown**: `tokio::sync::mpsc` channel in daemon `AppState`

### Hot-Reloadable Config

`Arc<ArcSwap<T>>` for lock-free reads:
- `DaemonConfig` — execution limits, server settings
- `LlmRouter.default_model` — default model ID
- `LlmRouter.runtime_config` — timeouts, endpoints
- Per-provider `KeyPool` — API keys

`Arc<RwLock<T>>` for persona documents: `system_persona`, `user_document`, `identity_document`.

### Error Handling

- **`thiserror`** for typed library errors: `LlmError`, `LlmRouterError`, `ConnectorError`, `KeyPoolError`
- **`anyhow`** for application-level code: storage, daemon startup, config loading
- **`Result<String, String>`** for orchestrator internal dispatch paths
- **Mutex poison recovery**: `lock().unwrap_or_else(|p| p.into_inner())` with `tracing::warn!`

### LLM Router Flow

1. Acquire global concurrency semaphore
2. Resolve provider from `ModelRegistry`
3. Pick key via selection strategy (RoundRobin / LeastRecentlyUsed / PrimaryFallback)
4. Per-key rate limiter: concurrency + RPM/TPM token buckets + circuit breaker
5. On rate limit → try next key (max 2 wait cycles, 30s each)
6. On failure → model-level fallback chain → CLI backend fallback (Claude Code CLI / Codex CLI)

### State Persistence

`TaskRepository::update_state()` uses optimistic locking (`state_version` column). On version conflict, callers retry with linear backoff plus pseudo-jitter (10ms + 5ms/attempt + task-id-derived jitter, max 3 attempts — `dispatcher/outcome.rs`).

### Storage

Single SQLite connection wrapped in `Arc<Mutex<Connection>>`. WAL mode, `busy_timeout=5000ms`. Schema managed via numbered migrations (currently 32). Memory search is hybrid: FTS5 full-text + sqlite-vec 768-dim KNN with cascading scope (workspace → global).

Data directory: `~/Library/Application Support/OpenAlpaca/` (macOS). DB: `openalpaca.db`, lock: `openalpacad.lock`, discovery: `discovery.json`.

## Configuration

| File | Purpose |
|---|---|
| `config/daemon.toml` | Execution limits, DAG settings, lead-agent defaults, context compaction, planner, orchestrator cost caps, memory/prompt budgets, server config (chat streams, embedding indexer), telemetry, upload governance, security |
| `config/llm.toml` | Provider credentials (AES-256-GCM encrypted), model registry with pricing, embedding config, fallback chains — generated on first daemon start, not checked in |
| `config/mcp.toml` | MCP server declarations (`[servers.<name>]`, stdio/http transports) + connect/request timeouts and reconnect defaults |
| `config/agents/*.md` | Agent templates — YAML frontmatter (id, capabilities, model, limits) + markdown persona |
| `config/orchestrator/templates/*_temp.md` | Tracked templates for the persona docs (SOUL, USER, IDENTITY, BOOTSTRAP) |
| `config/orchestrator/SOUL.md` etc. | Live persona docs — generated at first run from the templates; USER.md is auto-extracted from conversations |
| `config/skills/*/SKILL.md` | Skill definitions with trigger patterns (routing intents/keywords) and required tools |

Config dir resolution: `OPENALPACA_CONFIG_DIR` env var → walk up from exe looking for `config/llm.toml` → walk up from CWD → fallback `./config`.

LLM secret resolution order: `secret_env` (env var) > `secret_ref` (OS keychain) > `secret_encrypted` (AES-256-GCM local).

## Code Navigation

| System | Location |
|---|---|
| Agent loop reference doc | `docs/agent-loop.md` |
| Orchestrator | `crates/openalpaca_core/src/orchestrator/mod.rs` |
| Intent parsing | `crates/openalpaca_core/src/orchestrator/intent/` |
| Task dispatcher | `crates/openalpaca_core/src/orchestrator/dispatcher/` |
| Sequential pipeline | `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline.rs` |
| DAG execution | `crates/openalpaca_core/src/orchestrator/dispatcher/dag.rs` + `crates/openalpaca_core/src/runner/dag_executor/` |
| Lead agent dispatch | `crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs` |
| Task planner | `crates/openalpaca_core/src/orchestrator/task_planner/` |
| Prompt composition / persona | `crates/openalpaca_core/src/compose/` |
| Persona extraction middleware | `crates/openalpaca_core/src/middleware/` (soul/user/identity/bootstrap) |
| Daemon config types | `crates/openalpaca_core/src/daemon_config/` |
| LLM router | `crates/openalpaca_llm/src/routing/router/` |
| LLM providers | `crates/openalpaca_llm/src/providers/` (`anthropic/`, `openai/`, `ollama.rs`) |
| Key pool | `crates/openalpaca_llm/src/keys/key_pool/` |
| Rate limiter | `crates/openalpaca_llm/src/routing/rate_limiter/` |
| Model registry | `crates/openalpaca_llm/src/routing/model_registry/` |
| Cost tracking | `crates/openalpaca_llm/src/routing/cost_tracker/` |
| Database | `crates/openalpaca_storage/src/database/` |
| Migrations | `crates/openalpaca_storage/src/migrations/` |
| Repositories | `crates/openalpaca_storage/src/repository/` |
| Memory (hybrid search) | `crates/openalpaca_storage/src/repository/memory/` |
| Event bus | `crates/openalpaca_core/src/bus.rs` |
| Tool registry | `crates/openalpaca_core/src/tools/` |
| MCP tool bridge | `crates/openalpaca_core/src/tools/mcp/` |
| MCP client | `crates/openalpaca_mcp/src/` |
| Plugin system | `crates/openalpaca_plugins/src/` |
| Security gate | `crates/openalpaca_core/src/security/` |
| Agent definitions | `crates/openalpaca_core/src/agent/` |
| Daemon routes | `apps/openalpacad/src/routes/` |
| CLI commands | `apps/openalpaca/src/commands/` |
| GUI (Tauri) | `apps/openalpaca-gui/src-tauri/` (Rust) + `apps/openalpaca-gui/src/` (SvelteKit) |
| Wake/scheduler | `crates/openalpaca_wake/src/` |
| Connectors | `crates/openalpaca_connectors/src/` |

User-facing manuals live in `docs/` (`CLI_Manual.md`, `Daemon_Manual.md`, `GUI_Manual.md`, `Installation_Manual.md`, `QuickStart_Manual.md`, `Skill_Template_Reference.md`).

## Feature Flags

`openalpaca_llm` uses feature flags for provider compilation:
- `anthropic`, `openai`, `ollama` — each gates the respective provider + `reqwest`
- `local-embeddings` — gates `fastembed` for local embedding model

`openalpaca_connectors`:
- `telegram` — gates `teloxide`
- `imessage` — gates `rusqlite` (macOS only)
- `discord` — gates `twilight-gateway`/`twilight-http`/`twilight-model` + `reqwest`/`rustls`

The daemon (`openalpacad`) enables all provider features (`anthropic`, `openai`, `ollama`, `local-embeddings`) and all three connector features.
