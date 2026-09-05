//! OpenAlpaca Storage Module
//!
//! Provides the single path module (`store`), the discovery mechanism,
//! the singleton lock, and the SQLite database for daemon/GUI/CLI coordination.

pub mod config_schema;
pub mod database;
pub mod discovery;
pub mod migrations;
pub mod models;
pub mod repository;
pub mod store;

#[cfg(test)]
pub(crate) mod test_util;

pub use database::Database;
pub use models::{Agent, EventLog, Memory, MemoryRole};
pub use models::{AgentMetrics, AgentTaskHistory, SubAgentConfig};
pub use models::{OutcomeKind, Task, TaskStatus};
pub use models::{AttachmentRef, FileAsset, FileAssetStatus};
pub use models::{Conversation, ConversationMessage};
pub use models::{ExternalIdentity, GlobalUser, LinkToken};
pub use models::{MemoryKind, MemoryScope, MemorySource, MemoryV2};
pub use models::MessageFeedback;
pub use models::{SkillExecutionEntry, ToolExecutionEntry};
pub use models::SkillHealthMetrics;
pub use repository::{
    AgentRepository, ConfigRepository, ConversationRepository, EventLogRepository,
    FileAssetRepository, FollowupRecord, FollowupRepository, IdentityRepository,
    LlmUsageRepository, MemoryRepository,
    MessageFeedbackRepository, OrchestratorLatencyRepository, PreferenceRepository,
    SkillExecutionRepository, SubAgentRepository, TaskRepository,
};
pub use repository::llm_usage::LlmUsageDaily;
