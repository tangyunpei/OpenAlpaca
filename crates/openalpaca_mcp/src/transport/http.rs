// crates/openalpaca_mcp/src/transport/http.rs

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use url::{Origin, Url};

use crate::error::{McpError, RefusedRedirect};

use super::{HttpAuth, Transport, TransportConnection, TransportInner};

/// How long the TCP (and TLS) handshake may take before the server counts as
/// unreachable — the LLM clients' bound (`providers::build_http_client`), and
/// like theirs never longer than the caller's per-request budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The most hops a server without credentials may send a request through on
/// its own origin: reqwest's default limit.
const MAX_REDIRECT_HOPS: usize = 10;

/// Streamable-HTTP transport (per MCP 2025 spec) for remote MCP servers.
///
/// **Bounded by idleness, never by a total deadline.** The client it builds
/// has a connect bound and an idle bound — the longest it waits for the next
/// byte, reset by every byte — and nothing else, so a body is never cut while
/// bytes arrive: an SSE tool answer, the server-initiated GET stream. A
/// request's own deadline is the caller's ([`crate::McpClient`]'s
/// `request_timeout`). A total `reqwest` timeout runs until the body ends; the
/// 30 s one this client used to carry cut every streamed answer at 30 s,
/// whatever `request_timeout_secs` said.
///
/// **Redirects** follow the rule `refuse_redirect` states; a refused one is
/// [`McpError::RedirectRefused`], naming the origin it pointed at.
pub struct StreamableHttpTransport {
    server_name: String,
    url: Url,
    auth: Option<HttpAuth>,
    extra_headers: HashMap<String, String>,
    connect_timeout: Duration,
    idle_timeout: Duration,
}

impl StreamableHttpTransport {
    /// The idle bound unless a longer request timeout raises it. Long enough
    /// that a stream the server holds open with nothing to say is not taken
    /// for a dead one — cut, rmcp would re-open the GET stream and any event
    /// sent meanwhile would be lost — and short enough that a dead connection
    /// nobody waits on is noticed within minutes.
    pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

    pub fn new(server_name: impl Into<String>, url: Url) -> Self {
        Self {
            server_name: server_name.into(),
            url,
            auth: None,
            extra_headers: HashMap::new(),
            connect_timeout: CONNECT_TIMEOUT,
            idle_timeout: Self::DEFAULT_IDLE_TIMEOUT,
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

    /// The caller's per-request budget (`request_timeout_secs`). It sets no
    /// deadline here — the caller applies its own — but nothing is cut for
    /// silence before it: the idle bound is the longer of this and
    /// [`Self::DEFAULT_IDLE_TIMEOUT`]. The connect phase is held to the
    /// shorter of it and 30 s.
    pub fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.connect_timeout = CONNECT_TIMEOUT.min(request_timeout);
        self.idle_timeout = Self::DEFAULT_IDLE_TIMEOUT.max(request_timeout);
        self
    }

    /// The idle bound exactly — for the tests that show a stream outliving it
    /// several times over in seconds rather than minutes.
    #[cfg(test)]
    pub(crate) fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = idle_timeout;
        self
    }

    /// Whether every request carries something meant for this server alone:
    /// the credential and each extra header ride as default headers.
    fn carries_credentials(&self) -> bool {
        self.auth.is_some() || !self.extra_headers.is_empty()
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

/// The redirect rule (owner decision D6 (a), its third form). A hop is
/// refused when:
///
/// * it leaves the configured origin — scheme, host or port — whatever the
///   server: reqwest strips only `Authorization`, cookies and proxy
///   credentials on such a hop, so an API key under any other header name and
///   every `extra_headers` entry would follow it to the new origin (rmcp's
///   GHSA-9g45-5xwm-f3wc fixed the same leak in its own default client, which
///   this transport does not use);
/// * the server is given credentials or extra headers at all — even on its
///   own origin a redirect asks for them to be sent to an endpoint the owner
///   did not write down;
/// * the chain already holds [`MAX_REDIRECT_HOPS`] hops.
///
/// `previous` is reqwest's list of the URLs already requested in the chain,
/// the configured one first. The refusal names the target's origin, never
/// its path or query.
fn refuse_redirect(
    configured: &Origin,
    credentialed: bool,
    next: &Url,
    previous: &[Url],
) -> Option<RefusedRedirect> {
    let refused =
        credentialed || next.origin() != *configured || previous.len() > MAX_REDIRECT_HOPS;
    refused.then(|| RefusedRedirect {
        origin: origin_of(next),
    })
}

/// A URL's origin as the error names it: `scheme://host[:port]`, the port
/// left out when it is the scheme's default; for a URL with no such origin
/// (`data:`, `file:`), its scheme.
fn origin_of(url: &Url) -> String {
    match url.origin() {
        origin @ Origin::Tuple(..) => origin.ascii_serialization(),
        Origin::Opaque(_) => format!("{}:", url.scheme()),
    }
}

#[async_trait]
impl Transport for StreamableHttpTransport {
    async fn connect(&self) -> Result<TransportConnection, McpError> {
        let headers = self.build_headers()?;
        let configured = self.url.origin();
        let credentialed = self.carries_credentials();
        let redirects = reqwest::redirect::Policy::custom(move |attempt| {
            match refuse_redirect(&configured, credentialed, attempt.url(), attempt.previous()) {
                Some(refusal) => attempt.error(refusal),
                None => attempt.follow(),
            }
        });
        let http_client = reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .read_timeout(self.idle_timeout)
            .redirect(redirects)
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

    /// The configured request timeout only ever raises the idle bound, and
    /// only ever lowers the connect bound.
    #[test]
    fn a_request_timeout_raises_the_idle_bound_and_caps_the_connect_bound() {
        let url = || Url::parse("https://example.com/mcp").unwrap();
        let t = StreamableHttpTransport::new("srv", url());
        assert_eq!(
            t.idle_timeout,
            StreamableHttpTransport::DEFAULT_IDLE_TIMEOUT
        );
        assert_eq!(t.connect_timeout, CONNECT_TIMEOUT);

        let t = StreamableHttpTransport::new("srv", url()).with_request_timeout(secs(30));
        assert_eq!((t.idle_timeout, t.connect_timeout), (secs(300), secs(30)));

        let t = StreamableHttpTransport::new("srv", url()).with_request_timeout(secs(600));
        assert_eq!((t.idle_timeout, t.connect_timeout), (secs(600), secs(30)));

        let t = StreamableHttpTransport::new("srv", url()).with_request_timeout(secs(3));
        assert_eq!((t.idle_timeout, t.connect_timeout), (secs(300), secs(3)));
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// One row per arm of the redirect rule (D6 (a)), checked without a
    /// network: `previous` is the chain so far, the configured URL first.
    #[test]
    fn the_redirect_rule_refuses_another_origin_any_credentialed_hop_and_an_eleventh() {
        let configured = Url::parse("https://mcp.example.com/mcp").unwrap();
        let origin = configured.origin();
        let chain = |hops: usize| vec![configured.clone(); hops];
        let refused = |credentialed: bool, next: &str, previous: &[Url]| {
            refuse_redirect(&origin, credentialed, &Url::parse(next).unwrap(), previous)
                .map(|r| r.origin)
        };

        // Same origin, no credentials, within ten hops: followed.
        assert_eq!(
            refused(false, "https://mcp.example.com/v2/mcp", &chain(1)),
            None
        );
        assert_eq!(
            refused(false, "https://mcp.example.com/mcp", &chain(10)),
            None
        );
        // The eleventh hop.
        assert_eq!(
            refused(false, "https://mcp.example.com/mcp", &chain(11)).as_deref(),
            Some("https://mcp.example.com")
        );
        // Another origin, by host, port or scheme — http→https included.
        assert_eq!(
            refused(false, "https://other.example/mcp", &chain(1)).as_deref(),
            Some("https://other.example")
        );
        assert_eq!(
            refused(false, "https://mcp.example.com:8443/mcp", &chain(1)).as_deref(),
            Some("https://mcp.example.com:8443")
        );
        let plain = Url::parse("http://mcp.example.com/mcp").unwrap();
        assert_eq!(
            refuse_redirect(
                &plain.origin(),
                false,
                &Url::parse("https://mcp.example.com/mcp").unwrap(),
                std::slice::from_ref(&plain),
            )
            .map(|r| r.origin)
            .as_deref(),
            Some("https://mcp.example.com")
        );
        // Credentials or extra headers: any hop, its own origin included.
        assert_eq!(
            refused(true, "https://mcp.example.com/v2/mcp", &chain(1)).as_deref(),
            Some("https://mcp.example.com")
        );
        // The origin named never carries the path, the query or credentials
        // in the URL; a URL with no network origin is named by its scheme.
        assert_eq!(
            refused(false, "https://user:pw@other.example/p?token=t", &chain(1)).as_deref(),
            Some("https://other.example")
        );
        assert_eq!(
            refused(false, "data:text/plain,hi", &chain(1)).as_deref(),
            Some("data:")
        );
    }

    #[test]
    fn credentials_or_extra_headers_make_a_server_credentialed() {
        let url = || Url::parse("https://example.com/mcp").unwrap();
        assert!(!StreamableHttpTransport::new("srv", url()).carries_credentials());
        assert!(
            StreamableHttpTransport::new("srv", url())
                .with_bearer("t")
                .carries_credentials()
        );
        assert!(
            StreamableHttpTransport::new("srv", url())
                .with_api_key("X-API-Key", "k")
                .carries_credentials()
        );
        assert!(
            StreamableHttpTransport::new("srv", url())
                .with_header("X-Tenant", "t-1")
                .carries_credentials()
        );
    }
}
