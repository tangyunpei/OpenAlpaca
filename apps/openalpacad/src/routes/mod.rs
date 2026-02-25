//! Route handlers for daemon HTTP API

pub mod agents;
pub mod auth;
pub mod chat;
pub mod command;
pub mod connectors;
pub mod events;
pub mod events_history;
pub mod memory;
pub mod dispatch_decisions;
pub mod files;
pub mod orchestrator_latency;
pub mod preferences;
pub mod settings;
pub mod tasks;

pub use agents::{
    agent_action_handler,
    create_agent_from_chat_handler,
    create_agent_from_toml_handler,
    create_agent_handler,
    create_template_from_markdown_handler,
    create_template_handler,
    delete_agent_handler,
    delete_template_handler,
    destroy_instance_handler,
    get_agent_config_handler,
    get_agent_handler,
    get_template_handler,
    get_template_markdown_handler,
    list_agents_handler,
    // Instance endpoints
    list_instances_handler,
    // Template endpoints
    list_templates_handler,
    spawn_instance_handler,
    update_agent_config_handler,
    update_template_handler,
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
pub use files::{get_file_content_handler, get_file_metadata_handler, upload_file_handler};
pub use dispatch_decisions::dispatch_decisions_handler;
pub use orchestrator_latency::{orchestrator_latency_aggregate_handler, orchestrator_latency_handler};
pub use memory::{
    delete_memory_handler, get_memory_handler, index_status_handler, kb_ingest_handler,
    list_memories_handler, reindex_handler,
};
pub use preferences::{
    delete_preference_handler, get_preference_handler, list_preferences_handler,
    set_preference_handler,
};
pub use settings::{
    delete_key, estimate_cost, get_cli_backends, get_daemon_providers, get_discovered_credentials,
    get_key_status, get_llm_pricing, get_llm_settings, get_llm_usage, get_llm_usage_daily,
    get_orchestrator_config, get_provider_usage, list_models, refresh_models, reorder_keys,
    rescan_credentials, set_key_priority, update_orchestrator_config, update_web_search_config,
    upsert_key, validate_key,
};
pub use tasks::{
    create_task_handler, get_task_dag_handler, get_task_handler, list_tasks_handler,
    task_action_handler,
};
