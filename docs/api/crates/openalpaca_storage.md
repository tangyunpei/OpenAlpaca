# `openalpaca_storage`

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Member path: `crates/openalpaca_storage`
- Entry: `crates/openalpaca_storage/src/lib.rs`

- OpenAlpaca Storage Module
- Provides the single path module (`store`), the discovery mechanism,
- the singleton lock, and the SQLite database for daemon/GUI/CLI coordination.

## Modules

- `artifacts` (crates/openalpaca_storage/src/artifacts/mod.rs)
- `config_schema` (crates/openalpaca_storage/src/config_schema/mod.rs)
- `database` (crates/openalpaca_storage/src/database/mod.rs)
- `discovery` (crates/openalpaca_storage/src/discovery/mod.rs)
- `migrations` (crates/openalpaca_storage/src/migrations/mod.rs)
- `models` (crates/openalpaca_storage/src/models/mod.rs)
- `repository` (crates/openalpaca_storage/src/repository/mod.rs)
- `store` (crates/openalpaca_storage/src/store/mod.rs)
- `uploads` (crates/openalpaca_storage/src/uploads/mod.rs)

## Re-exports

- `pub use artifacts::{ ArtifactDiff, ArtifactError, ArtifactQuery, ArtifactRecord, ArtifactStore, ArtifactVersionRow, NewArtifact, };`
- `pub use database::Database;`
- `pub use models::{Agent, EventLog, Memory, MemoryRole};`
- `pub use models::{AgentMetrics, AgentTaskHistory, SubAgentConfig};`
- `pub use models::{ArtifactKind, ArtifactOrigin};`
- `pub use models::{OutcomeKind, Task, TaskStatus};`
- `pub use models::{AttachmentRef, FileAsset, FileAssetStatus, MessageArtifact};`
- `pub use models::{Conversation, ConversationMessage};`
- `pub use models::{ExternalIdentity, GlobalUser, LinkToken};`
- `pub use models::{MemoryKind, MemoryScope, MemorySource, MemoryV2};`
- `pub use models::MessageFeedback;`
- `pub use models::{PREVIEW_CHARS, SkillExecutionEntry, ToolExecutionEntry};`
- `pub use models::SkillHealthMetrics;`
- `pub use repository::{ ARTIFACT_ROLE, ATTACHMENT_ROLE, AgentRepository, ConfigRepository, ConversationRepository, EventLogRepository, FOLLOWUP_KIND_FOLLOWUP, FOLLOWUP_KIND_UNPROCESSED_STEERING, FileAssetRepository, FollowupRecord, FollowupRepository, IdentityRepository, LlmUsageRepository, MemoryRepository, MessageFeedbackRepository, OrchestratorLatencyRepository, PreferenceRepository, SESSION_ACTIVE, SESSION_ARCHIVED, SessionFilter, SkillExecutionRepository, SubAgentRepository, SubagentSpanRepository, TaskRepository, };`
- `pub use repository::llm_usage::LlmUsageDaily;`
- `pub use repository::subagent_span::{ NewSubagentSpan, SPAN_DETAIL_INTERRUPTED, SpanState, SubagentSpanRecord, TemplateRunCount, };`
- `pub use uploads::{NewUpload, StoredUpload, UploadError, UploadStore};`

## Related Links

- [API Index](../README.md)
