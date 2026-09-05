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
    ExtensionError, ExtensionId, ExtensionRecord, ExtensionState, UnapprovedReason,
};
use openalpaca_plugins::PluginError;
use serde::Deserialize;

use crate::AppState;
use crate::managers::extensions::Extensions;

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

/// `DELETE /v1/extensions/plugin/{id}`
pub async fn delete_extension_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
) -> Response {
    let Some(kind) = Extensions::parse_kind(&kind) else {
        return unknown_kind(&kind);
    };
    let ext = ExtensionId {
        kind,
        name: id.clone(),
    };
    let extensions = state.extensions.clone();
    match detached(async move { extensions.remove(&ext).await }).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "removed": id })),
        )
            .into_response(),
        Err(e) => extension_error(&e),
    }
}

/// `GET /v1/extensions/plugin/{id}/config` — the redacting read (design §8).
pub async fn get_extension_config_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
) -> Response {
    if kind != "plugin" {
        return extension_error(&ExtensionError::UnsupportedForKind);
    }
    let config = state.extensions.plugins().plugin_config_redacted(&id);
    (StatusCode::OK, Json(config_json(&config))).into_response()
}

/// `POST /v1/extensions/plugin/{id}/config`
pub async fn set_extension_config_handler(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
    Json(request): Json<SetConfigRequest>,
) -> Response {
    if kind != "plugin" {
        return extension_error(&ExtensionError::UnsupportedForKind);
    }
    let value = json_to_toml(&request.value);
    let extensions = state.extensions.clone();
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

/// Shared with the legacy `POST /v1/plugins/{name}/config` until C7 deletes it.
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
