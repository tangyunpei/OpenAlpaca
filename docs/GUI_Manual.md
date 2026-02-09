# OpenAlpaca GUI Manual

The OpenAlpaca GUI is a Tauri desktop app. It ensures a local daemon (`openalpacad`) is running, then connects to it over HTTP/WebSocket using the token from `discovery.json`.

Related docs:
- Daemon manual: `Daemon_Manual.md`
- Daemon HTTP API: `api/apps/openalpacad.md`

## Running The GUI

Development:

```bash
cd apps/openalpaca-gui
bun install
bunx tauri dev
```

`tauri dev` runs the configured `beforeDevCommand` which builds/prepares the `openalpacad` sidecar and starts the frontend dev server.

## Layout Overview

The GUI has two main regions:
- Left sidebar: **Chat** (always visible).

Right panel tabs:
- **Event Log**
- **Connectors**
- **Tasks**
- **Agents**
- **Conversations**
- **Settings**

## Connection Status

The header status pill reflects the connection state to the daemon:
- Disconnected: daemon not running or discovery not available.
- Connecting: GUI is bootstrapping (spawning daemon and/or connecting WebSocket).
- Connected: WebSocket is connected and events are flowing.
- Error: the GUI failed to connect; the error banner shows details.

## Chat (Left Sidebar)

The chat panel sends messages to the daemon and streams responses back.

Core behavior:
- Send: type a message and press `Enter` (use `Shift+Enter` for a newline).
- Streaming: while the daemon is streaming, the Send button is disabled.
- Clear: clears chat history in the database (calls `DELETE /v1/chat/history`).

Under the hood:
- Send message: `POST /v1/chat` (Bearer token).
- Stream response: `GET /v1/chat/stream/{stream_id}?token=...` (SSE).

## Event Log

Shows real-time `ServerEvent` messages from the daemon WebSocket:
- Clear: clears the local event list in the GUI (does not delete DB history).
- Quit OpenAlpaca: disconnects WebSocket, then sends `POST /v1/command { "command": "shutdown" }`.

Event log notes:
- The GUI keeps the most recent ~100 events in memory.
- WebSocket endpoint: `GET /v1/events?token=...`.

Common event types include:
- `heartbeat`
- `log`
- `wake`
- `command_received`
- `connector_status`
- `task_status`
- `agent_status`
- `key_status_changed`
- `chat_stream_started` / `chat_stream_ended`
- `orchestrator_config_changed`

## Connectors

Connectors are platform integrations (example: Telegram). The panel supports:
- Refresh: fetch connector status from the daemon.
- Configure: set a connector token in a modal (sends `POST /v1/connectors/{id}/config`).
- Toggle: enable/disable a connector (sends `POST /v1/connectors/{id}/action` with `enable`/`disable`).
- Clear Config: clears config and stops the connector (action `delete`).

### Generate Bind Token

Click **Generate Bind Token** to create a short-lived link token (5 minutes):
- Endpoint: `POST /v1/auth/link`
- You then send `/link <TOKEN>` to the platform bot (Telegram) to bind the external identity to the local OpenAlpaca user.

## Tasks

Tasks represent units of work tracked by the daemon and stored in SQLite.

In the Tasks tab:
- Refresh: reload tasks list.
- Active / Completed: filter the displayed list.
- Click a task: opens a detail modal with task metadata, assignments, and any available output.
- Actions: cancel/pause/resume (availability depends on task status).

Real-time updates:
- Task status updates are driven by WebSocket `task_status` events.

## Agents

Agents are managed sub-agents persisted in the DB and used by the orchestrator.

In the Agents tab:
- Refresh: reload agent list.
- New Agent: opens the agent creation flow.
- Click an agent: opens a detail view (status, metrics, config details).
- Actions: pause/resume (depending on current state).

Real-time updates:
- Agent status updates are driven by WebSocket `agent_status` events.

## Conversations

The Conversations tab shows stored conversations across sources (GUI, CLI, Telegram):
- Refresh the list and filter by source.
- Select a conversation to view messages.

Backed by:
- `GET /v1/conversations`
- `GET /v1/conversations/{id}/messages`

## Settings

Settings focuses on LLM configuration and usage visibility.

Two sub-tabs:
- Configuration: orchestrator model/fallbacks, provider keys, model refresh (`POST /v1/models/refresh`), discovered credentials and CLI backend status.
- Usage: daily aggregates and per-call logs (`GET /v1/llm/usage/daily`, `GET /v1/llm/usage`), with optional agent filter.

## FAQ / Troubleshooting

- “Not connected to daemon”: use the GUI normally; it should auto-spawn the daemon. If it fails, check the error banner and daemon logs.
- “Generate Bind Token failed”: the GUI must be connected and the daemon must be able to write to its database.
- “No agents”: ensure `config/agents/*.toml` exists (daemon loads agent configs at startup) or create agents from the Agents tab.
- “No models”: configure `config/llm.toml` and add keys; then use Settings -> Refresh Models.
