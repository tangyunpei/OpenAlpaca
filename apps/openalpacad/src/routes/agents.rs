//! Agent management endpoints
//!
//! GET  /v1/agents            -> list agents (query: status, skill, limit)
//! GET  /v1/agents/{id}       -> get agent config + metrics
//! POST /v1/agents/{id}/action -> perform action (pause, resume)

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    let _ = state.gateway.bus.publish(SystemEvent::AgentStatusChanged {
        agent_id: id.clone(),
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
