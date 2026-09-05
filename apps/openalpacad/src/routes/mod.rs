//! Route handlers for daemon HTTP API

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use openalpaca_core::memory::scope_context::MemoryScopeContext;
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
pub mod extensions;
pub mod files;
mod files_types;
pub mod orchestrator_latency;
pub mod settings;
mod settings_types;
pub mod skills;
pub mod status;
pub mod tasks;
mod tasks_types;
pub mod tools;

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
pub use auth::{generate_link_token_handler, get_me_handler};
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
pub use extensions::{
    delete_extension_handler, extension_action_handler, get_extension_config_handler,
    list_extensions_handler, set_extension_config_handler,
};
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
pub use skills::skill_health_handler;
pub use status::status_handler;
pub use tasks::{
    create_task_handler, get_task_handler, list_tasks_handler,
    task_action_handler,
};
pub use tools::list_tools_handler;

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

// ── The request's project (plan §4.7) ────────────────────────────────────
//
// Two routes now ask the same question of the same header — `GET /v1/status`
// reports where a turn's files would land, `POST /v1/files/upload` puts an
// upload there — so the header name and the resolution rule live here once.

/// The `x-workspace-path` header `/v1/chat` reads (`routes/chat.rs`), and the
/// same value the CLI sends as `workspace_path` on `/v1/command`.
pub(crate) fn workspace_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-workspace-path")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Resolve a client-sent workspace path the way a chat turn would.
///
/// One resolver, no second opinion: `MemoryScopeContext::for_request` owns the
/// rule (marker walk, canonicalisation, and the `$HOME`-is-not-a-project fold),
/// so a route never resolves a header a second way. `None` — no header, a path
/// under no marker, or one that resolves to the home store — means *no
/// project*, and content placement falls back to the home store. Never the
/// daemon's working directory (ruling R22).
pub(crate) fn request_project_root(workspace_path: Option<&str>) -> Option<String> {
    let path = workspace_path?;
    MemoryScopeContext::for_request(Some(path)).request_workspace_root
}
