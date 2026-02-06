//! Route handlers for daemon HTTP API

pub mod agents;
pub mod auth;
pub mod command;
pub mod connectors;
pub mod events;
pub mod events_history;
pub mod settings;
pub mod tasks;

pub use agents::{agent_action_handler, get_agent_handler, list_agents_handler};
pub use auth::generate_link_token_handler;
pub use command::command_handler;
pub use connectors::{connector_action_handler, connector_config_handler, list_connectors_handler};
pub use events::events_handler;
pub use events_history::events_history_handler;
pub use settings::{
    delete_key, get_key_status, get_llm_settings, reorder_keys, upsert_key, validate_key,
};
pub use tasks::{create_task_handler, get_task_handler, list_tasks_handler, task_action_handler};
