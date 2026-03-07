use crate::error::LlmError;
use crate::types::*;

pub(super) fn parse_response(
    default_model: &str,
    body: serde_json::Value,
) -> Result<ChatResponse, LlmError> {
    let model = body["model"].as_str().unwrap_or(default_model).to_string();

    let base_input = body["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
    let cache_creation = body["usage"]["cache_creation_input_tokens"]
        .as_u64()
        .unwrap_or(0) as u32;
    let cache_read = body["usage"]["cache_read_input_tokens"]
        .as_u64()
        .unwrap_or(0) as u32;

    let usage = Usage {
        input_tokens: base_input + cache_creation + cache_read,
        output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
    };

    let stop_reason = body["stop_reason"].as_str().unwrap_or("end_turn");
    let finish_reason = match stop_reason {
        "end_turn" => FinishReason::Stop,
        "tool_use" => FinishReason::ToolUse,
        "max_tokens" => FinishReason::MaxTokens,
        _ => FinishReason::Stop,
    };

    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut thinking = None;
    let mut parts_vec: Vec<ContentPart> = Vec::new();

    if let Some(content_blocks) = body["content"].as_array() {
        for block in content_blocks {
            match block["type"].as_str() {
                Some("thinking") => {
                    if let Some(thought) = block["thinking"].as_str() {
                        let existing = thinking.get_or_insert_with(String::new);
                        if !existing.is_empty() {
                            existing.push('\n');
                        }
                        existing.push_str(thought);
                    }
                }
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(text);
                        parts_vec.push(ContentPart::Text {
                            text: text.to_string(),
                        });
                    }
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        arguments: block["input"].clone(),
                    });
                }
                _ => {}
            }
        }
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        model,
        usage,
        finish_reason,
        thinking,
        parts: if parts_vec.is_empty() {
            None
        } else {
            Some(parts_vec)
        },
    })
}
