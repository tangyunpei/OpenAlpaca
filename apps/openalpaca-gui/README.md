# OpenAlpaca GUI

Desktop client for OpenAlpaca: a Tauri v2 shell around a React 19 + TypeScript + Tailwind CSS v4 frontend.

The Rust side is deliberately thin. It finds a running `openalpacad` daemon through `discovery.json`, and spawns one if none is alive. Everything else — chat, runs, library, settings — talks straight from the webview to the daemon's HTTP / WebSocket / SSE API. The daemon binary ships inside the app as a Tauri sidecar.

This file is for people changing the GUI. For using it, read [`docs/GUI_Manual.md`](../../docs/GUI_Manual.md).

## Stack

| Concern      | Choice                                                                                                              |
| ------------ | ------------------------------------------------------------------------------------------------------------------- |
| Shell        | Tauri 2 (`src-tauri/`), with the `dialog` plugin only                                                               |
| UI           | React 19, TypeScript 7 (native compiler, `strict`), Vite 7                                                          |
| Styling      | Tailwind CSS 4 via `@tailwindcss/vite`, CSS-first `@theme` in `src/styles.css`                                      |
| Server state | TanStack Query v5                                                                                                   |
| Client state | Zustand v5                                                                                                          |
| Variants     | `tailwind-variants` + `clsx` + `tailwind-merge` (`src/lib/tv.ts`, `src/lib/cn.ts`)                                  |
| Fonts        | `@fontsource/ibm-plex-sans` / `-mono`, self-hosted                                                                  |
| Markdown     | `marked` + `dompurify` for the artifact document preview; the chat transcript uses its own small parser (see below) |
| Code preview | No syntax highlighter. `CodePreview` renders plain, diff-annotated lines                                            |
| Tests        | Vitest + Testing Library + jsdom                                                                                    |
| Icons        | none — the design uses four inline SVGs (`src/lib/icons.tsx`) and text glyphs                                       |

Exact versions are pinned in `package.json`.

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

Two things to know before the first run:

- **`bun run tauri dev` uses your real store.** The shell reads `~/.openalpaca/state/discovery.json` and, when it has to spawn a daemon, starts it with `OPENALPACA_CONFIG_DIR=~/.openalpaca/config` — not the repository's `config/`. Set `OPENALPACA_HOME_STORE` to an absolute path to point the whole thing at a scratch root. If a daemon is already running (from the CLI, or `cargo run -p openalpacad`), the app connects to that one instead of spawning.
- **`bun run dev` alone has no daemon connection.** A plain browser has no Tauri bridge, so the two connection commands fail and the rail's status line reads `connection error`. It is good for layout work, not for talking to a daemon.

### The sidecar

`tauri dev` and `tauri build` run the sidecar step first (`beforeDevCommand` / `beforeBuildCommand` in `src-tauri/tauri.conf.json`). `scripts/prepare-sidecar.ts`:

1. detects the host target triple via `rustc -vV`,
2. runs `cargo build -p openalpacad` at the workspace root (`--release` for build),
3. copies the binary to `src-tauri/bin/openalpacad-{triple}[.exe]`.

Steps 2 and 3 are skipped when the copy already exists and is at least as new as `target/<profile>/openalpacad`. The script compares the two **binaries**, not the daemon's source: after editing daemon code, run `cargo build -p openalpacad` yourself (or delete the copy in `src-tauri/bin/`) so the next `tauri dev` picks the change up. Stop any daemon an earlier session left running too — it is detached, and the app reuses a live one rather than spawning the new binary.

Run it by hand with `bun run prepare:sidecar:dev` or `bun run prepare:sidecar:release`. `tauri.conf.json` declares `bundle.externalBin: ["bin/openalpacad"]`, so the daemon ships inside the app bundle.

### Before you push: the four CI gates

The `gui` job in [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) runs these four, in this order. Run all four locally.

```bash
bun run check
bun run test
bun run format:check
bun run build
```

**`check` is not `build`.** `check` is `tsc --noEmit` over the two tsconfigs. `build` is `tsc -b` followed by `vite build` — a different compiler mode plus the bundler — so it can fail where `check` passed (a bad dynamic import, a Tailwind token typo). A green `check` does not mean a green `build`.

`format:check` covers the Markdown here too (`README.md`, `API_MAP.md`, `DESIGN_SPEC.md`). Run `bun run format` after editing any of them.

## The two contracts

Everything in `src/` is written against two checked-in documents. Read the relevant section before changing a surface.

- **[`DESIGN_SPEC.md`](DESIGN_SPEC.md)** — §1 tokens (§1.9 is the Tailwind `@theme` block that `src/styles.css` carries), §2 layout skeleton, §3 component inventory (3.1–3.36), §4 interaction spec (state machine, handlers, the ordered Escape ladder), §5 views (§5.6 lists what the design only hints at, such as the composer's `Attach`), §6 assets, §7 backend wiring, §8 implementation notes. Fractional font sizes (9.5 / 10.5 / 11.5 / 12.5 / 13.5 / 14.5 px) are load-bearing tokens — never round them to Tailwind defaults.
- **[`API_MAP.md`](API_MAP.md)** — §1 connection and auth (`discovery.json` → Tauri commands → `Bearer` / `?token=`), §2 endpoint map per view, §3 the gap list and how each gap closed, §4 the streaming contract (SSE lifecycle + WebSocket), §5 notes for the implementer (error envelopes among them).

### No mock data, ever

The GUI renders what the daemon serves and nothing else. When the design shows a surface no route backs, the view shows the design's own empty-state copy plus a muted note naming what is missing — never placeholder rows that look like data.

`src/lib/unavailable.ts` is the single registry of those surfaces. Two entries are left:

| Gap      | Surface                                    | What is missing                                                                |
| -------- | ------------------------------------------ | ------------------------------------------------------------------------------ |
| `GAP-17` | Settings → Connectors, `Connect service`   | No route adds a connector; connectors are compiled into the daemon             |
| `GAP-20` | Settings → Agents, the per-template toggle | Agent templates have no `enabled` flag, and nothing would enforce one on spawn |

Each entry in `GAPS` carries a label, the missing API, a proposed endpoint and the surface it blocks. `gapNote(gap)` is the sentence a view prints, and it is read at the point of use — `CONNECTOR_ADD_NOTE` in `hooks/useConnectors.ts`, `TEMPLATE_TOGGLE_NOTE` in `hooks/useAgents.ts`. `Availability<T>` is the result type for an adapter whose route does not exist yet.

If you add a surface whose route does not exist, add a registry entry and render its note. Closing a gap is daemon work; when the route lands, the entry goes away. API_MAP §3 records how every earlier gap closed.

## Architecture

### Tauri shell (`src-tauri/src/lib.rs`)

Four commands. The first two return `{ baseUrl, token, instanceId }`:

- `ensure_daemon_running` — the boot path. It reads `discovery.json` and probes the daemon's listen address with a TCP connect (300 ms). A live daemon is used as-is, even when the file's own 24 h expiry has lapsed. A dead or missing one is replaced: the shell spawns `openalpacad` detached and polls every 200 ms, for up to 5 s, until the new daemon accepts connections. A stale file left by a crashed daemon therefore never gets returned. A daemon that does not come up is reported with the end of what this spawn wrote to `daemon.log` (up to 40 lines), or with the fact that it wrote nothing.
- `get_connection_info` — the reconnect path. It reads `discovery.json` and rejects it if the token has expired. It does not probe liveness.
- `read_daemon_log_tail(lines)` — the last `lines` lines of `state/logs/daemon.log`, `""` when there is none. It reads the file directly, so it answers when nothing serves `/v1/*`; it backs Settings → Connection's `Show daemon log`.
- `await_daemon_stopped` — after the webview has asked the daemon to shut down (`POST /v1/command`), waits up to 15 s for its process to exit and then for its singleton lock to be free, and returns `{ outcome, pid, waitedMs }` (`outcome`: `not_running`, `stopped`, `lock_still_held` or `still_alive`). It signals nothing: the shell never kills a process.

Spawning, in detail:

- The binary is looked up next to the GUI executable. Debug builds fall back to `PATH`; release builds fail with an "incorrectly installed" error.
- The shell creates `~/.openalpaca/` and `~/.openalpaca/config/` first (`store::ensure_store`, `store::ensure_runtime_config_dir`). The daemon ignores an `OPENALPACA_CONFIG_DIR` that does not exist, so the directory has to be there before the spawn.
- The child runs with its working directory at the store root and `OPENALPACA_CONFIG_DIR` set to that config directory. Its stdin is null; its stdout and stderr are appended to `state/logs/daemon.log` — the file `openalpaca daemon start` writes, rotated the same way (16 MB, three older generations) — and it is started with `store::MANAGED_LOG_ENV` set, so `GET /v1/status` reports that file as its `log_path` (T30). It is detached (`setsid` on Unix, `DETACHED_PROCESS` on Windows), so it outlives the app; on Unix a thread waits on it so that it is reaped when it exits.

The webview never reads `discovery.json` itself. `src/lib/connection.ts` wraps the four commands, caches the connection info the first two return, and treats a changed `instanceId` as a daemon restart: every id the client holds is dead, so it re-bootstraps and drops all server-derived state.

### Frontend (`src/`)

```
src/
  App.tsx                # the frame: providers, rail, view switch, overlays, global keys
  main.tsx               # fonts, styles, React root
  styles.css             # the @theme token block (DESIGN_SPEC §1.9)
  lib/
    connection.ts        # discovery → base URL + token; bootstrap and refresh
    http.ts              # fetch wrapper, ApiError, Bearer auth, the error envelopes
    chat-stream.ts       # the SSE state machine (API_MAP §4.1)
    events.ts            # the /v1/events WebSocket client + ServerEvent union
    query-client.ts      # QueryClient + the event → cache invalidation map
    query-provider.tsx   # mounts the cache and opens the socket
    query-keys.ts        # one key namespace per domain
    time.ts              # the one parser for daemon timestamps
    workspace-header.ts  # x-workspace-path encoding
    unavailable.ts       # the gap registry and Availability<T>
    api/                 # one REST client module per daemon resource
  hooks/                 # TanStack Query hooks over lib/api
  stores/
    ui.ts                # view, density, panes, overlays, the pin cache, the Escape ladder
    confirmation.ts      # the pending tool confirmation, published out of chat
    project.ts           # the chosen project path (sent as x-workspace-path)
    session.ts           # which conversation the composer addresses
    pane-widths.ts       # persisted column widths
  components/
    shell/               # AppShell, NavRail, Resizer, useGlobalKeys, pane-fit
    ui/                  # design-system primitives: Button, Badge, Tab, StatusDot, LaneBar, FileBadge, Toast, …
    chat/                # composer, attachment chips, transcript rows, prose parser, file panel
    work/                # WorkPane, run cards, diff view, the seven artifact renderers
    overlays/            # command palette, toast host, shortcut bindings
  views/                 # chat/, work/, library/, settings/ — one folder per view
```

**Data flow.** `QueryProvider` mounts the cache _and_ the daemon socket. Live `ServerEvent` frames map onto query keys through `invalidationKeysFor` (`lib/query-client.ts`), so most views follow the daemon without polling. The socket is best-effort — the daemon drops frames for a lagged subscriber and never replays — so a reconnect or a changed `instance_id` fires a resync signal that invalidates everything. The socket is also **daemon-wide**: it carries every lane's frames, so a consumer filters on the lane, run or stream a frame names before acting on it.

**One socket.** `daemonEvents.connect()` may be called twice in a row (React StrictMode mounts, cleans up and mounts again; so do two quick Reconnect clicks). A generation counter on the client makes the superseded attempt a no-op, so there is never a second, unreachable socket delivering every frame twice.

**Shared seams.** The chat aside is one slot with two modes: `components/work/WorkPane` in work mode, `views/chat/FilePanelSlot` in file mode. The artifact renderers (`components/work/preview`) and the diff view (`components/work/DiffView`) are shared between the chat file panel (`size="compact"`) and the Library detail (`size="full"`) — there is exactly one implementation of each.

**Views are swapped, not stacked.** `App.tsx` renders one lazy view at a time, so leaving the chat view unmounts it and everything it held in `useState`. State that must outlive a trip to the Library lives in a store.

**Narrow windows.** The window's minimum is 1000 × 640. Below the width its columns need, the chat view collapses its side columns — the conversation list first, then the aside (`components/shell/pane-fit.ts`). It only collapses; an explicit expand stands until the next resize.

**Keyboard.** `App.tsx` owns the whole global surface, once: `useGlobalKeys` binds ⌘K and the strictly ordered Escape ladder (palette → artifact picker → file panel → deny the pending tool call), plus Enter-approves while blocked in the chat view with the palette shut. `useCommandShortcuts` binds the palette's own shortcuts off the same catalogue the palette draws (`useCommandCatalog`). The chat lane publishes its pending confirmation through `stores/confirmation`, which is what lets the key ladder, the palette's `Approve` row and the rail's blocked lane bar all read one source.

**Timestamps.** The daemon sends both RFC 3339 stamps and SQLite's zone-less `YYYY-MM-DD HH:MM:SS`, which is UTC. Parse every daemon timestamp with `lib/time.ts`; `new Date()` reads the zone-less form as local time.

## A chat turn

`views/chat/useChatSession.ts` is the glue: history, the live turn, confirmations, run reports. `ChatView` stays a layout. The contracts below are worth knowing before you touch any of it.

### The stream and its terminal frame

`POST /v1/chat` answers `{ stream_id, lane_key, model_used }`; the client opens `GET /v1/chat/stream/{id}?token=…` as an `EventSource` at once. `lib/chat-stream.ts` is a pure reducer over the frames:

| Frame                    | Meaning                                                                                                                                                                                            |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `thinking`               | The turn started. Never rewinds a stream that already has deltas.                                                                                                                                  |
| `delta`                  | The provider's own tokens, as they arrive. A live preview only.                                                                                                                                    |
| `reasoning`              | The model thinking out loud. Shown in the thinking indicator, capped to its last 4 000 characters.                                                                                                 |
| `confirmation_requested` | A tool call needs approval. Can arrive at any point and does not end the stream.                                                                                                                   |
| `confirmation_resolved`  | That prompt is over — answered, timed out or withdrawn.                                                                                                                                            |
| `done`                   | Terminal. Carries the full answer, the model that answered, token counts and the attachment lists.                                                                                                 |
| `error`                  | Two things arrive under this name. The daemon's frame has a JSON body and is terminal. `EventSource`'s own transport error has no data: the client lost the stream, the turn may still be running. |

Rules that follow from it:

- **`done.content` replaces the buffer.** The deltas of a multi-round turn do not add up to the answer (text written before a tool call streams too), and the daemon drops frames for a slow client. Render `state.content`, never your own concatenation.
- **Reasoning is never the answer.** It stays out of `content`, the daemon stores it nowhere, and history never replays it. It dies with the turn.
- **Close on the terminal frame.** The daemon forgets a stream 5 s after it ends and `EventSource` reconnects by itself, so a stream left open comes back as a 404.
- **After a terminal frame the answer is frozen, the confirmations are not.** A background run raises its prompt minutes after the turn that started it went `done`, so `pendingConfirmations` stays live until the conversation changes.
- **A turn never ends empty.** When the model wrote no text, `done.content` is a sentence from the daemon saying why. Render it like any answer. A turn that failed outright ends with `error` instead, and its `message` is the sentence to show.
- `done.model` reports what actually answered (the literal `default` when the turn reports no model). `model_used` on the POST is only a prediction, made before the turn ran.
- `done.delegation` is a workflow the turn started. `done.attachments_skipped` lists files the model never received, each with the daemon's reason.

The composer's model is seeded from `GET /v1/status` → `llm.effective_default_model`, not from the configured default (`views/chat/chat-model.ts`). A send that names a model the daemon cannot route is refused with `400 UNKNOWN_MODEL`, so the view holds a routable id or sends no `model` at all. Never hard-code a model id.

### Attaching files

`Attach` is a ghost button left of the model button. It opens a hidden `<input type="file" multiple>`, which needs no Tauri plugin and no capability. Pasting a file and dropping one on the composer take the same path; the drop works because `tauri.conf.json` sets `dragDropEnabled: false`, so the webview sees ordinary HTML drag events.

- `components/chat/attachments.ts` is the pure half: the chip state machine (`uploading` → `ready` or `failed`, no retry) and the cap.
- `views/chat/useComposerAttachments.ts` is the moving half: the upload, the refusal, the `file_id → filename` table.
- `components/chat/Composer.tsx` only renders what it is handed.

How a chip behaves:

1. A file uploads the moment it is picked, through `uploadFile` in `lib/api/files.ts` (`POST /v1/files/upload`, multipart, the normal authenticated fetch plus the `x-workspace-path` header).
2. A refusal is shown as the daemon's own sentence. A failed chip never travels.
3. Send is off while any upload is in flight.
4. Only `ready` chips reach `POST /v1/chat`, as `attachments: [{ file_id }]`.
5. The cap is 10 files a turn. It mirrors the daemon's default, which the daemon serves nowhere; the daemon is still the authority and refuses a turn over its own limit.
6. Chips clear once the daemon accepts the turn, and when the conversation changes. A refused send keeps them.

After the turn, `done.attachments_skipped` becomes a muted "Not sent to the model" note under the answer (`SkippedAttachmentsNote`). It is live-only, because nothing persists a skip. A stored user turn gets its file cards back from the row's `content_json` parts; `GET /v1/chat/history` has no `attachments` field.

The draft **text** is window-level on purpose. Only the chips belong to a conversation.

### Tool confirmations

A pending confirmation blocks the composer: the textarea is not rendered and the approval bar takes its place. Because that state is modal, the view never trusts one signal to raise or to clear it.

A prompt reaches the view three ways:

- the turn's own SSE `confirmation_requested` frame,
- the WebSocket `tool_confirmation_requested` frame, which adds `agent_id`, `lane_key`, `task_id` and `stream_id`,
- `GET /v1/chat/confirmations`, a snapshot of what is pending now. It seeds cards after a reload or a reconnect, and it is polled every 10 s while a card is up and every 30 s otherwise.

`confirmationBelongsHere` decides whether a prompt is this window's: it is this stream's, or it belongs to a run this lane started, or its lane is this one (a prompt with no lane is kept, since prompts raised inside a workflow carry none). Answering a foreign prompt with `Always allow` would widen someone else's run, so this filter is not optional.

A card is retired on any of three signals:

1. **The resolved frame.** `tool_confirmation_resolved` carries `approved`, `denied`, `timed_out` or `cancelled`. A timeout leaves a `Timed out` row saying the tool did not run.
2. **The snapshot no longer lists it.** Absence counts only after the card has been on screen for 2 s (`SNAPSHOT_SETTLE_GRACE_MS`), because a prompt raised while the GET was in flight is legitimately missing from its answer.
3. **The turn's terminal frame — main-loop prompts only.** A main-loop tool call blocks the loop, so a daemon-sent `done` or `error` means the daemon has stopped waiting. It retires only cards stamped with the stream that just ended: the `{owner}:gui` lane is shared, so another client's prompt looks the same. A card with a `task_id` belongs to a run and is left to that run's own `task_status`. A transport error is not a terminal frame.

Answering is `POST /v1/chat/confirmations/{request_id}`. `Always allow` sends `approval_scope: "entire_tool"`. The daemon then stops asking about that tool, whatever its arguments, inside the sandbox that raised the prompt. Each main-loop turn, each lead run and each subagent builds its own sandbox, so the approval lasts for the rest of that turn or that agent's run. It is not a saved allowlist.

Where the state lives:

| State                                        | Home                                                                                       | Lifetime                                                                                                               |
| -------------------------------------------- | ------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| Pending cards                                | `pendingConfirmations` in the stream state (`lib/chat-stream.ts`, held by `useChatStream`) | until resolved, or the conversation changes; lost with the chat view and seeded again from the snapshot when it mounts |
| The one confirmation other surfaces react to | `stores/confirmation.ts`                                                                   | published while `ChatView` is mounted and blocked                                                                      |
| `Approved` / `Denied` / `Timed out` rows     | `views/chat/resolution-store.ts`                                                           | the window's life; cleared when the conversation changes and on reload                                                 |
| Run-report and written-file cards            | `useState` in `useChatSession`                                                             | die with the chat view, by decision — the completion message is in history                                             |

An `Approved` row reads "waiting" until the matching `tool_executed` frame arrives. The match is on the tool name **and** the `agent_id` / `task_id` pair, not the name alone, because the socket carries every run's tools. The card names the blocked agent from `GET /v1/agent-templates`, so it reads the same live and after a reload; the main loop's id is `orchestrator`, which is in no template list, and is shown under the assistant's own name.

### The transcript's text

The chat transcript does not run a markdown pipeline. `components/chat/prose.ts` is a small parser over a fixed vocabulary, and `MessageBody` maps its output to elements:

- fenced code (with a language label), `#`–`####` headings, thematic breaks, pipe tables, one level of `>` blockquote,
- paragraphs, ordered and unordered lists (an ordered list starts at its own first number),
- inline code, `**bold**` and `*italic*`, including emphasis wrapped around a code span.

There is no HTML in it, so there is nothing to sanitise. Links and images are plain text. `_underscores_` are deliberately not emphasis, because identifiers such as `task_id` are ordinary words here. The parser runs on every delta and never throws: an unclosed fence is already a code block, and a table whose delimiter row has not arrived is still a paragraph.

A **user** row's text is the message's `content`, never `display_text`. For a turn that carried files, `display_text` ends in an `[Attachments: …]` suffix the daemon builds for clients that can only print a string; this client draws the file cards instead.

## Security

- Strict CSP (`tauri.conf.json`): `connect-src` is limited to `'self'` plus localhost HTTP/WS — the webview can only reach a local daemon. `img-src` adds `data:`, `blob:` and localhost, which is how artifact images load. There is no `font-src`, hence self-hosted fonts.
- Native drag-and-drop is disabled (`dragDropEnabled: false`) so the webview handles file drops itself.
- One Tauri plugin is enabled: `dialog`. The capability file (`src-tauri/capabilities/default.json`) grants `core:default` and `dialog:default` and nothing else. There is no `fs` plugin — files reach the daemon over HTTP. The dialog is used for the directory picker in Settings → Extensions, and the form falls back to a text field when there is no Tauri shell.
- Artifact markdown is rendered through `marked` and sanitised with DOMPurify before it reaches the DOM (`components/work/preview/DocumentPreview.tsx`). HTML and SVG artifacts are shown **as source**, by choice: `MediaPreview.tsx` has a sanitising `HtmlPreview`, but no caller passes it markup today.
- `EventSource`, `WebSocket` and `<img>` cannot set headers, so the chat stream, `/v1/events` and the artifact content URLs take the token as `?token=`. Every other request uses `Authorization: Bearer`.

## Files and paths

- `~/.openalpaca/state/discovery.json` — daemon base URL, bearer token, instance id. Read by the Rust shell only.
- `~/.openalpaca/config/` — created by the shell and passed to a daemon it spawns as `OPENALPACA_CONFIG_DIR`.
- `OPENALPACA_HOME_STORE` (absolute path) moves the whole root, for the shell and the daemon alike.
- Dev server: port 1420, strict (`tauri.conf.json` `devUrl` points at it); `src-tauri/` is excluded from the watcher.
- Per-machine `localStorage` keys:
  - `oa-pane-widths` — the resizable column widths.
  - `oa-project` — the chosen project path.
  - `oa-pins` — a **cache** of artifact pins. The pin itself is daemon state (`PUT /v1/artifacts/{id}/pin`); every server answer overwrites the cache.

## Tests

```bash
bun run test
```

Vitest with the jsdom environment. `vitest.setup.ts` loads `@testing-library/jest-dom` and sets a 1600 × 1000 window before each test; jsdom's default 1024 px is a _narrow_ window here and would collapse the chat columns. A test about a narrow window sets `window.innerWidth` itself.

The suite covers logic, not pixel styling: the SSE and WebSocket state machines, the event → cache map, the prose parser, the attachment and confirmation rules, the routing and formatting helpers, the Escape ladder and command catalogue. View-level integration tests double only the two transports (`fetch`, `EventSource`) and the Tauri discovery command — so request bodies are asserted on the wire rather than on a spy.
