//! File upload and retrieval routes for multimodal chat
//!
//! POST /v1/files/upload   — Upload a file (multipart)
//! GET  /v1/files/{id}     — Get file metadata
//! GET  /v1/files/{id}/content — Stream file content
//! POST /v1/files/{id}/open — Open file with system default app

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
use std::path::{Path as FsPath, PathBuf};
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileOpenResponse {
    pub id: String,
    pub status: String,
}

type OpenFileFn = fn(&str, &str, &str) -> Result<(), String>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenFileApiError {
    NotFound,
    Db(String),
    OpenFailed(String),
}

fn sanitize_open_filename(filename: &str) -> String {
    let basename = FsPath::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment");
    let mut out: String = basename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out = out.trim().to_string();
    if out.is_empty() {
        "attachment".to_string()
    } else {
        out
    }
}

fn prepare_open_target_path(
    storage_path: &str,
    file_id: &str,
    filename: &str,
) -> Result<PathBuf, String> {
    let open_dir = std::env::temp_dir().join("openalpaca-open");
    std::fs::create_dir_all(&open_dir).map_err(|e| format!("Failed to prepare open dir: {e}"))?;
    let safe_name = sanitize_open_filename(filename);
    let target = open_dir.join(format!("{file_id}-{safe_name}"));
    if target.exists() {
        std::fs::remove_file(&target).map_err(|e| format!("Failed to refresh open target: {e}"))?;
    }
    std::fs::copy(storage_path, &target)
        .map_err(|e| format!("Failed to stage file for open: {e}"))?;
    Ok(target)
}

fn open_with_system_default(
    storage_path: &str,
    file_id: &str,
    filename: &str,
) -> Result<(), String> {
    let target = prepare_open_target_path(storage_path, file_id, filename)?;
    opener::open(target).map_err(|e| e.to_string())
}

async fn open_asset_for_user(
    db: &openalpaca_storage::Database,
    file_id: &str,
    local_user_id: &str,
    open_file_fn: OpenFileFn,
) -> Result<FileOpenResponse, OpenFileApiError> {
    let repo = FileAssetRepository::new(db);
    let asset = match repo.get_by_id(file_id) {
        Ok(Some(asset)) => {
            if asset.owner_id != local_user_id {
                tracing::debug!(
                    file_id = %file_id,
                    owner = %asset.owner_id,
                    "File owner mismatch — returning 404"
                );
                return Err(OpenFileApiError::NotFound);
            }
            asset
        }
        Ok(None) => return Err(OpenFileApiError::NotFound),
        Err(e) => return Err(OpenFileApiError::Db(e.to_string())),
    };

    let storage_path = asset.storage_path;
    let filename = asset.filename;
    let asset_id = asset.id;
    match tokio::task::spawn_blocking(move || open_file_fn(&storage_path, &asset_id, &filename))
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(OpenFileApiError::OpenFailed(e)),
        Err(e) => {
            return Err(OpenFileApiError::OpenFailed(format!(
                "Failed to join open task: {e}"
            )));
        }
    }

    Ok(FileOpenResponse {
        id: file_id.to_string(),
        status: "opened".to_string(),
    })
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

fn open_file_error_response(err: OpenFileApiError) -> axum::response::Response {
    match err {
        OpenFileApiError::NotFound => {
            error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found").into_response()
        }
        OpenFileApiError::Db(e) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", &e).into_response()
        }
        OpenFileApiError::OpenFailed(e) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "OPEN_FAILED", &e).into_response()
        }
    }
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

    let filename = field.file_name().unwrap_or("unnamed").to_string();
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
        Ok(None) => {
            error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "File not found").into_response()
        }
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

/// POST /v1/files/{id}/open — Open file with system default app
pub async fn open_file_handler(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match open_asset_for_user(
        &state.db,
        &id,
        &state.local_user_id,
        open_with_system_default,
    )
    .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => open_file_error_response(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::Database;
    use tempfile::TempDir;

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

    fn open_ok(_path: &str, _file_id: &str, _filename: &str) -> Result<(), String> {
        Ok(())
    }

    fn open_fail(_path: &str, _file_id: &str, _filename: &str) -> Result<(), String> {
        Err("open failed".to_string())
    }

    fn test_db() -> (TempDir, Database) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (dir, db)
    }

    fn sample_asset(id: &str, owner_id: &str) -> FileAsset {
        FileAsset {
            id: id.to_string(),
            owner_id: owner_id.to_string(),
            sha256: "sha".to_string(),
            filename: "file.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size_bytes: 123,
            storage_path: "/tmp/does-not-matter.pdf".to_string(),
            status: FileAssetStatus::Ready,
            extracted_text: None,
            extract_error: None,
            metadata_json: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn test_open_asset_for_user_not_found() {
        let (_dir, db) = test_db();
        let err = open_asset_for_user(&db, "missing", "user1", open_ok)
            .await
            .expect_err("missing file should return error");
        assert_eq!(err, OpenFileApiError::NotFound);
    }

    #[tokio::test]
    async fn test_open_asset_for_user_owner_mismatch_returns_not_found() {
        let (_dir, db) = test_db();
        let repo = FileAssetRepository::new(&db);
        repo.insert(&sample_asset("f1", "other-user"))
            .expect("insert asset");

        let err = open_asset_for_user(&db, "f1", "user1", open_ok)
            .await
            .expect_err("owner mismatch should return not found");
        assert_eq!(err, OpenFileApiError::NotFound);
    }

    #[tokio::test]
    async fn test_open_asset_for_user_open_failed_returns_open_failed() {
        let (_dir, db) = test_db();
        let repo = FileAssetRepository::new(&db);
        repo.insert(&sample_asset("f2", "user1"))
            .expect("insert asset");

        let err = open_asset_for_user(&db, "f2", "user1", open_fail)
            .await
            .expect_err("open failure should be surfaced");
        assert_eq!(err, OpenFileApiError::OpenFailed("open failed".to_string()));
    }

    #[tokio::test]
    async fn test_open_asset_for_user_success_returns_opened_status() {
        let (_dir, db) = test_db();
        let repo = FileAssetRepository::new(&db);
        repo.insert(&sample_asset("f3", "user1"))
            .expect("insert asset");

        let resp = open_asset_for_user(&db, "f3", "user1", open_ok)
            .await
            .expect("open should succeed");
        assert_eq!(
            resp,
            FileOpenResponse {
                id: "f3".to_string(),
                status: "opened".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn test_open_file_error_response_not_found_uses_404() {
        let response = open_file_error_response(OpenFileApiError::NotFound);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("parse json");
        assert_eq!(payload["error"]["code"], "NOT_FOUND");
    }

    #[tokio::test]
    async fn test_open_file_error_response_open_failed_uses_500_and_code() {
        let response = open_file_error_response(OpenFileApiError::OpenFailed("boom".to_string()));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("parse json");
        assert_eq!(payload["error"]["code"], "OPEN_FAILED");
    }

    #[test]
    fn test_prepare_open_target_path_keeps_original_extension() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let source = dir.path().join("source.bin");
        std::fs::write(&source, b"hello").expect("write source");
        let target = prepare_open_target_path(
            source.to_str().expect("source path"),
            "file-1",
            "Resume 2026.docx",
        )
        .expect("prepare target");
        assert!(target.to_string_lossy().ends_with(".docx"));
        let copied = std::fs::read(&target).expect("read copied");
        assert_eq!(copied, b"hello");
        let _ = std::fs::remove_file(&target);
    }
}
