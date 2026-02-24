# OpenAlpaca GUI Manual

OpenAlpaca GUI is a Tauri desktop app (`openalpaca-gui`) backed by local daemon APIs.

Related docs:
- [Daemon Manual](Daemon_Manual.md)
- [CLI Manual](CLI_Manual.md)
- [GUI API Reference](api/apps/openalpaca-gui.md)
- [Daemon API Reference](api/apps/openalpacad.md)

## Run

From repository root:

```bash
cd apps/openalpaca-gui
bun install
bunx tauri dev
```

`tauri dev` executes configured sidecar preparation and starts frontend dev server.

## Connection Lifecycle

- GUI calls Tauri command `ensure_daemon_running` to bootstrap daemon connection.
- It opens `GET /v1/events?token=...` WebSocket and tracks connection state:
  - `disconnected`
  - `connecting`
  - `connected`
  - `error`
- Reconnect uses exponential backoff and instance-id checks.

## Main Layout

Current default layout has two regions:

- Left pane: `ChatPanel`
- Right pane: switcher between:
  - `Tasks`
  - `Agents`

Header includes daemon connection indicator and settings drawer toggle.

## Settings Drawer (Primary Operations Surface)

The right-side drawer contains vertical tabs:

- `Configuration`
- `Agents`
- `Connectors`
- `Conversations`
- `Event Log`

### Configuration Sub-Tabs

Inside `Configuration` tab (`SettingsPanel`):

- `Configuration`: provider keys, model refresh, daemon provider settings (including web-search config)
- `Usage`: LLM usage (`/v1/llm/usage`, `/v1/llm/usage/daily`)
- `Latency`: orchestrator latency metrics (`/v1/orchestrator/latency*`)
- `Decisions`: dispatch decision history (`/v1/orchestrator/decisions`)

## Functional Areas

### Chat

- Send message: `POST /v1/chat`
- Stream reply: `GET /v1/chat/stream/{stream_id}?token=...`
- History: `GET /v1/chat/history`
- Clear history: `DELETE /v1/chat/history`

### Tasks

- List and filter active/completed tasks
- Task detail modal with assignment output and control actions
- Endpoints: `/v1/tasks`, `/v1/tasks/{id}`, `/v1/tasks/{id}/action`

### Agents

- Shows template-backed agent instances and orchestration stats
- Supports template/instance operations and config editing from drawer
- Core endpoints include `/v1/agent-templates*`, `/v1/agent-instances*`, `/v1/agents*`

### Connectors

- List, configure, enable/disable, delete connector config
- Endpoints: `/v1/connectors`, `/v1/connectors/{id}/action`, `/v1/connectors/{id}/config`

### Conversations

- Cross-source conversation list and message inspection
- Endpoints: `/v1/conversations`, `/v1/conversations/{id}/messages`

### Event Log

- Displays recent in-memory event feed from daemon WebSocket
- Event shape: `openalpaca_api::events::ServerEvent`

## Auth Model

- HTTP requests include bearer token from discovery connection info.
- WebSocket/SSE streaming uses query-token style auth.

## Troubleshooting

- Cannot connect: verify daemon starts and discovery token is valid.
- Connection flaps: check daemon restarts or instance-id changes.
- Missing models/usage data: refresh from Configuration tab and verify provider keys.
- Connector failures: inspect connector status/events in Event Log and daemon logs.
