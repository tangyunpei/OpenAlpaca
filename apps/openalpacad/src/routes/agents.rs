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
//! DELETE /v1/agent-templates/{id}         -> archive template
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

    fn delete(&self, id: &str) -> Result<(), String> {
        let deleted = self.service.delete_template(id);
        if deleted.is_ok() {
            self.rescan();
        }
        deleted
    }

    /// `LoadMode::Reload`: a written file naming an unknown `annotation:`
    /// capability is withheld at once (and still stops the next start).
    fn rescan(&self) {
        let known = self.tool_registry.known_virtual_capabilities();
        let agents_dir = self.service.agents_dir();
        if let Err(e) =
            rescan_file_layer(agents_dir, LoadMode::Reload, self.registry, &known, self.db)
        {
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

    let templates = state.gateway.shared_context.agent_registry.list_templates();

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

    let response: Vec<TemplateResponse> = templates
        .iter()
        .map(|t| TemplateResponse::from_template(t, runs.get(&t.frontmatter.id), window))
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::to_value(response).unwrap()),
    )
        .into_response()
}

/// GET /v1/agent-templates/{id}
pub async fn get_template_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state
        .gateway
        .shared_context
        .agent_registry
        .get_template(&id)
    {
        Some(template) => {
            // Not exposed as a query parameter here (only the list route
            // takes `?window=`, per the plan) — the single-template read
            // still needs *a* window to stay consistent with the same
            // `TemplateResponse` shape, so it takes the list route's default.
            let (window, since) =
                resolve_window(None, Utc::now()).expect("the default window always resolves");
            let runs = SubagentSpanRepository::new(&state.db)
                .run_counts_by_template(since)
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to read template run counts: {e}");
                    std::collections::HashMap::new()
                });
            let response = TemplateResponse::from_template(&template, runs.get(&id), window);
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
                .into_response()
        }
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
    match TemplateRoute::of(&state).create(body.config) {
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

/// PUT /v1/agent-templates/{id}
pub async fn update_template_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTemplateRequest>,
) -> impl IntoResponse {
    // Convert JSON config to AgentTemplate
    let template = body.config.into_template();
    match TemplateRoute::of(&state).update(&id, template) {
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
                    "status": "updated"
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
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": e })),
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
            let service = AgentConfigService::new(registry.clone(), agents.clone(), db.clone());
            rescan_file_layer(&agents, LoadMode::Start, &registry, &[], &db).unwrap();
            Self {
                _guard: guard,
                dir,
                registry,
                service,
                tool_registry: ToolRegistry::default(),
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
}
