//! Repository layer for database operations
//!
//! Provides CRUD operations for all entities.

pub mod agent;
pub mod config;
pub mod event_log;
pub mod identity;
pub mod memory;
pub mod preference;

pub use agent::AgentRepository;
pub use config::ConfigRepository;
pub use event_log::EventLogRepository;
pub use identity::IdentityRepository;
pub use memory::MemoryRepository;
pub use preference::PreferenceRepository;
