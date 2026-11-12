//! Agent management endpoints
//!
//! GET  /v1/agents            -> list agents (query: status, skill, limit)
//! GET  /v1/agents/{id}       -> get agent config + metrics
//! POST /v1/agents/{id}/action -> perform action (pause, resume)
//! GET  /v1/agents/{id}/config -> get agent config + version
//! PUT  /v1/agents/{id}/config -> update agent config (optimistic locking)
//! POST /v1/agents             -> create agent
//! POST /v1/agents/from-toml   -> create agent from raw TOML
//! DELETE /v1/agents/{id}      -> delete (archive) agent
//!
//! Template endpoints:
//! GET    /v1/agent-templates              -> list all templates (query: window)
//! GET    /v1/agent-templates/{id}         -> get template (JSON)
//! POST   /v1/agent-templates              -> create template (JSON body)
//! PUT    /v1/agent-templates/{id}         -> update template
//! DELETE /v1/agent-templates/{id}         -> archive template (409 for a
//!                                             bundled or a plugin's one)
//!
//! Instance endpoints:
//! GET    /v1/agent-instances              -> list active instances

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;

use openalpaca_core::agent::config_service::DeleteTemplateError;
use openalpaca_core::agent::file_layer::{LoadMode, rescan_file_layer};
use openalpaca_core::agent::{AgentConfigFile, AgentConfigService, AgentRegistry, AgentTemplate};
use openalpaca_core::events::SystemEvent;
use openalpaca_core::tools::ToolRegistry;
use openalpaca_storage::{Database, SubAgentConfig, SubAgentRepository, SubagentSpanRepository};

use super::agents_types::*;
use super::api_error;
use crate::AppState;

/// `?window=` on `GET /v1/agent-templates` (GAP-20, T48). Resolves to the
/// label the client echoes back on every row and the UTC cutoff the grouped
/// span query filters on; `all` has none. Anything else is the caller's
/// mistake — reported back as the token itself, so the 400 names exactly what
/// it did not recognise rather than guessing a default.
///
/// Default is `7d`, per the plan: an empty query string behaves the same as
/// asking for it explicitly.
fn resolve_window(
    raw: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<DateTime<Utc>>), String> {
    match raw.unwrap_or("7d") {
        "7d" => Ok(("7d", Some(now - Duration::days(7)))),
        "30d" => Ok(("30d", Some(now - Duration::days(30)))),
        "all" => Ok(("all", None)),
        other => Err(other.to_string()),
    }
}

/// The three template handlers' one path (subagent design §2.10 T-1's
/// catalog half): `AgentConfigService` writes or archives the file, then
/// the file layer is rescanned, so the change is in the catalog — and in
/// the singleton lead's idle instance (L-1a) — when the call returns. The
/// watcher's event for the same write rescans again and changes nothing.
struct TemplateRoute<'a> {
    service: &'a AgentConfigService,
    registry: &'a AgentRegistry,
    tool_registry: &'a ToolRegistry,
    db: &'a Database,
}

impl<'a> TemplateRoute<'a> {
    fn of(state: &'a AppState) -> Self {
        Self {
            service: &state.agent_config_service,
            registry: &state.gateway.shared_context.agent_registry,
            tool_registry: &state.tool_registry,
            db: &state.db,
        }
    }

    fn create(&self, config: AgentConfigFile) -> Result<String, String> {
        let created = self.service.create_template_from_toml_config(config);
        if created.is_ok() {
            self.rescan();
        }
        created
    }

    fn update(&self, id: &str, template: AgentTemplate) -> Result<(), String> {
        let updated = self.service.update_template(id, template);
        if updated.is_ok() {
            self.rescan();
        }
        updated
    }

    fn delete(&self, id: &str) -> Result<(), DeleteTemplateError> {
        let deleted = self.service.delete_template(id);
        if deleted.is_ok() {
            self.rescan();
        }
        deleted
    }

    /// Whether the file now holding `id` stands over a bundled template:
    /// what `POST` and `PUT` answer after their write (§2.10 T-1).
    fn overrides_bundled(&self, id: &str) -> bool {
        self.registry
            .provenance(id)
            .is_some_and(|p| p.overrides_bundled())
    }

    /// `LoadMode::Reload`: a written file naming an unknown `annotation:`
    /// capability is withheld at once (and still stops the next start).
    fn rescan(&self) {
        let agents_dir = self.service.agents_dir();
        if let Err(e) = rescan_file_layer(
            agents_dir,
            LoadMode::Reload,
            self.registry,
            self.tool_registry,
            self.db,
        ) {
            tracing::warn!("Agent templates rescan after a route's write failed: {e}");
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────

/// GET /v1/agents
pub async fn list_agents_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListAgentsQuery>,
) -> impl IntoResponse {
    let repo = SubAgentRepository::new(&state.db);
    let limit = query.limit.unwrap_or(50);

    // If filtering by skill/capability, use in-memory registry
    if let Some(ref skill) = query.skill {
        let agents = state
            .gateway
            .shared_context
            .agent_registry
            .find_by_capability(skill);
        let ids: Vec<String> = agents.iter().map(|a| a.id.clone()).collect();

        // Fetch full configs from DB for the matched IDs
        let all = match repo.list(limit) {
            Ok(configs) => configs,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                );
            }
        };

        let filtered: Vec<SubAgentConfig> =
            all.into_iter().filter(|c| ids.contains(&c.id)).collect();

        return (
            StatusCode::OK,
            Json(serde_json::to_value(filtered).unwrap()),
        );
    }

    let configs = if let Some(ref status) = query.status {
        repo.list_by_status(status, limit)
    } else {
        repo.list(limit)
    };

    match configs {
        Ok(configs) => (StatusCode::OK, Json(serde_json::to_value(configs).unwrap())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// GET /v1/agents/{id}
pub async fn get_agent_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let repo = SubAgentRepository::new(&state.db);

    match repo.get(&id) {
        Ok(Some(agent)) => {
            let metrics = repo.get_metrics(&id).unwrap_or(None);
            (
                StatusCode::OK,
                Json(serde_json::to_value(AgentResponse { agent, metrics }).unwrap()),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Agent not found" })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// POST /v1/agents/{id}/action
pub async fn agent_action_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<AgentActionRequest>,
) -> impl IntoResponse {
    let repo = SubAgentRepository::new(&state.db);

    // Fetch current agent
    let agent = match repo.get(&id) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Agent not found" })),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    };

    let new_status = match request.action.as_str() {
        "pause" => {
            if agent.status != "busy" && agent.status != "idle" {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("Cannot pause agent in '{}' state", agent.status)
                    })),
                );
            }
            "waiting"
        }
        "resume" => {
            if agent.status != "waiting" {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("Can only resume a waiting agent, current state: '{}'", agent.status)
                    })),
                );
            }
            "idle"
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Unknown action: '{}'. Valid: pause, resume", request.action)
                })),
            );
        }
    };

    // 1. Update DB
    if let Err(e) = repo.update_status(&id, new_status, None) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        );
    }

    // 2. Update in-memory registry
    let core_status = match new_status {
        "waiting" => openalpaca_core::agent::AgentStatus::Waiting {
            waiting_for: "user_action".to_string(),
        },
        "idle" => openalpaca_core::agent::AgentStatus::Idle,
        _ => openalpaca_core::agent::AgentStatus::Idle,
    };
    state
        .gateway
        .shared_context
        .agent_registry
        .update_status(&id, core_status);

    // 3. Emit event
    // Derive template_id from the instance or DB record
    let template_id = state
        .gateway
        .shared_context
        .agent_registry
        .get_instance(&id)
        .map(|a| a.template_id.clone())
        .unwrap_or_else(|| agent.template_id.clone());
    let _ = state.gateway.bus.publish(SystemEvent::AgentStatusChanged {
        agent_id: id.clone(),
        instance_id: id.clone(),
        template_id,
        name: agent.name.clone(),
        status: new_status.to_string(),
        current_task_id: None,
        timestamp: Utc::now(),
    });

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "agent_id": id,
            "status": new_status
        })),
    )
}

/// GET /v1/agents/{id}/config
pub async fn get_agent_config_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let service = &state.agent_config_service;

    match service.get_agent_config(&id) {
        Ok((config, version)) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(AgentConfigResponse {
                    config,
                    config_version: version,
                })
                .unwrap(),
            ),
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// PUT /v1/agents/{id}/config
pub async fn update_agent_config_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAgentConfigRequest>,
) -> impl IntoResponse {
    let service = &state.agent_config_service;

    match service.update_agent_config(&id, body.config, body.config_version) {
        Ok(new_version) => {
            let _ = state.gateway.bus.publish(SystemEvent::AgentConfigChanged {
                agent_id: id.clone(),
                action: "updated".to_string(),
                config_version: new_version,
                timestamp: Utc::now(),
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "agent_id": id,
                    "config_version": new_version,
                    "status": "updated"
                })),
            )
                .into_response()
        }
        Err(e) if e == "CONFIG_CONFLICT" => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": {
                    "code": "CONFIG_CONFLICT",
                    "message": "Config version mismatch — reload and retry"
                }
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// POST /v1/agents
pub async fn create_agent_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateAgentRequest>,
) -> impl IntoResponse {
    let service = &state.agent_config_service;

    match service.create_agent(body.config) {
        Ok(agent_id) => {
            let _ = state.gateway.bus.publish(SystemEvent::AgentConfigChanged {
                agent_id: agent_id.clone(),
                action: "created".to_string(),
                config_version: 0,
                timestamp: Utc::now(),
            });
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "agent_id": agent_id,
                    "status": "created"
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// POST /v1/agents/from-toml
pub async fn create_agent_from_toml_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateAgentFromTomlRequest>,
) -> impl IntoResponse {
    let service = &state.agent_config_service;

    match service.create_agent_from_toml(&body.toml_content) {
        Ok(agent_id) => {
            let _ = state.gateway.bus.publish(SystemEvent::AgentConfigChanged {
                agent_id: agent_id.clone(),
                action: "created".to_string(),
                config_version: 0,
                timestamp: Utc::now(),
            });
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "agent_id": agent_id,
                    "status": "created"
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// DELETE /v1/agents/{id}
pub async fn delete_agent_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let service = &state.agent_config_service;

    match service.delete_agent(&id) {
        Ok(()) => {
            let _ = state.gateway.bus.publish(SystemEvent::AgentConfigChanged {
                agent_id: id.clone(),
                action: "deleted".to_string(),
                config_version: 0,
                timestamp: Utc::now(),
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "agent_id": id,
                    "status": "archived"
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

// ── Template Handlers ─────────────────────────────────────────────

/// GET /v1/agent-templates
pub async fn list_templates_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListTemplatesQuery>,
) -> Response {
    let (window, since) = match resolve_window(query.window.as_deref(), Utc::now()) {
        Ok(resolved) => resolved,
        Err(bad) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "UNKNOWN_WINDOW",
                format!("Unknown window '{bad}' — use 7d, 30d, or all"),
            );
        }
    };

    // GAP-20/T48: one grouped query for the whole page, not one per template.
    // A read failure costs the counts, not the list — the panel then shows
    // every template with `0 runs`, which is what an empty span table means
    // too, so nothing renders as a fabricated number.
    let runs = SubagentSpanRepository::new(&state.db)
        .run_counts_by_template(since)
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to read template run counts: {e}");
            std::collections::HashMap::new()
        });

    let response = template_rows(&state.gateway.shared_context.agent_registry, &runs, window);

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap()),
    )
        .into_response()
}

/// The rows of `GET /v1/agent-templates`, sorted by id: every resolved
/// template with where it comes from (R-1), and a row for each id a file
/// withholds or switches off — in no list of templates, but listed, saying
/// why.
fn template_rows(
    registry: &AgentRegistry,
    runs: &std::collections::HashMap<String, openalpaca_storage::TemplateRunCount>,
    window: &str,
) -> Vec<TemplateResponse> {
    let mut rows: Vec<TemplateResponse> = registry
        .list_templates()
        .iter()
        .map(|t| {
            let id = &t.frontmatter.id;
            let provenance = registry.provenance(id);
            TemplateResponse::from_template(t, provenance.as_ref(), runs.get(id), window)
        })
        .collect();
    rows.extend(registry.held_off_ids().iter().filter_map(|id| {
        let provenance = registry.provenance(id)?;
        TemplateResponse::withheld(id, &provenance, runs.get(id), window)
    }));
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// GET /v1/agent-templates/{id}
pub async fn get_template_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let registry = &state.gateway.shared_context.agent_registry;
    let template = registry.get_template(&id);
    let provenance = registry.provenance(&id);
    if template.is_none() && registry.held_off_ids().binary_search(&id).is_err() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Template '{}' not found", id) })),
        )
            .into_response();
    }
    // Not exposed as a query parameter here (only the list route takes
    // `?window=`, per the plan) — the single-template read still needs *a*
    // window to stay consistent with the same `TemplateResponse` shape, so it
    // takes the list route's default.
    let (window, since) =
        resolve_window(None, Utc::now()).expect("the default window always resolves");
    let runs = SubagentSpanRepository::new(&state.db)
        .run_counts_by_template(since)
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to read template run counts: {e}");
            std::collections::HashMap::new()
        });
    // A withheld or switched-off id's row says why no definition runs under
    // it (R-1).
    let response = match (&template, &provenance) {
        (Some(template), _) => Some(TemplateResponse::from_template(
            template,
            provenance.as_ref(),
            runs.get(&id),
            window,
        )),
        (None, Some(provenance)) => {
            TemplateResponse::withheld(&id, provenance, runs.get(&id), window)
        }
        (None, None) => None,
    };
    match response {
        Some(response) => (
            StatusCode::OK,
            Json(serde_json::to_value(response).unwrap()),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Template '{}' not found", id) })),
        )
            .into_response(),
    }
}

/// POST /v1/agent-templates
pub async fn create_template_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTemplateRequest>,
) -> impl IntoResponse {
    let route = TemplateRoute::of(&state);
    match route.create(body.config) {
        Ok(template_id) => {
            let _ = state.gateway.bus.publish(SystemEvent::AgentConfigChanged {
                agent_id: template_id.clone(),
                action: "created".to_string(),
                config_version: 0,
                timestamp: Utc::now(),
            });
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "template_id": template_id,
                    "status": "created",
                    "overrides_bundled": route.overrides_bundled(&template_id)
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// PUT /v1/agent-templates/{id}
pub async fn update_template_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTemplateRequest>,
) -> impl IntoResponse {
    // Convert JSON config to AgentTemplate
    let template = body.config.into_template();
    let route = TemplateRoute::of(&state);
    match route.update(&id, template) {
        Ok(()) => {
            let _ = state.gateway.bus.publish(SystemEvent::AgentConfigChanged {
                agent_id: id.clone(),
                action: "updated".to_string(),
                config_version: 0,
                timestamp: Utc::now(),
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "template_id": id,
                    "status": "updated",
                    "overrides_bundled": route.overrides_bundled(&id)
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// DELETE /v1/agent-templates/{id}
pub async fn delete_template_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match TemplateRoute::of(&state).delete(&id) {
        Ok(()) => {
            let _ = state.gateway.bus.publish(SystemEvent::AgentConfigChanged {
                agent_id: id.clone(),
                action: "deleted".to_string(),
                config_version: 0,
                timestamp: Utc::now(),
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "template_id": id,
                    "status": "archived"
                })),
            )
                .into_response()
        }
        Err(e) => delete_refusal(&e),
    }
}

/// A refused `DELETE /v1/agent-templates/{id}`: a bundled template no file
/// holds is `409 {"error":"bundled","message":…}` (subagent design §2.10
/// T-2), a plugin's template `409 {"error":"plugin","plugin":<id>,
/// "message":…}`, the sentence naming the plugin too (PLUGIN-TEMPLATE-409),
/// anything else `404` as before.
fn delete_refusal(e: &DeleteTemplateError) -> Response {
    match e {
        DeleteTemplateError::Bundled => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "bundled", "message": e.to_string() })),
        )
            .into_response(),
        DeleteTemplateError::Plugin { plugin } => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "plugin",
                "plugin": plugin,
                "message": e.to_string(),
            })),
        )
            .into_response(),
        DeleteTemplateError::Other(message) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
    }
}

// ── Instance Handlers ─────────────────────────────────────────────

/// GET /v1/agent-instances
pub async fn list_instances_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let instances = state.gateway.shared_context.agent_registry.list_instances();

    let response: Vec<InstanceResponse> = instances
        .iter()
        .map(|a| InstanceResponse {
            id: a.id.clone(),
            template_id: a.template_id.clone(),
            name: a.name.clone(),
            status: a.status.as_str().to_string(),
            current_task: a.current_task.clone(),
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap()),
    )
}

// ── `?window=` parsing (GAP-20, T48) ────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap()
    }

    /// No `?window=` at all is the plan's default — `7d` — not an error.
    #[test]
    fn an_absent_window_defaults_to_7d() {
        let (label, since) = resolve_window(None, now()).unwrap();
        assert_eq!(label, "7d");
        assert_eq!(since, Some(now() - Duration::days(7)));
    }

    #[test]
    fn window_30d_cuts_off_30_days_back() {
        let (label, since) = resolve_window(Some("30d"), now()).unwrap();
        assert_eq!(label, "30d");
        assert_eq!(since, Some(now() - Duration::days(30)));
    }

    /// `all` is the one value with no cutoff at all.
    #[test]
    fn window_all_has_no_cutoff() {
        let (label, since) = resolve_window(Some("all"), now()).unwrap();
        assert_eq!(label, "all");
        assert_eq!(since, None);
    }

    /// An unrecognised window is refused with the token itself, so the 400 the
    /// handler builds from it can say exactly what it did not understand
    /// rather than a generic complaint.
    #[test]
    fn an_unknown_window_is_rejected_with_its_own_token() {
        assert_eq!(resolve_window(Some("90d"), now()), Err("90d".to_string()));
        assert_eq!(resolve_window(Some(""), now()), Err(String::new()));
    }

    // ── The template routes' catalog half (T-1, L-3) ─────────────────
    //
    // Driven over `TemplateRoute`, what the three handlers call: an
    // `AppState` is not constructible in a unit test. Every file lives in a
    // temporary directory.

    use crate::test_util::HomeStoreGuard;
    use openalpaca_core::agent::file_layer::{LoadMode, rescan_file_layer};
    use openalpaca_core::agent::template::parse_agent_markdown;
    use openalpaca_core::agent::{AgentConfigService, AgentRegistry};
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_storage::Database;

    struct Templates {
        // Declared first, so it drops first.
        _guard: HomeStoreGuard,
        dir: tempfile::TempDir,
        registry: Arc<AgentRegistry>,
        service: AgentConfigService,
        tool_registry: ToolRegistry,
        db: Database,
    }

    impl Templates {
        fn new(files: &[(&str, String)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let agents = dir.path().join("agents");
            std::fs::create_dir_all(&agents).unwrap();
            for (name, contents) in files {
                std::fs::write(agents.join(name), contents).unwrap();
            }
            let guard = HomeStoreGuard::set(&dir.path().canonicalize().unwrap().join("home"));
            let db = Database::open(&dir.path().join("test.db")).unwrap();
            let registry = Arc::new(AgentRegistry::new());
            registry.set_bundled_layer(openalpaca_core::bundled::bundled_templates());
            let service = AgentConfigService::new(registry.clone(), agents.clone(), db.clone());
            let tool_registry = ToolRegistry::default();
            rescan_file_layer(&agents, LoadMode::Start, &registry, &tool_registry, &db).unwrap();
            Self {
                _guard: guard,
                dir,
                registry,
                service,
                tool_registry,
                db,
            }
        }

        fn route(&self) -> TemplateRoute<'_> {
            TemplateRoute {
                service: &self.service,
                registry: &self.registry,
                tool_registry: &self.tool_registry,
                db: &self.db,
            }
        }

        fn agents(&self) -> std::path::PathBuf {
            self.dir.path().join("agents")
        }
    }

    fn lead(model: &str) -> String {
        format!(
            "---\nid: lead_agent\nname: Lead\ndescription: leads\nsingleton: true\n\
             model: {model}\ncapabilities:\n  - \"orchestration\"\n---\n\n## Persona\n\nLeads.\n"
        )
    }

    /// T-1: `PUT /v1/agent-templates/{id}` is in the catalog when it returns,
    /// the singleton lead's next run included — no longer only once the
    /// watcher has run, and never for a lead whose instance had gone.
    #[test]
    fn put_agent_template_is_in_the_catalog_when_it_returns() {
        let t = Templates::new(&[("lead_agent.md", lead("model-a"))]);
        let ran = t
            .registry
            .spawn_instance("lead_agent", "t1".to_string())
            .unwrap();
        t.registry.destroy_instance(&ran.id);

        let edited = parse_agent_markdown(&lead("model-b")).unwrap();
        t.route().update("lead_agent", edited).unwrap();

        let template = t.registry.get_template("lead_agent").unwrap();
        assert_eq!(template.frontmatter.model.as_deref(), Some("model-b"));
        let next = t
            .registry
            .spawn_instance("lead_agent", "t2".to_string())
            .unwrap();
        assert_eq!(next.llm_config.model.as_deref(), Some("model-b"));
        let on_disk = std::fs::read_to_string(t.agents().join("lead_agent.md")).unwrap();
        assert!(on_disk.contains("model: \"model-b\""), "{on_disk}");
    }

    /// LEAD-SINGLETON (C2's item 2, taken 2026-11-12): `GET
    /// /v1/agents/lead_agent/config` finds the lead before its first run and
    /// after it — the bundled lead's and a file's alike — where the first
    /// run used to take the instance it reads.
    #[test]
    fn the_lead_agents_config_answers_before_and_after_a_run() {
        for files in [vec![], vec![("lead_agent.md", lead("model-a"))]] {
            let t = Templates::new(&files);
            let (_, version) = t
                .service
                .get_agent_config("lead_agent")
                .expect("found before the first run");
            let ran = t
                .registry
                .spawn_instance("lead_agent", "t1".to_string())
                .unwrap();
            t.registry.destroy_instance(&ran.id);
            let (config, kept) = t
                .service
                .get_agent_config("lead_agent")
                .expect("found after a run");
            assert_eq!(config.agent.id, "lead_agent");
            assert_eq!(kept, version);
        }
    }

    /// LEAD-SINGLETON: a `PUT /v1/agents/lead_agent/config` (`agents set`)
    /// holds for the lead's later runs, not only its next one, until its
    /// template changes — the rescan of a changed definition rebuilds it.
    #[test]
    fn a_config_set_on_the_lead_holds_for_its_runs_until_its_template_changes() {
        let t = Templates::new(&[("lead_agent.md", lead("model-a"))]);
        let (mut config, version) = t.service.get_agent_config("lead_agent").unwrap();
        config.llm.as_mut().expect("the lead names a model").model = Some("model-set".into());
        t.service
            .update_agent_config("lead_agent", config, version)
            .unwrap();

        for task in ["t1", "t2"] {
            let run = t
                .registry
                .spawn_instance("lead_agent", task.to_string())
                .unwrap();
            assert_eq!(run.llm_config.model.as_deref(), Some("model-set"));
            t.registry.destroy_instance(&run.id);
        }

        let edited = parse_agent_markdown(&lead("model-b")).unwrap();
        t.route().update("lead_agent", edited).unwrap();
        let next = t
            .registry
            .spawn_instance("lead_agent", "t3".to_string())
            .unwrap();
        assert_eq!(next.llm_config.model.as_deref(), Some("model-b"));
    }

    /// T-2's present form: `DELETE /v1/agent-templates/{id}` archives the
    /// file, and the template, its idle instance and its row's status follow
    /// when the call returns.
    #[test]
    fn deleting_a_template_through_the_route_archives_it_and_leaves_the_catalog() {
        let reader = lead("model-a")
            .replace("id: lead_agent", "id: reader")
            .replace("singleton: true", "singleton: false");
        let t = Templates::new(&[("reader.md", reader), ("lead_agent.md", lead("m"))]);
        assert!(t.registry.get_with_version("reader").is_some());

        t.route().delete("reader").unwrap();

        assert!(!t.agents().join("reader.md").exists());
        let archived: Vec<String> = std::fs::read_dir(t.agents().join(".archived"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(archived.len(), 1);
        assert!(archived[0].starts_with("reader.") && archived[0].ends_with(".md"));
        assert!(t.registry.get_template("reader").is_none());
        assert!(t.registry.get_with_version("reader").is_none());
        let row = SubAgentRepository::new(&t.db)
            .get("reader")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "archived");
        assert!(t.registry.get_template("lead_agent").is_some());
    }

    // ── The bundled layer through the routes (§2.7 R-1, §2.10 T-1, T-2) ──

    /// The bundled bytes of `config/<path>`.
    fn bundled(path: &str) -> &'static str {
        openalpaca_core::bundled::BUNDLED_AGENTS
            .iter()
            .find(|file| file.path == path)
            .unwrap()
            .contents
    }

    fn edited(path: &str) -> String {
        format!("{}\nMy own note.\n", bundled(path))
    }

    /// `eb98b820`'s `system_agent.md`, as an earlier build seeded it.
    const SEEDED_SYSTEM_AGENT: &str = include_str!(
        "../../../../crates/openalpaca_core/src/bundled/fixtures/eb98b820/agents/system_agent.md"
    );

    struct IdleExecutor;

    #[async_trait::async_trait]
    impl openalpaca_api::plugin_traits::PluginAgentExecutor for IdleExecutor {
        async fn spawn(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &serde_json::Value,
        ) -> Result<bool, String> {
            Ok(true)
        }
        async fn step(
            &self,
            _: &str,
            _: Option<&serde_json::Value>,
        ) -> Result<(String, String, Vec<serde_json::Value>), String> {
            Ok(("complete".to_string(), String::new(), vec![]))
        }
        async fn stop(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn plugin_id(&self) -> &str {
            "acme"
        }
        fn agent_id(&self) -> &str {
            "general_agent"
        }
    }

    /// R-1: each row says where its template comes from — the bundle, a
    /// file (overriding a bundled one or not), a plugin — what is not used
    /// under its id (an unedited copy set aside, the bundled template
    /// overridden, a plugin refused), whether it is withheld, and what the
    /// daemon warns about it.
    #[test]
    fn agent_templates_rows_carry_source_plugin_path_overrides_state_not_used_and_warnings() {
        let mut code = edited("agents/code_agent.md");
        code = code.replace(
            "\n---\n",
            "\nrequire_confirmation_for:\n  - \"file_delete\"\n---\n",
        );
        let t = Templates::new(&[
            ("system_agent.md", SEEDED_SYSTEM_AGENT.to_string()),
            ("code_agent.md", code),
            (
                "review_agent.md",
                "---\nid: review_agent\ndescription: half edited\n---\n".to_string(),
            ),
            (
                "reader.md",
                lead("m").replace("id: lead_agent", "id: reader"),
            ),
        ]);
        let mut plugin =
            parse_agent_markdown("---\nid: general_agent\nname: A\ndescription: d\n---\n").unwrap();
        plugin.source = openalpaca_core::agent::AgentSource::Plugin {
            plugin_id: "acme".to_string(),
            executor: Arc::new(IdleExecutor),
        };
        assert!(!t.registry.register_template(plugin));

        let rows = serde_json::to_value(template_rows(
            &t.registry,
            &std::collections::HashMap::new(),
            "7d",
        ))
        .unwrap();
        let row = |id: &str| {
            rows.as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("no row for {id}"))
                .clone()
        };
        let ids: Vec<&str> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "sorted by id");
        assert_eq!(ids.len(), 10, "the nine and the person's own: {ids:?}");

        let system = row("system_agent");
        assert_eq!(system["source"], "bundled");
        assert_eq!(system["plugin"], serde_json::Value::Null);
        assert_eq!(system["path"], serde_json::Value::Null);
        assert_eq!(system["overrides_bundled"], false);
        assert_eq!(system["state"], "ready");
        assert_eq!(
            system["not_used"],
            serde_json::json!([{
                "source": "file",
                "path": t.agents().join("system_agent.md").display().to_string(),
                "plugin": null,
                "reason": "unedited_copy",
                "version": "earlier",
                "edit": null
            }])
        );
        assert_eq!(system["warnings"], serde_json::json!([]));

        let code = row("code_agent");
        assert_eq!(code["source"], "file");
        assert_eq!(
            code["path"],
            t.agents().join("code_agent.md").display().to_string()
        );
        assert_eq!(code["overrides_bundled"], true);
        assert_eq!(code["not_used"][0]["source"], "bundled");
        assert_eq!(code["not_used"][0]["reason"], "overridden");
        let warning = code["warnings"][0].as_str().unwrap();
        assert!(
            warning.contains("require_confirmation_for names \"file_delete\", which is no tool"),
            "{warning}"
        );

        let review = row("review_agent");
        assert_eq!(review["state"], "withheld");
        assert_eq!(
            review["path"],
            t.agents().join("review_agent.md").display().to_string()
        );
        assert!(
            review["warnings"][0]
                .as_str()
                .unwrap()
                .contains("does not parse"),
            "{review}"
        );

        let general = row("general_agent");
        assert_eq!(general["source"], "bundled");
        assert_eq!(
            general["not_used"],
            serde_json::json!([{
                "source": "plugin",
                "path": null,
                "plugin": "acme",
                "reason": "plugin_refused",
                "version": null,
                "edit": null
            }])
        );

        let reader = row("reader");
        assert_eq!(reader["source"], "file");
        assert_eq!(reader["overrides_bundled"], false);
    }

    /// T-1: `PUT` on a bundled template with no file writes `<id>.md`, an
    /// override, in effect when the call returns.
    #[test]
    fn put_on_a_bundled_id_writes_an_override() {
        let t = Templates::new(&[]);
        let mut template = parse_agent_markdown(bundled("agents/explore_agent.md")).unwrap();
        template.frontmatter.max_tool_calls = Some(7);

        t.route().update("explore_agent", template).unwrap();

        assert!(t.agents().join("explore_agent.md").exists());
        assert!(t.route().overrides_bundled("explore_agent"));
        assert_eq!(
            t.registry
                .get_template("explore_agent")
                .unwrap()
                .frontmatter
                .max_tool_calls,
            Some(7)
        );
    }

    /// T-1: `POST` with a bundled id is no longer "already exists": it
    /// writes an override and says so.
    #[test]
    fn post_with_a_bundled_id_writes_an_override() {
        let t = Templates::new(&[]);
        let mut template = parse_agent_markdown(bundled("agents/general_agent.md")).unwrap();
        template.frontmatter.max_tool_calls = Some(9);
        let config = AgentConfigFile::from_template(&template);

        assert_eq!(t.route().create(config).unwrap(), "general_agent");

        assert!(t.agents().join("general_agent.md").exists());
        assert!(t.route().overrides_bundled("general_agent"));
        assert_eq!(
            t.registry
                .get_template("general_agent")
                .unwrap()
                .frontmatter
                .max_tool_calls,
            Some(9)
        );
    }

    /// T-1: `PUT` on an override whose file is named otherwise rewrites that
    /// file — the one `provenance` names — and adds no `<id>.md` beside it.
    #[test]
    fn put_on_a_renamed_override_rewrites_that_file() {
        let t = Templates::new(&[("my system.md", edited("agents/system_agent.md"))]);
        let mut template = t.registry.get_template("system_agent").unwrap();
        template.frontmatter.max_tool_calls = Some(12);

        t.route().update("system_agent", template).unwrap();

        assert!(!t.agents().join("system_agent.md").exists());
        let on_disk = std::fs::read_to_string(t.agents().join("my system.md")).unwrap();
        assert!(on_disk.contains("max_tool_calls: 12"), "{on_disk}");
        assert_eq!(
            t.registry
                .get_template("system_agent")
                .unwrap()
                .frontmatter
                .max_tool_calls,
            Some(12)
        );
    }

    /// T-2: `DELETE` archives an override whose file is named otherwise —
    /// as `.archived/<file stem>.<timestamp>.md` — and the bundled template
    /// is back when the call returns.
    #[test]
    fn delete_archives_a_renamed_override() {
        let t = Templates::new(&[("my system.md", edited("agents/system_agent.md"))]);
        assert!(t.route().overrides_bundled("system_agent"));

        t.route().delete("system_agent").unwrap();

        assert!(!t.agents().join("my system.md").exists());
        let archived: Vec<String> = std::fs::read_dir(t.agents().join(".archived"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(archived.len(), 1);
        assert!(
            archived[0].starts_with("my system.") && archived[0].ends_with(".md"),
            "{archived:?}"
        );
        let provenance = t.registry.provenance("system_agent").unwrap();
        assert_eq!(
            provenance.layer,
            openalpaca_core::agent::registry::TemplateLayer::Bundled
        );
        let row = SubAgentRepository::new(&t.db)
            .get("system_agent")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "idle", "the bundled template's row, live again");
    }

    /// T-2: a bundled template no file holds cannot be removed: `409
    /// {"error":"bundled"}` with a sentence, and nothing changes.
    #[tokio::test]
    async fn deleting_a_bundled_template_with_no_file_is_409() {
        let t = Templates::new(&[]);

        let err = t.route().delete("general_agent").unwrap_err();
        assert_eq!(err, DeleteTemplateError::Bundled);
        let response = delete_refusal(&err);
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "bundled");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .starts_with("a built-in template cannot be removed"),
            "{body}"
        );
        assert!(t.registry.get_template("general_agent").is_some());
        assert!(!t.agents().join(".archived").exists());
    }

    /// PLUGIN-TEMPLATE-409 (C2's item (3), taken as (b) on 2026-11-12):
    /// `DELETE` of a plugin's template is refused as a bundled one is —
    /// `409 {"error":"plugin"}`, its sentence naming the plugin — and
    /// nothing changes: the template stays in the catalog, nothing is
    /// archived and its row is not touched.
    #[tokio::test]
    async fn deleting_a_plugin_s_template_is_409_naming_the_plugin() {
        let t = Templates::new(&[]);
        let mut plugin =
            parse_agent_markdown("---\nid: acme_agent\nname: A\ndescription: d\n---\n").unwrap();
        plugin.source = openalpaca_core::agent::AgentSource::Plugin {
            plugin_id: "acme".to_string(),
            executor: Arc::new(IdleExecutor),
        };
        assert!(t.registry.register_template(plugin));
        // An `agent` row is what a delete would archive, and a leftover
        // `<id>.toml` what it would move to `.archived`: both must stand
        // after the refusal.
        SubAgentRepository::new(&t.db)
            .upsert(&openalpaca_storage::SubAgentConfig {
                id: "acme_agent".to_string(),
                template_id: "acme_agent".to_string(),
                name: "A".to_string(),
                description: Some("d".to_string()),
                icon: None,
                status: "idle".to_string(),
                current_task_id: None,
                skills_json: "[]".to_string(),
                preset_json: "{}".to_string(),
                constraints_json: None,
                llm_config_json: None,
                persona: None,
                created_at: chrono::Utc::now(),
                updated_at: None,
            })
            .unwrap();
        let leftover = t.agents().join("acme_agent.toml");
        std::fs::write(&leftover, "id = \"acme_agent\"\n").unwrap();
        let row_before = SubAgentRepository::new(&t.db)
            .get("acme_agent")
            .unwrap()
            .map(|row| row.status);
        assert!(row_before.is_some(), "no agent row seeded");
        assert_ne!(row_before.as_deref(), Some("archived"));

        let err = t.route().delete("acme_agent").unwrap_err();
        let response = delete_refusal(&err);
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "plugin", "{body}");
        assert_eq!(body["plugin"], "acme", "{body}");
        let message = body["message"].as_str().unwrap();
        assert!(
            message.starts_with(
                "a plugin's template cannot be removed; it comes from the plugin acme"
            ),
            "{body}"
        );
        assert!(
            message.contains("openalpaca ext disable plugin acme"),
            "{body}"
        );

        assert!(t.registry.get_template("acme_agent").is_some());
        assert_eq!(
            t.registry.provenance("acme_agent").unwrap().layer,
            openalpaca_core::agent::registry::TemplateLayer::Plugin
        );
        assert!(!t.agents().join(".archived").exists());
        assert!(leftover.exists());
        let row_after = SubAgentRepository::new(&t.db)
            .get("acme_agent")
            .unwrap()
            .map(|row| row.status);
        assert_eq!(row_after, row_before);
        assert_ne!(row_after.as_deref(), Some("archived"));
    }

    // ── A file that switches a template off (§2.4 L-4a, §7 O-1 (b)) ──

    /// R-1: an id a file switches off keeps its row, saying `disabled` and
    /// naming the file — a bundled id and one of the person's own alike —
    /// while no definition runs under it.
    #[test]
    fn agent_templates_rows_say_disabled_with_the_path() {
        let t = Templates::new(&[
            (
                "research_agent.md",
                "---\nid: research_agent\ndisabled: true\n---\n".to_string(),
            ),
            (
                "reader_off.md",
                "---\nid: reader\ndisabled: true\n---\n".to_string(),
            ),
        ]);

        let rows = serde_json::to_value(template_rows(
            &t.registry,
            &std::collections::HashMap::new(),
            "7d",
        ))
        .unwrap();
        let row = |id: &str| {
            rows.as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == id)
                .unwrap_or_else(|| panic!("no row for {id}: {rows}"))
                .clone()
        };
        let research = row("research_agent");
        assert_eq!(research["state"], "disabled");
        assert_eq!(research["source"], "file");
        assert_eq!(
            research["path"],
            t.agents().join("research_agent.md").display().to_string()
        );
        assert_eq!(research["warnings"], serde_json::json!([]));
        let reader = row("reader");
        assert_eq!(reader["state"], "disabled");
        assert_eq!(
            reader["path"],
            t.agents().join("reader_off.md").display().to_string()
        );
        assert_eq!(rows.as_array().unwrap().len(), 10);
    }

    /// T-2: `DELETE` on a switched-off id archives the switching file, and
    /// the template it switched off is back when the call returns, its row
    /// `idle` again.
    #[test]
    fn delete_on_a_switched_off_id_archives_the_switching_file() {
        let t = Templates::new(&[(
            "no research.md",
            "---\nid: research_agent\ndisabled: true\n---\n".to_string(),
        )]);
        assert!(t.registry.get_template("research_agent").is_none());
        let rows = serde_json::to_value(template_rows(
            &t.registry,
            &std::collections::HashMap::new(),
            "7d",
        ))
        .unwrap();
        let research = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "research_agent")
            .unwrap_or_else(|| panic!("no row for research_agent: {rows}"));
        assert_eq!(research["state"], "disabled", "switched off, not withheld");

        t.route().delete("research_agent").unwrap();

        assert!(!t.agents().join("no research.md").exists());
        let archived: Vec<String> = std::fs::read_dir(t.agents().join(".archived"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(archived.len(), 1);
        assert!(
            archived[0].starts_with("no research.") && archived[0].ends_with(".md"),
            "{archived:?}"
        );
        assert!(t.registry.get_template("research_agent").is_some());
        let row = SubAgentRepository::new(&t.db)
            .get("research_agent")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "idle");
    }

    /// T-2: the refusal for a bundled template no file holds names the two
    /// ways a person has — a file overriding it, or one switching it off.
    #[test]
    fn the_bundled_refusal_names_the_switch() {
        let message = DeleteTemplateError::Bundled.to_string();
        assert!(message.contains("override it"), "{message}");
        assert!(message.contains("disabled: true"), "{message}");
    }
}
