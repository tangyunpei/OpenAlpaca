//! Route handlers for daemon HTTP API

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub mod agents;
mod agents_types;
pub mod auth;
pub mod chat;
mod chat_types;
pub mod command;
pub mod connectors;
pub mod dispatch_decisions;
pub mod events;
pub mod events_history;
pub mod files;
mod files_types;
pub mod orchestrator_latency;
pub mod settings;
mod settings_types;
pub mod plugins;
pub mod skills;
pub mod tasks;
mod tasks_types;

pub use agents::{
    agent_action_handler,
    create_agent_from_toml_handler,
    create_agent_handler,
    create_template_handler,
    delete_agent_handler,
    delete_template_handler,
    get_agent_config_handler,
    get_agent_handler,
    get_template_handler,
    list_agents_handler,
    // Instance endpoints
    list_instances_handler,
    // Template endpoints
    list_templates_handler,
    update_agent_config_handler,
    update_template_handler,
};
pub use auth::generate_link_token_handler;
pub use chat::{
    chat_stream_handler, confirm_tool, delete_chat_history_handler, delete_feedback_handler,
    get_chat_history_handler, get_conversation_messages_handler, get_feedback_handler,
    list_conversations_handler, send_chat_handler, upsert_feedback_handler,
};
pub use command::command_handler;
pub use connectors::{
    connector_action_handler, connector_config_handler, connector_settings_handler,
    list_connectors_handler, update_connector_settings_handler,
};
pub use dispatch_decisions::dispatch_decisions_handler;
pub use events::events_handler;
pub use events_history::events_history_handler;
pub use files::{
    get_file_content_handler, get_file_metadata_handler, open_file_handler, upload_file_handler,
};
pub use orchestrator_latency::{
    orchestrator_latency_aggregate_handler, orchestrator_latency_handler,
};
pub use settings::{
    delete_key, get_cli_backends, get_daemon_providers, get_discovered_credentials,
    get_key_status, get_llm_settings, get_llm_usage, get_llm_usage_daily,
    get_orchestrator_config, get_provider_usage, list_models, refresh_models, reorder_keys,
    rescan_credentials, set_key_priority, update_orchestrator_config, update_web_search_config,
    upsert_key, validate_key,
};
pub use plugins::{
    approve_plugin_handler, deny_plugin_handler, disable_plugin_handler, enable_plugin_handler,
    list_plugins_handler, set_plugin_config_handler,
};
pub use skills::skill_health_handler;
pub use tasks::{
    create_task_handler, get_task_handler, list_tasks_handler,
    task_action_handler,
};

// ── Shared error envelope ────────────────────────────────────────────
//
// Canonical `{"error":{"code":..,"message":..}}` JSON error body. This was
// previously duplicated byte-for-byte in `chat_types.rs` and `files_types.rs`
// (each with its own private `ErrorResponse`/`ErrorDetail`/`error_response`);
// both now delegate here. New routes should call `api_error` directly rather
// than reintroducing a local copy.
//
// Deliberately not applied to the ~30 pre-existing `{"error":"<string>"}`
// sites scattered across the route handlers (plan §7) — that retrofit is
// its own follow-up commit, not part of this cleanup.

#[derive(Serialize)]
struct ApiErrorBody {
    error: ApiErrorDetail,
}

#[derive(Serialize)]
struct ApiErrorDetail {
    code: String,
    message: String,
}

/// Build a JSON error response: `{"error":{"code":<code>,"message":<message>}}`.
pub(crate) fn api_error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErrorBody {
            error: ApiErrorDetail {
                code: code.to_string(),
                message: message.into(),
            },
        }),
    )
        .into_response()
}
