//! Agent management endpoints
//!
//! GET  /v1/agents            -> list agents (query: status, skill, limit)
//! GET  /v1/agents/{id}       -> get agent config + metrics
//! POST /v1/agents/{id}/action -> perform action (pause, resume)
//! GET  /v1/agents/{id}/config -> get agent config + version
//! PUT  /v1/agents/{id}/config -> update agent config (optimistic locking)
//! POST /v1/agents             -> create agent
//! POST /v1/agents/from-toml   -> create agent from raw TOML
//! POST /v1/agents/from-chat   -> stub (501)
//! DELETE /v1/agents/{id}      -> delete (archive) agent
//!
//! Template endpoints:
//! GET    /v1/agent-templates              -> list all templates
//! GET    /v1/agent-templates/{id}         -> get template (JSON)
//! GET    /v1/agent-templates/{id}/markdown -> get template as raw Markdown
//! POST   /v1/agent-templates              -> create template (JSON body)
//! POST   /v1/agent-templates/from-markdown -> create template from raw Markdown
//! PUT    /v1/agent-templates/{id}         -> update template
//! DELETE /v1/agent-templates/{id}         -> archive template
//!
//! Instance endpoints:
//! GET    /v1/agent-instances              -> list active instances
//! POST   /v1/agent-templates/{id}/instances -> spawn instance from template
//! DELETE /v1/agent-instances/{id}         -> destroy instance

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use openalpaca_core::agent::AgentConfigFile;
use openalpaca_core::agent::template::render_agent_markdown;
use openalpaca_core::events::SystemEvent;
use openalpaca_storage::{AgentMetrics, SubAgentConfig, SubAgentRepository};

use crate::AppState;

// ── Request / Response Types ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListAgentsQuery {
    pub status: Option<String>,
    pub skill: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AgentActionRequest {
    pub action: String, // "pause", "resume"
}

#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub agent: SubAgentConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<AgentMetrics>,
}

#[derive(Debug, Serialize)]
pub struct AgentConfigResponse {
    pub config: AgentConfigFile,
    pub config_version: u64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentConfigRequest {
    pub config: AgentConfigFile,
    pub config_version: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentFromTomlRequest {
    pub toml_content: String,
}

// ── Template Request / Response Types ─────────────────────────────

/// JSON representation of an agent template for the REST API.
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub singleton: bool,
    pub skills: Vec<String>,
    pub denied_skills: Vec<String>,
    pub temperature: f32,
    pub verbosity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub fallback_models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost_per_task: Option<f64>,
    pub require_confirmation_for: Vec<String>,
    pub persona: String,
    pub body: String,
}

impl TemplateResponse {
    fn from_template(t: &openalpaca_core::agent::AgentTemplate) -> Self {
        let persona = openalpaca_core::agent::template::extract_persona(t);
        let fm = &t.frontmatter;
        Self {
            id: fm.id.clone(),
            name: fm.name.clone(),
            description: fm.description.clone(),
            icon: fm.icon.clone(),
            singleton: fm.singleton,
            skills: fm.skills.clone(),
            denied_skills: fm.denied_skills.clone(),
            temperature: fm.temperature,
            verbosity: fm.verbosity.clone(),
            model: fm.model.clone(),
            fallback_models: fm.fallback_models.clone(),
            max_tool_calls: fm.max_tool_calls,
            timeout_seconds: fm.timeout_seconds,
            max_cost_per_task: fm.max_cost_per_task,
            require_confirmation_for: fm.require_confirmation_for.clone(),
            persona,
            body: t.body.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTemplateRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct CreateTemplateFromMarkdownRequest {
    pub markdown: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTemplateRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct SpawnInstanceRequest {
    pub task_id: String,
}

/// JSON representation of an active agent instance.
#[derive(Debug, Serialize)]
pub struct InstanceResponse {
    pub id: String,
    pub template_id: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────

/// GET /v1/agents
pub async fn list_agents_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListAgentsQuery>,
) -> impl IntoResponse {
    let repo = SubAgentRepository::new(&state.db);
    let limit = query.limit.unwrap_or(50);

    // If filtering by skill, use in-memory registry
    if let Some(ref skill) = query.skill {
        let agents = state
            .gateway
            .shared_context
            .agent_registry
            .find_by_skill(skill);
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

        let filtered: Vec<SubAgentConfig> = all
            .into_iter()
            .filter(|c| ids.contains(&c.id))
            .collect();

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
                Json(
                    serde_json::to_value(AgentResponse {
                        agent,
                        metrics,
                    })
                    .unwrap(),
                ),
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
    let template_id = state.gateway.shared_context.agent_registry
        .get_instance(&id)
        .map(|a| a.template_id.clone())
        .unwrap_or_else(|| agent.template_id.clone());
    let _ = state.gateway.bus.publish(SystemEvent::AgentStatusChanged {
        agent_id: id.clone(),
        instance_id: id.clone(),
        template_id,
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
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

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
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

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
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

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
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

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

/// POST /v1/agents/from-chat — stub: returns 501
pub async fn create_agent_from_chat_handler() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "Agent creation from chat is planned for Phase 6.0+"
        })),
    )
}

/// DELETE /v1/agents/{id}
pub async fn delete_agent_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

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
) -> impl IntoResponse {
    let templates = state
        .gateway
        .shared_context
        .agent_registry
        .list_templates();

    let response: Vec<TemplateResponse> = templates
        .iter()
        .map(TemplateResponse::from_template)
        .collect();

    (StatusCode::OK, Json(serde_json::to_value(response).unwrap()))
}

/// GET /v1/agent-templates/{id}
pub async fn get_template_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.gateway.shared_context.agent_registry.get_template(&id) {
        Some(template) => {
            let response = TemplateResponse::from_template(&template);
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

/// GET /v1/agent-templates/{id}/markdown
pub async fn get_template_markdown_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.gateway.shared_context.agent_registry.get_template(&id) {
        Some(template) => {
            let markdown = render_agent_markdown(&template);
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
                markdown,
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
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

    match service.create_template_from_toml_config(body.config) {
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

/// POST /v1/agent-templates/from-markdown
pub async fn create_template_from_markdown_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTemplateFromMarkdownRequest>,
) -> impl IntoResponse {
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

    match service.create_template_from_markdown(&body.markdown) {
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
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

    // Convert JSON config to AgentTemplate
    let template = body.config.into_template();
    match service.update_template(&id, template) {
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
    let service = match &state.agent_config_service {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Agent config service not available" })),
            )
                .into_response();
        }
    };

    match service.delete_template(&id) {
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
pub async fn list_instances_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let instances = state
        .gateway
        .shared_context
        .agent_registry
        .list_instances();

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

    (StatusCode::OK, Json(serde_json::to_value(response).unwrap()))
}

/// POST /v1/agent-templates/{id}/instances
///
/// Spawn a new instance from a template.
pub async fn spawn_instance_handler(
    State(state): State<Arc<AppState>>,
    Path(template_id): Path<String>,
    Json(body): Json<SpawnInstanceRequest>,
) -> impl IntoResponse {
    let registry = &state.gateway.shared_context.agent_registry;

    match registry.spawn_instance(&template_id, body.task_id) {
        Ok(agent) => {
            let now = Utc::now();
            let _ = state.gateway.bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: agent.id.clone(),
                instance_id: agent.id.clone(),
                template_id: agent.template_id.clone(),
                status: "spawned".to_string(),
                current_task_id: agent.current_task.clone(),
                timestamp: now,
            });
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "instance_id": agent.id,
                    "template_id": agent.template_id,
                    "name": agent.name,
                    "status": agent.status.as_str(),
                    "current_task": agent.current_task,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// DELETE /v1/agent-instances/{id}
///
/// Destroy (remove) an agent instance.
pub async fn destroy_instance_handler(
    State(state): State<Arc<AppState>>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    use openalpaca_core::agent::DestroyOutcome;

    let registry = &state.gateway.shared_context.agent_registry;

    // Capture template_id before destruction (extract from instance_id format)
    let template_id = registry.get_instance(&instance_id)
        .map(|a| a.template_id.clone())
        .unwrap_or_else(|| {
            instance_id.split("::").next().unwrap_or(&instance_id).to_string()
        });

    let outcome = registry.destroy_instance(&instance_id);
    match outcome {
        DestroyOutcome::Removed | DestroyOutcome::ResetToIdle => {
            let status = match outcome {
                DestroyOutcome::ResetToIdle => "idle",
                _ => "destroyed",
            };
            let now = Utc::now();
            let _ = state.gateway.bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: instance_id.clone(),
                instance_id: instance_id.clone(),
                template_id,
                status: status.to_string(),
                current_task_id: None,
                timestamp: now,
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "instance_id": instance_id,
                    "status": status
                })),
            )
                .into_response()
        }
        DestroyOutcome::NotFound => {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Instance '{}' not found", instance_id)
                })),
            )
                .into_response()
        }
    }
}
