use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::sync::Arc;

struct TextGenerateTool;

#[async_trait]
impl BuiltInTool for TextGenerateTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err(
            "text_generate is not yet implemented. Use the LLM directly for text generation."
                .to_string(),
        )
    }
}

pub(super) fn text_generate_tool() -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "text_generate".to_string(),
            description: "Generate text from a prompt".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The text generation prompt"
                    }
                },
                "required": ["prompt"]
            }),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(TextGenerateTool)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_text_generate_tool() {
        let tool = TextGenerateTool;
        let result = tool
            .execute(&serde_json::json!({"prompt": "Write a poem"}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not yet implemented"));
    }
}
