use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChannelError {
    #[error("operation not supported: {0}")]
    NotSupported(String),

    #[error("channel API error: {0}")]
    Api(#[from] reqwest::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
