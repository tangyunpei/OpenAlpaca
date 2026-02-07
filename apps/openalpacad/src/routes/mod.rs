//! Route handlers for daemon HTTP API

pub mod agents;
pub mod auth;
pub mod chat;
pub mod command;
pub mod connectors;
pub mod events;
pub mod events_history;
pub mod settings;
pub mod tasks;

pub use agents::{
    agent_action_handler, create_agent_from_chat_handler, create_agent_from_toml_handler,
    create_agent_handler, delete_agent_handler, get_agent_config_handler, get_agent_handler,
    list_agents_handler, update_agent_config_handler,
};
pub use auth::generate_link_token_handler;
pub use chat::{
    chat_stream_handler, delete_chat_history_handler, get_chat_history_handler,
    get_conversation_messages_handler, list_conversations_handler, send_chat_handler,
};
pub use command::command_handler;
pub use connectors::{connector_action_handler, connector_config_handler, list_connectors_handler};
pub use events::events_handler;
pub use events_history::events_history_handler;
pub use settings::{
    delete_key, estimate_cost, get_cli_backends, get_discovered_credentials, get_key_status,
    get_llm_pricing, get_llm_settings, get_llm_usage, get_llm_usage_daily,
    get_orchestrator_config, get_provider_usage, list_models, refresh_models, reorder_keys,
    rescan_credentials, update_orchestrator_config, upsert_key, validate_key,
};
pub use tasks::{create_task_handler, get_task_handler, list_tasks_handler, task_action_handler};
