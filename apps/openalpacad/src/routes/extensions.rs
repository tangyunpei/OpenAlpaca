//! Extension management endpoints — the ENABLE axis (design §8, ADR-030).
//!
//! ```text
//! GET    /v1/extensions[?include_orphaned=true]     -> both kinds, one bare array
//! POST   /v1/extensions/{kind}/{id}/enable          -> W then E0–E5
//! POST   /v1/extensions/{kind}/{id}/disable         -> W then T0–T5
//! POST   /v1/extensions/{kind}/{id}/reload          -> T0–T4 then E0–E5, no W
//! POST   /v1/extensions/plugin/{id}/approve|deny    -> consent (plugins only)
//! GET    /v1/extensions/plugin/{id}/config          -> redacted
//! POST   /v1/extensions/plugin/{id}/config          -> one key
//! DELETE /v1/extensions/plugin/{id}                 -> orphaned rows only
//!
//! GAP-24 — the extension itself, not its switch:
//! POST   /v1/extensions/plugin {source:"path",path} -> copy in, then E0–E5
//! POST   /v1/extensions/mcp {name,transport,…}      -> declare, then E0–E5
//! POST   /v1/extensions/plugin/validate {path}      -> the dry run
//! PUT    /v1/extensions/plugin/{id}                 -> T0–T5, replace, E0–E5
//! DELETE /v1/extensions/{kind}/{id}?uninstall=true  -> the real removal
//! ```
//!
//! Two rules run through the whole file.
//!
//! **The error envelope is `{"error":"<word>"}`** — the one the plugins, tasks
//! and agents routes already use (design §8: *"Deliberately **not** a third
//! envelope"*), and the `<word>` is [`ExtensionError`]'s `Display`, which is
//! `not_loaded` / `store_unreadable` / `unsupported_for_kind` / `not_orphaned`
//! / `orphaned` verbatim.
//!
//! **Every verb runs in a detached task whose handle the handler awaits** (R18)
//! — see [`detached`]. An axum request future that is dropped mid-flight (the
//! client hung up, a timeout layer fired) must not abandon a transition halfway
//! and strand a record in `Enabling`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use openalpaca_core::tools::extensions::{
    ExtensionError, ExtensionId, ExtensionKind, ExtensionRecord, ExtensionState, UnapprovedReason,
};
use openalpaca_plugins::{InstallError, InstallOutcome, PluginError};
use serde::Deserialize;

use crate::AppState;
use crate::managers::extensions::{Extensions, InstallFailure, Uninstalled};
use crate::managers::mcp::{DeclarationError, McpDeclaration};

// ── Request types ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SetConfigRequest {
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    /// `?include_orphaned=true`; default `false` (design §8).
    #[serde(default)]
    pub include_orphaned: bool,
}

/// `DELETE /v1/extensions/{kind}/{id}`'s two options (GAP-24).
#[derive(Deserialize)]
pub struct DeleteQuery {
    /// Off by default, and that default is the point: without it the DELETE is
    /// C6's orphan-row removal, which never touches a directory.
    #[serde(default)]
    pub uninstall: bool,
    /// `plugins/.data/<name>/` survives an uninstall unless this is cleared.
    /// When it is, the data is **moved to the trash**, not deleted.
    #[serde(default = "keep_data_default")]
    pub keep_data: bool,
}

fn keep_data_default() -> bool {
    true
}

impl Default for DeleteQuery {
    fn default() -> Self {
        Self {
            uninstall: false,
            keep_data: keep_data_default(),
        }
    }
}

// ── Status mapping ───────────────────────────────────────────────

/// The §8 status codes. Nothing below this line decides one.
///
/// * `404` — unknown id (and an unknown `{kind}` word, which names no resource).
/// * `409` — `not_loaded`, `store_unreadable`, `unsupported_for_kind`,
///   `orphaned`, `not_orphaned`: a refusal that took **no** transition.
/// * `500` — the step-W write failed, so nothing changed and the row still
///   reads what the disk says.
///
/// There is **no `503`**: `AppState.extensions` is non-optional.
pub(crate) fn extension_error_status(error: &ExtensionError) -> StatusCode {
    match error {
        ExtensionError::NotFound(_) => StatusCode::NOT_FOUND,
        ExtensionError::NotLoaded
        | ExtensionError::StoreUnreadable(_)
        | ExtensionError::UnsupportedForKind
        | ExtensionError::Orphaned
        | ExtensionError::NotOrphaned => StatusCode::CONFLICT,
        ExtensionError::WriteFailed(_) | ExtensionError::Internal(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// `{"error":"<word>"}` — the flat envelope of design §8. The word is the
/// error's own `Display`, so `not_loaded`, `store_unreadable`,
/// `unsupported_for_kind`, `orphaned` and `not_orphaned` are verbatim.
pub(crate) fn extension_error(error: &ExtensionError) -> Response {
    let mut body = serde_json::json!({ "error": error.to_string() });
    // `not_orphaned` is the one refusal that needs more than its word: a plugin
    // whose directory is present but no longer declares a `plugin.toml` is not
    // re-scanned into `Orphaned` until the next daemon start (C3 review), and
    // the caller has no way to know that from `not_orphaned` alone.
    if let ExtensionError::NotOrphaned = error
        && let Some(object) = body.as_object_mut()
    {
        object.insert(
            "message".to_string(),
            serde_json::json!(
                "only an orphaned row can be removed; a plugin whose directory is \
                 present but no longer declares a plugin.toml becomes orphaned at \
                 the next daemon start"
            ),
        );
    }
    (extension_error_status(error), Json(body)).into_response()
}

/// The status a failed plugin **config write** answers with. Same split as the
/// legacy route: a write the daemon could not perform is `500`, an unreadable
/// store is `409`, a caller mistake is `400`.
pub(crate) fn plugin_error_status(error: &PluginError) -> StatusCode {
    match error {
        PluginError::Io(_) | PluginError::Json(_) | PluginError::StoreWriteFailed(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        PluginError::StoreUnreadable(_) => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    }
}

// ── The row (design §8) ──────────────────────────────────────────

/// One `GET /v1/extensions` row, rendered from ledger state and the supervisor
/// data attached to it — never from an event payload (X-18).
pub(crate) fn row_json(record: &ExtensionRecord) -> serde_json::Value {
    let (reason, detail, added_capabilities) = match &record.state {
        ExtensionState::Failed { reason, detail, .. } => (
            Some(reason.word()),
            Some(detail.clone()),
            Vec::<String>::new(),
        ),
        ExtensionState::Unapproved { reason } => (
            Some(reason.word()),
            None,
            match reason {
                UnapprovedReason::CapabilitiesGrew { added } => added.clone(),
                _ => Vec::new(),
            },
        ),
        _ => (None, None, Vec::new()),
    };

    let mut row = serde_json::json!({
        "kind": record.id.kind.as_str(),
        "id": record.id.name,
        "version": record.version,
        "transport": record.transport,
        // PERSISTED DISPOSITION — `null` on the two rows whose bit nobody can
        // read (design §4, §8).
        "enabled": record.disposition_readable.then_some(record.disposition.0),
        "consent": record.consent.map(|c| c.word()),
        "state": record.state.word(),
        "reason": reason,
        "actionable": record.state.actionable(),
        "detail": detail,
        "hint": record.hint,
        "missing_config_keys": record.missing_config_keys,
        "added_capabilities": added_capabilities,
        // The **live** subset, not the retained set: a row must not advertise
        // names the gate refuses.
        "tools": record.live_tools(),
        "skipped_tools": record.skipped_tools,
        "withdrawn_by_server": record.withdrawn_by_server,
        "tools_changed_at": record.tools_changed_at.map(|t| t.to_rfc3339()),
        "declared": record.declared.as_ref().map(|d| serde_json::json!({
            "capabilities": d.capabilities,
            "virtual_capabilities": d.virtual_capabilities,
            "types": d.types,
        })),
        "skills": record.skills,
        "agents": record.agents,
        "connector": record.connector,
        "provider": record.provider,
        "since": record.since.to_rfc3339(),
    });

    // `warnings` is per-call, not row state: only the verb that produced one
    // carries it ("torn down with N call(s) in flight", "teardown pending: …").
    if !record.warnings.is_empty()
        && let Some(object) = row.as_object_mut()
    {
        object.insert("warnings".to_string(), serde_json::json!(record.warnings));
    }
    row
}

fn row_response(result: Result<ExtensionRecord, ExtensionError>) -> Response {
    match result {
        Ok(record) => (StatusCode::OK, Json(row_json(&record))).into_response(),
        Err(e) => extension_error(&e),
    }
}

// ── R18: a verb outlives its request ─────────────────────────────

/// Run a transition in a detached task and await its `JoinHandle`.
///
/// A dropped request future cancels the handler, not the task: the transition
/// runs to a terminal state either way. Without this an axum-dropped `enable`
/// leaves the record in `Enabling` for good — nothing else ever CASes it out —
/// and every call to that extension is refused as *"being turned on"* until the
/// daemon restarts.
pub(crate) async fn detached<F, T>(future: F) -> Result<T, ExtensionError>
where
    F: std::future::Future<Output = Result<T, ExtensionError>> + Send + 'static,
    T: Send + 'static,
{
    match tokio::spawn(future).await {
        Ok(result) => result,
        Err(join) => Err(ExtensionError::Internal(format!(
            "the extension verb did not complete: {join}"
        ))),
    }
}

/// A `{kind}` that is neither `mcp` nor `plugin` names no resource, so it is a
/// `404` — and the body says *kind*, not "unknown extension", which would send
/// the caller looking for a missing server.
fn unknown_kind(kind: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": format!("unknown extension kind '{kind}' (expected 'mcp' or 'plugin')")
        })),
    )
        .into_response()
}

/// The five row-returning verbs, as one word each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    Enable,
    Disable,
    Reload,
    Approve,
    Deny,
}

impl Verb {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "enable" => Some(Self::Enable),
            "disable" => Some(Self::Disable),
            "reload" => Some(Self::Reload),
            "approve" => Some(Self::Approve),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// Resolve `{kind}/{id}`, then run `verb` detached. The whole of every
/// `POST /v1/extensions/...` handler.
pub(crate) async fn run_verb(
    extensions: Arc<Extensions>,
    kind: &str,
    id: &str,
    verb: Verb,
) -> Response {
    let Some(kind) = Extensions::parse_kind(kind) else {
        return unknown_kind(kind);
    };
    let ext = ExtensionId {
        kind,
        name: id.to_string(),
    };
    row_response(
        detached(async move {
            match verb {
                Verb::Enable => extensions.enable(&ext).await,
                Verb::Disable => extensions.disable(&ext).await,
                Verb::Reload => extensions.reload(&ext).await,
                Verb::Approve => extensions.approve(&ext).await,
                Verb::Deny => extensions.deny(&ext).await,
            }
        })
        .await,
    )
}

// ── Handlers ─────────────────────────────────────────────────────

/// `GET /v1/extensions`
pub async fn list_extensions_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> Response {
    let rows = state.extensions.list(query.include_orphaned).await;
    let body: Vec<serde_json::Value> = rows.iter().map(row_json).collect();
    (StatusCode::OK, Json(body)).into_response()
}

/// `POST /v1/extensions/{kind}/{id}/{verb}`
pub async fn extension_action_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id, verb)): Path<(String, String, String)>,
) -> Response {
    let Some(verb) = Verb::parse(&verb) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("unknown extension verb '{verb}'") })),
        )
            .into_response();
    };
    run_verb(state.extensions.clone(), &kind, &id, verb).await
}

/// `DELETE /v1/extensions/{kind}/{id}[?uninstall=true[&keep_data=false]]`
pub async fn delete_extension_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
    Query(query): Query<DeleteQuery>,
) -> Response {
    delete_extension(state.extensions.clone(), &kind, &id, query).await
}

/// The two deletes, told apart by one query flag.
///
/// **Without `?uninstall=true`** this is C6's verb, unchanged: it removes an
/// **orphan's** `.permissions.toml` entry and its ledger record, and never
/// touches a directory (`409 not_orphaned` on anything else).
///
/// **With it** this is GAP-24's uninstall: T0–T5, the entry, the directory to
/// `plugins/.trash/`, the tombstones expired — or, for an MCP server, the
/// `[servers.<name>]` block, which requires the server to be `Disabled` first.
///
/// One path with a flag rather than two paths: the flag is what makes the
/// dangerous half impossible to reach by accident, and it is the shape
/// `API_MAP.md` proposed.
pub(crate) async fn delete_extension(
    extensions: Arc<Extensions>,
    kind: &str,
    id: &str,
    query: DeleteQuery,
) -> Response {
    let Some(kind) = Extensions::parse_kind(kind) else {
        return unknown_kind(kind);
    };
    let ext = ExtensionId {
        kind,
        name: id.to_string(),
    };

    if !query.uninstall {
        let removed = ext.name.clone();
        let extensions = extensions.clone();
        return match detached(async move { extensions.remove(&ext).await }).await {
            Ok(()) => (
                StatusCode::OK,
                Json(serde_json::json!({ "removed": removed })),
            )
                .into_response(),
            Err(e) => extension_error(&e),
        };
    }

    let keep_data = query.keep_data;
    match detached(async move { Ok(extensions.uninstall(&ext, keep_data).await) }).await {
        Ok(Ok(removed)) => (StatusCode::OK, Json(uninstalled_body(&removed))).into_response(),
        Ok(Err(e)) => gap24_error(&e),
        Err(e) => extension_error(&e),
    }
}

/// What an uninstall removed, and — for a plugin — where it went. Nothing was
/// deleted: both paths are moves into `plugins/.trash/`, so the body names them.
fn uninstalled_body(removed: &Uninstalled) -> serde_json::Value {
    match removed {
        Uninstalled::Plugin(outcome) => serde_json::json!({
            "removed": outcome.removed,
            "trashed": outcome.trashed.as_ref().map(|p| p.display().to_string()),
            "kept_data": outcome.kept_data,
            "data_trashed": outcome.data_trashed.as_ref().map(|p| p.display().to_string()),
        }),
        Uninstalled::Mcp { removed } => serde_json::json!({
            "removed": removed,
            "trashed": serde_json::Value::Null,
            "kept_data": true,
            "data_trashed": serde_json::Value::Null,
        }),
    }
}

// ── GAP-24: install, validate, update ────────────────────────────

/// The GAP-24 status codes. Every one of them is decided here; the supervisors
/// answer with a fact and a word (design §8).
///
/// * `400` — the caller's mistake: a relative path, a symlink out of the tree,
///   a source that is not `path`, a declaration that is not a declaration.
/// * `404` — the source directory is not there, or the extension is not known.
/// * `422` — the source *is* there but its `plugin.toml` cannot make a plugin,
///   or an MCP declaration hands the daemon a literal secret to write down
///   (`secret_literal_refused`, R65). Distinct from `400`, because retrying the
///   request will not help: the directory, or the shape of the declaration, is
///   what has to change. (No apostrophe in that sentence on purpose — the
///   doc-comment scanner in `scripts/gen_api_docs.py` reads one as the start of
///   a char literal and loses the next type it would have qualified.)
/// * `409` — a name that is taken, a transition in flight, an MCP server that
///   is still running.
/// * `500` — the copy or the write failed.
pub(crate) fn gap24_error(failure: &InstallFailure) -> Response {
    let status = match failure {
        InstallFailure::Plugin(e) => match e {
            InstallError::InvalidPath(_) | InstallError::EscapingSymlink(_) => {
                StatusCode::BAD_REQUEST
            }
            InstallError::SourceNotFound(_) => StatusCode::NOT_FOUND,
            InstallError::InvalidManifest(_) => StatusCode::UNPROCESSABLE_ENTITY,
            InstallError::AlreadyInstalled(_) | InstallError::Busy(_) => StatusCode::CONFLICT,
            InstallError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            InstallError::Extension(e) => extension_error_status(e),
        },
        InstallFailure::Mcp(e) => match e {
            DeclarationError::Invalid(_) => StatusCode::BAD_REQUEST,
            DeclarationError::SecretLiteral(_) => StatusCode::UNPROCESSABLE_ENTITY,
            DeclarationError::AlreadyDeclared(_) | DeclarationError::NotDisabled(_) => {
                StatusCode::CONFLICT
            }
            DeclarationError::Extension(e) => extension_error_status(e),
        },
    };
    (
        status,
        Json(serde_json::json!({
            "error": failure.code(),
            // The word is what a client branches on; the sentence is what a
            // person reads. Both, because a refused install is something the
            // owner has to act on ("the manifest calls this plugin 'x'…").
            "message": failure.to_string(),
        })),
    )
        .into_response()
}

fn bad_request(code: &'static str, message: impl Into<String>) -> Response {
    SourceRefusal {
        code,
        message: message.into(),
    }
    .into_response()
}

/// A `400` the request body earned before any supervisor saw it.
///
/// Carried as its two fields rather than as a built `Response`, which is 128
/// bytes and would make every `Result<PathBuf, _>` on this path an oversized
/// error type.
struct SourceRefusal {
    code: &'static str,
    message: String,
}

impl IntoResponse for SourceRefusal {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": self.code, "message": self.message })),
        )
            .into_response()
    }
}

/// `{source: "path", path: "/…"}` — the **only** source (plan §8 item 9).
///
/// `source: "url"` stays declined: fetching and unpacking an archive from the
/// network is its own security review, and refusing it by name is more useful
/// than a serde error about an unknown variant.
fn plugin_source(body: &serde_json::Value) -> Result<std::path::PathBuf, SourceRefusal> {
    match body.get("source").and_then(|s| s.as_str()) {
        Some("path") | None => match body.get("path").and_then(|p| p.as_str()) {
            Some(path) if !path.is_empty() => Ok(std::path::PathBuf::from(path)),
            _ => Err(SourceRefusal {
                code: "invalid_path",
                message: "give the plugin directory as an absolute 'path'".to_string(),
            }),
        },
        Some(other) => Err(SourceRefusal {
            code: "unsupported_source",
            message: format!(
                "source '{other}' is not supported; only 'path' is — installing from a \
                 URL is its own security review"
            ),
        }),
    }
}

fn install_body(outcome: &InstallOutcome) -> serde_json::Value {
    serde_json::json!({
        "extension": row_json(&outcome.record),
        "manifest": outcome.manifest,
        "added_capabilities": outcome.added_capabilities,
        "consent_reset": outcome.consent_reset,
    })
}

/// `POST /v1/extensions/{kind}` — install a plugin, or declare an MCP server.
///
/// Both answer `201` with the same envelope: the ledger row, plus the manifest
/// summary a plugin needs beside it. **An install grants nothing** — the plugin
/// lands `unapproved`/`never_seen` and approving is the single action that
/// starts it — so the summary is the approval preview, and `manifest` is `null`
/// for an MCP server, which has no consent gate at all (writing a server into
/// your own `config/mcp.toml` *is* the consent).
pub(crate) async fn install_extension(
    extensions: Arc<Extensions>,
    kind: &str,
    body: serde_json::Value,
) -> Response {
    match kind {
        "plugin" => {
            let path = match plugin_source(&body) {
                Ok(path) => path,
                Err(refusal) => return refusal.into_response(),
            };
            match detached(async move { Ok(extensions.install_plugin(&path).await) }).await {
                Ok(Ok(outcome)) => {
                    (StatusCode::CREATED, Json(install_body(&outcome))).into_response()
                }
                Ok(Err(e)) => gap24_error(&e),
                Err(e) => extension_error(&e),
            }
        }
        "mcp" => {
            let declaration: McpDeclaration = match serde_json::from_value(body) {
                Ok(declaration) => declaration,
                Err(e) => return bad_request("invalid_declaration", e.to_string()),
            };
            match detached(async move { Ok(extensions.add_mcp(declaration).await) }).await {
                Ok(Ok(record)) => (
                    StatusCode::CREATED,
                    Json(serde_json::json!({
                        "extension": row_json(&record),
                        "manifest": serde_json::Value::Null,
                    })),
                )
                    .into_response(),
                Ok(Err(e)) => gap24_error(&e),
                Err(e) => extension_error(&e),
            }
        }
        other => unknown_kind(other),
    }
}

/// `POST /v1/extensions/{kind}` handler.
pub async fn install_extension_handler(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    install_extension(state.extensions.clone(), &kind, body).await
}

/// `POST /v1/extensions/plugin/validate {path}` — the dry run.
///
/// It parses the manifest and reports it without copying anything, so the
/// owner can see what an install would land — and what approving it would grant
/// — before the directory is in the store at all.
pub(crate) async fn validate_plugin(
    extensions: Arc<Extensions>,
    body: serde_json::Value,
) -> Response {
    let path = match plugin_source(&body) {
        Ok(path) => path,
        Err(refusal) => return refusal.into_response(),
    };
    match extensions.validate_plugin(&path) {
        Ok(manifest) => {
            let installed = extensions.plugin_installed(&manifest.name);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "manifest": manifest, "installed": installed })),
            )
                .into_response()
        }
        Err(e) => gap24_error(&e),
    }
}

/// `POST /v1/extensions/plugin/validate` handler.
pub async fn validate_plugin_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    validate_plugin(state.extensions.clone(), body).await
}

/// `PUT /v1/extensions/{kind}/{id} {source:"path", path}` — replace a plugin's
/// tree in place.
///
/// Plugins only. An MCP server's declaration is a block in the owner's own
/// `config/mcp.toml`: editing it is a text edit plus `reload`, so there is
/// nothing here to update and `kind=mcp` is `409 unsupported_for_kind` — the
/// same answer the rest of the plugin-only family gives.
pub(crate) async fn update_extension(
    extensions: Arc<Extensions>,
    kind: &str,
    id: &str,
    body: serde_json::Value,
) -> Response {
    match Extensions::parse_kind(kind) {
        Some(ExtensionKind::Plugin) => {}
        Some(ExtensionKind::Mcp) => {
            return extension_error(&ExtensionError::UnsupportedForKind);
        }
        None => return unknown_kind(kind),
    }
    let path = match plugin_source(&body) {
        Ok(path) => path,
        Err(refusal) => return refusal.into_response(),
    };
    let id = id.to_string();
    match detached(async move { Ok(extensions.update_plugin(&id, &path).await) }).await {
        Ok(Ok(outcome)) => (StatusCode::OK, Json(install_body(&outcome))).into_response(),
        Ok(Err(e)) => gap24_error(&e),
        Err(e) => extension_error(&e),
    }
}

/// `PUT /v1/extensions/{kind}/{id}` handler.
pub async fn update_extension_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    update_extension(state.extensions.clone(), &kind, &id, body).await
}

/// The config pair's two guards, in the order the rest of the family checks
/// them.
///
/// * `{kind}` — an unknown word is the same `404` `run_verb` answers, and
///   `mcp` is `409 unsupported_for_kind`: an MCP server's configuration is its
///   own block in `config/mcp.toml`, which the daemon does not edit key by key.
/// * `{id}` — a plugin the daemon has never heard of is a `404` on **both**
///   verbs. Without it a `GET` on a typo answers `200 {}` (an empty config is
///   indistinguishable from a missing plugin) and a `POST` writes
///   `.config/<typo>.toml` for a plugin that does not exist.
async fn require_config_target(
    extensions: &Extensions,
    kind: &str,
    id: &str,
) -> Result<(), Response> {
    let Some(kind) = Extensions::parse_kind(kind) else {
        return Err(unknown_kind(kind));
    };
    let ext = ExtensionId {
        kind,
        name: id.to_string(),
    };
    extensions
        .known_plugin(&ext)
        .await
        .map_err(|e| extension_error(&e))
}

/// `GET /v1/extensions/plugin/{id}/config` — the redacting read (design §8).
pub(crate) async fn get_config(extensions: Arc<Extensions>, kind: &str, id: &str) -> Response {
    if let Err(refusal) = require_config_target(&extensions, kind, id).await {
        return refusal;
    }
    let config = extensions.plugins().plugin_config_redacted(id).await;
    (StatusCode::OK, Json(config_json(&config))).into_response()
}

/// `GET /v1/extensions/{kind}/{id}/config`
pub async fn get_extension_config_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
) -> Response {
    get_config(state.extensions.clone(), &kind, &id).await
}

/// `POST /v1/extensions/plugin/{id}/config`
pub(crate) async fn set_config(
    extensions: Arc<Extensions>,
    kind: &str,
    id: &str,
    request: SetConfigRequest,
) -> Response {
    if let Err(refusal) = require_config_target(&extensions, kind, id).await {
        return refusal;
    }
    let id = id.to_string();
    let value = json_to_toml(&request.value);
    let key = request.key.clone();
    let name = id.clone();
    // R18 again: the write is followed by the `enable` verb when the row was
    // parked on the key that has just arrived, so this is a transition too.
    let joined = tokio::spawn(async move {
        extensions
            .plugins()
            .set_plugin_config(&name, &key, value)
            .await
    })
    .await;

    match joined {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ok", "name": id, "key": request.key })),
        )
            .into_response(),
        Ok(Err(e)) => (
            plugin_error_status(&e),
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
        Err(join) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("the config write did not complete: {join}")
            })),
        )
            .into_response(),
    }
}

/// `POST /v1/extensions/{kind}/{id}/config`
pub async fn set_extension_config_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
    Json(request): Json<SetConfigRequest>,
) -> Response {
    set_config(state.extensions.clone(), &kind, &id, request).await
}

// ── TOML ⇄ JSON ──────────────────────────────────────────────────

fn config_json(config: &HashMap<String, toml::Value>) -> serde_json::Value {
    let mut map = serde_json::Map::with_capacity(config.len());
    for (key, value) in config {
        map.insert(key.clone(), toml_to_json(value));
    }
    serde_json::Value::Object(map)
}

pub(crate) fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => serde_json::Value::Object(
            table
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

pub(crate) fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Array(arr) => {
            toml::Value::Array(arr.iter().map(json_to_toml).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut map = toml::map::Map::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_toml(v));
            }
            toml::Value::Table(map)
        }
        serde_json::Value::Null => toml::Value::String(String::new()),
    }
}

#[cfg(test)]
mod tests;
