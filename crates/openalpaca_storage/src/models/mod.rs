//! Data models for OpenAlpaca storage layer
//!
//! Organizes core, identity, and task models into a single module.

pub mod conversation;
mod core;
pub mod file_asset;
pub mod identity;
pub mod memory;
pub mod subagent;
pub mod task;

pub use conversation::{Conversation, ConversationMessage};
pub use core::{Agent, EventLog, Memory, MemoryRole};
pub use file_asset::{AttachmentRef, FileAsset, FileAssetStatus};
pub use identity::{ConversationMap, ExternalIdentity, GlobalUser, LinkToken};
pub use memory::{MemoryKind, MemoryScope, MemorySource, MemoryV2};
pub use subagent::{AgentMetrics, AgentTaskHistory, SubAgentConfig};
pub use task::{AssignmentStatus, OutcomeKind, Task, TaskAgentAssignment, TaskStatus};
