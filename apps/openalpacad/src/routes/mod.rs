//! Route handlers for daemon HTTP API

pub mod command;
pub mod events;

pub use command::command_handler;
pub use events::events_handler;
