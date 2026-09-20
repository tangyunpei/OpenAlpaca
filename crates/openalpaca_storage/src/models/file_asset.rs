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

/// One artifact a conversation message points at — the `role='artifact'` half
/// of `conversation_message_attachments`, joined to the file row (GAP-23).
///
/// Exactly the three fields a transcript chip needs: the id the Library opens,
/// the name it shows, and the kind its badge is drawn from. Everything else
/// about the file is a `GET /v1/artifacts/{id}` away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageArtifact {
    pub id: String,
    /// `file_assets.filename` — the head file's own name.
    pub name: String,
    /// The stored `ArtifactKind` spelling; `None` for a row that predates 036.
    pub kind: Option<String>,
}

/// Reference to a file attachment in a chat message request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub file_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}
