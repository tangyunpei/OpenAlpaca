use crate::AppState;
use axum::{
    Router,
    extract::{DefaultBodyLimit, State},
    response::Json,
    routing::{delete, get, post, put},
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

/// Build the complete HTTP router with public, protected, websocket, and SSE routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    // Public routes (no auth required)
    let public = Router::new()
        .route("/", get(root_handler))
        .route("/v1/health", get(health_handler));

    // Protected routes (require token)
    let protected_routes = Router::new()
        .route("/v1/command", post(crate::routes::command_handler))
        .route(
            "/v1/events/history",
            get(crate::routes::events_history_handler),
        )
        .route(
            "/v1/orchestrator/latency",
            get(crate::routes::orchestrator_latency_handler),
        )
        .route(
            "/v1/orchestrator/latency/aggregate",
            get(crate::routes::orchestrator_latency_aggregate_handler),
        )
        .route(
            "/v1/orchestrator/decisions",
            get(crate::routes::dispatch_decisions_handler),
        )
        .route(
            "/v1/connectors",
            get(crate::routes::list_connectors_handler),
        )
        .route(
            "/v1/connectors/{id}/action",
            post(crate::routes::connector_action_handler),
        )
        .route(
            "/v1/connectors/{id}/config",
            post(crate::routes::connector_config_handler),
        )
        .route(
            "/v1/auth/link",
            post(crate::routes::generate_link_token_handler),
        )
        .route("/v1/tasks", post(crate::routes::create_task_handler))
        .route("/v1/tasks", get(crate::routes::list_tasks_handler))
        .route("/v1/tasks/{id}", get(crate::routes::get_task_handler))
        .route(
            "/v1/tasks/{id}/action",
            post(crate::routes::task_action_handler),
        )
        .route(
            "/v1/tasks/{id}/dag",
            get(crate::routes::get_task_dag_handler),
        )
        // Preferences routes (Phase 5)
        .route(
            "/v1/preferences",
            get(crate::routes::list_preferences_handler),
        )
        .route(
            "/v1/preferences/{key}",
            get(crate::routes::get_preference_handler),
        )
        .route(
            "/v1/preferences/{key}",
            put(crate::routes::set_preference_handler),
        )
        .route(
            "/v1/preferences/{key}",
            delete(crate::routes::delete_preference_handler),
        )
        .route("/v1/agents", get(crate::routes::list_agents_handler))
        .route("/v1/agents", post(crate::routes::create_agent_handler))
        .route(
            "/v1/agents/from-toml",
            post(crate::routes::create_agent_from_toml_handler),
        )
        .route(
            "/v1/agents/from-chat",
            post(crate::routes::create_agent_from_chat_handler),
        )
        .route("/v1/agents/{id}", get(crate::routes::get_agent_handler))
        .route(
            "/v1/agents/{id}",
            delete(crate::routes::delete_agent_handler),
        )
        .route(
            "/v1/agents/{id}/config",
            get(crate::routes::get_agent_config_handler),
        )
        .route(
            "/v1/agents/{id}/config",
            put(crate::routes::update_agent_config_handler),
        )
        .route(
            "/v1/agents/{id}/action",
            post(crate::routes::agent_action_handler),
        )
        // Agent template endpoints
        .route(
            "/v1/agent-templates",
            get(crate::routes::list_templates_handler),
        )
        .route(
            "/v1/agent-templates",
            post(crate::routes::create_template_handler),
        )
        .route(
            "/v1/agent-templates/from-markdown",
            post(crate::routes::create_template_from_markdown_handler),
        )
        .route(
            "/v1/agent-templates/{id}",
            get(crate::routes::get_template_handler),
        )
        .route(
            "/v1/agent-templates/{id}",
            put(crate::routes::update_template_handler),
        )
        .route(
            "/v1/agent-templates/{id}",
            delete(crate::routes::delete_template_handler),
        )
        .route(
            "/v1/agent-templates/{id}/markdown",
            get(crate::routes::get_template_markdown_handler),
        )
        // Agent instance endpoints
        .route(
            "/v1/agent-instances",
            get(crate::routes::list_instances_handler),
        )
        .route(
            "/v1/agent-templates/{id}/instances",
            post(crate::routes::spawn_instance_handler),
        )
        .route(
            "/v1/agent-instances/{id}",
            delete(crate::routes::destroy_instance_handler),
        )
        // File upload routes (multimodal)
        .route(
            "/v1/files/upload",
            post(crate::routes::upload_file_handler)
                .route_layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route(
            "/v1/files/{id}",
            get(crate::routes::get_file_metadata_handler),
        )
        .route(
            "/v1/files/{id}/content",
            get(crate::routes::get_file_content_handler),
        )
        .route(
            "/v1/files/{id}/open",
            post(crate::routes::open_file_handler),
        )
        // Chat routes (Phase 5.6)
        .route("/v1/chat", post(crate::routes::send_chat_handler))
        .route(
            "/v1/chat/history",
            get(crate::routes::get_chat_history_handler),
        )
        .route(
            "/v1/chat/history",
            delete(crate::routes::delete_chat_history_handler),
        )
        // Cross-platform conversation API (Phase 5.6)
        .route(
            "/v1/conversations",
            get(crate::routes::list_conversations_handler),
        )
        .route(
            "/v1/conversations/{id}/messages",
            get(crate::routes::get_conversation_messages_handler),
        )
        // Settings routes (Phase 5.5)
        .route("/v1/settings/llm", get(crate::routes::get_llm_settings))
        .route("/v1/settings/llm", put(crate::routes::upsert_key))
        .route(
            "/v1/settings/llm/keys/{provider}/{key_id}",
            delete(crate::routes::delete_key),
        )
        .route(
            "/v1/settings/llm/keys/reorder",
            put(crate::routes::reorder_keys),
        )
        .route(
            "/v1/settings/llm/keys/priority",
            put(crate::routes::set_key_priority),
        )
        .route(
            "/v1/settings/llm/validate",
            post(crate::routes::validate_key),
        )
        .route(
            "/v1/settings/llm/status",
            get(crate::routes::get_key_status),
        )
        // LLM Usage routes (Phase 5.5.5)
        .route("/v1/llm/usage", get(crate::routes::get_llm_usage))
        .route(
            "/v1/llm/usage/daily",
            get(crate::routes::get_llm_usage_daily),
        )
        // Pricing routes
        .route("/v1/llm/pricing", get(crate::routes::get_llm_pricing))
        .route(
            "/v1/llm/pricing/estimate",
            get(crate::routes::estimate_cost),
        )
        // Model discovery routes
        .route("/v1/models", get(crate::routes::list_models))
        .route("/v1/models/refresh", post(crate::routes::refresh_models))
        // Credential discovery routes
        .route(
            "/v1/settings/llm/credentials",
            get(crate::routes::get_discovered_credentials),
        )
        .route(
            "/v1/settings/llm/credentials/rescan",
            post(crate::routes::rescan_credentials),
        )
        // CLI backend routes
        .route(
            "/v1/settings/llm/cli-backends",
            get(crate::routes::get_cli_backends),
        )
        // Provider usage routes
        .route(
            "/v1/settings/llm/providers/usage",
            get(crate::routes::get_provider_usage),
        )
        // Orchestrator config routes (Phase 5.7)
        .route(
            "/v1/orchestrator/config",
            get(crate::routes::get_orchestrator_config),
        )
        .route(
            "/v1/orchestrator/config",
            put(crate::routes::update_orchestrator_config),
        )
        // Daemon config routes
        .route(
            "/v1/daemon/config/providers",
            get(crate::routes::get_daemon_providers),
        )
        .route(
            "/v1/daemon/config/providers/web-search",
            put(crate::routes::update_web_search_config),
        )
        // Memory routes
        .route("/v1/memory", get(crate::routes::list_memories_handler))
        .route("/v1/memory/reindex", post(crate::routes::reindex_handler))
        .route(
            "/v1/memory/status",
            get(crate::routes::index_status_handler),
        )
        .route(
            "/v1/memory/kb/ingest",
            post(crate::routes::kb_ingest_handler),
        )
        .route(
            "/v1/memory/{id}",
            get(crate::routes::get_memory_handler).delete(crate::routes::delete_memory_handler),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::auth_middleware,
        ));

    // WebSocket routes (token validated in handler via query param)
    let websocket = Router::new().route("/v1/events", get(crate::routes::events_handler));

    // SSE chat stream route (token validated inline via query param)
    let chat_sse = Router::new().route(
        "/v1/chat/stream/{stream_id}",
        get(crate::routes::chat_stream_handler),
    );

    // Merge all routes
    public
        .merge(protected_routes)
        .merge(websocket)
        .merge(chat_sse)
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// Health check endpoint (includes instance_id for validation)
/// No token required - minimal info only
async fn health_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "instance_id": state.instance_id
    }))
}

/// Root endpoint (basic info)
async fn root_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "OpenAlpaca Daemon",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
