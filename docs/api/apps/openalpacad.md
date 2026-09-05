# openalpacad HTTP API

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Router source: `apps/openalpacad/src/router.rs`.
- Total documented method/path endpoints: 73.
- Includes public, bearer-protected, WebSocket, and SSE routes.

## Auth

- `none`: public endpoints (`/`, `/v1/health`).
- `bearer`: `Authorization: Bearer <token>` from discovery metadata.
- `query_token`: token via query string for streaming (`/v1/events`, `/v1/chat/stream/{stream_id}`).

## Endpoints

| Method | Path | Auth | Handler | JSON Body | Query | Source |
|---|---|---|---|---|---|---|
| GET | `/` | `none` | `root_handler` | - | - | `apps/openalpacad/src/router.rs` |
| GET | `/v1/agent-instances` | `bearer` | `list_instances_handler` | - | - | `apps/openalpacad/src/routes/agents.rs` |
| GET | `/v1/agent-templates` | `bearer` | `list_templates_handler` | - | - | `apps/openalpacad/src/routes/agents.rs` |
| POST | `/v1/agent-templates` | `bearer` | `create_template_handler` | `CreateTemplateRequest` | - | `apps/openalpacad/src/routes/agents.rs` |
| GET | `/v1/agent-templates/{id}` | `bearer` | `get_template_handler` | - | - | `apps/openalpacad/src/routes/agents.rs` |
| PUT | `/v1/agent-templates/{id}` | `bearer` | `update_template_handler` | `UpdateTemplateRequest` | - | `apps/openalpacad/src/routes/agents.rs` |
| DELETE | `/v1/agent-templates/{id}` | `bearer` | `delete_template_handler` | - | - | `apps/openalpacad/src/routes/agents.rs` |
| GET | `/v1/agents` | `bearer` | `list_agents_handler` | - | `ListAgentsQuery` | `apps/openalpacad/src/routes/agents.rs` |
| POST | `/v1/agents` | `bearer` | `create_agent_handler` | `CreateAgentRequest` | - | `apps/openalpacad/src/routes/agents.rs` |
| POST | `/v1/agents/from-toml` | `bearer` | `create_agent_from_toml_handler` | `CreateAgentFromTomlRequest` | - | `apps/openalpacad/src/routes/agents.rs` |
| GET | `/v1/agents/{id}` | `bearer` | `get_agent_handler` | - | - | `apps/openalpacad/src/routes/agents.rs` |
| DELETE | `/v1/agents/{id}` | `bearer` | `delete_agent_handler` | - | - | `apps/openalpacad/src/routes/agents.rs` |
| POST | `/v1/agents/{id}/action` | `bearer` | `agent_action_handler` | `AgentActionRequest` | - | `apps/openalpacad/src/routes/agents.rs` |
| GET | `/v1/agents/{id}/config` | `bearer` | `get_agent_config_handler` | - | - | `apps/openalpacad/src/routes/agents.rs` |
| PUT | `/v1/agents/{id}/config` | `bearer` | `update_agent_config_handler` | `UpdateAgentConfigRequest` | - | `apps/openalpacad/src/routes/agents.rs` |
| POST | `/v1/auth/link` | `bearer` | `generate_link_token_handler` | - | - | `apps/openalpacad/src/routes/auth.rs` |
| POST | `/v1/chat` | `bearer` | `send_chat_handler` | `ChatSendRequest` | - | `apps/openalpacad/src/routes/chat.rs` |
| POST | `/v1/chat/confirmations/{request_id}` | `bearer` | `confirm_tool` | `ConfirmationBody` | - | `apps/openalpacad/src/routes/chat.rs` |
| GET | `/v1/chat/history` | `bearer` | `get_chat_history_handler` | - | `HistoryQuery` | `apps/openalpacad/src/routes/chat.rs` |
| DELETE | `/v1/chat/history` | `bearer` | `delete_chat_history_handler` | - | `DeleteHistoryQuery` | `apps/openalpacad/src/routes/chat.rs` |
| GET | `/v1/chat/messages/{message_id}/feedback` | `bearer` | `get_feedback_handler` | - | - | `apps/openalpacad/src/routes/chat.rs` |
| PUT | `/v1/chat/messages/{message_id}/feedback` | `bearer` | `upsert_feedback_handler` | `FeedbackRequest` | - | `apps/openalpacad/src/routes/chat.rs` |
| DELETE | `/v1/chat/messages/{message_id}/feedback` | `bearer` | `delete_feedback_handler` | - | - | `apps/openalpacad/src/routes/chat.rs` |
| GET | `/v1/chat/stream/{stream_id}` | `query_token` | `chat_stream_handler` | - | `HashMap<String, String>` | `apps/openalpacad/src/routes/chat.rs` |
| POST | `/v1/command` | `bearer` | `command_handler` | `command::CommandRequest` | - | `apps/openalpacad/src/routes/command.rs` |
| GET | `/v1/connectors` | `bearer` | `list_connectors_handler` | - | - | `apps/openalpacad/src/routes/connectors.rs` |
| POST | `/v1/connectors/{id}/action` | `bearer` | `connector_action_handler` | `connectors::ConnectorActionBody` | - | `apps/openalpacad/src/routes/connectors.rs` |
| POST | `/v1/connectors/{id}/config` | `bearer` | `connector_config_handler` | `connectors::ConnectorConfigBody` | - | `apps/openalpacad/src/routes/connectors.rs` |
| GET | `/v1/connectors/{id}/settings` | `bearer` | `connector_settings_handler` | - | - | `apps/openalpacad/src/routes/connectors.rs` |
| PUT | `/v1/connectors/{id}/settings` | `bearer` | `update_connector_settings_handler` | `connectors::ConnectorSettingsBody` | - | `apps/openalpacad/src/routes/connectors.rs` |
| GET | `/v1/conversations` | `bearer` | `list_conversations_handler` | - | `ConversationsQuery` | `apps/openalpacad/src/routes/chat.rs` |
| GET | `/v1/conversations/{id}/messages` | `bearer` | `get_conversation_messages_handler` | - | `HistoryQuery` | `apps/openalpacad/src/routes/chat.rs` |
| GET | `/v1/daemon/config/providers` | `bearer` | `get_daemon_providers` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| PUT | `/v1/daemon/config/providers/web-search` | `bearer` | `update_web_search_config` | `UpdateWebSearchRequest` | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/events` | `query_token` | `events_handler` | - | `HashMap<String, String>` | `apps/openalpacad/src/routes/events.rs` |
| GET | `/v1/events/history` | `bearer` | `events_history_handler` | - | `events_history::HistoryParams` | `apps/openalpacad/src/routes/events_history.rs` |
| POST | `/v1/files/upload` | `bearer` | `upload_file_handler` | - | - | `apps/openalpacad/src/routes/files.rs` |
| GET | `/v1/files/{id}` | `bearer` | `get_file_metadata_handler` | - | - | `apps/openalpacad/src/routes/files.rs` |
| GET | `/v1/files/{id}/content` | `bearer` | `get_file_content_handler` | - | - | `apps/openalpacad/src/routes/files.rs` |
| POST | `/v1/files/{id}/open` | `bearer` | `open_file_handler` | - | - | `apps/openalpacad/src/routes/files.rs` |
| GET | `/v1/health` | `none` | `health_handler` | - | - | `apps/openalpacad/src/router.rs` |
| GET | `/v1/llm/usage` | `bearer` | `get_llm_usage` | - | `LlmUsageQuery` | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/llm/usage/daily` | `bearer` | `get_llm_usage_daily` | - | `LlmUsageDailyQuery` | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/me` | `bearer` | `get_me_handler` | - | - | `apps/openalpacad/src/routes/auth.rs` |
| GET | `/v1/models` | `bearer` | `list_models` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| POST | `/v1/models/refresh` | `bearer` | `refresh_models` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/orchestrator/config` | `bearer` | `get_orchestrator_config` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| PUT | `/v1/orchestrator/config` | `bearer` | `update_orchestrator_config` | `UpdateOrchestratorRequest` | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/orchestrator/decisions` | `bearer` | `dispatch_decisions_handler` | - | `dispatch_decisions::DecisionParams` | `apps/openalpacad/src/routes/dispatch_decisions.rs` |
| GET | `/v1/orchestrator/latency` | `bearer` | `orchestrator_latency_handler` | - | `orchestrator_latency::LatencyParams` | `apps/openalpacad/src/routes/orchestrator_latency.rs` |
| GET | `/v1/orchestrator/latency/aggregate` | `bearer` | `orchestrator_latency_aggregate_handler` | - | `orchestrator_latency::AggregateParams` | `apps/openalpacad/src/routes/orchestrator_latency.rs` |
| GET | `/v1/plugins` | `bearer` | `list_plugins_handler` | - | - | `apps/openalpacad/src/routes/plugins.rs` |
| POST | `/v1/plugins/{name}/approve` | `bearer` | `approve_plugin_handler` | - | - | `apps/openalpacad/src/routes/plugins.rs` |
| POST | `/v1/plugins/{name}/config` | `bearer` | `set_plugin_config_handler` | `plugins::SetConfigRequest` | - | `apps/openalpacad/src/routes/plugins.rs` |
| POST | `/v1/plugins/{name}/deny` | `bearer` | `deny_plugin_handler` | - | - | `apps/openalpacad/src/routes/plugins.rs` |
| POST | `/v1/plugins/{name}/disable` | `bearer` | `disable_plugin_handler` | - | - | `apps/openalpacad/src/routes/plugins.rs` |
| POST | `/v1/plugins/{name}/enable` | `bearer` | `enable_plugin_handler` | - | - | `apps/openalpacad/src/routes/plugins.rs` |
| GET | `/v1/settings/llm` | `bearer` | `get_llm_settings` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| PUT | `/v1/settings/llm` | `bearer` | `upsert_key` | `AddKeyRequest` | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/settings/llm/cli-backends` | `bearer` | `get_cli_backends` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/settings/llm/credentials` | `bearer` | `get_discovered_credentials` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| POST | `/v1/settings/llm/credentials/rescan` | `bearer` | `rescan_credentials` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| PUT | `/v1/settings/llm/keys/priority` | `bearer` | `set_key_priority` | `SetKeyPriorityRequest` | - | `apps/openalpacad/src/routes/settings.rs` |
| PUT | `/v1/settings/llm/keys/reorder` | `bearer` | `reorder_keys` | `ReorderKeysRequest` | - | `apps/openalpacad/src/routes/settings.rs` |
| DELETE | `/v1/settings/llm/keys/{provider}/{key_id}` | `bearer` | `delete_key` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/settings/llm/providers/usage` | `bearer` | `get_provider_usage` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/settings/llm/status` | `bearer` | `get_key_status` | - | - | `apps/openalpacad/src/routes/settings.rs` |
| POST | `/v1/settings/llm/validate` | `bearer` | `validate_key` | `ValidateKeyRequest` | - | `apps/openalpacad/src/routes/settings.rs` |
| GET | `/v1/skills/health` | `bearer` | `skill_health_handler` | - | - | `apps/openalpacad/src/routes/skills.rs` |
| GET | `/v1/tasks` | `bearer` | `list_tasks_handler` | - | `ListTasksQuery` | `apps/openalpacad/src/routes/tasks.rs` |
| POST | `/v1/tasks` | `bearer` | `create_task_handler` | `CreateTaskRequest` | - | `apps/openalpacad/src/routes/tasks.rs` |
| GET | `/v1/tasks/{id}` | `bearer` | `get_task_handler` | - | - | `apps/openalpacad/src/routes/tasks.rs` |
| POST | `/v1/tasks/{id}/action` | `bearer` | `task_action_handler` | `TaskActionRequest` | - | `apps/openalpacad/src/routes/tasks.rs` |

## Request/Query Types

### `AddKeyRequest`

- External or generic type; see handler source.

### `AgentActionRequest`

- External or generic type; see handler source.

### `ChatSendRequest`

- External or generic type; see handler source.

### `ConfirmationBody`

- External or generic type; see handler source.

### `ConversationsQuery`

- External or generic type; see handler source.

### `CreateAgentFromTomlRequest`

- External or generic type; see handler source.

### `CreateAgentRequest`

- External or generic type; see handler source.

### `CreateTaskRequest`

- External or generic type; see handler source.

### `CreateTemplateRequest`

- External or generic type; see handler source.

### `DeleteHistoryQuery`

- External or generic type; see handler source.

### `FeedbackRequest`

- External or generic type; see handler source.

### `HashMap<String, String>`

- External or generic type; see handler source.

### `HistoryQuery`

- External or generic type; see handler source.

### `ListAgentsQuery`

- External or generic type; see handler source.

### `ListTasksQuery`

- External or generic type; see handler source.

### `LlmUsageDailyQuery`

- External or generic type; see handler source.

### `LlmUsageQuery`

- External or generic type; see handler source.

### `ReorderKeysRequest`

- External or generic type; see handler source.

### `SetKeyPriorityRequest`

- External or generic type; see handler source.

### `TaskActionRequest`

- External or generic type; see handler source.

### `UpdateAgentConfigRequest`

- External or generic type; see handler source.

### `UpdateOrchestratorRequest`

- External or generic type; see handler source.

### `UpdateTemplateRequest`

- External or generic type; see handler source.

### `UpdateWebSearchRequest`

- External or generic type; see handler source.

### `ValidateKeyRequest`

- External or generic type; see handler source.

### `command::CommandRequest`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/command.rs`

| Field | Type |
|---|---|
| `command` | `String` |
| `args` | `HashMap<String, serde_json::Value>` |
| `target_agent` | `Option<String>` |

### `connectors::ConnectorActionBody`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/connectors.rs`

| Field | Type |
|---|---|
| `action` | `String` |

### `connectors::ConnectorConfigBody`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/connectors.rs`

| Field | Type |
|---|---|
| `token` | `String` |

### `connectors::ConnectorSettingsBody`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/connectors.rs`

| Field | Type |
|---|---|
| `settings` | `HashMap<String, String>` |

### `dispatch_decisions::DecisionParams`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/dispatch_decisions.rs`

| Field | Type |
|---|---|
| `mode` | `Option<String>` |
| `from` | `Option<String>` |
| `to` | `Option<String>` |
| `limit` | `Option<usize>` |

### `events_history::HistoryParams`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/events_history.rs`

| Field | Type |
|---|---|
| `limit` | `Option<usize>` |
| `agent_id` | `Option<String>` |

### `orchestrator_latency::AggregateParams`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/orchestrator_latency.rs`

| Field | Type |
|---|---|
| `from` | `Option<String>` |
| `to` | `Option<String>` |

### `orchestrator_latency::LatencyParams`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/orchestrator_latency.rs`

| Field | Type |
|---|---|
| `mode` | `Option<String>` |
| `from` | `Option<String>` |
| `to` | `Option<String>` |
| `limit` | `Option<usize>` |

### `plugins::SetConfigRequest`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/plugins.rs`

| Field | Type |
|---|---|
| `key` | `String` |
| `value` | `serde_json::Value` |

## Response Shapes

### `auth::LinkTokenResponse`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/auth.rs`

| Field | Type |
|---|---|
| `token` | `String` |

### `auth::MeResponse`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/auth.rs`

| Field | Type |
|---|---|
| `user_id` | `String` |
| `default_lane_key` | `String` |
| `sources` | `Vec<String>` |

### `command::CommandResponse`

- Kind: `struct`
- Source: `apps/openalpacad/src/routes/command.rs`

| Field | Type |
|---|---|
| `request_id` | `String` |
| `status` | `String` |

## Streaming

- WebSocket `GET /v1/events?token=...` sends `openalpaca_api::events::ServerEvent` JSON payloads.
- SSE `GET /v1/chat/stream/{stream_id}?token=...` emits events: `thinking`, `delta`, `done`, `error`.

## Related Links

- [CLI API doc](openalpaca.md)
- [GUI API doc](openalpaca-gui.md)
- [Database Schema](../database/schema.md)
