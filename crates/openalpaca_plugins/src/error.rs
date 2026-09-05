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
    /// A JSON-RPC error object. `data` is carried verbatim because §4.2's
    /// plugin classification reads `error.data.reason == "needs_authorization"`
    /// and its optional `hint`; dropping it here is what made that branch of
    /// the classifier unreachable (extension design §4.2, §8 `hint`).
    #[error("plugin returned error: {code} {message}")]
    RpcError {
        code: i64,
        message: String,
        data: Option<serde_json::Value>,
    },
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
    /// `.permissions.toml` does not parse. Fail-closed: nothing loads, nothing
    /// is written, and every verb on an affected row is `409 store_unreadable`
    /// (extension design §4, §5.1).
    #[error("permissions store unreadable: {0}")]
    StoreUnreadable(String),
    /// Step W could not persist. No CAS was taken and nothing changed, so the
    /// route answers `500` and the row still reads what the disk says
    /// (extension design §3.2 W).
    #[error("extension store write failed: {0}")]
    StoreWriteFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
