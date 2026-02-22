//! Shared DaemonClient for CLI commands
//!
//! Wraps discovery.json reading + authenticated reqwest client.
//! All CLI commands that talk to the daemon should use this.

use anyhow::{Context, Result, bail};
use reqwest::Response;
use serde::{Serialize, de::DeserializeOwned};

use openalpaca_storage::discovery;

/// Authenticated HTTP client for the daemon.
pub struct DaemonClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl DaemonClient {
    /// Connect to the running daemon by reading discovery.json.
    pub fn connect() -> Result<Self> {
        let disc =
            discovery::read_discovery()?.context("Daemon is not running (no discovery file)")?;
        discovery::ensure_not_expired(&disc)?;
        let info = discovery::ConnectionInfo::from(&disc);

        let mut headers = reqwest::header::HeaderMap::new();
        let auth_value = format!("Bearer {}", info.token);
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&auth_value)?,
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self {
            base_url: info.base_url,
            token: info.token,
            http,
        })
    }

    #[allow(dead_code)]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// GET with JSON deserialization.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.get(&url).send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET returning raw Response.
    #[allow(dead_code)]
    pub async fn get_raw(&self, path: &str) -> Result<Response> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.get(&url).send().await?;
        check_response(resp).await
    }

    /// POST with JSON body and JSON response.
    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.post(&url).json(body).send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// POST returning raw Response.
    pub async fn post_raw<B: Serialize>(&self, path: &str, body: &B) -> Result<Response> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.post(&url).json(body).send().await?;
        check_response(resp).await
    }

    /// PUT with JSON body and JSON response.
    pub async fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.put(&url).json(body).send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// DELETE with JSON response.
    pub async fn delete_req<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.delete(&url).send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// GET for SSE streams (token passed as query parameter, not header).
    pub async fn get_sse_stream(&self, path_with_token: &str) -> Result<Response> {
        let url = format!("{}{}", self.base_url, path_with_token);
        let resp = self.http.get(&url).send().await?;
        check_response(resp).await
    }
}

/// Check HTTP response status. On non-2xx, try to parse daemon JSON error.
async fn check_response(resp: Response) -> Result<Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();

    // Try daemon's structured error format: { "error": { "message": "..." } }
    if let Some(msg) = body["error"]["message"].as_str() {
        bail!("{} (HTTP {})", msg, status.as_u16());
    }
    // Try flat format: { "error": "..." }
    if let Some(msg) = body["error"].as_str() {
        bail!("{} (HTTP {})", msg, status.as_u16());
    }

    bail!("Request failed (HTTP {})", status.as_u16());
}
