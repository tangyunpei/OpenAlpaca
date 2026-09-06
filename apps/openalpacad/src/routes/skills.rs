//! Skill routes: `GET /v1/skills` (the catalog) and `GET /v1/skills/health`
//! (the metrics).
//!
//! The catalog is GAP-18's remaining half — the listing that lets a client name
//! a skill instead of printing the id `skill_execution_log` happens to key on.
//! It is the `/v1/tools` shape one axis over, and follows the same two rules:
//!
//!  * **Read-only.** There is no per-skill enable state, and no route that
//!    would accept one. A skill runs when the agent's capabilities allow every
//!    entry in its `requires_capabilities` and — for a plugin skill — the
//!    plugin serving it is enabled. That is derived, never asserted (S1).
//!  * **`origin` is the one place an enable state appears**, and it is `null`
//!    for a file skill, which is on no ENABLE axis at all. A file-skill row
//!    carries no enable field of any kind, exactly as a builtin tool row does.
//!
//! The catalog is served whole. A disabled plugin's skills are already gone
//! from it — T2 removes them and leaves a tombstone (design §10 case 5(a)) — so
//! there is nothing to filter; adding an availability filter here would be a
//! second enforcement point rather than a rendering, and the row's `origin`
//! already says what the extension's state is.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::{IntoResponse, Response}};
use openalpaca_core::orchestrator::skill_catalog::{SkillCatalog, SkillSource};
use openalpaca_core::tools::extensions::{ExtensionId, ExtensionLedger};
use openalpaca_storage::SkillExecutionRepository;

use crate::AppState;

/// `GET /v1/skills` — bare array (plan §7: unbounded lists are bare arrays).
pub async fn list_skills_handler(State(state): State<Arc<AppState>>) -> Response {
    let counts = invocations_today(&state.db);
    let body = skills_json(
        &state.orchestrator.skill_catalog,
        state.tool_registry.extensions(),
        &counts,
    );
    (StatusCode::OK, Json(body)).into_response()
}

/// Today's per-skill invocation counts, keyed by skill id.
///
/// The tool catalog's `invocations_today`, one table over — same instant, same
/// conversion, same reason. `skill_execution_log.timestamp` defaults to
/// `datetime('now')`, which is **UTC** text (migration 030), so today's *local*
/// midnight is converted to UTC before it becomes a predicate; a bare
/// `date('now')` would be off by the daemon's UTC offset.
///
/// `.earliest()`, not `.single()`: in a zone whose DST transition lands at
/// 00:00 the local midnight is ambiguous or does not exist, and `.single()`
/// returns `None` there — which would report `invocations_today: 0` for every
/// skill for that whole day.
fn invocations_today(db: &openalpaca_storage::Database) -> HashMap<String, i64> {
    use chrono::{Local, TimeZone, Utc};

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

    match SkillExecutionRepository::new(db).skill_invocations_since(&since) {
        Ok(counts) => counts,
        Err(e) => {
            tracing::warn!(error = %e, "could not read today's skill invocation counts");
            HashMap::new()
        }
    }
}

/// Today's counts, re-keyed from what the **log** holds onto the catalog id.
///
/// `skill_execution_log.skill_id` is not the catalog id. Every invocation path
/// resolves the entry and then passes `entry.frontmatter.name` on as the id it
/// logs: `/slash` and router selection build `Intent::SkillInvocation` from it
/// (`intent/skill_match.rs:31,55` → `skill/invocation.rs:140`), and the model's
/// `invoke_skill` does the same (`builtins/invoke_skill.rs:173`). Older rows,
/// and the cron arm, can carry the id instead.
///
/// So a logged key is resolved the way `SkillCatalog::get` resolves one —
/// lowercased, **id first, then frontmatter name** — and both spellings of one
/// skill add up. A key no entry claims is counted for nobody rather than
/// attached to a row it does not belong to.
fn counts_by_skill_id(
    entries: &[(String, openalpaca_core::orchestrator::skill_catalog::SkillEntry)],
    counts: &HashMap<String, i64>,
) -> HashMap<String, i64> {
    let mut key_to_id: HashMap<String, &str> = HashMap::new();
    for (id, entry) in entries {
        key_to_id.insert(entry.frontmatter.name.to_lowercase(), id.as_str());
    }
    // Ids win: `get` tries the id map before it scans names.
    for (id, _) in entries {
        key_to_id.insert(id.to_lowercase(), id.as_str());
    }

    let mut by_id: HashMap<String, i64> = HashMap::new();
    for (logged, count) in counts {
        if let Some(id) = key_to_id.get(&logged.to_lowercase()) {
            *by_id.entry((*id).to_string()).or_insert(0) += count;
        }
    }
    by_id
}

/// The catalog array, sorted by id — the catalog is a `HashMap`, and a listing
/// that reorders between two reads is unusable in a diff.
pub(crate) fn skills_json(
    catalog: &SkillCatalog,
    ledger: &ExtensionLedger,
    counts: &HashMap<String, i64>,
) -> Vec<serde_json::Value> {
    let entries = catalog.entries_snapshot();
    let counts = counts_by_skill_id(&entries, counts);

    let mut rows: Vec<(String, serde_json::Value)> = entries
        .into_iter()
        .map(|(id, entry)| {
            let front = &entry.frontmatter;
            let (source, origin, author) = match &entry.source {
                SkillSource::FileBased => {
                    // No extension, so no enable field at all. `author` names
                    // the provenance the same way a tool row's does — the
                    // source, then the producer — and for a file skill the
                    // producer is the scope it was discovered in.
                    let scope = match entry.scope {
                        openalpaca_core::middleware::skill::SkillScope::Project => "project",
                        openalpaca_core::middleware::skill::SkillScope::User => "user",
                    };
                    ("file", serde_json::Value::Null, format!("file:{scope}"))
                }
                SkillSource::Plugin { plugin_id, .. } => {
                    let ext = ExtensionId::plugin(plugin_id.clone());
                    let record = ledger.record(&ext);
                    let origin = serde_json::json!({
                        "kind": ext.kind.as_str(),
                        "id": ext.name,
                        // An extension with no ledger entry reads as enabled —
                        // the §6.2a fail-open default, stated the same way the
                        // tool catalog states it.
                        "enabled": record.as_ref().map(|r| r.disposition.0).unwrap_or(true),
                        "state": record.as_ref().map(|r| r.state.word()).unwrap_or("enabled"),
                    });
                    ("plugin", origin, format!("plugin:{plugin_id}"))
                }
            };
            let row = serde_json::json!({
                "id": id,
                "name": front.name,
                "description": front.description,
                "source": source,
                "origin": origin,
                "requires_capabilities": front.requires_capabilities,
                "triggers": {
                    // Without the leading '/', the way the command index keys
                    // it and the way a client would print it.
                    "slash": front.effective_slash_command(),
                    "keywords": front.routing.keywords,
                },
                "schedule": front.invoke.cron,
                "invocations_today": counts.get(&id).copied().unwrap_or(0),
                "version": front.version,
                "author": author,
            });
            (id, row)
        })
        .collect();

    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// `GET /v1/skills/health` — per-skill lifetime metrics keyed by `skill_id`.
pub async fn skill_health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let repo = SkillExecutionRepository::new(&state.db);
    match repo.all_skill_health() {
        Ok(metrics) => Json(metrics).into_response(),
        Err(e) => {
            tracing::warn!("Failed to query skill health: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query skill health: {e}"),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests;
