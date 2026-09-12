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
pub mod workspaces;

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
    install_extension_handler, list_extensions_handler, set_extension_config_handler,
    update_extension_handler, validate_plugin_handler,
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
    get_usage_summary, rescan_credentials, set_key_priority, set_provider_enabled,
    update_orchestrator_config, update_web_search_config, upsert_key, validate_key,
};
pub use sessions::{
    activate_session_handler, archive_session_handler, create_session_handler,
    delete_session_handler, get_session_events_handler, get_session_handler,
    get_session_messages_handler, list_sessions_handler, patch_session_handler,
};
pub use skills::{list_skills_handler, skill_health_handler};
pub use status::status_handler;
pub use tasks::{
    create_task_handler, get_task_handler, get_task_timeline_handler, list_tasks_handler,
    rerun_task_handler, steer_task_handler, task_action_handler,
};
pub use tools::list_tools_handler;
pub use workspaces::{get_workspace_handler, purge_workspace_handler, rebase_workspace_handler};

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
///
/// **Every XML essence counts (ruling R83).** `text/xml` and `application/xml`
/// are rendered as documents, and an XML document whose root is XHTML or SVG —
/// or one carrying an `xml-stylesheet` XSLT that produces either — runs script
/// on the daemon origin with the bearer token sitting in `?token=`. The `+xml`
/// suffix is matched for the same reason, so a structured syntax nobody has
/// enumerated here (`application/mathml+xml`, `image/svg+xml`) is covered by the
/// rule rather than by this list being complete.
fn is_script_bearing_document(mime_type: &str) -> bool {
    let essence = mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase();
    matches!(essence.as_str(), "text/html" | "text/xml" | "application/xml")
        || essence.ends_with("+xml")
}

/// `inline; filename="…"`, plus RFC 5987's `filename*=UTF-8''…` whenever the
/// name is not plain ASCII.
///
/// A `HeaderValue` is bytes, and `HeaderValue::from_str`/`parse` refuses any
/// char above `\x7f` — so a CJK (or accented, or emoji) filename used to drop
/// the whole `Content-Disposition` header on the floor, and the browser fell
/// back to the last path segment of the URL: the artifact's id. The ASCII
/// fallback keeps a value every client can read, and `filename*` carries the
/// real name for the ones that implement RFC 6266 (all of them, since 2011).
///
/// The fallback substitutes `_` for anything outside printable ASCII and for the
/// two characters that would end the quoted string early (`"` and `\`), plus
/// CR/LF, which is the header-injection guard this used to do on its own.
fn inline_disposition(filename: &str) -> String {
    let fallback: String = filename
        .chars()
        .map(|c| match c {
            '"' | '\\' => '_',
            c if c.is_ascii_graphic() || c == ' ' => c,
            _ => '_',
        })
        .collect();
    let fallback = match fallback.trim() {
        "" => "file".to_string(),
        trimmed => trimmed.to_string(),
    };
    let mut value = format!("inline; filename=\"{fallback}\"");
    if filename != fallback {
        value.push_str("; filename*=UTF-8''");
        value.push_str(&percent_encode_attr(filename));
    }
    value
}

/// Percent-encode everything outside RFC 5987's `attr-char` set — the encoding
/// `filename*` takes, over the name's UTF-8 bytes.
fn percent_encode_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '!' | '#' | '$' | '&' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
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
///   script from (R27, widened to every XML essence by R83). `?token=` made
///   these routes reachable by navigation,
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
    if let Ok(value) = inline_disposition(filename).parse() {
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
///
/// **Percent-encoded UTF-8 on the wire (ruling R81).** A header value is bytes
/// and `HeaderValue::to_str` refuses anything above `\x7f`, so a CJK project
/// path — an ordinary path here — used to be dropped silently: the turn ran
/// with no project at all. Clients encode (`apps/openalpaca/src/chat_stream`,
/// the GUI's `lib/api`) and this decodes. A plain ASCII value carrying no `%`
/// decodes to itself, so nothing that worked before changes; a value whose
/// escapes are not valid UTF-8 is taken as written rather than refused, because
/// a path the daemon cannot resolve is already answered downstream.
pub(crate) fn workspace_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-workspace-path")
        .and_then(|v| v.to_str().ok())
        .map(decode_workspace_header)
}

/// The decoding half of [`workspace_header`], shared with `/v1/chat`'s own read
/// of the same header (`routes/chat.rs`).
pub(crate) fn decode_workspace_header(raw: &str) -> String {
    urlencoding::decode(raw)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| raw.to_string())
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

/// [`request_project_root`], answering from the filesystem every time.
///
/// The marker walk behind it memoises `canonical start path → root` for the
/// process lifetime (`memory::workspace`), which is right for a chat turn — a
/// project's markers do not move while the daemon runs — and wrong for the three
/// `/v1/workspaces` routes, whose whole job is to answer about a directory the
/// user is *changing*. `422 WORKSPACE_NOT_A_ROOT` tells the caller to "give it a
/// project marker of its own first"; the cached answer made that instruction
/// impossible to follow until the daemon restarted, because the refusal's own
/// call wrote the entry. Same rule, same fold — no cache.
pub(crate) fn request_project_root_uncached(workspace_path: Option<&str>) -> Option<String> {
    let path = workspace_path?;
    MemoryScopeContext::for_request_uncached(Some(path)).request_workspace_root
}

// ── Paging bounds, shared (R26) ──────────────────────────────────────────
//
// `artifacts.rs` keeps its own `page_limit`: its `None` means "the store's
// default", a contract these routes do not have (they pass an integer straight
// into SQL, where a non-positive value means *no limit* — the whole table).

/// The largest page any list route will build, whatever `?limit=` asks for.
pub(crate) const MAX_PAGE_LIMIT: i64 = 500;

/// `?limit=`, resolved against the route's default and [`MAX_PAGE_LIMIT`].
///
/// A non-positive value is the default rather than SQLite's "no limit": `-1`
/// must not be the way to read every row of a table over the one connection
/// every subsystem shares. An oversized value is *clamped* rather than refused —
/// the caller gets a smaller page and an exact `total`, which is how it learns
/// there is more to fetch.
pub(crate) fn page_limit(requested: Option<i64>, default: i64) -> i64 {
    requested
        .filter(|n| *n > 0)
        .unwrap_or(default)
        .min(MAX_PAGE_LIMIT)
}

/// `?offset=`, floored at zero — a negative offset is not a page anyone can
/// serve, and SQLite silently reads it as none.
pub(crate) fn page_offset(requested: Option<i64>) -> i64 {
    requested.unwrap_or(0).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R83: every XML essence is a document a browser may run script from.
    #[test]
    fn the_sandbox_covers_html_svg_and_every_xml_essence() {
        for mime in [
            "text/html",
            "text/html; charset=utf-8",
            "TEXT/HTML",
            "application/xhtml+xml",
            "image/svg+xml",
            "text/xml",
            "application/xml",
            "application/xml; charset=utf-8",
            "application/mathml+xml",
        ] {
            assert!(
                is_script_bearing_document(mime),
                "{mime} must be sandboxed"
            );
        }
        for mime in [
            "image/png",
            "text/plain",
            "application/pdf",
            "application/json",
            "text/xmlish",
        ] {
            assert!(
                !is_script_bearing_document(mime),
                "{mime} is not a script-bearing document"
            );
        }
    }

    /// The sandbox and the disposition are on the shared body, so the assertion
    /// is about a real response rather than about the predicate alone.
    #[tokio::test]
    async fn a_content_response_sandboxes_xml_and_not_an_image() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("doc.xml");
        std::fs::write(&path, b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").expect("write");

        let xml = content_response(&path, "application/xml", "doc.xml").await;
        assert_eq!(
            xml.headers()
                .get(axum::http::header::CONTENT_SECURITY_POLICY)
                .map(|v| v.to_str().unwrap()),
            Some("sandbox"),
        );

        let png = content_response(&path, "image/png", "shot.png").await;
        assert!(
            png.headers()
                .get(axum::http::header::CONTENT_SECURITY_POLICY)
                .is_none(),
            "an image is not a document; the sandbox is not a blanket",
        );
    }

    /// A non-ASCII filename used to drop the whole header (a `HeaderValue`
    /// refuses those bytes), leaving the browser to name the file after the id
    /// in the URL. RFC 5987: an ASCII fallback *and* the real name.
    #[test]
    fn a_non_ascii_filename_is_sent_in_both_forms() {
        let value = inline_disposition("季度报告.pdf");
        assert_eq!(
            value,
            "inline; filename=\"____.pdf\"; filename*=UTF-8''%E5%AD%A3%E5%BA%A6%E6%8A%A5%E5%91%8A.pdf"
        );
        assert!(
            value.parse::<axum::http::HeaderValue>().is_ok(),
            "the whole point: the header is sendable",
        );

        // Plain ASCII keeps exactly the value it had before R83's sibling fix —
        // one form, no `filename*` noise.
        assert_eq!(
            inline_disposition("notes.md"),
            "inline; filename=\"notes.md\""
        );
        // Header injection and quote-escaping are still handled.
        assert_eq!(
            inline_disposition("a\"b\\c\r\n.txt"),
            "inline; filename=\"a_b_c__.txt\"; filename*=UTF-8''a%22b%5Cc%0D%0A.txt"
        );
        // A name with nothing ASCII in it keeps one placeholder per character,
        // and a name that is nothing but blanks still has a usable fallback.
        assert_eq!(
            inline_disposition("报告"),
            "inline; filename=\"__\"; filename*=UTF-8''%E6%8A%A5%E5%91%8A"
        );
        assert_eq!(
            inline_disposition("  "),
            "inline; filename=\"file\"; filename*=UTF-8''%20%20"
        );
    }

    /// R81: the header is percent-encoded UTF-8, and a plain ASCII value with no
    /// escapes is its own encoding.
    #[test]
    fn a_cjk_workspace_path_round_trips_through_the_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-workspace-path",
            "%2FUsers%2Fjun%2F%E9%A1%B9%E7%9B%AE%2Fopenalpaca"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            workspace_header(&headers).as_deref(),
            Some("/Users/jun/项目/openalpaca")
        );

        let mut plain = HeaderMap::new();
        plain.insert("x-workspace-path", "/Users/jun/repo".parse().unwrap());
        assert_eq!(
            workspace_header(&plain).as_deref(),
            Some("/Users/jun/repo"),
            "a value with no escapes is unchanged",
        );

        // An escape that is not valid UTF-8 is taken as written rather than
        // dropping the header: the path simply will not resolve.
        let mut broken = HeaderMap::new();
        broken.insert("x-workspace-path", "/tmp/%FF".parse().unwrap());
        assert_eq!(workspace_header(&broken).as_deref(), Some("/tmp/%FF"));
    }

    /// `?limit=-1` must not be the way to read a whole table.
    #[test]
    fn a_page_limit_defaults_on_non_positive_and_caps_on_oversized() {
        assert_eq!(page_limit(None, 50), 50);
        assert_eq!(page_limit(Some(-1), 50), 50);
        assert_eq!(page_limit(Some(0), 50), 50);
        assert_eq!(page_limit(Some(20), 50), 20);
        assert_eq!(page_limit(Some(100_000), 50), MAX_PAGE_LIMIT);
        assert_eq!(page_limit(Some(i64::MAX), 50), MAX_PAGE_LIMIT);

        assert_eq!(page_offset(None), 0);
        assert_eq!(page_offset(Some(-5)), 0);
        assert_eq!(page_offset(Some(7)), 7);
    }
}
