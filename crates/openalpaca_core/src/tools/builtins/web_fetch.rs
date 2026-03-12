use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
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
        crate::tools::url_validation::validate_url(url)?;

        // Custom redirect policy: validate each redirect target via SSRF checks.
        // The default `Policy::limited(5)` follows redirects blindly, allowing a
        // malicious external server to redirect to internal/private endpoints
        // (e.g., http://169.254.169.254/ or http://127.0.0.1/).
        let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if let Err(e) =
                crate::tools::url_validation::validate_url(attempt.url().as_str())
            {
                attempt.error(format!("redirect blocked by SSRF policy: {}", e))
            } else {
                attempt.follow()
            }
        });

        let client = reqwest::Client::builder()
            .redirect(redirect_policy)
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
        if let Some(ct) = response.headers().get(reqwest::header::CONTENT_TYPE)
            && let Ok(ct_str) = ct.to_str()
        {
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

        // Limit download size: read up to 1MB then truncate output to 8KB.
        // Prevents downloading multi-GB files into memory.
        const MAX_DOWNLOAD_SIZE: usize = 1024 * 1024; // 1 MB
        let body = tokio::time::timeout(std::time::Duration::from_secs(15), async {
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
            String::from_utf8(bytes).map_err(|_| "Response body is not valid UTF-8".to_string())
        })
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

pub(super) fn web_fetch_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetch and return the text content of a web page. Only text-based \
                content types are supported (HTML, JSON, XML, plain text). Response is \
                truncated to 8KB. Use web_search first to find relevant URLs, then \
                web_fetch to retrieve specific pages. For downloading files, use \
                shell_execute with curl instead."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The full URL to fetch (must be https:// or http://). Internal/private IPs are blocked."
                    }
                },
                "required": ["url"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({"url": "https://api.example.com/data"}),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(WebFetchTool)),
        provides_capabilities: vec![],
    }
}
