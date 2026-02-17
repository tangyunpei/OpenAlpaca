use arc_swap::ArcSwap;
use async_trait::async_trait;
use crate::daemon_config::DaemonConfig;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct WebSearchTool {
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
}

#[async_trait]
impl BuiltInTool for WebSearchTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: query".to_string())?;

        let count = arguments
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20) as u32;

        // Read config (hot-reloadable via ArcSwap)
        let cfg = self.daemon_config.load();
        let ws_cfg = &cfg.providers.web_search;

        if ws_cfg.api_key.is_empty() {
            return Err(
                "web_search is not configured. \
                 Set providers.web_search.api_key in daemon.toml \
                 with your Brave Search API key."
                    .to_string(),
            );
        }

        let client = reqwest::Client::new();
        let timeout = std::time::Duration::from_secs(ws_cfg.timeout_secs);

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding::encode(query),
            count,
        );

        let response = client
            .get(&url)
            .header("X-Subscription-Token", &ws_cfg.api_key)
            .header("Accept", "application/json")
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| format!("Brave Search request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Brave Search API error (HTTP {}): {}",
                status,
                body.chars().take(512).collect::<String>()
            ));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Brave Search response: {}", e))?;

        // Extract web.results[] into a simplified structure
        let results: Vec<serde_json::Value> = body
            .get("web")
            .and_then(|w| w.get("results"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        serde_json::json!({
                            "title": item.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                            "url": item.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                            "description": item.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        if results.is_empty() {
            return Ok(serde_json::json!({
                "results": [],
                "message": "No results found for the query."
            })
            .to_string());
        }

        serde_json::to_string(&serde_json::json!({ "results": results }))
            .map_err(|e| format!("JSON serialization failed: {}", e))
    }
}

pub(super) fn web_search_tool(
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web for information using Brave Search. \
                Returns titles, URLs, and descriptions."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "count": {
                        "type": "integer",
                        "description": "Number of results to return (default: 5, max: 20)"
                    }
                },
                "required": ["query"]
            }),
        },
        backend: ToolBackend::BuiltIn(Arc::new(WebSearchTool { daemon_config })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_web_search_no_api_key_returns_helpful_error() {
        let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        let tool = WebSearchTool { daemon_config: dc };
        let result = tool
            .execute(&serde_json::json!({"query": "rust language"}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("not configured"), "Error should guide user: {}", err);
        assert!(err.contains("api_key"), "Error should mention api_key: {}", err);
    }

    #[tokio::test]
    async fn test_web_search_missing_query() {
        let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        let tool = WebSearchTool { daemon_config: dc };
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn test_web_search_count_clamped() {
        // Without a valid API key, execution fails at the key check.
        // This verifies the parameter extraction path doesn't panic.
        let dc = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        let tool = WebSearchTool { daemon_config: dc };
        let result = tool
            .execute(&serde_json::json!({"query": "test", "count": 100}))
            .await;
        // Fails at API key check, not at count — proves no panic on large count
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not configured"));
    }
}
