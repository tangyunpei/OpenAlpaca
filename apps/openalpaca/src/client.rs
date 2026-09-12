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

    /// POST with JSON body, per-request headers, and JSON response.
    ///
    /// The one caller is chat: `x-workspace-path` is header-only on
    /// `POST /v1/chat`, and it must not become a default header on the client
    /// — every other route would then receive a project it never asked for.
    pub async fn post_with_headers<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        headers: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.http.post(&url).json(body);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let resp = check_response(request.send().await?).await?;
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

    /// PATCH with JSON body and JSON response.
    pub async fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.patch(&url).json(body).send().await?;
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

    /// POST /v1/files/upload — Upload a file via multipart form.
    pub async fn upload_file(&self, path: &std::path::Path) -> Result<serde_json::Value> {
        use reqwest::multipart;

        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());

        let data = tokio::fs::read(path)
            .await
            .with_context(|| format!("Failed to read file: {}", path.display()))?;

        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let part = multipart::Part::bytes(data)
            .file_name(filename)
            .mime_str(&mime)?;

        let form = multipart::Form::new().part("file", part);

        let url = format!("{}/v1/files/upload", self.base_url);
        let resp = self.http.post(&url).multipart(form).send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }
}

/// Check HTTP response status. On non-2xx, try to parse daemon JSON error.
async fn check_response(resp: Response) -> Result<Response> {
    if resp.status().is_success() {
        return Ok(resp);
    }

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    bail!("{}", render_error(status.as_u16(), &body));
}

/// What a refusal reads as on the terminal.
///
/// The daemon's canonical body is `{"error":{"code":…,"message":…}}`
/// (`routes::api_error`) and **both halves are load-bearing**: the sentence is
/// what a person reads, the code is the word the manuals and the routes name a
/// refusal by — `WORKSPACE_BUSY`, `SESSION_LANE_MISMATCH`, `WORKSPACE_NOT_A_ROOT`
/// — and the thing to search for. Printing only the sentence left a caller with
/// no way to tie what it read to what the manual documents, so the code is
/// printed beside it, in the same `code: message` shape the flat bodies below
/// already used.
fn render_error(status: u16, body: &serde_json::Value) -> String {
    // The structured format: { "error": { "code": "...", "message": "..." } }.
    // The code is optional in what this prints, not in what the daemon sends —
    // a body that carries only a message still renders as the sentence alone.
    if let Some(msg) = body["error"]["message"].as_str() {
        return match body["error"]["code"].as_str() {
            Some(code) if !code.is_empty() => format!("{code}: {msg} (HTTP {status})"),
            _ => format!("{msg} (HTTP {status})"),
        };
    }
    // The flat format: { "error": "..." }, optionally with a sibling
    // { "message": "..." }. The extension family answers a refusal as a word a
    // client branches on plus a sentence a person reads (GAP-24), and dropping
    // the sentence left `invalid_manifest` as the whole of what an operator was
    // told about a plugin that could not be installed.
    if let Some(msg) = body["error"].as_str() {
        return match body["message"].as_str() {
            Some(detail) => format!("{msg}: {detail} (HTTP {status})"),
            None => format!("{msg} (HTTP {status})"),
        };
    }

    format!("Request failed (HTTP {status})")
}

#[cfg(test)]
mod tests {
    use super::render_error;
    use serde_json::json;

    /// The canonical envelope every new route answers with. The code is what
    /// `docs/CLI_Manual.md` and the route comments call the refusal by, so it
    /// is printed, not dropped.
    #[test]
    fn a_structured_refusal_prints_its_code_beside_the_sentence() {
        let rendered = render_error(
            409,
            &json!({"error": {
                "code": "WORKSPACE_BUSY",
                "message": "2 run(s) under /repo are queued, running or paused",
            }}),
        );
        assert_eq!(
            rendered,
            "WORKSPACE_BUSY: 2 run(s) under /repo are queued, running or paused (HTTP 409)"
        );
    }

    /// One of the ~30 older `{"error":{"message":…}}` sites, and a code sent as
    /// an empty string: the sentence alone, never a bare `: ` with nothing
    /// before it.
    #[test]
    fn a_refusal_with_no_code_is_still_its_sentence() {
        assert_eq!(
            render_error(404, &json!({"error": {"message": "no such run"}})),
            "no such run (HTTP 404)"
        );
        assert_eq!(
            render_error(404, &json!({"error": {"code": "", "message": "no such run"}})),
            "no such run (HTTP 404)"
        );
    }

    /// The flat shape the extension routes answer with, both halves and one.
    #[test]
    fn a_flat_refusal_keeps_the_word_and_the_sentence() {
        assert_eq!(
            render_error(
                422,
                &json!({"error": "invalid_manifest", "message": "plugin.toml names no entry point"}),
            ),
            "invalid_manifest: plugin.toml names no entry point (HTTP 422)"
        );
        assert_eq!(
            render_error(422, &json!({"error": "invalid_manifest"})),
            "invalid_manifest (HTTP 422)"
        );
    }

    /// A body that is not JSON at all deserializes to `null`; the status is
    /// then the whole of what is known, and saying so beats inventing a reason.
    #[test]
    fn a_bodyless_failure_says_only_what_it_knows() {
        assert_eq!(
            render_error(500, &serde_json::Value::Null),
            "Request failed (HTTP 500)"
        );
    }
}
