use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("invalid identifier: {0}")]
    InvalidId(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("port {port} is already in use")]
    PortInUse { port: u16 },

    #[error("lock error: {0}")]
    Lock(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
