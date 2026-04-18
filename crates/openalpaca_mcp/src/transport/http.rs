// crates/openalpaca_mcp/src/transport/http.rs

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use url::Url;

use crate::error::McpError;

use super::{HttpAuth, Transport, TransportConnection, TransportInner};

/// Streamable-HTTP transport (per MCP 2025 spec) for remote MCP servers.
pub struct StreamableHttpTransport {
    server_name: String,
    url: Url,
    auth: Option<HttpAuth>,
    extra_headers: HashMap<String, String>,
    request_timeout: Duration,
}

impl StreamableHttpTransport {
    pub fn new(server_name: impl Into<String>, url: Url) -> Self {
        Self {
            server_name: server_name.into(),
            url,
            auth: None,
            extra_headers: HashMap::new(),
            request_timeout: Duration::from_secs(30),
        }
    }

    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        self.auth = Some(HttpAuth::Bearer(token.into()));
        self
    }

    pub fn with_api_key(mut self, header: impl Into<String>, value: impl Into<String>) -> Self {
        self.auth = Some(HttpAuth::ApiKey {
            header: header.into(),
            value: value.into(),
        });
        self
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.insert(key.into(), value.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    fn build_headers(&self) -> Result<HeaderMap, McpError> {
        let mut headers = HeaderMap::new();

        match &self.auth {
            Some(HttpAuth::Bearer(token)) => {
                let val = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| McpError::HandshakeFailed(format!("invalid bearer token: {e}")))?;
                headers.insert(AUTHORIZATION, val);
            }
            Some(HttpAuth::ApiKey { header, value }) => {
                let name = HeaderName::from_bytes(header.as_bytes()).map_err(|e| {
                    McpError::HandshakeFailed(format!("invalid api key header name: {e}"))
                })?;
                let val = HeaderValue::from_str(value).map_err(|e| {
                    McpError::HandshakeFailed(format!("invalid api key value: {e}"))
                })?;
                headers.insert(name, val);
            }
            None => {}
        }

        for (k, v) in &self.extra_headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| {
                McpError::HandshakeFailed(format!("invalid header name '{k}': {e}"))
            })?;
            let val = HeaderValue::from_str(v).map_err(|e| {
                McpError::HandshakeFailed(format!("invalid header value for '{k}': {e}"))
            })?;
            headers.insert(name, val);
        }

        Ok(headers)
    }
}

#[async_trait]
impl Transport for StreamableHttpTransport {
    async fn connect(&self) -> Result<TransportConnection, McpError> {
        let headers = self.build_headers()?;
        let http_client = reqwest::Client::builder()
            .timeout(self.request_timeout)
            .default_headers(headers)
            .build()
            .map_err(|e| {
                McpError::Transport(std::io::Error::other(
                    format!("reqwest client build failed: {e}"),
                ))
            })?;

        tracing::info!(
            server_name = %self.server_name,
            url = %self.url,
            "connecting MCP streamable-http transport"
        );

        let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
            http_client,
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                self.url.to_string(),
            ),
        );

        Ok(TransportConnection {
            inner: TransportInner::Http(transport),
        })
    }

    fn kind(&self) -> &'static str {
        "streamable-http"
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_header_set() {
        let t = StreamableHttpTransport::new("srv", Url::parse("https://example.com/mcp").unwrap())
            .with_bearer("secret-token");
        let headers = t.build_headers().unwrap();
        let auth = headers.get(AUTHORIZATION).unwrap().to_str().unwrap();
        assert_eq!(auth, "Bearer secret-token");
    }

    #[test]
    fn api_key_header_set() {
        let t = StreamableHttpTransport::new("srv", Url::parse("https://example.com/mcp").unwrap())
            .with_api_key("X-API-Key", "abc123");
        let headers = t.build_headers().unwrap();
        assert_eq!(headers.get("X-API-Key").unwrap(), "abc123");
    }

    #[test]
    fn extra_headers_merged() {
        let t = StreamableHttpTransport::new("srv", Url::parse("https://example.com/mcp").unwrap())
            .with_bearer("t")
            .with_header("X-Client-Id", "oa-1")
            .with_header("X-Request-Id", "req-42");
        let headers = t.build_headers().unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer t");
        assert_eq!(headers.get("X-Client-Id").unwrap(), "oa-1");
        assert_eq!(headers.get("X-Request-Id").unwrap(), "req-42");
    }

    #[test]
    fn invalid_header_name_errors() {
        let t = StreamableHttpTransport::new("srv", Url::parse("https://example.com/mcp").unwrap())
            .with_header("invalid header", "x"); // space is invalid
        let err = t.build_headers().unwrap_err();
        assert!(matches!(err, McpError::HandshakeFailed(_)));
    }

    #[test]
    fn kind_and_name() {
        let t =
            StreamableHttpTransport::new("remote", Url::parse("https://example.com/mcp").unwrap());
        assert_eq!(t.kind(), "streamable-http");
        assert_eq!(t.server_name(), "remote");
    }
}
