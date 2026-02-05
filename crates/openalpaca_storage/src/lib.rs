//! OpenAlpaca Storage Module
//!
//! Provides unified app directory management, discovery mechanism,
//! singleton lock, and SQLite database for daemon/GUI/CLI coordination.

pub mod database;
pub mod discovery;
pub mod migrations;
pub mod models;
pub mod paths;
pub mod repository;

pub use database::Database;
pub use models::{Agent, EventLog, Memory, MemoryRole};
pub use models::{ConversationMap, ExternalIdentity, GlobalUser, LinkToken};
pub use repository::{
    AgentRepository, ConfigRepository, EventLogRepository, IdentityRepository, MemoryRepository,
    PreferenceRepository,
};
