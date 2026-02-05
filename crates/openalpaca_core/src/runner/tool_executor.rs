//! Stub tool executor for Phase 5.2.
//!
//! Returns errors for all tool calls. Will be replaced with real
//! implementations in future phases.

use crate::security::sandbox::ToolExecutor;
use async_trait::async_trait;

/// A stub executor that rejects all tool calls.
pub struct StubToolExecutor;

#[async_trait]
impl ToolExecutor for StubToolExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<String, String> {
        Err(format!("Tool '{}' not yet implemented", tool_name))
    }

    fn registered_tools(&self) -> Vec<String> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stub_returns_error() {
        let executor = StubToolExecutor;
        let result = executor
            .execute("web_search", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not yet implemented"));
    }

    #[test]
    fn test_registered_tools_empty() {
        let executor = StubToolExecutor;
        assert!(executor.registered_tools().is_empty());
    }
}
