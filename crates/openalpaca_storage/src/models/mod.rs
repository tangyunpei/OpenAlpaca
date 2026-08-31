//! Data models for OpenAlpaca storage layer
//!
//! Organizes core, identity, and task models into a single module.

pub mod conversation;
mod core;
pub mod feedback;
pub mod file_asset;
pub mod identity;
pub mod memory;
pub mod skill_execution;
pub mod skill_health;
pub mod subagent;
pub mod task;

pub use conversation::{Conversation, ConversationMessage};
pub use core::{Agent, EventLog, Memory, MemoryRole};
pub use file_asset::{AttachmentRef, FileAsset, FileAssetStatus};
pub use identity::{ExternalIdentity, GlobalUser, LinkToken};
pub use memory::{MemoryKind, MemoryScope, MemorySource, MemoryV2};
pub use subagent::{AgentMetrics, AgentTaskHistory, SubAgentConfig};
pub use feedback::MessageFeedback;
pub use skill_execution::{SkillExecutionEntry, ToolExecutionEntry};
pub use skill_health::SkillHealthMetrics;
pub use task::{OutcomeKind, Task, TaskStatus};
