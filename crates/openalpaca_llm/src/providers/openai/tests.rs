use super::*;
use reqwest::header::{HeaderMap, HeaderValue};
use std::sync::Arc;

#[test]
fn test_request_format() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("Hello"),
        ]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };

    let body = provider.build_request_body(&request);
    assert_eq!(body["model"], DEFAULT_MODEL);
    assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2); // system + user (inline, unlike Anthropic)
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "You are helpful.");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "Hello");
}

#[test]
fn test_response_parsing() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let response_json = serde_json::json!({
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "model": "gpt-4o-2024-05-13",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello! How can I help you?"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 8,
            "total_tokens": 28
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.content, "Hello! How can I help you?");
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.model, "gpt-4o-2024-05-13");
    assert_eq!(response.usage.input_tokens, 20);
    assert_eq!(response.usage.output_tokens, 8);
    assert_eq!(response.finish_reason, FinishReason::Stop);
}

#[test]
fn test_tool_calls_response() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let response_json = serde_json::json!({
        "id": "chatcmpl-456",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\": \"Paris\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 50,
            "completion_tokens": 20,
            "total_tokens": 70
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "call_abc");
    assert_eq!(response.tool_calls[0].name, "get_weather");
    assert_eq!(response.tool_calls[0].arguments["location"], "Paris");
    assert_eq!(response.finish_reason, FinishReason::ToolUse);
}

#[test]
fn test_base_url_custom() {
    let provider = OpenAiProvider::new(
        "key".to_string(),
        Some("custom-model".to_string()),
        Some("http://localhost:8080/v1".to_string()),
        None,
    );
    assert_eq!(provider.base_url, "http://localhost:8080/v1");
    assert_eq!(provider.model, "custom-model");
}

#[test]
fn test_request_serialization_filters_empty_text_parts() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user_with_parts(vec![
            ContentPart::Text {
                text: "".to_string(),
            },
            ContentPart::Image {
                source: ImageSource::Url {
                    url: "https://example.com/test.jpg".to_string(),
                },
                detail: None,
            },
        ])]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };

    let body = provider.build_request_body(&request);
    let messages = body["messages"]
        .as_array()
        .expect("messages should be an array");
    assert_eq!(messages.len(), 1);
    let blocks = messages[0]["content"]
        .as_array()
        .expect("content should be an array");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "image_url");
}

#[test]
fn test_request_serialization_empty_parts_get_placeholder() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user_with_parts(vec![ContentPart::Text {
            text: "  \n\t".to_string(),
        }])]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };

    let body = provider.build_request_body(&request);
    let messages = body["messages"]
        .as_array()
        .expect("messages should be an array");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"], "[empty message]");
}

#[test]
fn test_parse_retry_after_ms_fractional() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("2.25"));
    assert_eq!(parse_retry_after_ms(&headers), Some(2250));
}

#[test]
fn test_parse_retry_after_ms_missing() {
    let headers = HeaderMap::new();
    assert_eq!(parse_retry_after_ms(&headers), None);
}

#[test]
fn test_openai_tool_strict_mode() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("test")]),
        tools: Arc::new(vec![ToolDefinition {
            name: "calc".to_string(),
            description: "Calculate".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: Some(true),
            input_examples: None,
        }]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools[0]["function"]["strict"], true);
}

// ── SSE Parser Tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_openai_sse_parser_text_event() {
    use futures_util::StreamExt;

    let raw = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let events: Vec<_> = parse_openai_sse(byte_stream).collect().await;

    let mut texts = Vec::new();
    let mut got_done = false;
    let mut got_usage = false;
    for event in &events {
        match event.as_ref().unwrap() {
            StreamEvent::TextDelta { text } => texts.push(text.clone()),
            StreamEvent::Done { finish_reason } => {
                assert_eq!(finish_reason, &FinishReason::Stop);
                got_done = true;
            }
            StreamEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 5);
                got_usage = true;
            }
            _ => {}
        }
    }
    assert_eq!(texts.join(""), "Hello world");
    assert!(got_done);
    assert!(got_usage);
}

#[tokio::test]
async fn test_openai_sse_parser_tool_call_event() {
    use futures_util::StreamExt;

    let raw = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_abc\",\"type\":\"function\",\"function\":{\"name\":\"search\",\"arguments\":\"\"}}]}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"q\\\"\"}}]}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":\\\"rust\\\"}\"}}]}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":10}}\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let events: Vec<_> = parse_openai_sse(byte_stream).collect().await;

    let mut got_tool_start = false;
    let mut json_parts = Vec::new();
    let mut got_done = false;
    for event in &events {
        match event.as_ref().unwrap() {
            StreamEvent::ToolUseStart { id, name, .. } => {
                assert_eq!(id, "call_abc");
                assert_eq!(name, "search");
                got_tool_start = true;
            }
            StreamEvent::InputJsonDelta { partial_json, .. } => {
                json_parts.push(partial_json.clone());
            }
            StreamEvent::Done { finish_reason } => {
                assert_eq!(finish_reason, &FinishReason::ToolUse);
                got_done = true;
            }
            _ => {}
        }
    }
    assert!(got_tool_start);
    assert!(!json_parts.is_empty());
    assert!(got_done);
}

#[test]
fn test_openai_cached_tokens_parsing() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let response_json = serde_json::json!({
        "id": "chatcmpl-789",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Cached response"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 1000,
            "completion_tokens": 200,
            "total_tokens": 1200,
            "prompt_tokens_details": {
                "cached_tokens": 800
            }
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.usage.input_tokens, 1000);
    assert_eq!(response.usage.output_tokens, 200);
    assert_eq!(response.usage.cache_read_input_tokens, 800);
    assert_eq!(response.usage.cache_creation_input_tokens, 0);
}

#[test]
fn test_openai_no_cached_tokens_defaults_to_zero() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let response_json = serde_json::json!({
        "id": "chatcmpl-790",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Non-cached response"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 500,
            "completion_tokens": 100,
            "total_tokens": 600
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.usage.cache_read_input_tokens, 0);
    assert_eq!(response.usage.cache_creation_input_tokens, 0);
}

#[tokio::test]
async fn test_openai_sse_cached_tokens_in_usage() {
    use futures_util::StreamExt;

    let raw = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":50,\"prompt_tokens_details\":{\"cached_tokens\":800}}}\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let events: Vec<_> = parse_openai_sse(byte_stream).collect().await;

    for event in &events {
        if let Ok(StreamEvent::Usage(u)) = event {
            assert_eq!(u.input_tokens, 1000);
            assert_eq!(u.output_tokens, 50);
            assert_eq!(u.cache_read_input_tokens, 800);
            assert_eq!(u.cache_creation_input_tokens, 0);
            return;
        }
    }
    panic!("Expected a Usage event with cached_tokens");
}

#[test]
fn test_openai_reasoning_effort_mapping() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);

    // Enabled → reasoning_effort: "high", temperature stripped
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("Think about this")]),
        tools: Arc::new(vec![]),
        model: Some("o3".to_string()),
        temperature: Some(0.7),
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Enabled { budget_tokens: 4096 }),
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    assert_eq!(body["reasoning_effort"], "high");
    assert!(body.get("temperature").is_none() || body["temperature"].is_null());

    // Adaptive → reasoning_effort: "medium"
    let request2 = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("Think about this")]),
        tools: Arc::new(vec![]),
        model: Some("o3".to_string()),
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Adaptive),
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body2 = provider.build_request_body(&request2);
    assert_eq!(body2["reasoning_effort"], "medium");

    // Disabled → no reasoning_effort
    let request3 = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("Think about this")]),
        tools: Arc::new(vec![]),
        model: Some("o3".to_string()),
        temperature: Some(0.5),
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Disabled),
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body3 = provider.build_request_body(&request3);
    assert!(body3.get("reasoning_effort").is_none() || body3["reasoning_effort"].is_null());
    assert_eq!(body3["temperature"], 0.5); // temperature preserved
}

/// M2: `ThinkingConfig::Disabled` is how a small-budget internal call says it
/// wants no reasoning. Ollama thinks unless told otherwise, so it must hear
/// `reasoning_effort = "none"`; OpenAI has no "off" and rejects the key on a
/// model that cannot reason, so the cloud flavour must still say nothing.
#[test]
fn test_disabled_thinking_asks_ollama_for_no_reasoning() {
    use crate::providers::openai::request::build_request_body;

    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("extract traits")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: Some(0.2),
        max_tokens: Some(256),
        tool_choice: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Disabled),
        context_management: None,
        ephemeral_system_notice: None,
    };

    let ollama = build_request_body("qwen3.8:27b", 8192, OpenAiFlavour::Ollama, &request);
    assert_eq!(
        ollama["reasoning_effort"], "none",
        "a local thinking model must be told not to think for this call"
    );
    assert!(
        ollama["temperature"].is_number(),
        "asking for no reasoning does not strip temperature, unlike asking for more"
    );

    let cloud = build_request_body("gpt-4o", 4096, OpenAiFlavour::OpenAi, &request);
    assert!(
        cloud.get("reasoning_effort").is_none_or(|v| v.is_null()),
        "OpenAI rejects reasoning_effort on a model that does not reason: {cloud}"
    );
}

/// The Ollama flavour changes nothing else: `Enabled`/`Adaptive` map exactly as
/// they do for OpenAI, and an absent `thinking` still says nothing at all.
#[test]
fn test_ollama_flavour_leaves_the_other_thinking_arms_alone() {
    use crate::providers::openai::request::build_request_body;

    let mut request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("think hard")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };

    let body = build_request_body("qwen3.8:27b", 8192, OpenAiFlavour::Ollama, &request);
    assert!(
        body.get("reasoning_effort").is_none_or(|v| v.is_null()),
        "no thinking config means the model reasons as it normally would"
    );

    request.thinking = Some(ThinkingConfig::Adaptive);
    let body = build_request_body("qwen3.8:27b", 8192, OpenAiFlavour::Ollama, &request);
    assert_eq!(body["reasoning_effort"], "medium");

    request.thinking = Some(ThinkingConfig::Enabled { budget_tokens: 2048 });
    let body = build_request_body("qwen3.8:27b", 8192, OpenAiFlavour::Ollama, &request);
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn test_openai_reasoning_response_parsing() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let response_json = serde_json::json!({
        "id": "chatcmpl-reasoning",
        "model": "o3",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "The answer is 42.",
                "reasoning_content": "Let me think step by step about this problem..."
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 500,
            "total_tokens": 600,
            "completion_tokens_details": {
                "reasoning_tokens": 400
            }
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.content, "The answer is 42.");
    assert_eq!(
        response.thinking.as_deref(),
        Some("Let me think step by step about this problem...")
    );
    assert_eq!(response.model, "o3");
}

/// M1: Ollama 0.34 names the field `reasoning`, not `reasoning_content`.
/// Reading only the OpenAI spelling dropped every thinking token a local model
/// produced.
#[test]
fn test_ollama_reasoning_field_is_parsed() {
    let provider = OpenAiProvider::new_without_auth(
        "qwen3.8:27b".to_string(),
        "http://localhost:11434/v1".to_string(),
        None,
    );
    let response_json = serde_json::json!({
        "id": "chatcmpl-244",
        "model": "qwen3.8:27b",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello, friend!",
                "reasoning": "The user wants a simple hello in one short sentence."
            },
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 47, "completion_tokens": 19, "total_tokens": 66}
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.content, "Hello, friend!");
    assert_eq!(
        response.thinking.as_deref(),
        Some("The user wants a simple hello in one short sentence.")
    );
}

#[test]
fn test_openai_no_reasoning_content_is_none() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let response_json = serde_json::json!({
        "id": "chatcmpl-normal",
        "model": "gpt-5.2",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Just a normal response"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 50,
            "completion_tokens": 10,
            "total_tokens": 60
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert!(response.thinking.is_none());
}

#[tokio::test]
async fn test_openai_sse_reasoning_delta() {
    use futures_util::StreamExt;

    let raw = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Let me\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\" think...\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"The answer is 42.\"}}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":500}}\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let events: Vec<_> = parse_openai_sse(byte_stream).collect().await;

    let mut thinking_parts = Vec::new();
    let mut text_parts = Vec::new();
    for event in &events {
        match event.as_ref().unwrap() {
            StreamEvent::ThinkingDelta { thinking } => thinking_parts.push(thinking.clone()),
            StreamEvent::TextDelta { text } => text_parts.push(text.clone()),
            _ => {}
        }
    }
    assert_eq!(thinking_parts.join(""), "Let me think...");
    assert_eq!(text_parts.join(""), "The answer is 42.");
}

/// M1, the streamed half: a live Ollama frame is
/// `{"delta":{"content":"","reasoning":"The"}}` — the empty `content` must not
/// swallow the frame, and the reasoning text must arrive as the same
/// `ThinkingDelta` Anthropic's thinking already uses.
#[tokio::test]
async fn test_ollama_sse_reasoning_delta() {
    use futures_util::StreamExt;

    let raw = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\",\"reasoning\":\"The\"},\"finish_reason\":null}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"reasoning\":\" user\"},\"finish_reason\":null}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n",
        "\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":19,\"completion_tokens\":10}}\n",
        "\n",
        "data: [DONE]\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let events: Vec<_> = parse_openai_sse(byte_stream).collect().await;

    let mut thinking_parts = Vec::new();
    let mut text_parts = Vec::new();
    for event in &events {
        match event.as_ref().unwrap() {
            StreamEvent::ThinkingDelta { thinking } => thinking_parts.push(thinking.clone()),
            StreamEvent::TextDelta { text } => text_parts.push(text.clone()),
            _ => {}
        }
    }
    assert_eq!(thinking_parts.join(""), "The user");
    assert_eq!(text_parts.join(""), "Hi");
}

// ── V1: one frame, every event it carries ────────────────────────────

/// Collect a raw SSE body into the events the parser yields, in order.
async fn events_of(raw: &str) -> Vec<StreamEvent> {
    use futures_util::StreamExt;
    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw.to_string()))]);
    parse_openai_sse(byte_stream)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|e| e.expect("no stream error"))
        .collect()
}

/// The arguments the accumulator would assemble for one tool index, exactly
/// the way `collect_stream` does it.
fn accumulated_args(events: &[StreamEvent], index: usize) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::InputJsonDelta {
                index: i,
                partial_json,
            } if *i == index => Some(partial_json.as_str()),
            _ => None,
        })
        .collect()
}

/// **V1, the blocker.** Ollama sends a whole tool call in ONE frame: the `id`,
/// the name and the complete `arguments` string together. The parser used to
/// return the moment it saw `id` and drop everything after it in that frame,
/// so every local-model tool call arrived with `input: {}` and the main loop
/// spent its whole round budget on "missing required parameter".
///
/// Captured verbatim from a live `qwen3.8:27b-mtp-q8_0` run.
#[tokio::test]
async fn an_ollama_single_frame_tool_call_keeps_its_arguments() {
    let raw = concat!(
        r#"data: {"id":"chatcmpl-452","object":"chat.completion.chunk","created":1789787752,"model":"qwen3.8:27b-mtp-q8_0","system_fingerprint":"fp_ollama","choices":[{"index":0,"delta":{"content":"","reasoning":" notes"},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"id":"chatcmpl-452","object":"chat.completion.chunk","created":1789787752,"model":"qwen3.8:27b-mtp-q8_0","system_fingerprint":"fp_ollama","choices":[{"index":0,"delta":{"content":"","tool_calls":[{"id":"call_8st7b155","index":0,"type":"function","function":{"name":"start_workflow","arguments":"{\"goal\":\"write alpaca notes\"}"}}]},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"id":"chatcmpl-452","object":"chat.completion.chunk","created":1789787752,"model":"qwen3.8:27b-mtp-q8_0","system_fingerprint":"fp_ollama","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    let events = events_of(raw).await;

    let start = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::ToolUseStart { index, id, name } => Some((*index, id.clone(), name.clone())),
            _ => None,
        })
        .expect("the frame announces the tool call");
    assert_eq!(start, (0, "call_8st7b155".to_string(), "start_workflow".to_string()));

    let args = accumulated_args(&events, 0);
    let parsed: serde_json::Value =
        serde_json::from_str(&args).expect("the accumulated arguments are whole JSON");
    assert_eq!(parsed, serde_json::json!({"goal": "write alpaca notes"}));

    assert!(
        events.iter().any(|e| matches!(
            e,
            StreamEvent::Done {
                finish_reason: FinishReason::ToolUse
            }
        )),
        "the finish frame still closes the stream: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ThinkingDelta { thinking } if thinking == " notes")),
        "the reasoning frame before it is untouched: {events:?}"
    );
}

/// **V1.** Two tool calls in one frame: both starts and both argument strings,
/// each under its own index.
#[tokio::test]
async fn two_tool_calls_in_one_frame_both_survive() {
    let raw = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":["#,
        r#"{"id":"call_a","index":0,"type":"function","function":{"name":"first","arguments":"{\"a\":1}"}},"#,
        r#"{"id":"call_b","index":1,"type":"function","function":{"name":"second","arguments":"{\"b\":2}"}}"#,
        r#"]},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    let events = events_of(raw).await;

    let starts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolUseStart { index, name, .. } => Some((*index, name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec![(0, "first".to_string()), (1, "second".to_string())],
        "both calls are announced, in wire order: {events:?}"
    );
    assert_eq!(accumulated_args(&events, 0), r#"{"a":1}"#);
    assert_eq!(accumulated_args(&events, 1), r#"{"b":2}"#);
}

/// **V1.** Content and a tool call in the same frame: the text is not swallowed
/// by the tool call, and the tool call is not swallowed by the text.
#[tokio::test]
async fn content_and_a_tool_call_in_one_frame_both_survive() {
    let raw = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"content":"Looking that up.","tool_calls":[{"id":"call_c","index":0,"type":"function","function":{"name":"search","arguments":"{\"q\":\"rust\"}"}}]},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    let events = events_of(raw).await;

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Looking that up.");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolUseStart { name, .. } if name == "search")),
        "the tool call in the same frame is not lost: {events:?}"
    );
    assert_eq!(accumulated_args(&events, 0), r#"{"q":"rust"}"#);
}

/// **V1.** A tool call and `finish_reason` in the same frame: the call, the
/// usage and the done event all come out, in that order.
#[tokio::test]
async fn a_tool_call_and_finish_reason_in_one_frame_both_survive() {
    let raw = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_d","index":0,"type":"function","function":{"name":"lookup","arguments":"{\"k\":\"v\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    let events = events_of(raw).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolUseStart { name, .. } if name == "lookup")),
        "the call is not eaten by the finish reason: {events:?}"
    );
    assert_eq!(accumulated_args(&events, 0), r#"{"k":"v"}"#);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::Usage(u) if u.input_tokens == 7 && u.output_tokens == 3)),
        "the usage on the finish frame is still read: {events:?}"
    );
    let last = events.last().expect("some events");
    assert!(
        matches!(
            last,
            StreamEvent::Done {
                finish_reason: FinishReason::ToolUse
            }
        ),
        "done is last: {last:?}"
    );
}

/// **V1.** A tool call whose arguments arrive empty (`""`) is still a tool
/// call: the start is emitted, no empty argument delta is, and the accumulator
/// turns "nothing" into `{}` rather than a parse error.
#[tokio::test]
async fn a_tool_call_with_no_arguments_is_still_a_call() {
    let raw = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_e","index":0,"type":"function","function":{"name":"ping","arguments":""}}]},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    let events = events_of(raw).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolUseStart { name, .. } if name == "ping")),
        "the call is announced: {events:?}"
    );
    assert_eq!(accumulated_args(&events, 0), "");

    let response = crate::streaming::collect_stream(
        Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok).collect::<Vec<_>>(),
        )),
        "mock".to_string(),
    )
    .await
    .expect("the stream collects");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(
        response.tool_calls[0].arguments,
        serde_json::json!({}),
        "empty arguments become an empty object, never a parse failure"
    );
}

/// **V1, end to end in this crate.** The captured Ollama body, through the
/// parser and the accumulator both, is one tool call with its arguments —
/// which is what the agentic loop executes.
#[tokio::test]
async fn the_captured_ollama_body_collects_into_a_tool_call_with_arguments() {
    let raw = concat!(
        r#"data: {"choices":[{"index":0,"delta":{"content":"","reasoning":"The user"},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{"content":"","tool_calls":[{"id":"call_8st7b155","index":0,"type":"function","function":{"name":"start_workflow","arguments":"{\"goal\":\"write alpaca notes\"}"}}]},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "\n\n",
        "data: [DONE]\n\n",
    );

    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let response = crate::streaming::collect_stream(
        Box::pin(parse_openai_sse(byte_stream)),
        "qwen3.8:27b-mtp-q8_0".to_string(),
    )
    .await
    .expect("the stream collects");

    assert_eq!(response.tool_calls.len(), 1, "one tool call");
    assert_eq!(response.tool_calls[0].name, "start_workflow");
    assert_eq!(
        response.tool_calls[0].arguments,
        serde_json::json!({"goal": "write alpaca notes"}),
        "the arguments reach the loop"
    );
    assert_eq!(response.finish_reason, FinishReason::ToolUse);
    assert_eq!(response.thinking.as_deref(), Some("The user"));
}

#[test]
fn test_openai_ephemeral_notice_placement() {
    use crate::types::{ChatMessage, ChatRequest};
    use std::sync::Arc;

    let request = ChatRequest {
        messages: Arc::new(vec![
            ChatMessage::system("real system"),
            ChatMessage::user("hello"),
        ]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: Some("[budget_notice]\nwatch out\n[/budget_notice]".to_string()),
    };

    let body =
        super::request::build_request_body("gpt-test", 1024, OpenAiFlavour::OpenAi, &request);
    let messages = body["messages"].as_array().unwrap();
    let last = messages.last().unwrap();
    assert_eq!(last["role"], "system");
    assert!(last["content"].as_str().unwrap().contains("budget_notice"));
}
