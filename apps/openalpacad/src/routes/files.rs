//! File upload and retrieval routes for multimodal chat
//!
//! POST /v1/files/upload   — Upload a file (multipart)
//! GET  /v1/files/{id}     — Get file metadata
//! GET  /v1/files/{id}/content — Stream file content

use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use openalpaca_storage::{FileAsset, FileAssetRepository, FileAssetStatus};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::io::ReaderStream;

use crate::AppState;

#[derive(Serialize)]
pub struct FileUploadResponse {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub status: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

fn error_response(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
    (
        status,
        Json(ErrorResponse {
            error: ErrorDetail {
                code: code.to_string(),
                message: message.to_string(),
            },
        }),
    )
}

/// POST /v1/files/upload — Multipart file upload
pub async fn upload_file_handler(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let config = state.daemon_config.load();

    // Extract file field from multipart
    let field = match multipart.next_field().await {
        Ok(Some(field)) => field,
        Ok(None) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "NO_FILE",
                "No file field in multipart request",
            )
            .into_response();
        }
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "MULTIPART_ERROR",
                &format!("Failed to read multipart: {e}"),
            )
            .into_response();
        }
    };

    let filename = field
        .file_name()
        .unwrap_or("unnamed")
        .to_string();
    let content_type = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_string();

    // Read file data
    let data = match field.bytes().await {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "READ_ERROR",
                &format!("Failed to read file data: {e}"),
            )
            .into_response();
        }
    };

    // Check file size
    if data.len() as u64 > config.upload.max_file_size_bytes {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "FILE_TOO_LARGE",
            &format!(
                "File exceeds maximum size of {} bytes",
                config.upload.max_file_size_bytes
            ),
        )
        .into_response();
    }

    // Check total storage quota
    let repo_check = FileAssetRepository::new(&state.db);
    if let Ok(total_used) = repo_check.total_storage_bytes() {
        if total_used as u64 + data.len() as u64 > config.upload.max_total_storage_bytes {
            return error_response(
                StatusCode::INSUFFICIENT_STORAGE,
                "STORAGE_QUOTA_EXCEEDED",
                &format!(
                    "Total storage quota ({} bytes) would be exceeded",
                    config.upload.max_total_storage_bytes
                ),
            )
            .into_response();
        }
    }

    // MIME prefix validation
    let mime_allowed = config
        .upload
        .allowed_mime_prefixes
        .iter()
        .any(|prefix| content_type.starts_with(prefix));
    if !mime_allowed {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "UNSUPPORTED_MIME",
            &format!("MIME type '{}' is not allowed", content_type),
        )
        .into_response();
    }

    // Magic bytes validation via `infer` crate.
    // Note: text/* types are excluded because text files lack reliable magic bytes.
    // An attacker could upload binary data as text/plain — the risk is accepted since
    // the file is stored as-is and only served back with its declared Content-Type.
    if let Some(detected) = infer::get(&data) {
        let detected_type = detected.mime_type();
        if !content_type.starts_with("text/") {
            let declared_cat = content_type.split('/').next().unwrap_or("");
            let detected_cat = detected_type.split('/').next().unwrap_or("");

            if declared_cat != detected_cat {
                // Cross-category mismatch — definite spoofing attempt
                return error_response(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "MIME_MISMATCH",
                    &format!(
                        "Declared MIME '{}' doesn't match detected '{}'",
                        content_type, detected_type
                    ),
                )
                .into_response();
            }

            if content_type != detected_type && detected_type != "application/octet-stream" {
                // Same category but different subtype — log but allow
                // (infer's subtype detection isn't reliable for all formats)
                tracing::warn!(
                    declared = %content_type,
                    detected = %detected_type,
                    "MIME subtype mismatch (same category, allowing)"
                );
            }
        }
    }

    // Compute SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let sha256 = format!("{:x}", hasher.finalize());

    let repo = FileAssetRepository::new(&state.db);

    // Dedup: check if file with same hash already exists and is owned by this user
    if let Ok(Some(existing)) = repo.get_by_sha256(&sha256) {
        if existing.owner_id == state.local_user_id {
            return Json(FileUploadResponse {
                id: existing.id,
                filename: existing.filename,
                mime_type: existing.mime_type,
                size_bytes: existing.size_bytes,
                status: existing.status.as_str().to_string(),
            })
            .into_response();
        }
        // Same content but different owner — fall through to create a new record
    }

    // Compute storage path
    let storage_path = match openalpaca_storage::paths::asset_storage_path(&sha256) {
        Ok(p) => p,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PATH_ERROR",
                &format!("Failed to compute storage path: {e}"),
            )
            .into_response();
        }
    };

    // Create parent directories and write file (async to avoid blocking executor)
    if let Some(parent) = storage_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "IO_ERROR",
                &format!("Failed to create storage directory: {e}"),
            )
            .into_response();
        }
    }
    if let Err(e) = tokio::fs::write(&storage_path, &data).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "IO_ERROR",
            &format!("Failed to write file: {e}"),
        )
        .into_response();
    }

    // Insert into database
    let id = uuid::Uuid::new_v4().to_string();
    let asset = FileAsset {
        id: id.clone(),
        owner_id: state.local_user_id.clone(),
        sha256,
        filename: filename.clone(),
        mime_type: content_type.clone(),
        size_bytes: data.len() as i64,
        storage_path: storage_path.to_string_lossy().to_string(),
        status: FileAssetStatus::Uploaded,
        extracted_text: None,
        extract_error: None,
        metadata_json: None,
        created_at: String::new(),
        updated_at: String::new(),
    };

    if let Err(e) = repo.insert(&asset) {
        // Clean up written file on DB error
        let _ = tokio::fs::remove_file(&storage_path).await;
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            &format!("Failed to insert file record: {e}"),
        )
        .into_response();
    }

    Json(FileUploadResponse {
        id,
        filename,
        mime_type: content_type,
        size_bytes: data.len() as i64,
        status: "uploaded".to_string(),
    })
    .into_response()
}

/// GET /v1/files/{id} — Get file metadata
pub async fn get_file_metadata_handler(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let repo = FileAssetRepository::new(&state.db);
    match repo.get_by_id(&id) {
        Ok(Some(asset)) => {
            if asset.owner_id != state.local_user_id {
                tracing::debug!(file_id = %id, owner = %asset.owner_id, "File owner mismatch — returning 404");
                return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found")
                    .into_response();
            }
            Json(asset).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found")
            .into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            &e.to_string(),
        )
        .into_response(),
    }
}

/// GET /v1/files/{id}/content — Stream file content
pub async fn get_file_content_handler(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let repo = FileAssetRepository::new(&state.db);
    let asset = match repo.get_by_id(&id) {
        Ok(Some(a)) => {
            if a.owner_id != state.local_user_id {
                tracing::debug!(file_id = %id, owner = %a.owner_id, "File owner mismatch — returning 404");
                return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found")
                    .into_response();
            }
            a
        }
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found")
                .into_response();
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                &e.to_string(),
            )
            .into_response();
        }
    };

    let file = match tokio::fs::File::open(&asset.storage_path).await {
        Ok(f) => f,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "IO_ERROR",
                &format!("Failed to open file: {e}"),
            )
            .into_response();
        }
    };

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    if let Ok(ct) = asset.mime_type.parse() {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    // Sanitize filename to prevent Content-Disposition header injection
    let safe_filename: String = asset
        .filename
        .chars()
        .filter(|c| *c != '"' && *c != '\\' && *c != '\r' && *c != '\n')
        .collect();
    if let Ok(cd) = format!("inline; filename=\"{}\"", safe_filename).parse() {
        headers.insert(header::CONTENT_DISPOSITION, cd);
    }

    (headers, body).into_response()
}
