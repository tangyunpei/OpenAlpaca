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
cargo build                      # debug build (all crates + apps)
cargo build --release            # release build
cargo test                       # run all tests
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
  openalpaca_core/      # Orchestrator, agents, tools, runtime, bus, security
  openalpaca_llm/       # LLM router, providers (Anthropic/OpenAI/Ollama), key management
  openalpaca_storage/   # SQLite (rusqlite + sqlite-vec), repositories, migrations
  openalpaca_api/       # Shared event types (WakeEvent)
  openalpaca_wake/      # Cron scheduler + filesystem watcher
  openalpaca_connectors/# Chat platform adapters (Telegram, etc.)
  openalpaca_platform/  # Platform abstractions (placeholder)
  openalpaca_platform_macos/  # macOS-specific (placeholder)
config/
  daemon.toml           # Execution limits, server settings, memory/cost budgets
  llm.toml              # Provider keys, model registry, embedding config
  agents/               # Agent template definitions (markdown with YAML frontmatter)
  orchestrator/         # SOUL.md, USER.md, IDENTITY.md persona documents
  skills/               # Skill definitions (SKILL.md files)
  tools/                # Tool configuration
```

### Dependency Graph

```
openalpaca_api          ← leaf (no internal deps)
openalpaca_llm          ← leaf (no internal deps)
openalpaca_storage      ← leaf (no internal deps)
openalpaca_wake         ← openalpaca_api
openalpaca_connectors   ← openalpaca_api, openalpaca_core, openalpaca_storage
openalpaca_core         ← openalpaca_api, openalpaca_llm, openalpaca_storage
openalpacad             ← all crates
openalpaca (CLI)        ← openalpaca_core, openalpaca_llm, openalpaca_storage
openalpaca_gui          ← openalpaca_api, openalpaca_storage
```

### Three Execution Modes

The orchestrator dispatches complex tasks via `TaskDispatcher` with three paths:

1. **Sequential Pipeline** (`dispatcher/pipeline.rs`) — agents run in order, each receiving the previous agent's output. One `tokio::spawn` per pipeline.
2. **DAG Execution** (`dispatcher/dag.rs` + `runner/dag_executor.rs`) — independent nodes run concurrently up to `max_concurrent_agents` (default 3). Optional replanning after N nodes.
3. **Lead Agent** (`dispatcher/lead_agent.rs`) — single orchestrating agent with `lead_orchestration` skill handles delegation autonomously.

Selection is driven by the task planner's output: `plan.use_lead_agent` → lead agent, `plan.dag` present → DAG, otherwise → sequential pipeline.

### Intent Routing

The `Orchestrator` classifies incoming messages into intents before dispatch:
- `SimpleQuery` → direct LLM call
- `TaskQuery` → query task registry
- `ComplexTask` → dispatch to agents via `TaskDispatcher`
- `TaskControl` → manage task lifecycle (cancel/pause/resume)

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

`TaskRepository::update_state()` uses optimistic locking (`state_version` column). On version conflict, callers retry with exponential backoff (10ms × 2^attempt, max 3 retries).

### Storage

Single SQLite connection wrapped in `Arc<Mutex<Connection>>`. WAL mode, `busy_timeout=5000ms`. Schema managed via 21 numbered migrations. Memory search is hybrid: FTS5 full-text + sqlite-vec 768-dim KNN with cascading scope (workspace → global).

Data directory: `~/Library/Application Support/OpenAlpaca/` (macOS). DB: `openalpaca.db`, lock: `openalpacad.lock`, discovery: `discovery.json`.

## Configuration

| File | Purpose |
|---|---|
| `config/daemon.toml` | Execution limits (max_rounds, max_cost), DAG settings, server config, memory budgets, security |
| `config/llm.toml` | Provider credentials (AES-256-GCM encrypted), model registry with pricing, embedding config, fallback chains |
| `config/agents/*.md` | Agent templates — YAML frontmatter (id, skills, model, limits) + markdown persona |
| `config/orchestrator/SOUL.md` | System personality/guidelines |
| `config/orchestrator/USER.md` | User profile (auto-extracted from conversations) |
| `config/orchestrator/IDENTITY.md` | Agent identity |
| `config/skills/*/SKILL.md` | Skill definitions with trigger patterns and required tools |

Config dir resolution: `OPENALPACA_CONFIG_DIR` env var → walk up from exe looking for `config/llm.toml` → walk up from CWD → fallback `./config`.

LLM secret resolution order: `secret_env` (env var) > `secret_ref` (OS keychain) > `secret_encrypted` (AES-256-GCM local).

## Code Navigation

| System | Location |
|---|---|
| Agent loop reference doc | `docs/agent-loop.md` |
| Orchestrator | `crates/openalpaca_core/src/orchestrator/mod.rs` |
| Intent parsing | `crates/openalpaca_core/src/orchestrator/intent.rs` |
| Task dispatcher | `crates/openalpaca_core/src/orchestrator/dispatcher/` |
| Sequential pipeline | `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline.rs` |
| DAG execution | `crates/openalpaca_core/src/orchestrator/dispatcher/dag.rs` + `runner/dag_executor.rs` |
| Lead agent dispatch | `crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs` |
| Task planner | `crates/openalpaca_core/src/orchestrator/task_planner.rs` |
| LLM router | `crates/openalpaca_llm/src/router.rs` |
| LLM providers | `crates/openalpaca_llm/src/providers/{anthropic,openai,ollama}.rs` |
| Key pool / rate limiter | `crates/openalpaca_llm/src/key_pool.rs`, `rate_limiter.rs` |
| Model registry | `crates/openalpaca_llm/src/model_registry.rs` |
| Cost tracking | `crates/openalpaca_llm/src/cost_tracker.rs` |
| Database | `crates/openalpaca_storage/src/database.rs` |
| Migrations | `crates/openalpaca_storage/src/migrations/` |
| Repositories | `crates/openalpaca_storage/src/repository/` |
| Memory (hybrid search) | `crates/openalpaca_storage/src/repository/memory.rs` |
| Event bus | `crates/openalpaca_core/src/bus.rs` |
| Tool registry | `crates/openalpaca_core/src/tools/` |
| Security gate | `crates/openalpaca_core/src/security/` |
| Agent definitions | `crates/openalpaca_core/src/agent/` |
| Daemon routes | `apps/openalpacad/src/routes/` |
| CLI commands | `apps/openalpaca/src/commands/` |
| GUI (Tauri) | `apps/openalpaca-gui/src-tauri/` (Rust) + `apps/openalpaca-gui/src/` (SvelteKit) |
| Wake/scheduler | `crates/openalpaca_wake/src/` |
| Connectors | `crates/openalpaca_connectors/src/` |

## Feature Flags

`openalpaca_llm` uses feature flags for provider compilation:
- `anthropic`, `openai`, `ollama` — each gates the respective provider + `reqwest`
- `local-embeddings` — gates `fastembed` for local embedding model

`openalpaca_connectors`:
- `telegram` — gates `teloxide` dependency

The daemon (`openalpacad`) enables all features.
