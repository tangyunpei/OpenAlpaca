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
pub mod artifacts;
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
pub mod followups;
pub mod orchestrator_latency;
pub mod sessions;
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
pub use artifacts::{
    get_artifact_content_handler, get_artifact_diff_handler, get_artifact_handler,
    get_artifact_version_content_handler, list_artifact_versions_handler, list_artifacts_handler,
    pin_artifact_handler,
};
pub use auth::{generate_link_token_handler, get_me_handler};
pub use chat::{
    chat_stream_handler, confirm_tool, delete_chat_history_handler, delete_feedback_handler,
    get_chat_history_handler, get_feedback_handler, send_chat_handler, upsert_feedback_handler,
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
pub use followups::{cancel_followup_handler, list_followups_handler, queue_followup_handler};
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
pub use sessions::{
    activate_session_handler, archive_session_handler, create_session_handler,
    delete_session_handler, get_session_events_handler, get_session_handler,
    get_session_messages_handler, list_sessions_handler, patch_session_handler,
};
pub use skills::skill_health_handler;
pub use status::status_handler;
pub use tasks::{
    create_task_handler, get_task_handler, get_task_timeline_handler, list_tasks_handler,
    rerun_task_handler, steer_task_handler, task_action_handler,
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

// ── The content routes' inline auth (GAP-11) ─────────────────────────────
//
// `/v1/files/{id}/content`, `/v1/artifacts/{id}/content` and
// `/v1/artifacts/{id}/versions/{n}/content` sit outside the bearer middleware
// so a webview `<img src>`/`<iframe src>` — which cannot set a request header —
// can load bytes. They authenticate inline instead, accepting the token in
// *either* form. Authorization is untouched: each handler still 404s a row this
// owner cannot see.

/// `?token=<bearer>` — the query half of a content route's inline auth. Shared
/// by `/v1/files/{id}/content` and `/v1/artifacts/{id}/versions/{n}/content`;
/// the artifact head's content route adds `?version=` and has its own type.
#[derive(Debug, Default, serde::Deserialize)]
pub struct TokenParams {
    pub token: Option<String>,
}

/// Whether a content request carries the daemon token, as `?token=<t>` (the way
/// `/v1/chat/stream` does) or as `Authorization: Bearer <t>` (the way every
/// other route does, so the GUI's existing `apiFetchBlob` keeps working).
pub(crate) fn content_token_ok(
    headers: &HeaderMap,
    query_token: Option<&str>,
    expected: &str,
) -> bool {
    if query_token == Some(expected) {
        return true;
    }
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected)
}

/// The plain-text `401` the other inline-auth route answers with
/// (`chat_stream_handler`), so the two speak with one voice.
pub(crate) fn invalid_token() -> Response {
    (StatusCode::UNAUTHORIZED, "Invalid token").into_response()
}

/// The MIME types a browser will execute script from when it *navigates* to a
/// response — the reason [`content_response`] sends a sandboxing CSP (R27).
///
/// Compared on the essence, so a `;charset=` parameter or an odd case cannot
/// slip past. `image/svg+xml` belongs here: an SVG document can carry a
/// `<script>`, and an uploaded one is user-supplied bytes.
fn is_script_bearing_document(mime_type: &str) -> bool {
    let essence = mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase();
    matches!(
        essence.as_str(),
        "text/html" | "application/xhtml+xml" | "image/svg+xml"
    )
}

/// Stream a file as a content response: its MIME type, an inline
/// `Content-Disposition` with the filename sanitised against header injection,
/// and the three headers that make serving those bytes on the daemon origin
/// safe.
///
/// - `Referrer-Policy: no-referrer` — the §9 mitigation for a bearer token that
///   can now ride in a URL.
/// - `X-Content-Type-Options: nosniff` — the response is what it says it is; a
///   `text/plain` artifact is never sniffed into a document.
/// - `Content-Security-Policy: sandbox` for a document a browser executes
///   script from (R27). `?token=` made these routes reachable by navigation,
///   and an `html` artifact is agent output — possibly assembled from
///   untrusted web content — rendered on the daemon origin, where its own
///   script could read the token straight out of `location.search` and drive
///   every other `/v1/*` route as the user. The sandbox is the bare directive:
///   no `allow-scripts`, no `allow-same-origin`.
///
/// All three are on the shared body rather than per route, so no content
/// response can be missing one.
pub(crate) async fn content_response(
    path: &std::path::Path,
    mime_type: &str,
    filename: &str,
) -> Response {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "IO_ERROR",
                format!("Failed to open file: {e}"),
            );
        }
    };

    let mut headers = HeaderMap::new();
    if let Ok(value) = mime_type.parse() {
        headers.insert(axum::http::header::CONTENT_TYPE, value);
    }
    let safe_filename: String = filename
        .chars()
        .filter(|c| *c != '"' && *c != '\\' && *c != '\r' && *c != '\n')
        .collect();
    if let Ok(value) = format!("inline; filename=\"{safe_filename}\"").parse() {
        headers.insert(axum::http::header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    if is_script_bearing_document(mime_type) {
        headers.insert(
            axum::http::header::CONTENT_SECURITY_POLICY,
            axum::http::HeaderValue::from_static("sandbox"),
        );
    }

    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    (headers, body).into_response()
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
