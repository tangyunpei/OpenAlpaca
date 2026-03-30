use crate::error::LlmError;
use crate::types::*;

pub(super) fn parse_response(
    default_model: &str,
    body: serde_json::Value,
) -> Result<ChatResponse, LlmError> {
    let model = body["model"].as_str().unwrap_or(default_model).to_string();

    let usage = Usage {
        input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32,
        output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32,
        cache_read_input_tokens: body["usage"]["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0) as u32,
        ..Default::default()
    };

    let choice = &body["choices"][0];
    let message = &choice["message"];

    let content = message["content"].as_str().unwrap_or("").to_string();

    let finish_reason_str = choice["finish_reason"].as_str().unwrap_or("stop");
    let finish_reason = match finish_reason_str {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolUse,
        "length" => FinishReason::MaxTokens,
        _ => FinishReason::Stop,
    };

    let mut tool_calls = Vec::new();
    if let Some(tcs) = message["tool_calls"].as_array() {
        for tc in tcs {
            let id = tc["id"].as_str().unwrap_or_default().to_string();
            let name = tc["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let arguments: serde_json::Value = serde_json::from_str(args_str)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    let parts = if content.is_empty() {
        None
    } else {
        Some(vec![ContentPart::Text {
            text: content.clone(),
        }])
    };

    Ok(ChatResponse {
        content,
        tool_calls,
        model,
        usage,
        finish_reason,
        thinking: message["reasoning_content"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        parts,
    })
}
