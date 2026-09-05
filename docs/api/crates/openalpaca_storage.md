# `openalpaca_storage`

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Member path: `crates/openalpaca_storage`
- Entry: `crates/openalpaca_storage/src/lib.rs`

- OpenAlpaca Storage Module
- Provides the single path module (`store`), the discovery mechanism,
- the singleton lock, and the SQLite database for daemon/GUI/CLI coordination.

## Modules

- `config_schema` (crates/openalpaca_storage/src/config_schema/mod.rs)
- `database` (crates/openalpaca_storage/src/database/mod.rs)
- `discovery` (crates/openalpaca_storage/src/discovery/mod.rs)
- `migrations` (crates/openalpaca_storage/src/migrations/mod.rs)
- `models` (crates/openalpaca_storage/src/models/mod.rs)
- `repository` (crates/openalpaca_storage/src/repository/mod.rs)
- `store` (crates/openalpaca_storage/src/store/mod.rs)

## Re-exports

- `pub use database::Database;`
- `pub use models::{Agent, EventLog, Memory, MemoryRole};`
- `pub use models::{AgentMetrics, AgentTaskHistory, SubAgentConfig};`
- `pub use models::{OutcomeKind, Task, TaskStatus};`
- `pub use models::{AttachmentRef, FileAsset, FileAssetStatus};`
- `pub use models::{Conversation, ConversationMessage};`
- `pub use models::{ExternalIdentity, GlobalUser, LinkToken};`
- `pub use models::{MemoryKind, MemoryScope, MemorySource, MemoryV2};`
- `pub use models::MessageFeedback;`
- `pub use models::{SkillExecutionEntry, ToolExecutionEntry};`
- `pub use models::SkillHealthMetrics;`
- `pub use repository::{ AgentRepository, ConfigRepository, ConversationRepository, EventLogRepository, FileAssetRepository, FollowupRecord, FollowupRepository, IdentityRepository, LlmUsageRepository, MemoryRepository, MessageFeedbackRepository, OrchestratorLatencyRepository, PreferenceRepository, SkillExecutionRepository, SubAgentRepository, TaskRepository, };`
- `pub use repository::llm_usage::LlmUsageDaily;`

## Related Links

- [API Index](../README.md)
