//! Wake module
//!
//! Produces [`WakeEvent`]s from time-based schedules and external watchers (e.g. filesystem).

pub mod manager;
pub mod models;
pub mod scheduler;
pub mod watcher;

// Common exports (crate-root facade)
pub use manager::WakeManager;
pub use models::ScheduledTask;
pub use scheduler::WakeScheduler;
pub use watcher::EventWatcher;
pub use watcher::filesystem::{FileWatchHandle, FilesystemWatcher};

// Re-export WakeEvent for convenience
pub use openalpaca_api::events::WakeEvent;
