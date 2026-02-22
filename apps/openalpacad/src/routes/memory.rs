//! Memory endpoints
//!
//! GET    /v1/memory            -> list memories (paginated)
//! GET    /v1/memory/{id}       -> get a single memory
//! DELETE /v1/memory/{id}       -> delete a memory
//! POST   /v1/memory/reindex    -> trigger embedding reindex
//! GET    /v1/memory/status     -> return embedding stats
//! POST   /v1/memory/kb/ingest  -> ingest KB from directory

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use openalpaca_storage::MemoryRepository;
use openalpaca_storage::models::memory::MemoryKind;

use crate::AppState;

// ── Request / Response Types ─────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct KbIngestRequest {
    pub path: String,
    pub scope_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListMemoriesQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListMemoriesResponse {
    pub items: Vec<openalpaca_storage::models::memory::MemoryV2>,
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}

// ── CRUD Handlers ────────────────────────────────────────────────

/// GET /v1/memory
/// List memories for the local user with pagination and optional kind filter.
pub async fn list_memories_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListMemoriesQuery>,
) -> impl IntoResponse {
    let repo = MemoryRepository::new(&state.db);
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);

    let kind_filter: Option<MemoryKind> = match query.kind.as_deref() {
        Some(k) => match k.parse::<MemoryKind>() {
            Ok(kf) => Some(kf),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        },
        None => None,
    };

    match repo.list_paginated(&state.local_user_id, limit, offset, kind_filter) {
        Ok((items, total)) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(ListMemoriesResponse {
                    items,
                    total,
                    limit,
                    offset,
                })
                .unwrap(),
            ),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /v1/memory/{id}
/// Get a single memory by ID.
pub async fn get_memory_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let repo = MemoryRepository::new(&state.db);
    match repo.get(id) {
        Ok(Some(mem)) => (StatusCode::OK, Json(serde_json::to_value(mem).unwrap())),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Memory {} not found", id) })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// DELETE /v1/memory/{id}
/// Delete a single memory by ID (including its vector embedding).
pub async fn delete_memory_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let repo = MemoryRepository::new(&state.db);
    match repo.delete(id) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "deleted", "id": id })),
        ),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Memory {} not found", id) })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

// ── Admin Handlers ───────────────────────────────────────────────

/// POST /v1/memory/reindex
/// Trigger embedding reindex for the local user's memories.
pub async fn reindex_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let embedder = match &state.embedder {
        Some(e) => e.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "Embedder not configured" })),
            );
        }
    };

    let repo = MemoryRepository::new(&state.db);
    let missing = match repo.list_missing_embeddings(&state.local_user_id, 200) {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    };

    if missing.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "indexed": 0, "message": "No missing embeddings" })),
        );
    }

    let texts: Vec<&str> = missing.iter().map(|(_, c)| c.as_str()).collect();
    match embedder.embed(&texts).await {
        Ok(embeddings) => {
            let mut indexed = 0usize;
            for ((id, _), embedding) in missing.iter().zip(embeddings.iter()) {
                if embedding.len() == embedder.dimensions() as usize
                    && repo.insert_embedding(*id, embedding).is_ok()
                {
                    indexed += 1;
                }
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "indexed": indexed })),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Embedding failed: {e}") })),
        ),
    }
}

/// GET /v1/memory/status
/// Return embedding statistics for the local user.
pub async fn index_status_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let repo = MemoryRepository::new(&state.db);
    match repo.embedding_stats(&state.local_user_id) {
        Ok((total, embedded)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "total": total,
                "embedded": embedded,
                "pending": total - embedded,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// POST /v1/memory/kb/ingest
/// Ingest KB from a specified directory path.
pub async fn kb_ingest_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<KbIngestRequest>,
) -> impl IntoResponse {
    let dir = std::path::Path::new(&request.path);
    if !dir.is_absolute() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Path must be absolute" })),
        );
    }
    if !dir.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Not a directory: {}", request.path) })),
        );
    }

    let scope_id = request.scope_id.unwrap_or_else(|| request.path.clone());

    match openalpaca_core::memory::kb::ingest_directory(
        &state.db,
        &state.local_user_id,
        dir,
        &scope_id,
    ) {
        Ok(result) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "chunks_added": result.chunks_added,
                "chunks_skipped": result.chunks_skipped,
                "files_processed": result.files_processed,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}
