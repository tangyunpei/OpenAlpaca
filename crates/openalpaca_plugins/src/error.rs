use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("manifest not found: {0}")]
    ManifestNotFound(String),
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("process spawn failed: {0}")]
    SpawnFailed(String),
    #[error("process crashed")]
    ProcessCrashed,
    #[error("timeout waiting for plugin response")]
    Timeout,
    #[error("plugin returned error: {code} {message}")]
    RpcError { code: i64, message: String },
    #[error("channel closed")]
    ChannelClosed,
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("plugin unavailable: {0}")]
    Unavailable(String),
    #[error("config missing required keys: {0:?}")]
    MissingConfig(Vec<String>),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
