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

| Method | Path | Module | Source |
|---|---|---|---|

## Request/Query Types

- Request/query payloads are represented as TypeScript interfaces in `apps/openalpaca-gui/src/lib/types.ts`
  and module-local request types under `apps/openalpaca-gui/src/lib/api/*.ts`.

## Response Shapes

- Response interfaces include task/agent/settings/conversation usage models in `apps/openalpaca-gui/src/lib/types.ts`.

## Streaming

- WebSocket client source: `apps/openalpaca-gui/src/lib/events.ts`.
- Parsed `ServerEvent` discriminators:
- `agent_config_changed`, `agent_status`, `artifact_written`, `chat_stream_ended`, `chat_stream_started`, `circuit_breaker_tripped`, `command_received`, `connector_status`, `daemon_config_changed`, `dag_node_status`, `extension_capability_withdrawn`, `extension_capability_withheld`, `extension_state_changed`, `followup_cancelled`, `followup_queued`, `heartbeat`, `key_status_changed`, `llm_call_completed`, `orchestrator_config_changed`, `security_violation`, `session_changed`, `skill_catalog_updated`, `skill_completed`, `skill_failed`, `skill_invocation_started`, `soul_updated`, `subagent_span`, `task_status`, `tool_confirmation_requested`, `tool_executed`, `wake`, `workflow_progress`, `workflow_started`, `workflow_steered`

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
- Endpoints: none

### `artifacts.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/artifacts.test.ts`
- Exported functions: none
- Endpoints: none

### `artifacts.ts`

- Source: `apps/openalpaca-gui/src/lib/api/artifacts.ts`
- Exported functions: `getArtifact`, `getArtifactDiff`, `getArtifactText`, `listArtifactVersions`, `listArtifacts`, `setArtifactPinned`
- Endpoints: none

### `chat.ts`

- Source: `apps/openalpaca-gui/src/lib/api/chat.ts`
- Exported functions: `clearChatHistory`, `deleteMessageFeedback`, `getChatHistory`, `getMessageFeedback`, `respondToConfirmation`, `setMessageFeedback`
- Endpoints: none

### `connectors.ts`

- Source: `apps/openalpaca-gui/src/lib/api/connectors.ts`
- Exported functions: `configureConnector`, `getConnectorSettings`, `listConnectors`, `performConnectorAction`, `updateConnectorSettings`
- Endpoints: none

### `extensions.ts`

- Source: `apps/openalpaca-gui/src/lib/api/extensions.ts`
- Exported functions: `listExtensions`, `removeExtension`, `runExtensionVerb`, `setExtensionConfig`
- Endpoints: none

### `files.ts`

- Source: `apps/openalpaca-gui/src/lib/api/files.ts`
- Exported functions: `downloadFile`, `getFileMetadata`, `openFileWithSystemDefault`
- Endpoints: none

### `followups.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/followups.test.ts`
- Exported functions: none
- Endpoints: none

### `followups.ts`

- Source: `apps/openalpaca-gui/src/lib/api/followups.ts`
- Exported functions: `cancelFollowup`, `listFollowups`, `queueFollowup`
- Endpoints: none

### `orchestrator.ts`

- Source: `apps/openalpaca-gui/src/lib/api/orchestrator.ts`
- Exported functions: `getDispatchDecisions`, `getLatencyAggregates`, `getLatencyRecords`, `getOrchestratorConfig`, `updateOrchestratorConfig`
- Endpoints: none

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
- Endpoints: none

### `settings.ts`

- Source: `apps/openalpaca-gui/src/lib/api/settings.ts`
- Exported functions: `getCliBackends`, `getDiscoveredCredentials`, `getKeyStatus`, `getLlmSettings`, `getProviderUsage`, `listModels`, `refreshModels`, `removeKey`, `reorderKeys`, `rescanCredentials`, `setKeyPriority`, `upsertKey`, `validateKey`
- Endpoints: none

### `skills.ts`

- Source: `apps/openalpaca-gui/src/lib/api/skills.ts`
- Exported functions: `getSkillHealth`
- Endpoints: none

### `status.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/status.test.ts`
- Exported functions: none
- Endpoints: none

### `status.ts`

- Source: `apps/openalpaca-gui/src/lib/api/status.ts`
- Exported functions: `getDaemonStatus`
- Endpoints: none

### `tasks.test.ts`

- Source: `apps/openalpaca-gui/src/lib/api/tasks.test.ts`
- Exported functions: none
- Endpoints: none

### `tasks.ts`

- Source: `apps/openalpaca-gui/src/lib/api/tasks.ts`
- Exported functions: `createTask`, `getTask`, `getTaskTimeline`, `listTasks`, `performTaskAction`, `rerunTask`, `startTaskNow`, `steerTask`
- Endpoints: none

### `telemetry.ts`

- Source: `apps/openalpaca-gui/src/lib/api/telemetry.ts`
- Exported functions: `getEventHistory`, `getHealth`, `getRunEventLog`
- Endpoints: none

### `tools.ts`

- Source: `apps/openalpaca-gui/src/lib/api/tools.ts`
- Exported functions: `listTools`
- Endpoints: none

### `types.ts`

- Source: `apps/openalpaca-gui/src/lib/api/types.ts`
- Exported functions: none
- Endpoints: none

### `unbacked.ts`

- Source: `apps/openalpaca-gui/src/lib/api/unbacked.ts`
- Exported functions: none
- Endpoints: none

### `usage.ts`

- Source: `apps/openalpaca-gui/src/lib/api/usage.ts`
- Exported functions: `getLlmUsage`, `getLlmUsageDaily`
- Endpoints: none
