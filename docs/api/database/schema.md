# Database Schema (SQLite)

> Generated from migration SQL in `crates/openalpaca_storage/src/migrations/*.sql`.

## Files

- DB path resolver: `openalpaca_storage::paths::database_path()`
- Migrations entrypoint: `openalpaca_storage::migrations::MIGRATIONS`
- Registered migrations: 35

## Tables

### `agent` (table)

Source migration: `021_agent_template_id_not_null.sql`

```sql
id TEXT PRIMARY KEY
name TEXT NOT NULL
persona TEXT
config TEXT
created_at TEXT DEFAULT (datetime('now'))
description TEXT
icon TEXT
status TEXT DEFAULT 'idle'
current_task_id TEXT
skills_json TEXT DEFAULT '[]'
preset_json TEXT DEFAULT '{}'
constraints_json TEXT
updated_at TEXT
llm_config_json TEXT
template_id TEXT NOT NULL DEFAULT ''
```

### `agent_metrics` (table)

Source migration: `007_subagents.sql`

```sql
agent_id TEXT PRIMARY KEY REFERENCES agent(id) ON DELETE CASCADE
tasks_completed INTEGER DEFAULT 0
tasks_failed INTEGER DEFAULT 0
total_runtime_seconds INTEGER DEFAULT 0
average_runtime_seconds REAL DEFAULT 0
success_rate REAL DEFAULT 1.0
updated_at TEXT DEFAULT (datetime('now'))
```

### `agent_task_history` (table)

Source migration: `007_subagents.sql`

```sql
id TEXT PRIMARY KEY
agent_id TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE
task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE
role TEXT NOT NULL
status TEXT NOT NULL
runtime_seconds INTEGER
completed_at TEXT DEFAULT (datetime('now'))
```

### `conversation_map` (table)

Source migration: `011_unified_conversations.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
provider TEXT NOT NULL
provider_conversation_id TEXT NOT NULL
global_user_id TEXT
created_at TEXT DEFAULT (datetime('now'))
FOREIGN KEY (global_user_id) REFERENCES global_user(id)
UNIQUE(provider, provider_conversation_id)
lane_key TEXT
```

### `conversation_message_attachments` (table)

Source migration: `028_message_attachments.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
message_id INTEGER NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE
file_id TEXT NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE
sort_order INTEGER NOT NULL DEFAULT 0
role TEXT NOT NULL DEFAULT 'attachment'
caption TEXT
created_at TEXT NOT NULL DEFAULT (datetime('now'))
```

### `conversation_messages` (table)

Source migration: `028_message_attachments.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
lane_key TEXT NOT NULL
role TEXT NOT NULL
content TEXT NOT NULL
model TEXT
tokens_in INTEGER
tokens_out INTEGER
duration_ms INTEGER
created_at TEXT NOT NULL DEFAULT (datetime('now'))
source TEXT
content_json TEXT
display_text TEXT
```

### `conversations` (table)

Source migration: `014_conversation_summary.sql`

```sql
id TEXT PRIMARY KEY
lane_key TEXT NOT NULL UNIQUE
source TEXT NOT NULL
title TEXT DEFAULT ''
message_count INTEGER DEFAULT 0
last_message_at TEXT
created_at TEXT NOT NULL DEFAULT (datetime('now'))
updated_at TEXT NOT NULL DEFAULT (datetime('now'))
summary TEXT NOT NULL DEFAULT ''
summary_version INTEGER NOT NULL DEFAULT 0
last_summarized_message_id INTEGER NOT NULL DEFAULT 0
summary_updated_at TEXT
```

### `discovered_models` (table)

Source migration: `010_discovered_models.sql`

```sql
model_id TEXT NOT NULL
provider TEXT NOT NULL
input_price_per_million REAL DEFAULT 0
output_price_per_million REAL DEFAULT 0
context_window INTEGER DEFAULT 0
discovered_at TEXT DEFAULT (datetime('now'))
PRIMARY KEY (model_id)
```

### `dispatch_decisions` (table)

Source migration: `025_dispatch_decision_error_message.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
request_id TEXT NOT NULL
task_id TEXT
mode TEXT NOT NULL
reason TEXT NOT NULL
agent_count INTEGER DEFAULT 0
dag_node_count INTEGER
predictability_score REAL
planner_requested_mode TEXT
timestamp TEXT DEFAULT (datetime('now'))
error_message TEXT
```

### `event_log` (table)

Source migration: `001_init.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
timestamp TEXT DEFAULT (datetime('now'))
agent_id TEXT
event_type TEXT NOT NULL
detail TEXT
result TEXT
```

### `external_identity` (table)

Source migration: `003_identity.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
provider TEXT NOT NULL
provider_user_id TEXT NOT NULL
global_user_id TEXT
display_name TEXT
metadata TEXT
created_at TEXT DEFAULT (datetime('now'))
linked_at TEXT
FOREIGN KEY (global_user_id) REFERENCES global_user(id)
UNIQUE(provider, provider_user_id)
```

### `file_assets` (table)

Source migration: `027_file_assets.sql`

```sql
id TEXT PRIMARY KEY
owner_id TEXT NOT NULL
sha256 TEXT NOT NULL
filename TEXT NOT NULL
mime_type TEXT NOT NULL
size_bytes INTEGER NOT NULL
storage_path TEXT NOT NULL
status TEXT NOT NULL DEFAULT 'uploaded'
extracted_text TEXT
extract_error TEXT
metadata_json TEXT
created_at TEXT NOT NULL DEFAULT (datetime('now'))
updated_at TEXT NOT NULL DEFAULT (datetime('now'))
```

### `global_user` (table)

Source migration: `003_identity.sql`

```sql
id TEXT PRIMARY KEY
display_name TEXT
created_at TEXT DEFAULT (datetime('now'))
updated_at TEXT DEFAULT (datetime('now'))
```

### `lane_followups` (table)

Source migration: `033_lane_followups.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
lane_key TEXT NOT NULL
kind TEXT NOT NULL CHECK(kind IN ('followup','unprocessed_steering'))
content TEXT NOT NULL
principal_json TEXT NOT NULL
workspace_path TEXT
source_task_id TEXT
status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','running','done','cancelled'))
created_at TEXT NOT NULL DEFAULT (datetime('now'))
updated_at TEXT NOT NULL DEFAULT (datetime('now'))
```

### `link_token` (table)

Source migration: `003_identity.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
token TEXT NOT NULL UNIQUE
global_user_id TEXT NOT NULL
expires_at TEXT NOT NULL
used_at TEXT
created_at TEXT DEFAULT (datetime('now'))
FOREIGN KEY (global_user_id) REFERENCES global_user(id)
```

### `llm_call_log` (table)

Source migration: `008_llm_usage.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
timestamp TEXT DEFAULT (datetime('now'))
agent_id TEXT
task_id TEXT
provider TEXT NOT NULL
model TEXT NOT NULL
key_id TEXT
input_tokens INTEGER DEFAULT 0
output_tokens INTEGER DEFAULT 0
cost_usd REAL DEFAULT 0
status TEXT DEFAULT 'success'
latency_ms INTEGER
error_message TEXT
```

### `llm_usage_daily` (table)

Source migration: `008_llm_usage.sql`

```sql
date TEXT NOT NULL
agent_id TEXT NOT NULL
model TEXT NOT NULL
total_requests INTEGER DEFAULT 0
total_input_tokens INTEGER DEFAULT 0
total_output_tokens INTEGER DEFAULT 0
total_cost_usd REAL DEFAULT 0
PRIMARY KEY (date, agent_id, model)
```

### `memory` (table)

Source migration: `018_memory_lifecycle.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
owner_id TEXT NOT NULL
kind TEXT NOT NULL
scope TEXT NOT NULL
scope_id TEXT NOT NULL DEFAULT ''
source TEXT NOT NULL
content TEXT NOT NULL
content_hash TEXT NOT NULL
importance REAL NOT NULL DEFAULT 0.5
confidence REAL NOT NULL DEFAULT 0.7
created_at TEXT NOT NULL DEFAULT (datetime('now'))
metadata TEXT
updated_at TEXT
supersedes_id INTEGER REFERENCES memory(id) ON DELETE SET NULL
last_accessed_at TEXT
```

### `message_feedback` (table)

Source migration: `031_message_feedback.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
message_id INTEGER NOT NULL UNIQUE
feedback TEXT NOT NULL CHECK(feedback IN ('positive', 'negative'))
comment TEXT
created_at TEXT DEFAULT (datetime('now'))
updated_at TEXT DEFAULT (datetime('now'))
FOREIGN KEY (message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE
```

### `orchestrator_latency` (table)

Source migration: `022_orchestrator_latency.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
request_id TEXT NOT NULL
mode TEXT NOT NULL
planner_ms INTEGER DEFAULT 0
dispatch_ms INTEGER DEFAULT 0
ack_ms INTEGER DEFAULT 0
fallback_reason TEXT
auto_promotion_reason TEXT
timestamp TEXT DEFAULT (datetime('now'))
```

### `preference` (table)

Source migration: `005_preference.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
user_id TEXT NOT NULL
key TEXT NOT NULL
value TEXT NOT NULL
version INTEGER NOT NULL DEFAULT 1
created_at TEXT DEFAULT (datetime('now'))
updated_at TEXT DEFAULT (datetime('now'))
UNIQUE(user_id, key)
```

### `schema_version` (table)

Source migration: `001_init.sql`

```sql
version INTEGER PRIMARY KEY
```

### `skill_execution_log` (table)

Source migration: `031_message_feedback.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
request_id TEXT NOT NULL
skill_id TEXT NOT NULL
agent_id TEXT NOT NULL DEFAULT 'orchestrator'
status TEXT NOT NULL
finish_reason TEXT
error_message TEXT
validation_failures TEXT
duration_ms INTEGER NOT NULL
rounds_used INTEGER
tool_calls_made INTEGER
input_tokens INTEGER DEFAULT 0
output_tokens INTEGER DEFAULT 0
cost_usd REAL DEFAULT 0.0
model_used TEXT
query_preview TEXT
route_score REAL
was_auto_selected INTEGER DEFAULT 0
repair_attempted INTEGER DEFAULT 0
repair_succeeded INTEGER DEFAULT 0
timestamp TEXT DEFAULT (datetime('now'))
response_message_id INTEGER
```

### `system_config` (table)

Source migration: `004_config.sql`

```sql
key TEXT PRIMARY KEY
value TEXT NOT NULL
kind TEXT NOT NULL
updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
```

### `task` (table)

Source migration: `029_task_outcome.sql`

```sql
id TEXT PRIMARY KEY
title TEXT NOT NULL
description TEXT
status TEXT NOT NULL DEFAULT 'queued'
priority INTEGER NOT NULL DEFAULT 0
progress_current INTEGER
progress_total INTEGER
result_summary TEXT
created_by TEXT NOT NULL
source_lane TEXT NOT NULL
created_at TEXT DEFAULT (datetime('now'))
updated_at TEXT DEFAULT (datetime('now'))
completed_at TEXT
state_json TEXT
state_version INTEGER NOT NULL DEFAULT 0
outcome_json TEXT
outcome_kind TEXT
artifact_count INTEGER NOT NULL DEFAULT 0
```

### `task_agent_assignment` (table)

Source migration: `012_assignment_output.sql`

```sql
id TEXT PRIMARY KEY
task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE
agent_id TEXT NOT NULL
role TEXT NOT NULL
status TEXT NOT NULL DEFAULT 'pending'
step_order INTEGER
started_at TEXT
completed_at TEXT
result_output TEXT
```

### `tool_execution_log` (table)

Source migration: `030_skill_tool_execution_log.sql`

```sql
id INTEGER PRIMARY KEY AUTOINCREMENT
request_id TEXT
agent_id TEXT NOT NULL
tool_name TEXT NOT NULL
success INTEGER NOT NULL
duration_ms INTEGER NOT NULL
error_message TEXT
timestamp TEXT DEFAULT (datetime('now'))
```

## Indexes

| Name | Table | Kind | Columns/Expr | Source |
|---|---|---|---|---|
| `idx_agent_status` | `agent` | `INDEX` | `status` | `007_subagents.sql` |
| `idx_agent_task_history` | `agent_task_history` | `INDEX` | `agent_id, completed_at DESC` | `007_subagents.sql` |
| `idx_conversation_map_provider` | `conversation_map` | `INDEX` | `provider, provider_conversation_id` | `003_identity.sql` |
| `idx_msg_attach_message` | `conversation_message_attachments` | `INDEX` | `message_id` | `028_message_attachments.sql` |
| `idx_conv_msg_created` | `conversation_messages` | `INDEX` | `created_at` | `009_conversation_messages.sql` |
| `idx_conv_msg_lane` | `conversation_messages` | `INDEX` | `lane_key` | `009_conversation_messages.sql` |
| `idx_conv_msg_lane_id` | `conversation_messages` | `INDEX` | `lane_key, id` | `014_conversation_summary.sql` |
| `idx_conversations_source` | `conversations` | `INDEX` | `source` | `011_unified_conversations.sql` |
| `idx_conversations_updated` | `conversations` | `INDEX` | `updated_at DESC` | `011_unified_conversations.sql` |
| `idx_discovered_models_provider` | `discovered_models` | `INDEX` | `provider` | `010_discovered_models.sql` |
| `idx_dd_mode` | `dispatch_decisions` | `INDEX` | `mode, timestamp DESC` | `024_dispatch_decision_request_id.sql` |
| `idx_dd_request` | `dispatch_decisions` | `INDEX` | `request_id` | `024_dispatch_decision_request_id.sql` |
| `idx_dd_ts` | `dispatch_decisions` | `INDEX` | `timestamp DESC` | `024_dispatch_decision_request_id.sql` |
| `idx_event_log_agent` | `event_log` | `INDEX` | `agent_id` | `001_init.sql` |
| `idx_event_log_timestamp` | `event_log` | `INDEX` | `timestamp` | `001_init.sql` |
| `idx_event_log_type` | `event_log` | `INDEX` | `event_type` | `001_init.sql` |
| `idx_external_identity_global_user` | `external_identity` | `INDEX` | `global_user_id` | `003_identity.sql` |
| `idx_external_identity_provider` | `external_identity` | `INDEX` | `provider, provider_user_id` | `003_identity.sql` |
| `idx_file_assets_owner` | `file_assets` | `INDEX` | `owner_id` | `027_file_assets.sql` |
| `idx_file_assets_sha256` | `file_assets` | `INDEX` | `sha256` | `027_file_assets.sql` |
| `idx_lane_followups_lane_status` | `lane_followups` | `INDEX` | `lane_key, status, id` | `033_lane_followups.sql` |
| `idx_link_token_token` | `link_token` | `INDEX` | `token` | `003_identity.sql` |
| `idx_llm_call_log_agent` | `llm_call_log` | `INDEX` | `agent_id, timestamp DESC` | `008_llm_usage.sql` |
| `idx_llm_call_log_task` | `llm_call_log` | `INDEX` | `task_id, timestamp DESC` | `008_llm_usage.sql` |
| `idx_memory_agent` | `memory` | `INDEX` | `agent_id` | `001_init.sql` |
| `idx_memory_content_hash` | `memory` | `UNIQUE` | `owner_id, scope, scope_id, content_hash` | `026_memory_scope_dedup.sql` |
| `idx_memory_decay` | `memory` | `INDEX` | `owner_id, kind, last_accessed_at` | `018_memory_lifecycle.sql` |
| `idx_memory_importance` | `memory` | `INDEX` | `owner_id, importance` | `018_memory_lifecycle.sql` |
| `idx_memory_owner` | `memory` | `INDEX` | `owner_id` | `015_memory_v2.sql` |
| `idx_memory_owner_created` | `memory` | `INDEX` | `owner_id, created_at DESC` | `019_memory_fixes.sql` |
| `idx_memory_owner_kind` | `memory` | `INDEX` | `owner_id, kind` | `015_memory_v2.sql` |
| `idx_memory_owner_scope` | `memory` | `INDEX` | `owner_id, scope, scope_id` | `015_memory_v2.sql` |
| `idx_memory_supersedes` | `memory` | `INDEX` | `supersedes_id` | `018_memory_lifecycle.sql` |
| `idx_memory_timestamp` | `memory` | `INDEX` | `timestamp` | `001_init.sql` |
| `idx_mf_feedback` | `message_feedback` | `INDEX` | `feedback` | `031_message_feedback.sql` |
| `idx_orch_latency_mode` | `orchestrator_latency` | `INDEX` | `mode, timestamp DESC` | `022_orchestrator_latency.sql` |
| `idx_orch_latency_ts` | `orchestrator_latency` | `INDEX` | `timestamp DESC` | `022_orchestrator_latency.sql` |
| `idx_preference_user` | `preference` | `INDEX` | `user_id` | `005_preference.sql` |
| `idx_sel_agent` | `skill_execution_log` | `INDEX` | `agent_id, skill_id` | `030_skill_tool_execution_log.sql` |
| `idx_sel_request_id` | `skill_execution_log` | `UNIQUE` | `request_id` | `030_skill_tool_execution_log.sql` |
| `idx_sel_response_msg` | `skill_execution_log` | `INDEX` | `response_message_id` | `031_message_feedback.sql` |
| `idx_sel_skill_ts` | `skill_execution_log` | `INDEX` | `skill_id, timestamp DESC` | `030_skill_tool_execution_log.sql` |
| `idx_sel_status` | `skill_execution_log` | `INDEX` | `skill_id, status` | `030_skill_tool_execution_log.sql` |
| `idx_task_created_by` | `task` | `INDEX` | `created_by` | `006_tasks.sql` |
| `idx_task_status` | `task` | `INDEX` | `status` | `006_tasks.sql` |
| `idx_task_agent_task` | `task_agent_assignment` | `INDEX` | `task_id` | `006_tasks.sql` |
| `idx_tel_request` | `tool_execution_log` | `INDEX` | `request_id` | `030_skill_tool_execution_log.sql` |
| `idx_tel_tool_ts` | `tool_execution_log` | `INDEX` | `tool_name, timestamp DESC` | `030_skill_tool_execution_log.sql` |

## Triggers

| Name | Source |
|---|---|
| `memory_ad` | `015_memory_v2.sql` |
| `memory_ai` | `015_memory_v2.sql` |
| `memory_au` | `018_memory_lifecycle.sql` |
