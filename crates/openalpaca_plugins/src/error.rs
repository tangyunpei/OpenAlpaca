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
    /// A load tried to replace the state of a plugin that still holds a live
    /// handle (a child process or a capability provider). Replacing it would
    /// orphan whatever the old entry held, so the load is refused instead.
    #[error("plugin '{0}' still holds a live handle; unload it before loading again")]
    HandleHeld(String),
    #[error("config missing required keys: {0:?}")]
    MissingConfig(Vec<String>),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
