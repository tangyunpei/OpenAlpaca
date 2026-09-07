//! OpenAlpaca Storage Module
//!
//! Provides the single path module (`store`), the discovery mechanism,
//! the singleton lock, and the SQLite database for daemon/GUI/CLI coordination.

pub mod artifacts;
pub mod config_schema;
mod content_io;
pub mod database;
pub mod discovery;
pub mod migrations;
pub mod models;
pub mod repository;
pub mod store;
pub mod uploads;

#[cfg(test)]
pub(crate) mod test_util;

pub use artifacts::{
    ArtifactDiff, ArtifactError, ArtifactQuery, ArtifactRecord, ArtifactStore, ArtifactVersionRow,
    NewArtifact,
};
pub use database::Database;
pub use models::{Agent, EventLog, Memory, MemoryRole};
pub use models::{AgentMetrics, AgentTaskHistory, SubAgentConfig};
pub use models::{ArtifactKind, ArtifactOrigin};
pub use models::{OutcomeKind, Task, TaskStatus};
pub use models::{AttachmentRef, FileAsset, FileAssetStatus, MessageArtifact};
pub use models::{Conversation, ConversationMessage};
pub use models::{ExternalIdentity, GlobalUser, LinkToken};
pub use models::{MemoryKind, MemoryScope, MemorySource, MemoryV2};
pub use models::MessageFeedback;
pub use models::{PREVIEW_CHARS, SkillExecutionEntry, ToolExecutionEntry};
pub use models::SkillHealthMetrics;
pub use repository::{
    ARTIFACT_ROLE, ATTACHMENT_ROLE, AgentRepository, ConfigRepository, ConversationRepository,
    EventLogRepository, FOLLOWUP_KIND_FOLLOWUP, FOLLOWUP_KIND_UNPROCESSED_STEERING,
    FileAssetRepository, FollowupRecord, FollowupRepository, IdentityRepository,
    LlmUsageRepository, MemoryRepository,
    MessageFeedbackRepository, OrchestratorLatencyRepository, PreferenceRepository,
    SESSION_ACTIVE, SESSION_ARCHIVED, SessionFilter, SkillExecutionRepository, StorageBytes,
    SubAgentRepository, SubagentSpanRepository, TaskRepository, resolve_skill_key,
};
pub use repository::llm_usage::{LlmUsageDaily, ProviderCallUsage};
pub use repository::subagent_span::{
    NewSubagentSpan, SPAN_DETAIL_INTERRUPTED, SpanState, SubagentSpanRecord, TemplateRunCount,
};
pub use uploads::{NewUpload, StoredUpload, UploadError, UploadStore};
