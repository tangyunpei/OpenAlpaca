# Plugin SDK Phase 4: GUI Plugins Panel

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan.

**Goal:** Add a Plugins panel to the Tauri GUI so users can view plugin status, approve/deny pending plugins, enable/disable, and see registered tools/connectors/providers.

**Architecture:** Follows existing GUI patterns: API module → Svelte store (writable + derived) → Panel component with event subscription. Plugin events already defined in daemon.ts.

**Tech Stack:** SvelteKit (Svelte 5), TypeScript, Tailwind CSS

**Depends on:** Phase 1-3 (daemon routes at `/v1/plugins/*` already exist)

---

## File Map

### New Files

| File | Responsibility |
|---|---|
| `apps/openalpaca-gui/src/lib/api/plugins.ts` | HTTP API calls to /v1/plugins/* |
| `apps/openalpaca-gui/src/lib/stores/plugins.ts` | Reactive plugin state store |
| `apps/openalpaca-gui/src/lib/components/PluginsPanel.svelte` | Main plugins panel component |

### Modified Files

| File | Change |
|---|---|
| `apps/openalpaca-gui/src/routes/+page.svelte` | Add "plugins" tab + PluginsPanel |

---

## Task 1: Create API module

**File:** `apps/openalpaca-gui/src/lib/api/plugins.ts`

Follow the exact pattern from `api/tasks.ts`: import `ensureConnection`, fetch with auth header, return typed response.

Endpoints:
- `getPlugins()` → `GET /v1/plugins`
- `approvePlugin(name)` → `POST /v1/plugins/{name}/approve`
- `denyPlugin(name)` → `POST /v1/plugins/{name}/deny`
- `enablePlugin(name)` → `POST /v1/plugins/{name}/enable`
- `disablePlugin(name)` → `POST /v1/plugins/{name}/disable`
- `setPluginConfig(name, key, value)` → `POST /v1/plugins/{name}/config`

---

## Task 2: Create plugin store

**File:** `apps/openalpaca-gui/src/lib/stores/plugins.ts`

Follow the pattern from `stores/tasks.ts`:
- `pluginMap` writable Map
- `pluginList` derived sorted array
- `loadPlugins()` async function with in-flight guard
- `subscribeToPluginEvents()` listening for plugin_loaded/unloaded/crashed/etc.

---

## Task 3: Create PluginsPanel component

**File:** `apps/openalpaca-gui/src/lib/components/PluginsPanel.svelte`

Follow TaskPanel pattern:
- Header with refresh button + count
- List of plugin cards showing: name, version, status badge, registered tools/connectors/providers
- Action buttons: Approve/Deny (for pending), Enable/Disable (for running/disabled)
- Status badges: green=Running, yellow=WaitingApproval/NeedsConfig, red=Crashed/Disabled, gray=Stopped

---

## Task 4: Wire into +page.svelte

- Add `"plugins"` to tab union type
- Add Plugins tab button with badge
- Add `PluginsPanel` import and conditional render
- Add `loadPlugins()` to tab change handler
