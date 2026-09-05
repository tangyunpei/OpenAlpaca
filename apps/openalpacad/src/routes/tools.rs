//! `GET /v1/tools` — the tool catalog (GAP-18, respecified by extension design
//! §8).
//!
//! Read-only. There is **no `PUT` and no per-tool toggle (S1)**: availability is
//! *derived* — (the agent's capabilities) ∩ (its extension being enabled) —
//! never asserted per tool. A builtin row carries **no enable field at all**;
//! `origin` is the one place an enable state appears, and it is `null` for
//! builtins and for `config/tools/*.toml` tools.
//!
//! This supersedes `ToolCatalogEntry.denied: boolean` and folds that
//! interface's `provider: string | null` into `origin.id` (C7 deletes both on
//! the frontend side). `global_tool_deny` is not the source of anything here.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::{IntoResponse, Response}};
use chrono::{Local, TimeZone, Utc};
use openalpaca_core::tools::extensions::ExtensionLedger;
use openalpaca_core::tools::registry::ToolBackend;
use openalpaca_core::tools::ToolRegistry;

use crate::AppState;

/// `GET /v1/tools`
pub async fn list_tools_handler(State(state): State<Arc<AppState>>) -> Response {
    let counts = invocations_today(&state.db);
    let body = tools_json(&state.tool_registry, state.tool_registry.extensions(), &counts);
    (StatusCode::OK, Json(body)).into_response()
}

/// Today's per-tool call counts, keyed by tool name.
///
/// **Local midnight, converted to UTC.** `tool_execution_log.timestamp`
/// defaults to `datetime('now')` — UTC text (migration 030) — so a bare
/// `date('now')` predicate would be off by the daemon's UTC offset. The count
/// also lags a call by one bus hop: the sandbox publishes
/// `SystemEvent::ToolExecuted` and the daemon's event persistence writes the
/// row.
///
/// `.earliest()`, not `.single()`: in a zone whose DST transition lands at
/// 00:00 the local midnight is ambiguous or does not exist at all (Cuba, Chile,
/// Lord Howe and others), and `.single()` returns `None` there — which would
/// report `invocations_today: 0` for every tool for that whole day. The
/// earliest candidate is at worst an hour early, which is a count that starts
/// slightly too soon rather than no data.
fn invocations_today(db: &openalpaca_storage::Database) -> HashMap<String, i64> {
    let Some(local_midnight) = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).earliest())
    else {
        tracing::warn!("could not resolve local midnight; reporting no invocations today");
        return HashMap::new();
    };
    let since = local_midnight
        .with_timezone(&Utc)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    match openalpaca_storage::repository::SkillExecutionRepository::new(db)
        .tool_invocations_since(&since)
    {
        Ok(counts) => counts,
        Err(e) => {
            tracing::warn!(error = %e, "could not read today's tool invocation counts");
            HashMap::new()
        }
    }
}

/// The §8 array, sorted by name — `DashMap` iteration jitters, and a catalog
/// that reorders between two reads is unusable in a diff.
pub(crate) fn tools_json(
    registry: &ToolRegistry,
    ledger: &ExtensionLedger,
    counts: &HashMap<String, i64>,
) -> Vec<serde_json::Value> {
    let mut rows: Vec<(String, serde_json::Value)> = registry
        .iter_registered_tools()
        .map(|(name, tool)| {
            let source = match &tool.backend {
                ToolBackend::BuiltIn(_) => "builtin",
                ToolBackend::Mcp { .. } => "mcp",
                ToolBackend::Plugin(_) => "plugin",
                // `config/tools/*.toml` declares exactly these two backends.
                ToolBackend::Http { .. } | ToolBackend::Command { .. } => "config",
            };
            // `null` for builtins and config tools — they are never on the
            // ENABLE axis, so they carry no enable field at all.
            let origin = tool.extension_id().map(|ext| {
                let record = ledger.record(&ext);
                serde_json::json!({
                    "kind": ext.kind.as_str(),
                    "id": ext.name,
                    // An extension with no ledger entry reads as enabled — the
                    // §6.2a fail-open default, stated the same way here.
                    "enabled": record.as_ref().map(|r| r.disposition.0).unwrap_or(true),
                    "state": record
                        .as_ref()
                        .map(|r| r.state.word())
                        .unwrap_or("enabled"),
                })
            });
            let row = serde_json::json!({
                "name": name,
                "description": tool.definition.description,
                "source": source,
                "origin": origin,
                "provides_capabilities": tool.provides_capabilities,
                "requires_confirmation": tool
                    .annotations
                    .as_ref()
                    .and_then(|a| a.destructive_hint)
                    .unwrap_or(false),
                "invocations_today": counts.get(&name).copied().unwrap_or(0),
                "version": tool.version,
                "author": tool.author,
            });
            (name, row)
        })
        .collect();

    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

#[cfg(test)]
mod tests;
