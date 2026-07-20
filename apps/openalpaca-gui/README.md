# OpenAlpaca GUI

Desktop client for OpenAlpaca, built with Tauri v2 and a SvelteKit (Svelte 5) + Tailwind CSS v4 frontend.

The Rust side of the app is deliberately thin: it discovers a running `openalpacad` daemon via `discovery.json` and spawns one if none is running. Everything else — chat, tasks, agents, settings, plugins, connectors — talks directly from the webview to the daemon's HTTP/WebSocket/SSE API. The daemon binary is bundled with the app as a Tauri sidecar.

## Prerequisites

- [Bun](https://bun.sh/) — package manager and script runner
- Rust toolchain (pinned by the workspace `rust-toolchain.toml`) — needed to build both the Tauri backend and the `openalpacad` sidecar

## Development

```bash
bun install                      # install JS dependencies
bun run tauri dev                # full app: builds sidecar, starts Vite + Tauri with hot reload
bun run dev                      # frontend-only Vite dev server (port 1420, strict)
bun run build                    # production frontend build (output: build/)
bun run check                    # type-check with svelte-check
bun run check:watch              # type-check in watch mode
```

`tauri dev` and `tauri build` automatically run the sidecar preparation step first (`beforeDevCommand` / `beforeBuildCommand` in `src-tauri/tauri.conf.json`). The script `scripts/prepare-sidecar.ts`:

1. detects the host target triple via `rustc -vV`,
2. runs `cargo build -p openalpacad` at the workspace root (`--release` for build),
3. copies the binary to `src-tauri/bin/openalpacad-{triple}[.exe]`, skipping the rebuild if it is already up to date.

You can also run it manually with `bun run prepare:sidecar:dev` or `bun run prepare:sidecar:release`. `tauri.conf.json` declares `bundle.externalBin: ["bin/openalpacad"]`, so the daemon ships inside the app bundle.

## Architecture

### Tauri backend (`src-tauri/src/lib.rs`)

Exposes exactly two commands:

- `get_connection_info` — reads `discovery.json` from the app data directory, rejects it if the token is expired, and returns `{ baseUrl, token, instanceId }`.
- `ensure_daemon_running` — returns the existing connection if discovery is valid; otherwise spawns `openalpacad` as a detached process (looked up next to the GUI executable; debug builds fall back to `PATH`) and polls `discovery.json` every 200 ms for up to 5 s. The spawned daemon gets `OPENALPACA_CONFIG_DIR` pointed at `<app data dir>/config`.

Note: a non-expired `discovery.json` is trusted without a health check, so a stale file left by a crashed daemon can be returned as valid.

### Frontend (`src/`)

Single-page app: `adapter-static` with `fallback: "index.html"`, `ssr = false`, and one route (`src/routes/+page.svelte`). Tabs and the settings drawer are component state, not routes.

```
src/lib/
  daemon.ts           # WebSocket to /v1/events (ServerEvent union, auto-reconnect with backoff)
  daemon_control.ts   # daemon shutdown via POST /v1/command
  connectors.ts       # connector management client
  markdown.ts         # marked + highlight.js + KaTeX + DOMPurify rendering
  types.ts            # REST payload types
  api/                # one REST client module per daemon resource (17 files)
  stores/             # Svelte stores per domain (tasks, agents, chat, settings, ...)
  components/         # 32 feature components (ChatPanel, SettingsDrawer, panels, ...)
  ui/                 # small design-system primitives (Badge, Button, Card, Dialog, ...)
```

On mount, the app invokes `ensure_daemon_running`, opens a WebSocket to `/v1/events`, and loads initial data. Reconnects use exponential backoff (1 s → 30 s with jitter); if the daemon's `instanceId` changed after a reconnect, the app fully re-bootstraps. Chat uses `POST /v1/chat` followed by an SSE stream (`/v1/chat/stream/:streamId`) for thinking/delta/done events, including tool-confirmation approve/deny prompts. All REST calls send `Authorization: Bearer <token>`; WS and SSE pass the token as a `?token=` query parameter.

Feature surface: streaming chat with markdown/KaTeX/code rendering and file attachments, conversation history, task and agent monitoring/management, agent templates, skill health, plugin approval/enable/disable, connector control, LLM provider/API key management, usage and cost stats, orchestrator latency and dispatch decisions, and an event log.

## Security

- Strict CSP (`tauri.conf.json`): `connect-src` is limited to `'self'` plus localhost HTTP/WS — the webview can only talk to a local daemon.
- Native drag-and-drop is disabled (`dragDropEnabled: false`) so the webview handles file drops itself.
- Tauri plugins enabled: `dialog` and `fs` (default capabilities only).

## Files and paths

- `~/Library/Application Support/OpenAlpaca/discovery.json` (macOS) — daemon base URL, bearer token, and instance id; read-only for the GUI.
- `~/Library/Application Support/OpenAlpaca/config/` — created by the GUI and passed to the spawned daemon as `OPENALPACA_CONFIG_DIR`.
- Dev server: port 1420 (strict); HMR on 1421 when `TAURI_DEV_HOST` is set.

## Tests

`src/lib/stores/chat.test.js` uses `bun:test`; there is no `test` script in `package.json`, so run it manually:

```bash
bun test
```
