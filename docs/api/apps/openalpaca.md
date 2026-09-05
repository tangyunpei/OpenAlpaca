# openalpaca (CLI)

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Entry point: `apps/openalpaca/src/main.rs`.
- Command modules: `apps/openalpaca/src/commands/*.rs`.
- The CLI resolves daemon connection/auth from `discovery.json`.

## Auth

- Reads discovery token and sends `Authorization: Bearer <token>` to protected daemon routes.
- Uses query token for chat/event streaming endpoints where required.

## Endpoints

| Command | Purpose | Source |
|---|---|---|
| `openalpaca daemon` | Manage daemon process (status, tail, start, stop) | `apps/openalpaca/src/commands/daemon.rs` |
| `openalpaca config` | Manage system configuration (interactive if no subcommands) | `apps/openalpaca/src/commands/config.rs` |
| `openalpaca gui` | Manage GUI process | `apps/openalpaca/src/commands/gui.rs` |
| `openalpaca connector` | Manage platform connectors (Telegram, etc.) | `apps/openalpaca/src/commands/connector.rs` |
| `openalpaca tasks` | Manage tasks (list, status, create, cancel, pause, resume) | `apps/openalpaca/src/commands/tasks.rs` |
| `openalpaca agents` | Manage agents (list, status, config, create, remove) | `apps/openalpaca/src/commands/agents.rs` |
| `openalpaca llm` | Manage LLM settings, keys, and usage | `apps/openalpaca/src/commands/llm.rs` |
| `openalpaca chat` | Chat with the Orchestrator | `apps/openalpaca/src/commands/chat.rs` |
| `openalpaca plugin` | Manage plugins (list, approve, deny, enable, disable, config) | `apps/openalpaca/src/commands/plugin.rs` |
| `openalpaca ext` | Manage extensions — MCP servers and plugins (list, info, enable, disable, reload, approve, deny, remove) | `apps/openalpaca/src/commands/ext.rs` |

## Request/Query Types

- CLI argument and subcommand types are defined with `clap` derive structs/enums in command modules.

## Response Shapes

- Output is user-facing table/json text emitted by each command module.
- Machine-readable outputs are gated by `--format json` where supported.

## Streaming

- `openalpaca daemon tail` consumes daemon event WebSocket stream.
- `openalpaca chat` streams SSE chat output when interacting with daemon chat routes.

## Related Links

- [Daemon API](openalpacad.md)
- [GUI API](openalpaca-gui.md)

## Command Source Map

### `agents`

- Source: `apps/openalpaca/src/commands/agents.rs`
- Enum `AgentsCommands` variants:
  - `list` (fields: `status`, `format`)
  - `status` (fields: `agent_id`, `format`)
  - `config` (fields: `agent_id`, `format`)
  - `pause` (fields: `agent_id`)
  - `resume` (fields: `agent_id`)
  - `set` (fields: `agent_id`, `key_path`, `value`)
  - `create` (fields: `from_file`, `interactive`)
  - `remove` (fields: `agent_id`)
- Parsed flags: `--format`, `--from-file`, `--interactive`, `--status`

### `ai_config`

- Source: `apps/openalpaca/src/commands/ai_config.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `ai_config_helpers`

- Source: `apps/openalpaca/src/commands/ai_config_helpers.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `chat`

- Source: `apps/openalpaca/src/commands/chat.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `config`

- Source: `apps/openalpaca/src/commands/config.rs`
- Enum `ConfigAction` variants:
  - `set` (fields: `key`, `value`)
  - `get` (fields: `key`)
  - `list` (fields: `all`, `format`, `verbose`)
  - `reset` (fields: `key`, `factory`)
- Parsed flags: `--all`, `--factory`, `--format`, `--verbose`

### `config_handlers`

- Source: `apps/openalpaca/src/commands/config_handlers.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `config_tui`

- Source: `apps/openalpaca/src/commands/config_tui.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `connector`

- Source: `apps/openalpaca/src/commands/connector.rs`
- Enum `ConnectorCommands` variants:
  - `list`
  - `enable` (fields: `name`)
  - `disable` (fields: `name`)
  - `delete` (fields: `name`)
- Parsed flags: none

### `daemon`

- Source: `apps/openalpaca/src/commands/daemon.rs`
- Enum `DaemonAction` variants:
  - `status`
  - `tail` (fields: `count`)
  - `start` (fields: `daemon_only`)
  - `stop`
  - `restart`
- Parsed flags: `--count`, `--daemon-only`

### `ext`

- Source: `apps/openalpaca/src/commands/ext.rs`
- Enum `ExtCommands` variants:
  - `list` (fields: `include_orphaned`, `format`)
  - `info` (fields: `kind`, `id`)
  - `enable` (fields: `kind`, `id`)
  - `disable` (fields: `kind`, `id`)
  - `reload` (fields: `kind`, `id`)
  - `approve` (fields: `id`)
  - `deny` (fields: `id`)
  - `remove` (fields: `id`)
- Parsed flags: `--format`, `--include-orphaned`

### `gui`

- Source: `apps/openalpaca/src/commands/gui.rs`
- Enum `GuiAction` variants:
  - `start`
  - `stop`
- Parsed flags: none

### `llm`

- Source: `apps/openalpaca/src/commands/llm.rs`
- Enum `LlmCommands` variants:
  - `status` (fields: `format`)
  - `keys`
  - `usage` (fields: `agent`, `date`, `key`, `daily`, `format`)
  - `models` (fields: `format`)
  - `strategy` (fields: `provider`, `strategy`)
  - `credentials` (fields: `format`)
  - `backends` (fields: `format`)
  - `provider-usage` (fields: `format`)
- Parsed flags: `--agent`, `--daily`, `--date`, `--format`, `--key`, `--provider`

### `llm_keys`

- Source: `apps/openalpaca/src/commands/llm_keys.rs`
- Enum `KeysCommands` variants:
  - `list` (fields: `format`)
  - `add` (fields: `provider`, `secret`, `priority`, `source`, `notes`)
  - `remove` (fields: `provider`, `key_id`)
  - `validate` (fields: `provider`, `secret`)
  - `set-primary` (fields: `provider`, `key_id`)
  - `reorder` (fields: `key_ids`)
- Parsed flags: `--format`, `--notes`, `--priority`, `--provider`, `--secret`, `--source`

### `llm_status`

- Source: `apps/openalpaca/src/commands/llm_status.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `plugin`

- Source: `apps/openalpaca/src/commands/plugin.rs`
- Enum `PluginCommands` variants:
  - `list` (fields: `format`)
  - `approve` (fields: `name`)
  - `deny` (fields: `name`)
  - `enable` (fields: `name`)
  - `disable` (fields: `name`)
  - `info` (fields: `name`)
  - `config` (fields: `name`, `action`)
- Enum `ConfigAction` variants:
  - `set` (fields: `key`, `value`)
  - `get` (fields: `key`)
- Parsed flags: `--format`

### `status`

- Source: `apps/openalpaca/src/commands/status.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `tail`

- Source: `apps/openalpaca/src/commands/tail.rs`
- No `Subcommand` enum found in module.
- Parsed flags: none

### `tasks`

- Source: `apps/openalpaca/src/commands/tasks.rs`
- Enum `TasksCommands` variants:
  - `list` (fields: `status`, `limit`, `format`)
  - `status` (fields: `task_id`, `format`)
  - `log` (fields: `task_id`, `limit`)
  - `create` (fields: `description`, `priority`)
  - `cancel` (fields: `task_id`)
  - `pause` (fields: `task_id`)
  - `resume` (fields: `task_id`)
- Parsed flags: `--format`, `--limit`, `--priority`, `--status`
