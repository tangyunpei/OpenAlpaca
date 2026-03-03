use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct SummarizeTool;

#[async_trait]
impl BuiltInTool for SummarizeTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("summarize is not yet implemented. Use the LLM directly for summarization.".to_string())
    }
}

pub(super) fn summarize_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "summarize".to_string(),
            description: "Condense text into a shorter summary".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text to summarize"
                    }
                },
                "required": ["text"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(SummarizeTool)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_summarize_tool() {
        let tool = SummarizeTool;
        let result = tool
            .execute(&serde_json::json!({"text": "Hello world"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not yet implemented"));
    }
}
