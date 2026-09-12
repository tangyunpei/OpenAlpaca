# openalpaca-gui (Tauri + React)

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Frontend API wrappers: `apps/openalpaca-gui/src/lib/api/*.ts`.
- Tauri backend commands: `apps/openalpaca-gui/src-tauri/src/lib.rs`.
- Daemon event stream client: `apps/openalpaca-gui/src/lib/events.ts`.

## Auth

- HTTP API calls use `Authorization: Bearer <token>` from discovery connection info.
- WebSocket uses query token: `/v1/events?token=...`.
- SSE chat stream uses query token: `/v1/chat/stream/{stream_id}?token=...`.

## Endpoints

- One row per exported wrapper, read from the route named in its doc comment
  (the module header when the wrapper names none). Paths are spelled as the
  client documents them, so a path parameter can differ from the router's
  (`{id}` for `{message_id}`) and a `{kind}` segment can arrive already filled
  in (`/v1/extensions/plugin/{id}`); query strings are dropped.

| Method | Path | Module | Source |
|---|---|---|---|
| GET | `/v1/agent-instances` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| GET | `/v1/agent-templates/{id}` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| PUT | `/v1/agent-templates/{id}` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| DELETE | `/v1/agent-templates/{id}` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| GET | `/v1/agent-templates` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| POST | `/v1/agent-templates` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| POST | `/v1/agents/{id}/action` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| GET | `/v1/agents/{id}/config` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| PUT | `/v1/agents/{id}/config` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| GET | `/v1/agents/{id}` | `agents.ts` | `apps/openalpaca-gui/src/lib/api/agents.ts` |
| GET | `/v1/artifacts/{id}/content` | `artifacts.ts` | `apps/openalpaca-gui/src/lib/api/artifacts.ts` |
| GET | `/v1/artifacts/{id}/diff` | `artifacts.ts` | `apps/openalpaca-gui/src/lib/api/artifacts.ts` |
| PUT | `/v1/artifacts/{id}/pin` | `artifacts.ts` | `apps/openalpaca-gui/src/lib/api/artifacts.ts` |
| GET | `/v1/artifacts/{id}/versions` | `artifacts.ts` | `apps/openalpaca-gui/src/lib/api/artifacts.ts` |
| GET | `/v1/artifacts/{id}` | `artifacts.ts` | `apps/openalpaca-gui/src/lib/api/artifacts.ts` |
| GET | `/v1/artifacts` | `artifacts.ts` | `apps/openalpaca-gui/src/lib/api/artifacts.ts` |
| POST | `/v1/chat/confirmations/{request_id}` | `chat.ts` | `apps/openalpaca-gui/src/lib/api/chat.ts` |
| GET | `/v1/chat/history` | `chat.ts` | `apps/openalpaca-gui/src/lib/api/chat.ts` |
| DELETE | `/v1/chat/history` | `chat.ts` | `apps/openalpaca-gui/src/lib/api/chat.ts` |
| GET | `/v1/chat/messages/{id}/feedback` | `chat.ts` | `apps/openalpaca-gui/src/lib/api/chat.ts` |
| PUT | `/v1/chat/messages/{id}/feedback` | `chat.ts` | `apps/openalpaca-gui/src/lib/api/chat.ts` |
| DELETE | `/v1/chat/messages/{id}/feedback` | `chat.ts` | `apps/openalpaca-gui/src/lib/api/chat.ts` |
| POST | `/v1/connectors/{id}/action` | `connectors.ts` | `apps/openalpaca-gui/src/lib/api/connectors.ts` |
| POST | `/v1/connectors/{id}/config` | `connectors.ts` | `apps/openalpaca-gui/src/lib/api/connectors.ts` |
| GET | `/v1/connectors/{id}/settings` | `connectors.ts` | `apps/openalpaca-gui/src/lib/api/connectors.ts` |
| PUT | `/v1/connectors/{id}/settings` | `connectors.ts` | `apps/openalpaca-gui/src/lib/api/connectors.ts` |
| GET | `/v1/connectors` | `connectors.ts` | `apps/openalpaca-gui/src/lib/api/connectors.ts` |
| GET | `/v1/events/history` | `telemetry.ts` | `apps/openalpaca-gui/src/lib/api/telemetry.ts` |
| POST | `/v1/extensions/mcp` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| POST | `/v1/extensions/plugin/validate` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| POST | `/v1/extensions/plugin/{id}/config` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| PUT | `/v1/extensions/plugin/{id}` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| DELETE | `/v1/extensions/plugin/{id}` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| POST | `/v1/extensions/plugin` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| POST | `/v1/extensions/{kind}/{id}/{verb}` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| DELETE | `/v1/extensions/{kind}/{id}` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| GET | `/v1/extensions` | `extensions.ts` | `apps/openalpaca-gui/src/lib/api/extensions.ts` |
| GET | `/v1/files/{id}/content` | `files.ts` | `apps/openalpaca-gui/src/lib/api/files.ts` |
| POST | `/v1/files/{id}/open` | `files.ts` | `apps/openalpaca-gui/src/lib/api/files.ts` |
| GET | `/v1/files/{id}` | `files.ts` | `apps/openalpaca-gui/src/lib/api/files.ts` |
| GET | `/v1/health` | `telemetry.ts` | `apps/openalpaca-gui/src/lib/api/telemetry.ts` |
| DELETE | `/v1/lanes/{lane_key}/followups/{id}` | `followups.ts` | `apps/openalpaca-gui/src/lib/api/followups.ts` |
| GET | `/v1/lanes/{lane_key}/followups` | `followups.ts` | `apps/openalpaca-gui/src/lib/api/followups.ts` |
| POST | `/v1/lanes/{lane_key}/followups` | `followups.ts` | `apps/openalpaca-gui/src/lib/api/followups.ts` |
| GET | `/v1/llm/usage/daily` | `usage.ts` | `apps/openalpaca-gui/src/lib/api/usage.ts` |
| GET | `/v1/llm/usage` | `usage.ts` | `apps/openalpaca-gui/src/lib/api/usage.ts` |
| POST | `/v1/models/refresh` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| GET | `/v1/models` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| GET | `/v1/orchestrator/config` | `orchestrator.ts` | `apps/openalpaca-gui/src/lib/api/orchestrator.ts` |
| PUT | `/v1/orchestrator/config` | `orchestrator.ts` | `apps/openalpaca-gui/src/lib/api/orchestrator.ts` |
| GET | `/v1/orchestrator/decisions` | `orchestrator.ts` | `apps/openalpaca-gui/src/lib/api/orchestrator.ts` |
| GET | `/v1/orchestrator/latency/aggregate` | `orchestrator.ts` | `apps/openalpaca-gui/src/lib/api/orchestrator.ts` |
| GET | `/v1/orchestrator/latency` | `orchestrator.ts` | `apps/openalpaca-gui/src/lib/api/orchestrator.ts` |
| POST | `/v1/sessions/{id}/activate` | `sessions.ts` | `apps/openalpaca-gui/src/lib/api/sessions.ts` |
| POST | `/v1/sessions/{id}/archive` | `sessions.ts` | `apps/openalpaca-gui/src/lib/api/sessions.ts` |
| GET | `/v1/sessions/{id}/messages` | `sessions.ts` | `apps/openalpaca-gui/src/lib/api/sessions.ts` |
| DELETE | `/v1/sessions/{id}` | `sessions.ts` | `apps/openalpaca-gui/src/lib/api/sessions.ts` |
| PATCH | `/v1/sessions/{id}` | `sessions.ts` | `apps/openalpaca-gui/src/lib/api/sessions.ts` |
| GET | `/v1/sessions` | `sessions.ts` | `apps/openalpaca-gui/src/lib/api/sessions.ts` |
| POST | `/v1/sessions` | `sessions.ts` | `apps/openalpaca-gui/src/lib/api/sessions.ts` |
| GET | `/v1/settings/llm/cli-backends` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| POST | `/v1/settings/llm/credentials/rescan` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| GET | `/v1/settings/llm/credentials` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| PUT | `/v1/settings/llm/keys/priority` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| PUT | `/v1/settings/llm/keys/reorder` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| DELETE | `/v1/settings/llm/keys/{provider}/{keyId}` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| GET | `/v1/settings/llm/providers/usage` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| PUT | `/v1/settings/llm/providers/{provider}/enabled` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| GET | `/v1/settings/llm/status` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| POST | `/v1/settings/llm/validate` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| GET | `/v1/settings/llm` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| PUT | `/v1/settings/llm` | `settings.ts` | `apps/openalpaca-gui/src/lib/api/settings.ts` |
| GET | `/v1/skills/health` | `skills.ts` | `apps/openalpaca-gui/src/lib/api/skills.ts` |
| GET | `/v1/skills` | `skills.ts` | `apps/openalpaca-gui/src/lib/api/skills.ts` |
| GET | `/v1/status` | `status.ts` | `apps/openalpaca-gui/src/lib/api/status.ts` |
| POST | `/v1/tasks/{id}/action` | `tasks.ts` | `apps/openalpaca-gui/src/lib/api/tasks.ts` |
| POST | `/v1/tasks/{id}/rerun` | `tasks.ts` | `apps/openalpaca-gui/src/lib/api/tasks.ts` |
| POST | `/v1/tasks/{id}/steer` | `tasks.ts` | `apps/openalpaca-gui/src/lib/api/tasks.ts` |
| GET | `/v1/tasks/{id}/timeline` | `tasks.ts` | `apps/openalpaca-gui/src/lib/api/tasks.ts` |
| GET | `/v1/tasks/{id}` | `tasks.ts` | `apps/openalpaca-gui/src/lib/api/tasks.ts` |
| GET | `/v1/tasks` | `tasks.ts` | `apps/openalpaca-gui/src/lib/api/tasks.ts` |
| POST | `/v1/tasks` | `tasks.ts` | `apps/openalpaca-gui/src/lib/api/tasks.ts` |
| GET | `/v1/tools` | `tools.ts` | `apps/openalpaca-gui/src/lib/api/tools.ts` |
| GET | `/v1/usage/summary` | `usage.ts` | `apps/openalpaca-gui/src/lib/api/usage.ts` |
| GET | `/v1/workspaces` | `workspaces.ts` | `apps/openalpaca-gui/src/lib/api/workspaces.ts` |
| PATCH | `/v1/workspaces` | `workspaces.ts` | `apps/openalpaca-gui/src/lib/api/workspaces.ts` |

## Request/Query Types

- Request/query payloads are represented as TypeScript interfaces in `apps/openalpaca-gui/src/lib/types.ts`
  and module-local request types under `apps/openalpaca-gui/src/lib/api/*.ts`.

## Response Shapes

- Response interfaces include task/agent/settings/conversation usage models in `apps/openalpaca-gui/src/lib/types.ts`.

## Streaming

- WebSocket client source: `apps/openalpaca-gui/src/lib/events.ts`.
- Parsed `ServerEvent` discriminators:
- `agent_config_changed`, `agent_status`, `artifact_written`, `chat_stream_ended`, `chat_stream_started`, `circuit_breaker_tripped`, `command_received`, `connector_status`, `daemon_config_changed`, `extension_capability_withdrawn`, `extension_capability_withheld`, `extension_state_changed`, `followup_cancelled`, `followup_queued`, `heartbeat`, `key_status_changed`, `llm_call_completed`, `orchestrator_config_changed`, `security_violation`, `session_changed`, `skill_catalog_updated`, `skill_completed`, `skill_failed`, `skill_invocation_started`, `soul_updated`, `subagent_span`, `task_status`, `tool_confirmation_requested`, `tool_executed`, `wake`, `workflow_progress`, `workflow_started`, `workflow_steered`

## Related Links

- [Daemon API](openalpacad.md)
- [CLI API](openalpaca.md)

## Tauri Commands

- Source: `apps/openalpaca-gui/src-tauri/src/lib.rs`
- Commands: `ensure_daemon_running`, `get_connection_info`

## API Module Map

### `agents.ts`

- Source: `apps/openalpaca-gui/src/lib/api/agents.ts`
- Exported functions: `createAgentTemplate`, `deleteAgentTemplate`, `getAgent`, `getAgentConfig`, `getAgentTemplate`, `listAgentInstances`, `listAgentTemplates`, `performAgentAction`, `updateAgentConfig`, `updateAgentTemplate`
- Endpoints: `GET /v1/agent-templates`, `GET /v1/agent-templates/{id}`, `POST /v1/agent-templates`, `PUT /v1/agent-templates/{id}`, `DELETE /v1/agent-templates/{id}`, `GET /v1/agent-instances`, `GET /v1/agents/{id}`, `GET /v1/agents/{id}/config`, `PUT /v1/agents/{id}/config`, `POST /v1/agents/{id}/action`

### `artifacts.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/artifacts.test.ts`
- Exported functions: none
- Endpoints: none

### `artifacts.ts`

- Source: `apps/openalpaca-gui/src/lib/api/artifacts.ts`
- Exported functions: `getArtifact`, `getArtifactDiff`, `getArtifactText`, `listArtifactVersions`, `listArtifacts`, `setArtifactPinned`
- Endpoints: `GET /v1/artifacts`, `GET /v1/artifacts/{id}`, `GET /v1/artifacts/{id}/versions`, `GET /v1/artifacts/{id}/diff`, `GET /v1/artifacts/{id}/content`, `PUT /v1/artifacts/{id}/pin`

### `chat.ts`

- Source: `apps/openalpaca-gui/src/lib/api/chat.ts`
- Exported functions: `clearChatHistory`, `deleteMessageFeedback`, `getChatHistory`, `getMessageFeedback`, `respondToConfirmation`, `setMessageFeedback`
- Endpoints: `GET /v1/chat/history`, `DELETE /v1/chat/history`, `POST /v1/chat/confirmations/{request_id}`, `PUT /v1/chat/messages/{id}/feedback`, `GET /v1/chat/messages/{id}/feedback`, `DELETE /v1/chat/messages/{id}/feedback`

### `connectors.ts`

- Source: `apps/openalpaca-gui/src/lib/api/connectors.ts`
- Exported functions: `configureConnector`, `getConnectorSettings`, `listConnectors`, `performConnectorAction`, `updateConnectorSettings`
- Endpoints: `GET /v1/connectors`, `POST /v1/connectors/{id}/action`, `POST /v1/connectors/{id}/config`, `GET /v1/connectors/{id}/settings`, `PUT /v1/connectors/{id}/settings`

### `extensions.ts`

- Source: `apps/openalpaca-gui/src/lib/api/extensions.ts`
- Exported functions: `addMcpServer`, `installPlugin`, `listExtensions`, `removeExtension`, `runExtensionVerb`, `setExtensionConfig`, `uninstallExtension`, `updatePlugin`, `validatePlugin`
- Endpoints: `GET /v1/extensions`, `POST /v1/extensions/{kind}/{id}/{verb}`, `DELETE /v1/extensions/plugin/{id}`, `POST /v1/extensions/plugin/{id}/config`, `POST /v1/extensions/plugin`, `POST /v1/extensions/plugin/validate`, `PUT /v1/extensions/plugin/{id}`, `POST /v1/extensions/mcp`, `DELETE /v1/extensions/{kind}/{id}`

### `files.ts`

- Source: `apps/openalpaca-gui/src/lib/api/files.ts`
- Exported functions: `downloadFile`, `getFileMetadata`, `openFileWithSystemDefault`
- Endpoints: `GET /v1/files/{id}`, `GET /v1/files/{id}/content`, `POST /v1/files/{id}/open`

### `followups.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/followups.test.ts`
- Exported functions: none
- Endpoints: none

### `followups.ts`

- Source: `apps/openalpaca-gui/src/lib/api/followups.ts`
- Exported functions: `cancelFollowup`, `listFollowups`, `queueFollowup`
- Endpoints: `GET /v1/lanes/{lane_key}/followups`, `POST /v1/lanes/{lane_key}/followups`, `DELETE /v1/lanes/{lane_key}/followups/{id}`

### `orchestrator.ts`

- Source: `apps/openalpaca-gui/src/lib/api/orchestrator.ts`
- Exported functions: `getDispatchDecisions`, `getLatencyAggregates`, `getLatencyRecords`, `getOrchestratorConfig`, `updateOrchestratorConfig`
- Endpoints: `GET /v1/orchestrator/config`, `PUT /v1/orchestrator/config`, `GET /v1/orchestrator/latency`, `GET /v1/orchestrator/latency/aggregate`, `GET /v1/orchestrator/decisions`

### `run-events.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/run-events.test.ts`
- Exported functions: none
- Endpoints: none

### `run-events.ts`

- Source: `apps/openalpaca-gui/src/lib/api/run-events.ts`
- Exported functions: none
- Endpoints: none

### `sessions.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/sessions.test.ts`
- Exported functions: none
- Endpoints: none

### `sessions.ts`

- Source: `apps/openalpaca-gui/src/lib/api/sessions.ts`
- Exported functions: `activateSession`, `archiveSession`, `createSession`, `deleteSession`, `getSessionMessages`, `listSessions`, `updateSession`
- Endpoints: `GET /v1/sessions`, `GET /v1/sessions/{id}/messages`, `POST /v1/sessions`, `POST /v1/sessions/{id}/activate`, `POST /v1/sessions/{id}/archive`, `PATCH /v1/sessions/{id}`, `DELETE /v1/sessions/{id}`

### `settings.ts`

- Source: `apps/openalpaca-gui/src/lib/api/settings.ts`
- Exported functions: `getCliBackends`, `getDiscoveredCredentials`, `getKeyStatus`, `getLlmSettings`, `getProviderUsage`, `listModels`, `refreshModels`, `removeKey`, `reorderKeys`, `rescanCredentials`, `setKeyPriority`, `setProviderEnabled`, `upsertKey`, `validateKey`
- Endpoints: `GET /v1/settings/llm`, `PUT /v1/settings/llm`, `DELETE /v1/settings/llm/keys/{provider}/{keyId}`, `PUT /v1/settings/llm/keys/reorder`, `PUT /v1/settings/llm/keys/priority`, `PUT /v1/settings/llm/providers/{provider}/enabled`, `POST /v1/settings/llm/validate`, `GET /v1/settings/llm/status`, `GET /v1/settings/llm/credentials`, `POST /v1/settings/llm/credentials/rescan`, `GET /v1/settings/llm/cli-backends`, `GET /v1/settings/llm/providers/usage`, `GET /v1/models`, `POST /v1/models/refresh`

### `skills.ts`

- Source: `apps/openalpaca-gui/src/lib/api/skills.ts`
- Exported functions: `getSkillHealth`, `listSkills`
- Endpoints: `GET /v1/skills`, `GET /v1/skills/health`

### `status.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/status.test.ts`
- Exported functions: none
- Endpoints: none

### `status.ts`

- Source: `apps/openalpaca-gui/src/lib/api/status.ts`
- Exported functions: `getDaemonStatus`
- Endpoints: `GET /v1/status`

### `tasks.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/tasks.test.ts`
- Exported functions: none
- Endpoints: none

### `tasks.ts`

- Source: `apps/openalpaca-gui/src/lib/api/tasks.ts`
- Exported functions: `createTask`, `getTask`, `getTaskTimeline`, `listTasks`, `performTaskAction`, `rerunTask`, `startTaskNow`, `steerTask`
- Endpoints: `GET /v1/tasks`, `GET /v1/tasks/{id}`, `POST /v1/tasks`, `GET /v1/tasks/{id}/timeline`, `POST /v1/tasks/{id}/action`, `POST /v1/tasks/{id}/rerun`, `POST /v1/tasks/{id}/steer`

### `telemetry.ts`

- Source: `apps/openalpaca-gui/src/lib/api/telemetry.ts`
- Exported functions: `getEventHistory`, `getHealth`, `getRunEventLog`
- Endpoints: `GET /v1/events/history`, `GET /v1/health`

### `tools.ts`

- Source: `apps/openalpaca-gui/src/lib/api/tools.ts`
- Exported functions: `listTools`
- Endpoints: `GET /v1/tools`

### `types.ts`

- Source: `apps/openalpaca-gui/src/lib/api/types.ts`
- Exported functions: none
- Endpoints: none

### `usage.ts`

- Source: `apps/openalpaca-gui/src/lib/api/usage.ts`
- Exported functions: `getLlmUsage`, `getLlmUsageDaily`, `getUsageSummary`
- Endpoints: `GET /v1/llm/usage`, `GET /v1/llm/usage/daily`, `GET /v1/usage/summary`

### `workspaces.ts`

- Source: `apps/openalpaca-gui/src/lib/api/workspaces.ts`
- Exported functions: `getWorkspace`, `rebaseWorkspace`
- Endpoints: `GET /v1/workspaces`, `PATCH /v1/workspaces`
