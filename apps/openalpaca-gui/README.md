# OpenAlpaca GUI

Desktop client for OpenAlpaca: a Tauri v2 shell around a React 19 + TypeScript + Tailwind CSS v4 frontend.

The Rust side is deliberately thin — it discovers a running `openalpacad` daemon via `discovery.json` and spawns one if none is running. Everything else (chat, runs, library, settings) talks straight from the webview to the daemon's HTTP / WebSocket / SSE API. The daemon binary ships with the app as a Tauri sidecar.

## Stack

| Concern      | Choice                                                                                                                                                 |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Shell        | Tauri 2 (`src-tauri/`, unchanged by the UI rework)                                                                                                     |
| UI           | React 19, TypeScript 5.9 (`strict`), Vite 7                                                                                                            |
| Styling      | Tailwind CSS 4 via `@tailwindcss/vite`, CSS-first `@theme` in `src/styles.css`                                                                         |
| Server state | TanStack Query v5                                                                                                                                      |
| Client state | Zustand v5                                                                                                                                             |
| Variants     | `tailwind-variants` + `clsx` + `tailwind-merge` (`src/lib/tv.ts`, `src/lib/cn.ts`)                                                                     |
| Fonts        | `@fontsource/ibm-plex-sans` / `-mono`, self-hosted                                                                                                     |
| Markdown     | `marked` + `dompurify` (document preview); `highlight.js` is declared for code highlighting but not wired yet — `CodePreview` renders plain diff lines |
| Tests        | Vitest + Testing Library + jsdom                                                                                                                       |
| Icons        | none — the design uses four inline SVGs (`src/lib/icons.tsx`) and text glyphs                                                                          |

Fonts are bundled, never linked: the Tauri CSP declares no `font-src`, so a Google Fonts stylesheet would be blocked at runtime. The design is **light-only** (warm paper); there is no dark mode and no `prefers-color-scheme` branch.

## Prerequisites

- [Bun](https://bun.sh/) — package manager and script runner
- Rust toolchain (pinned by the workspace `rust-toolchain.toml`) — builds both the Tauri shell and the `openalpacad` sidecar

## Commands

Run these from `apps/openalpaca-gui`.

```bash
bun install                # install JS dependencies
bun run tauri dev          # full app: builds the sidecar, then Vite + Tauri with hot reload
bun run dev                # frontend-only Vite dev server (port 1420, strict)
bun run build              # production build — tsc -b, then vite build (output: dist/)
bun run check              # type-check only (app + node configs), no emit
bun run test               # vitest run
bun run test:watch         # vitest in watch mode
bun run format             # prettier --write .
bun run format:check       # prettier --check .
```

The Rust shell is checked from the workspace root with `cargo check -p openalpaca_gui`.

`tauri dev` and `tauri build` run the sidecar step first (`beforeDevCommand` / `beforeBuildCommand` in `src-tauri/tauri.conf.json`). `scripts/prepare-sidecar.ts`:

1. detects the host target triple via `rustc -vV`,
2. runs `cargo build -p openalpacad` at the workspace root (`--release` for build),
3. copies the binary to `src-tauri/bin/openalpacad-{triple}[.exe]`, skipping the rebuild if it is already up to date.

Run it by hand with `bun run prepare:sidecar:dev` or `bun run prepare:sidecar:release`. `tauri.conf.json` declares `bundle.externalBin: ["bin/openalpacad"]`, so the daemon ships inside the app bundle.

## The two contracts

Everything in `src/` is written against two checked-in documents. Read the relevant section before changing a surface.

- **`DESIGN_SPEC.md`** — §1 tokens (§1.9 is the Tailwind `@theme` block that `src/styles.css` carries), §2 layout skeleton, §3 component inventory (3.1–3.36), §4 interaction spec (state machine, handlers, the ordered Escape ladder), §5 views, §6 icons. Fractional font sizes (9.5 / 10.5 / 11.5 / 12.5 / 13.5 / 14.5 px) are load-bearing tokens — never round them to Tailwind defaults.
- **`API_MAP.md`** — §1 connection and auth (`discovery.json` → Tauri commands → `Bearer` / `?token=`), §2 endpoint map per view, §3 the gap list, §4 the streaming contract (SSE lifecycle + WS).

### No mock data, ever

Twenty-three design surfaces have no daemon route behind them (API_MAP §3): the Library listing, artifact versioning and diffs, the subagent timeline, per-run event history, uptime, and so on. None of them is faked.

`src/lib/unavailable.ts` is the single registry: `GAPS` describes every gap (label, missing API, proposed endpoint, what it blocks) and `Availability<T>` is the result type every unbacked adapter returns. A view renders the design's own empty-state copy plus a muted note naming the missing route — `gapNote(gap)` for the short form, `gapDetail(result)` when the proposed endpoint belongs on screen too. Adapters live in `src/lib/api/unbacked.ts` and are exposed as hooks in `src/hooks/useUnbacked.ts`; each `available` branch is written against the proposed contract, so the day a route lands only the adapter body changes.

The daemon is never modified to close a gap. The one allowance is sending fields the daemon currently ignores (e.g. `approval_scope`) — serde drops unknown fields, and it keeps the client honest and forward-compatible.

## Architecture

### Tauri shell (`src-tauri/src/lib.rs`)

Two commands, unchanged by the rework:

- `get_connection_info` — reads `discovery.json` from the app data directory, rejects it if the token is expired, returns `{ baseUrl, token, instanceId }`.
- `ensure_daemon_running` — returns the existing connection if discovery is valid; otherwise spawns `openalpacad` detached (looked up next to the GUI executable; debug builds fall back to `PATH`) and polls `discovery.json` every 200 ms for up to 5 s, with `OPENALPACA_CONFIG_DIR` pointed at `<app data dir>/config`.

Note: a non-expired `discovery.json` is trusted without a health check, so a stale file left by a crashed daemon can be returned as valid.

### Frontend (`src/`)

```
src/
  App.tsx              # the frame: providers, rail, view switch, overlays, global keys
  main.tsx             # fonts, styles, React root
  styles.css           # the @theme token block (DESIGN_SPEC §1.9)
  lib/
    connection.ts      # discovery → base URL + token; bootstrap and refresh
    http.ts            # fetch wrapper, ApiError, Bearer auth
    chat-stream.ts     # the SSE state machine (API_MAP §4.1)
    events.ts          # the /v1/events WebSocket client + ServerEvent union
    query-client.ts    # QueryClient + the event → cache invalidation map
    query-keys.ts      # one key namespace per domain
    unavailable.ts     # the gap registry and Availability<T>
    api/               # one REST client module per daemon resource
  hooks/               # TanStack Query hooks over lib/api
  stores/
    ui.ts              # view, density, panes, overlays, pins, the Escape ladder
    confirmation.ts    # the pending tool confirmation, published out of chat
    pane-widths.ts     # persisted column widths
  components/
    shell/             # AppShell, NavRail, Resizer, useGlobalKeys
    ui/                # design-system primitives (§3.1–§3.9)
    chat/              # composer, transcript rows, file panel
    work/              # WorkPane, run cards, diff view, the seven artifact renderers
    overlays/          # command palette, toast host, shortcut bindings
  views/               # chat/, work/, library/, settings/ — one folder per view
```

**Data flow.** `QueryProvider` mounts the cache _and_ the daemon socket. Live `ServerEvent` frames map onto query keys through `invalidationKeysFor` (`lib/query-client.ts`), so views follow the daemon without polling. The socket is best-effort — the daemon drops frames for a lagged subscriber and never replays — so a reconnect or a changed `instance_id` fires a resync signal that invalidates everything.

**Shared seams.** The chat aside is one slot with two modes: `components/work/WorkPane` in work mode, `views/chat/FilePanelSlot` in file mode. The artifact renderers (`components/work/preview`) and the diff view (`components/work/DiffView`) are shared between the chat file panel (`size="compact"`) and the Library detail (`size="full"`) — there is exactly one implementation of each.

**Keyboard.** `App.tsx` owns the whole global surface, once: `useGlobalKeys` binds ⌘K and the strictly ordered Escape ladder (palette → artifact picker → file panel → deny the pending tool call), plus Enter-approves while blocked in the chat view with the palette shut. `useCommandShortcuts` binds the palette's own shortcuts off the same catalogue the palette draws (`useCommandCatalog`). The chat lane publishes its pending confirmation through `stores/confirmation`, which is what lets the key ladder, the palette's `Approve` row and the rail's blocked lane bar all read one source.

## Security

- Strict CSP (`tauri.conf.json`): `connect-src` is limited to `'self'` plus localhost HTTP/WS — the webview can only reach a local daemon. There is no `font-src`, hence self-hosted fonts.
- Native drag-and-drop is disabled (`dragDropEnabled: false`) so the webview handles file drops itself.
- Tauri plugins enabled: `dialog` and `fs` (default capabilities only).
- Markdown and any embedded HTML are rendered through `marked` and sanitised with DOMPurify before they reach the DOM (`components/work/preview/DocumentPreview.tsx`, `MediaPreview.tsx`). The chat transcript deliberately does _not_ run a markdown pipeline — §3.10's body is prose plus inline code, parsed by two rules in `components/chat/prose.ts`.

## Files and paths

- `~/Library/Application Support/OpenAlpaca/discovery.json` (macOS) — daemon base URL, bearer token, instance id; read-only for the GUI.
- `~/Library/Application Support/OpenAlpaca/config/` — created by the GUI and passed to the spawned daemon as `OPENALPACA_CONFIG_DIR`.
- Dev server: port 1420, strict (`tauri.conf.json` `devUrl` points at it); `src-tauri/` is excluded from the watcher.
- Pins and pane widths are per-machine `localStorage` keys (`oa-pins`, `oa-pane-widths`) — GAP-12 records that there is no server-side pin.

## Tests

```bash
bun run test
```

Vitest with the jsdom environment; `vitest.setup.ts` loads `@testing-library/jest-dom`. The suite covers logic, not pixel styling: the SSE and WebSocket state machines, the event → cache map, the routing and formatting helpers, the Escape ladder and command catalogue, and view-level integration tests that double only the two transports (`fetch`, `EventSource`) and the Tauri discovery command — so request bodies are asserted on the wire rather than on a spy.
