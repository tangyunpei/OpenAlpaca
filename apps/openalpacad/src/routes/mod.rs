//! Route handlers for daemon HTTP API

pub mod command;
pub mod events;
pub mod events_history;

pub use command::command_handler;
pub use events::events_handler;
pub use events_history::events_history_handler;
