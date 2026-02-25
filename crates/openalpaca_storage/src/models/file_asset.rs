//! File asset model for multimodal chat attachments

use serde::{Deserialize, Serialize};

/// Status of a file asset in the processing pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAssetStatus {
    Uploaded,
    Processing,
    Ready,
    Error,
}

impl FileAssetStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Uploaded => "uploaded",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "processing" => Self::Processing,
            "ready" => Self::Ready,
            "error" => Self::Error,
            _ => Self::Uploaded,
        }
    }
}

/// A stored file asset (image, document, audio).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAsset {
    pub id: String,
    pub owner_id: String,
    pub sha256: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub storage_path: String,
    pub status: FileAssetStatus,
    pub extracted_text: Option<String>,
    pub extract_error: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Reference to a file attachment in a chat message request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}
