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

#[derive(Debug, Clone, PartialEq, Eq)]
enum MimeMagicValidationError {
    Mismatch { detected: String },
    Undetectable,
}

fn validate_magic_mime(declared_mime: &str, data: &[u8]) -> Result<(), MimeMagicValidationError> {
    use openalpaca_core::security::sanitizer::InputSanitizer;
    match infer::get(data) {
        Some(detected) => {
            let detected_type = detected.mime_type();
            if declared_mime == detected_type {
                return Ok(());
            }
            // Allow container-based formats where detection returns the container type
            if InputSanitizer::is_container_compatible_mime(declared_mime, detected_type) {
                return Ok(());
            }
            Err(MimeMagicValidationError::Mismatch {
                detected: detected_type.to_string(),
            })
        }
        None => {
            if declared_mime.starts_with("text/") {
                Ok(())
            } else {
                Err(MimeMagicValidationError::Undetectable)
            }
        }
    }
}

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
    if let Ok(total_used) = repo_check.total_storage_bytes()
        && total_used as u64 + data.len() as u64 > config.upload.max_total_storage_bytes
    {
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
    // Strict mode: when a type is detected, it must exactly match the declared MIME.
    // Text/* is only exempt when detection fails entirely.
    match validate_magic_mime(&content_type, &data) {
        Ok(()) => {}
        Err(MimeMagicValidationError::Mismatch { detected }) => {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "MIME_MISMATCH",
                &format!(
                    "Declared MIME '{}' doesn't match detected '{}'",
                    content_type, detected
                ),
            )
            .into_response();
        }
        Err(MimeMagicValidationError::Undetectable) => {
            if !content_type.starts_with("text/") {
                return error_response(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "MIME_UNDETECTABLE",
                    &format!(
                        "Could not detect file type from content for declared MIME '{}'",
                        content_type
                    ),
                )
                .into_response();
            }
        }
    }

    // Archive bomb + image dimension checks
    {
        use openalpaca_core::security::sanitizer::InputSanitizer;
        let max_img_dim = config.upload.governance.max_image_dimension;
        if let Err(violation) = InputSanitizer::validate_upload_with_image_limit(
            &filename,
            &data,
            &content_type,
            config.upload.max_file_size_bytes,
            max_img_dim,
        ) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "UPLOAD_VALIDATION_FAILED",
                &format!("{violation}"),
            )
            .into_response();
        }
    }

    // Compute SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let sha256 = format!("{:x}", hasher.finalize());

    let repo = FileAssetRepository::new(&state.db);

    // Dedup: check if file with same hash already exists and is owned by this user
    if let Ok(Some(existing)) = repo.get_by_sha256(&sha256)
        && existing.owner_id == state.local_user_id
    {
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
    if let Some(parent) = storage_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "IO_ERROR",
            &format!("Failed to create storage directory: {e}"),
        )
        .into_response();
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

#[cfg(test)]
mod tests {
    use super::*;

    // Real file signatures to exercise infer-based MIME detection.
    const JPEG_BYTES: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00,
    ];
    const ZIP_BYTES: &[u8] = &[0x50, 0x4B, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00];
    const CFB_BYTES: &[u8] = &[
        0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0x00, 0x00, 0x00, 0x00,
    ];
    const UNDETECTABLE_BYTES: &[u8] = &[0x01, 0x02, 0x03, 0x04, 0x05];

    #[test]
    fn test_validate_magic_mime_rejects_image_subtype_mismatch() {
        let err = validate_magic_mime("image/png", JPEG_BYTES).expect_err("must reject mismatch");
        match err {
            MimeMagicValidationError::Mismatch { detected } => {
                assert_eq!(detected, "image/jpeg");
            }
            other => panic!("expected mismatch error, got {other:?}"),
        }
    }

    #[test]
    fn test_validate_magic_mime_rejects_pdf_vs_zip_mismatch() {
        let err =
            validate_magic_mime("application/pdf", ZIP_BYTES).expect_err("must reject mismatch");
        match err {
            MimeMagicValidationError::Mismatch { detected } => {
                assert_eq!(detected, "application/zip");
            }
            other => panic!("expected mismatch error, got {other:?}"),
        }
    }

    #[test]
    fn test_validate_magic_mime_allows_undetectable_text() {
        let result = validate_magic_mime("text/plain", UNDETECTABLE_BYTES);
        assert!(result.is_ok(), "text/* should be allowed when undetectable");
    }

    #[test]
    fn test_validate_magic_mime_rejects_undetectable_non_text() {
        let err = validate_magic_mime("application/pdf", UNDETECTABLE_BYTES)
            .expect_err("non-text undetectable should be rejected");
        assert_eq!(err, MimeMagicValidationError::Undetectable);
    }

    // --- Office/iWork container-compatible MIME tests ---

    #[test]
    fn test_validate_magic_mime_allows_docx_as_zip() {
        let result = validate_magic_mime(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ZIP_BYTES,
        );
        assert!(result.is_ok(), "DOCX (ZIP container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_allows_xlsx_as_zip() {
        let result = validate_magic_mime(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ZIP_BYTES,
        );
        assert!(result.is_ok(), "XLSX (ZIP container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_allows_pptx_as_zip() {
        let result = validate_magic_mime(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ZIP_BYTES,
        );
        assert!(result.is_ok(), "PPTX (ZIP container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_allows_pages_as_zip() {
        let result = validate_magic_mime("application/vnd.apple.pages", ZIP_BYTES);
        assert!(result.is_ok(), "Pages (ZIP container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_allows_numbers_as_zip() {
        let result = validate_magic_mime("application/vnd.apple.numbers", ZIP_BYTES);
        assert!(result.is_ok(), "Numbers (ZIP container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_allows_keynote_as_zip() {
        let result = validate_magic_mime("application/vnd.apple.keynote", ZIP_BYTES);
        assert!(result.is_ok(), "Keynote (ZIP container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_rejects_random_type_as_zip() {
        let err = validate_magic_mime("application/octet-stream", ZIP_BYTES)
            .expect_err("unknown type should not be container-compatible");
        match err {
            MimeMagicValidationError::Mismatch { detected } => {
                assert_eq!(detected, "application/zip");
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn test_validate_magic_mime_allows_doc_as_cfb() {
        let result = validate_magic_mime("application/msword", CFB_BYTES);
        assert!(result.is_ok(), "DOC (CFB container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_allows_xls_as_cfb() {
        let result = validate_magic_mime("application/vnd.ms-excel", CFB_BYTES);
        assert!(result.is_ok(), "XLS (CFB container) should be allowed");
    }

    #[test]
    fn test_validate_magic_mime_allows_ppt_as_cfb() {
        let result = validate_magic_mime("application/vnd.ms-powerpoint", CFB_BYTES);
        assert!(result.is_ok(), "PPT (CFB container) should be allowed");
    }
}
