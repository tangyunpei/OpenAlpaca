use async_trait::async_trait;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct WebFetchTool;

#[async_trait]
impl BuiltInTool for WebFetchTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let url = arguments
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: url".to_string())?;

        // SSRF protection: validate URL before fetching
        validate_fetch_url(url)?;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;
        let response = client
            .get(url)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch URL: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("HTTP error: {}", status));
        }

        // Reject non-text content types (images, executables, etc.)
        if let Some(ct) = response.headers().get(reqwest::header::CONTENT_TYPE) {
            if let Ok(ct_str) = ct.to_str() {
                let ct_lower = ct_str.to_lowercase();
                let is_text = ct_lower.starts_with("text/")
                    || ct_lower.starts_with("application/json")
                    || ct_lower.starts_with("application/xml")
                    || ct_lower.starts_with("application/xhtml")
                    || ct_lower.starts_with("application/javascript")
                    || ct_lower.starts_with("application/x-yaml")
                    || ct_lower.starts_with("application/yaml");
                if !is_text {
                    return Err(format!(
                        "Non-text content type '{}': web_fetch only supports text-based responses",
                        ct_str
                    ));
                }
            }
        }

        // Limit download size: read up to 1MB then truncate output to 8KB.
        // Prevents downloading multi-GB files into memory.
        const MAX_DOWNLOAD_SIZE: usize = 1024 * 1024; // 1 MB
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            async {
                let mut bytes = Vec::new();
                let mut stream = response.bytes_stream();
                use futures_util::StreamExt;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|e| format!("Failed to read response: {}", e))?;
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() > MAX_DOWNLOAD_SIZE {
                        break;
                    }
                }
                String::from_utf8(bytes)
                    .map_err(|_| "Response body is not valid UTF-8".to_string())
            },
        )
        .await
        .map_err(|_| "Response body read timed out after 15s".to_string())?
        .map_err(|e: String| e)?;

        // Truncate output to 8KB (char-boundary-safe)
        let mut end = 8192.min(body.len());
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        Ok(body[..end].to_string())
    }
}

/// Validate a URL before fetching to prevent SSRF attacks.
/// Blocks:
/// - Non-HTTP(S) schemes (file://, ftp://, etc.)
/// - Cloud metadata endpoints (169.254.169.254, metadata.google.internal, etc.)
/// - Private/reserved IP ranges (127.x, 10.x, 172.16-31.x, 192.168.x, [::1])
/// - Localhost variations
fn validate_fetch_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url)
        .map_err(|e| format!("Invalid URL: {}", e))?;

    // Only allow http and https
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => return Err(format!(
            "URL scheme '{}' is not allowed; only http and https are permitted", scheme
        )),
    }

    let host = parsed.host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // Block cloud metadata endpoints
    let blocked_hosts = [
        "169.254.169.254",           // AWS/GCP/Azure metadata
        "metadata.google.internal",  // GCP metadata
        "metadata.internal",         // Generic cloud metadata
    ];
    let host_lower = host.to_lowercase();
    for blocked in &blocked_hosts {
        if host_lower == *blocked {
            return Err(format!(
                "Access to '{}' is blocked (cloud metadata endpoint)", host
            ));
        }
    }

    // Block localhost variants
    let localhost_patterns = ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"];
    for pattern in &localhost_patterns {
        if host_lower == *pattern {
            return Err(format!("Access to '{}' is blocked (localhost)", host));
        }
    }

    // Block private IP ranges
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let is_private = match ip {
            std::net::IpAddr::V4(ipv4) => {
                ipv4.is_loopback()                          // 127.0.0.0/8
                    || ipv4.is_private()                    // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                    || ipv4.is_link_local()                 // 169.254.0.0/16
                    || ipv4.is_unspecified()                // 0.0.0.0
                    || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xC0) == 64)  // 100.64.0.0/10 (CGN)
            }
            std::net::IpAddr::V6(ipv6) => {
                ipv6.is_loopback()                          // ::1
                    || ipv6.is_unspecified()                 // ::
            }
        };
        if is_private {
            return Err(format!(
                "Access to private/reserved IP '{}' is blocked", ip
            ));
        }
    }

    Ok(())
}

pub(super) fn web_fetch_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch content from a URL".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(WebFetchTool)),
    }
}
