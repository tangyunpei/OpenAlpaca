//! Route handlers for daemon HTTP API

pub mod auth;
pub mod command;
pub mod connectors;
pub mod events;
pub mod events_history;

pub use auth::generate_link_token_handler;
pub use command::command_handler;
pub use connectors::{connector_action_handler, connector_config_handler, list_connectors_handler};
pub use events::events_handler;
pub use events_history::events_history_handler;
