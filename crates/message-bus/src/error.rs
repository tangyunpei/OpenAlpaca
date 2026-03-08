use thiserror::Error;

#[derive(Debug, Error)]
pub enum MessageBusError {
    #[error("routing error: {0}")]
    Routing(String),

    #[error("dispatch error: {0}")]
    Dispatch(String),

    #[error("delivery error: {0}")]
    Delivery(String),
}
