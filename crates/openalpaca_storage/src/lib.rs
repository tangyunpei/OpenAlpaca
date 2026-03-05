//! OpenAlpaca Storage Module
//!
//! Provides unified app directory management, discovery mechanism,
//! singleton lock, and SQLite database for daemon/GUI/CLI coordination.

pub mod config_schema;
pub mod database;
pub mod discovery;
pub mod migrations;
pub mod models;
pub mod paths;
pub mod repository;

#[cfg(test)]
pub(crate) mod test_util;

pub use database::Database;
pub use models::{Agent, EventLog, Memory, MemoryRole};
pub use models::{AgentMetrics, AgentTaskHistory, SubAgentConfig};
pub use models::{AssignmentStatus, OutcomeKind, Task, TaskAgentAssignment, TaskStatus};
pub use models::{AttachmentRef, FileAsset, FileAssetStatus};
pub use models::{Conversation, ConversationMessage};
pub use models::{ConversationMap, ExternalIdentity, GlobalUser, LinkToken};
pub use models::{MemoryKind, MemoryScope, MemorySource, MemoryV2};
pub use repository::{
    AgentRepository, ConfigRepository, ConversationRepository, EventLogRepository,
    FileAssetRepository, IdentityRepository, LlmUsageRepository, MemoryRepository,
    OrchestratorLatencyRepository, PreferenceRepository, SubAgentRepository, TaskRepository,
};
pub use repository::llm_usage::LlmUsageDaily;
