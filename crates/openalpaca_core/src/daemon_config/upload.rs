use serde::{Deserialize, Serialize};

/// Configuration for file uploads and attachment handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadConfig {
    /// Maximum single file size in bytes (default: 50 MB).
    pub max_file_size_bytes: u64,
    /// Maximum total asset storage in bytes (default: 500 MB).
    pub max_total_storage_bytes: u64,
    /// Allowed MIME type prefixes for uploads.
    pub allowed_mime_prefixes: Vec<String>,
    /// Maximum files per chat message.
    pub max_files_per_message: usize,
    /// Governance sub-section for background processing & cleanup.
    pub governance: UploadGovernanceConfig,
}

/// Background processing and cleanup governance settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UploadGovernanceConfig {
    /// Seconds between file processing worker polls (default: 10).
    pub processing_poll_interval_secs: u64,
    /// Number of assets to process per batch (default: 5).
    pub processing_batch_size: usize,
    /// Maximum characters to keep from extracted text (default: 50,000).
    pub max_extracted_text_chars: usize,
    /// Hours between orphan cleanup runs (default: 6).
    pub cleanup_interval_hours: u64,
    /// Hours before an unlinked asset is considered orphaned (default: 24).
    pub orphan_grace_period_hours: u64,
    /// Maximum concurrent extraction tasks (default: 2).
    pub max_concurrent_extractions: usize,
    /// Number of times to retry a failed extraction before giving up (default: 1).
    pub extraction_retry_count: u32,
    /// Max time to wait for attachment processing before chat proceeds (default: 8000ms).
    pub attachment_ready_wait_ms: u64,
    /// Poll interval when waiting for attachment readiness (default: 200ms).
    pub attachment_ready_poll_interval_ms: u64,
    /// Maximum image dimension (width or height) in pixels (default: 8192).
    pub max_image_dimension: u32,
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            max_file_size_bytes: 50 * 1024 * 1024,
            max_total_storage_bytes: 500 * 1024 * 1024,
            allowed_mime_prefixes: vec![
                "image/".to_string(),
                "application/pdf".to_string(),
                "text/".to_string(),
                "audio/".to_string(),
                "application/msword".to_string(),
                "application/vnd.openxmlformats-officedocument.".to_string(),
                "application/vnd.ms-excel".to_string(),
                "application/vnd.ms-powerpoint".to_string(),
                "application/vnd.apple.".to_string(),
            ],
            max_files_per_message: 10,
            governance: UploadGovernanceConfig::default(),
        }
    }
}

impl Default for UploadGovernanceConfig {
    fn default() -> Self {
        Self {
            processing_poll_interval_secs: 10,
            processing_batch_size: 5,
            max_extracted_text_chars: 50_000,
            cleanup_interval_hours: 6,
            orphan_grace_period_hours: 24,
            max_concurrent_extractions: 2,
            extraction_retry_count: 1,
            attachment_ready_wait_ms: 8_000,
            attachment_ready_poll_interval_ms: 200,
            max_image_dimension: 8192,
        }
    }
}
