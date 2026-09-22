//! OpenAI-compatible response decoding, independent of the HTTP transport.

use crate::error::LlmError;
use crate::types::*;

/// The model's own reasoning, under whichever name the server used.
///
/// OpenAI and the proxies that copy it say `reasoning_content`; Ollama 0.34
/// says `reasoning` (`message.reasoning` here, `delta.reasoning` on the stream).
/// Reading only the first name dropped every thinking token a local model
/// produced, so a thinking model showed dead air where the indicator belongs
/// (M1). Both names are accepted, `reasoning_content` first.
///
/// Emptiness is decided **per key**, inside the closure: a gateway that always
/// emits `reasoning_content` — empty when it has nothing of its own — while
/// passing the vendor's `reasoning` through beside it used to short-circuit on
/// the empty first name and return `None`, because `find_map` stops at the
/// first `Some` and the filter then ran once on that already-chosen value
/// (F7). A present-but-empty name is now skipped, not matched.
pub(crate) fn reasoning_text(message: &serde_json::Value) -> Option<String> {
    ["reasoning_content", "reasoning"]
        .iter()
        .find_map(|name| message[*name].as_str().filter(|s| !s.is_empty()))
        .map(|s| s.to_string())
}

/// Decode the common response shape, using `default_model` only when omitted.
pub fn parse_response(
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
        thinking: reasoning_text(message),
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_codec_preserves_tools_usage_and_reasoning_fallback() {
        let response = parse_response("requested-model", serde_json::json!({
            "usage": {"prompt_tokens": 12, "completion_tokens": 3, "prompt_tokens_details": {"cached_tokens": 8}},
            "choices": [{"finish_reason": "tool_calls", "message": {
                "content": "hello", "reasoning_content": "", "reasoning": "thinking",
                "tool_calls": [{"id": "call-1", "function": {"name": "read", "arguments": "{\"path\":\"fixture\"}"}},
                               {"id": "call-2", "function": {"name": "bad", "arguments": "invalid json"}}]
            }}]
        })).unwrap();
        assert_eq!(response.model, "requested-model");
        assert_eq!(response.content, "hello");
        assert_eq!(response.thinking.as_deref(), Some("thinking"));
        assert_eq!(response.usage.input_tokens, 12);
        assert_eq!(response.usage.output_tokens, 3);
        assert_eq!(response.usage.cache_read_input_tokens, 8);
        assert_eq!(response.finish_reason, FinishReason::ToolUse);
        assert_eq!(response.tool_calls[0].arguments["path"], "fixture");
        assert_eq!(response.tool_calls[1].arguments, serde_json::json!({}));
        assert!(
            matches!(&response.parts.unwrap()[0], ContentPart::Text { text } if text == "hello")
        );
        let empty = parse_response("unknown", serde_json::json!({"model":"reported", "choices":[{"message": {"content":null}, "finish_reason":"length"}]})).unwrap();
        assert_eq!(empty.model, "reported");
        assert_eq!(empty.finish_reason, FinishReason::MaxTokens);
        assert!(empty.parts.is_none());
    }
}
