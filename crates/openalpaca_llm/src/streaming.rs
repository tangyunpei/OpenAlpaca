use crate::error::LlmError;
use crate::types::*;
use futures_util::StreamExt;
use std::collections::HashMap;

/// Collect a streaming response into a complete ChatResponse.
pub async fn collect_stream(
    mut stream: ChatStream,
    model: String,
) -> Result<ChatResponse, LlmError> {
    let mut content = String::new();
    let mut thinking = None;
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut partial_json: HashMap<usize, String> = HashMap::new();
    let mut tool_meta: HashMap<usize, (String, String)> = HashMap::new();
    let mut usage = Usage::default();
    let mut finish_reason = FinishReason::Stop;

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { text } => content.push_str(&text),
            StreamEvent::ThinkingDelta { thinking: t } => {
                thinking.get_or_insert_with(String::new).push_str(&t);
            }
            StreamEvent::ToolUseStart { index, id, name } => {
                tool_meta.insert(index, (id, name));
                partial_json.entry(index).or_default();
            }
            StreamEvent::InputJsonDelta {
                index,
                partial_json: pj,
            } => {
                partial_json.entry(index).or_default().push_str(&pj);
            }
            StreamEvent::Usage(u) => usage = u,
            StreamEvent::Done { finish_reason: fr } => finish_reason = fr,
            StreamEvent::Error { message } => {
                return Err(LlmError::Stream(message));
            }
        }
    }

    // Assemble tool calls from accumulated JSON
    let mut indices: Vec<usize> = partial_json.keys().copied().collect();
    indices.sort();
    for index in indices {
        if let Some((id, name)) = tool_meta.remove(&index) {
            let json_str = partial_json.remove(&index).unwrap_or_default();
            let arguments: serde_json::Value = serde_json::from_str(&json_str)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        model,
        usage,
        finish_reason,
        thinking,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collect_stream_text_only() {
        let events = vec![
            Ok(StreamEvent::TextDelta {
                text: "Hello ".to_string(),
            }),
            Ok(StreamEvent::TextDelta {
                text: "world!".to_string(),
            }),
            Ok(StreamEvent::Usage(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            })),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let response = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();
        assert_eq!(response.content, "Hello world!");
        assert!(response.tool_calls.is_empty());
        assert!(response.thinking.is_none());
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.finish_reason, FinishReason::Stop);
    }

    #[tokio::test]
    async fn test_collect_stream_with_tools() {
        let events = vec![
            Ok(StreamEvent::ToolUseStart {
                index: 0,
                id: "tc1".to_string(),
                name: "search".to_string(),
            }),
            Ok(StreamEvent::InputJsonDelta {
                index: 0,
                partial_json: r#"{"query":"#.to_string(),
            }),
            Ok(StreamEvent::InputJsonDelta {
                index: 0,
                partial_json: r#""rust"}"#.to_string(),
            }),
            Ok(StreamEvent::Usage(Usage::default())),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::ToolUse,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let response = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "search");
        assert_eq!(response.tool_calls[0].arguments["query"], "rust");
        assert_eq!(response.finish_reason, FinishReason::ToolUse);
    }

    #[tokio::test]
    async fn test_collect_stream_with_thinking() {
        let events = vec![
            Ok(StreamEvent::ThinkingDelta {
                thinking: "Let me think...".to_string(),
            }),
            Ok(StreamEvent::TextDelta {
                text: "Answer.".to_string(),
            }),
            Ok(StreamEvent::Usage(Usage::default())),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let response = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();
        assert_eq!(response.thinking.as_deref(), Some("Let me think..."));
        assert_eq!(response.content, "Answer.");
    }

    #[tokio::test]
    async fn test_collect_stream_error() {
        let events: Vec<Result<StreamEvent, LlmError>> = vec![
            Ok(StreamEvent::TextDelta {
                text: "partial".to_string(),
            }),
            Ok(StreamEvent::Error {
                message: "connection reset".to_string(),
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let result = collect_stream(stream, "test-model".to_string()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, LlmError::Stream(ref s) if s.contains("connection reset")));
    }
}
