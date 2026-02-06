//! Data models for OpenAlpaca storage layer
//!
//! Organizes core, identity, and task models into a single module.

mod core;
pub mod conversation;
pub mod identity;
pub mod subagent;
pub mod task;

pub use conversation::ConversationMessage;
pub use core::{Agent, EventLog, Memory, MemoryRole};
pub use identity::{ConversationMap, ExternalIdentity, GlobalUser, LinkToken};
pub use subagent::{AgentMetrics, AgentTaskHistory, SubAgentConfig};
pub use task::{AssignmentStatus, Task, TaskAgentAssignment, TaskStatus};
