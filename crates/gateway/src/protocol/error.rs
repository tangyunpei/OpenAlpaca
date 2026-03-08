use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorShape {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl ErrorShape {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            code: "NOT_FOUND".into(),
            message: msg.into(),
            retryable: None,
            retry_after_ms: None,
        }
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: "INVALID_PARAMS".into(),
            message: msg.into(),
            retryable: None,
            retry_after_ms: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: "INTERNAL".into(),
            message: msg.into(),
            retryable: Some(true),
            retry_after_ms: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: "METHOD_NOT_FOUND".into(),
            message: format!("unknown method: {method}"),
            retryable: None,
            retry_after_ms: None,
        }
    }

    pub fn auth_failed(msg: impl Into<String>) -> Self {
        Self {
            code: "AUTH_FAILED".into(),
            message: msg.into(),
            retryable: None,
            retry_after_ms: None,
        }
    }

    pub fn rate_limited(retry_after_ms: u64) -> Self {
        Self {
            code: "RATE_LIMITED".into(),
            message: "too many requests".into(),
            retryable: Some(true),
            retry_after_ms: Some(retry_after_ms),
        }
    }
}
