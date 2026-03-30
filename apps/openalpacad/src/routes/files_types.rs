//! Types and validation helpers for file upload/retrieval routes.

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use std::path::{Path as FsPath, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MimeMagicValidationError {
    Mismatch { detected: String },
    Undetectable,
}

pub(super) fn validate_magic_mime(
    declared_mime: &str,
    data: &[u8],
) -> Result<(), MimeMagicValidationError> {
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
            // Allow audio format aliases (e.g. audio/mp4 vs audio/m4a)
            if InputSanitizer::is_audio_mime_compatible(declared_mime, detected_type) {
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

pub(super) type OpenFileFn = fn(&str, &str, &str) -> Result<(), String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OpenFileApiError {
    NotFound,
    Db(String),
    OpenFailed(String),
}

pub(super) fn sanitize_open_filename(filename: &str) -> String {
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

pub(super) fn prepare_open_target_path(
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

pub(super) fn open_with_system_default(
    storage_path: &str,
    file_id: &str,
    filename: &str,
) -> Result<(), String> {
    let target = prepare_open_target_path(storage_path, file_id, filename)?;
    opener::open(target).map_err(|e| e.to_string())
}

pub(super) async fn open_asset_for_user(
    db: &openalpaca_storage::Database,
    file_id: &str,
    local_user_id: &str,
    open_file_fn: OpenFileFn,
) -> Result<FileOpenResponse, OpenFileApiError> {
    let repo = openalpaca_storage::FileAssetRepository::new(db);
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
pub(super) struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Serialize)]
pub(super) struct ErrorDetail {
    pub code: String,
    pub message: String,
}

pub(super) fn error_response(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
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

pub(super) fn open_file_error_response(err: OpenFileApiError) -> axum::response::Response {
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
