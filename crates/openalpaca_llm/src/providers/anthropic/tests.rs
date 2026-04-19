use super::*;
use reqwest::header::{HeaderMap, HeaderValue};
use std::sync::Arc;

#[test]
fn test_request_serialization() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("Hello"),
        ]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: Some(0.7),
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
    assert_eq!(body["system"], "You are helpful.");
    let temp = body["temperature"].as_f64().unwrap();
    assert!((temp - 0.7).abs() < 0.01);

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1); // system extracted, only user remains
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "Hello");
}

#[test]
fn test_response_parsing() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response_json = serde_json::json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5-20250929",
        "content": [
            {"type": "text", "text": "Hello! How can I help?"}
        ],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 25,
            "output_tokens": 10
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.content, "Hello! How can I help?");
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.model, "claude-sonnet-4-5-20250929");
    assert_eq!(response.usage.input_tokens, 25);
    assert_eq!(response.usage.output_tokens, 10);
    assert_eq!(response.finish_reason, FinishReason::Stop);
}

#[test]
fn test_tool_use_response() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response_json = serde_json::json!({
        "id": "msg_456",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5-20250929",
        "content": [
            {"type": "text", "text": "Let me search that."},
            {
                "type": "tool_use",
                "id": "toolu_01",
                "name": "web_search",
                "input": {"query": "Rust programming"}
            }
        ],
        "stop_reason": "tool_use",
        "usage": {
            "input_tokens": 50,
            "output_tokens": 30
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.content, "Let me search that.");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "toolu_01");
    assert_eq!(response.tool_calls[0].name, "web_search");
    assert_eq!(
        response.tool_calls[0].arguments["query"],
        "Rust programming"
    );
    assert_eq!(response.finish_reason, FinishReason::ToolUse);
}

#[test]
fn test_cache_tokens_included_in_total() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response_json = serde_json::json!({
        "id": "msg_cache",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5-20250929",
        "content": [
            {"type": "text", "text": "Cached response"}
        ],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 50,
            "output_tokens": 20,
            "cache_creation_input_tokens": 1500,
            "cache_read_input_tokens": 300
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    // input_tokens should include base + cache_creation + cache_read
    assert_eq!(response.usage.input_tokens, 50 + 1500 + 300);
    assert_eq!(response.usage.output_tokens, 20);
    assert_eq!(response.usage.cache_creation_input_tokens, 1500);
    assert_eq!(response.usage.cache_read_input_tokens, 300);
}

#[test]
fn test_no_cache_tokens_unchanged() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response_json = serde_json::json!({
        "id": "msg_nocache",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5-20250929",
        "content": [
            {"type": "text", "text": "No cache"}
        ],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 100,
            "output_tokens": 30
        }
    });

    let response = provider.parse_response(response_json).unwrap();
    // Without cache tokens, input_tokens stays as-is
    assert_eq!(response.usage.input_tokens, 100);
    assert_eq!(response.usage.cache_creation_input_tokens, 0);
    assert_eq!(response.usage.cache_read_input_tokens, 0);
}

#[test]
fn test_error_handling() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let error_json = serde_json::json!({
        "error": {
            "type": "invalid_request_error",
            "message": "Invalid API key"
        }
    });
    // parse_response works on success payloads; API errors are handled before parsing
    // Just verify parse_response doesn't panic on unexpected structure
    let result = provider.parse_response(error_json);
    assert!(result.is_ok()); // It parses but fields are empty/default
}

#[test]
fn test_request_serialization_filters_empty_text_parts() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
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
    assert_eq!(blocks[0]["type"], "image");
}

#[test]
fn test_request_serialization_empty_parts_get_placeholder() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user_with_parts(vec![ContentPart::Text {
            text: " \n\t ".to_string(),
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
    let blocks = messages[0]["content"]
        .as_array()
        .expect("content should be an array");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["text"], "[empty message]");
}

#[test]
fn test_parse_retry_after_ms_fractional() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("1.5"));
    assert_eq!(parse_retry_after_ms(&headers), Some(1500));
}

#[test]
fn test_parse_retry_after_ms_invalid() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("nope"));
    assert_eq!(parse_retry_after_ms(&headers), None);
}

#[test]
fn test_tool_with_strict_and_examples() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("test")]),
        tools: Arc::new(vec![ToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
            strict: Some(true),
            input_examples: Some(vec![serde_json::json!({"query": "rust programming"})]),
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
    // strict is not serialized for Anthropic (Anthropic API does not support it)
    assert!(tools[0].get("strict").is_none() || tools[0]["strict"].is_null());
    assert!(tools[0]["input_examples"].as_array().unwrap().len() == 1);
}

#[test]
fn test_tool_without_strict_no_extra_fields() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("test")]),
        tools: Arc::new(vec![ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({}),
            strict: None,
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
    assert!(tools[0].get("strict").is_none());
    assert!(tools[0].get("input_examples").is_none());
}

#[test]
fn test_system_prompt_caching_enabled() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
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
        enable_caching: true,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    // System should be array with cache_control
    let system = body["system"].as_array().unwrap();
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_system_prompt_caching_disabled() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
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
    // System should be plain string
    assert!(body["system"].is_string());
}

#[test]
fn test_tool_caching_breakpoint() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("test")]),
        tools: Arc::new(vec![
            ToolDefinition {
                name: "a".into(),
                description: "A".into(),
                parameters: serde_json::json!({}),
                strict: None,
                input_examples: None,
            },
            ToolDefinition {
                name: "b".into(),
                description: "B".into(),
                parameters: serde_json::json!({}),
                strict: None,
                input_examples: None,
            },
        ]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: true,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    let tools = body["tools"].as_array().unwrap();
    // Only last tool should have cache_control
    assert!(tools[0].get("cache_control").is_none());
    assert_eq!(tools[1]["cache_control"]["type"], "ephemeral");
}

#[test]
fn test_caching_with_empty_tools() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
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
        enable_caching: true,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    // System should still have cache_control
    let system = body["system"].as_array().unwrap();
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    // No tools array at all
    assert!(body.get("tools").is_none());
}

#[test]
fn test_thinking_enabled_serialization() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("Analyze this")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: Some(0.5),
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Enabled { budget_tokens: 2048 }),
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 2048);
    // Temperature must be removed when thinking is enabled
    assert!(body.get("temperature").is_none());
}

#[test]
fn test_thinking_disabled_keeps_temperature() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("test")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: Some(0.7),
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Disabled),
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    assert_eq!(body["thinking"]["type"], "disabled");
    // Temperature should be preserved
    assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.01);
}

#[test]
fn test_parse_thinking_block() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response_json = serde_json::json!({
        "id": "msg_think",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5-20250929",
        "content": [
            {"type": "thinking", "thinking": "Let me reason about this..."},
            {"type": "text", "text": "Here is my answer."}
        ],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 100, "output_tokens": 50 }
    });
    let response = provider.parse_response(response_json).unwrap();
    assert_eq!(response.thinking.as_deref(), Some("Let me reason about this..."));
    assert_eq!(response.content, "Here is my answer.");
}

#[test]
fn test_no_thinking_block_returns_none() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response_json = serde_json::json!({
        "id": "msg_nothink",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5-20250929",
        "content": [{"type": "text", "text": "Simple response."}],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 10, "output_tokens": 5 }
    });
    let response = provider.parse_response(response_json).unwrap();
    assert!(response.thinking.is_none());
}

#[test]
fn test_thinking_adaptive_serialization() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("test")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: Some(0.5),
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Adaptive),
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    assert_eq!(body["thinking"]["type"], "adaptive");
    // Temperature must be removed for adaptive thinking too
    assert!(body.get("temperature").is_none());
}

#[test]
fn test_parse_multiple_thinking_blocks() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response_json = serde_json::json!({
        "id": "msg_multi",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-5-20250929",
        "content": [
            {"type": "thinking", "thinking": "First thought."},
            {"type": "thinking", "thinking": "Second thought."},
            {"type": "text", "text": "Final answer."}
        ],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 100, "output_tokens": 80 }
    });
    let response = provider.parse_response(response_json).unwrap();
    // Multiple thinking blocks should be concatenated with newline
    assert_eq!(response.thinking.as_deref(), Some("First thought.\nSecond thought."));
    assert_eq!(response.content, "Final answer.");
}

// ── SSE Parser Tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_anthropic_sse_parser_text_event() {
    use futures_util::StreamExt;

    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n",
        "\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n",
        "\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![
        Ok(bytes::Bytes::from(raw)),
    ]);
    let events: Vec<_> = parse_anthropic_sse(byte_stream).collect().await;

    // Should get: Usage(input), TextDelta("Hello"), TextDelta(" world"), Usage(output), Done
    assert!(events.len() >= 4);
    let mut texts = Vec::new();
    let mut got_done = false;
    for event in &events {
        match event.as_ref().unwrap() {
            StreamEvent::TextDelta { text } => texts.push(text.clone()),
            StreamEvent::Done { finish_reason } => {
                assert_eq!(finish_reason, &FinishReason::Stop);
                got_done = true;
            }
            _ => {}
        }
    }
    assert_eq!(texts.join(""), "Hello world");
    assert!(got_done);
}

#[tokio::test]
async fn test_anthropic_sse_parser_tool_use_event() {
    use futures_util::StreamExt;

    let raw = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"search\",\"input\":{}}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\"\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\":\\\"rust\\\"}\"}}\n",
        "\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":20}}\n",
        "\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![
        Ok(bytes::Bytes::from(raw)),
    ]);
    let events: Vec<_> = parse_anthropic_sse(byte_stream).collect().await;

    let mut got_tool_start = false;
    let mut json_parts = Vec::new();
    let mut got_done = false;
    for event in &events {
        match event.as_ref().unwrap() {
            StreamEvent::ToolUseStart { id, name, .. } => {
                assert_eq!(id, "toolu_1");
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

#[tokio::test]
async fn test_anthropic_sse_parser_thinking_event() {
    use futures_util::StreamExt;

    let raw = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me think...\"}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Answer.\"}}\n",
        "\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":10}}\n",
        "\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    );

    let byte_stream = futures_util::stream::iter(vec![
        Ok(bytes::Bytes::from(raw)),
    ]);
    let events: Vec<_> = parse_anthropic_sse(byte_stream).collect().await;

    let mut got_thinking = false;
    let mut got_text = false;
    for event in &events {
        match event.as_ref().unwrap() {
            StreamEvent::ThinkingDelta { thinking } => {
                assert_eq!(thinking, "Let me think...");
                got_thinking = true;
            }
            StreamEvent::TextDelta { text } => {
                assert_eq!(text, "Answer.");
                got_text = true;
            }
            _ => {}
        }
    }
    assert!(got_thinking);
    assert!(got_text);
}

// ── Context Management Tests ─────────────────────────────────────

#[test]
fn test_build_request_body_with_context_management() {
    use crate::context_management::{ContextEdit, ContextManagement};

    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
        tools: Arc::new(vec![]),
        model: Some("claude-sonnet-4-20250514".to_string()),
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: Some(ContextManagement {
            edits: vec![
                ContextEdit::ClearThinking {
                    keep_thinking_turns: 2,
                },
                ContextEdit::ClearToolUses {
                    trigger_tokens: 100_000,
                    keep_tool_uses: 5,
                },
            ],
        }),
        ephemeral_system_notice: None,
    };

    let body = request::build_request_body("claude-sonnet-4-20250514", 4096, &request);
    assert!(body.get("context_management").is_some());
    let edits = body["context_management"]["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0]["type"], "clear_thinking_20251015");
    assert_eq!(edits[1]["type"], "clear_tool_uses_20250919");
}

#[test]
fn test_build_request_body_without_context_management() {
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
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

    let body = request::build_request_body("claude-sonnet-4-20250514", 4096, &request);
    assert!(body.get("context_management").is_none());
}

#[test]
fn test_anthropic_last_message_has_cache_control_when_caching_enabled() {
    let request = ChatRequest {
        messages: Arc::new(vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("First user turn."),
            ChatMessage::assistant("Assistant reply."),
            ChatMessage::user("Second user turn."),
        ]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: true,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    };

    let body = request::build_request_body("claude-test", 1024, &request);
    let messages = body["messages"].as_array().expect("messages array");
    let last = messages.last().expect("at least one message");
    let content = &last["content"];

    let arr = content
        .as_array()
        .expect("last message content must be an array after P1 lands");
    let last_block = arr.last().expect("at least one block");
    assert_eq!(
        last_block["cache_control"]["type"], "ephemeral",
        "last message's last content block must carry cache_control: ephemeral"
    );
}

#[test]
fn test_anthropic_no_cache_control_when_caching_disabled() {
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
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

    let body = request::build_request_body("claude-test", 1024, &request);
    let messages = body["messages"].as_array().unwrap();
    let last = messages.last().unwrap();
    let last_str = serde_json::to_string(last).unwrap();
    assert!(
        !last_str.contains("cache_control"),
        "no cache_control when enable_caching=false"
    );
}

#[test]
fn test_anthropic_ephemeral_notice_placement() {
    use crate::types::{ChatMessage, ChatRequest};
    use std::sync::Arc;

    let request = ChatRequest {
        messages: Arc::new(vec![
            ChatMessage::system("cached system"),
            ChatMessage::user("hello"),
        ]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: true,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: Some("[budget_notice]\n80% used\n[/budget_notice]".to_string()),
    };

    let body = super::request::build_request_body("claude-test", 1024, &request);
    let system = body["system"].as_array().expect("system must be an array when caching+notice");
    assert_eq!(system.len(), 2, "expected cached block + notice block");

    let cached = &system[0];
    assert_eq!(cached["text"], "cached system");
    assert_eq!(cached["cache_control"]["type"], "ephemeral");

    let notice = &system[1];
    assert!(notice["text"].as_str().unwrap().contains("budget_notice"));
    assert!(
        notice.get("cache_control").is_none(),
        "notice block must NOT carry cache_control"
    );
}

#[test]
fn test_anthropic_ephemeral_notice_placement_caching_off() {
    use crate::types::{ChatMessage, ChatRequest};
    use std::sync::Arc;

    let request = ChatRequest {
        messages: Arc::new(vec![
            ChatMessage::system("plain system"),
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
        ephemeral_system_notice: Some("[notice]".to_string()),
    };

    let body = super::request::build_request_body("claude-test", 1024, &request);
    let system = body["system"].as_array().expect("system must be an array when notice present");
    assert_eq!(system.len(), 2);
    assert_eq!(system[0]["text"], "plain system");
    assert!(system[0].get("cache_control").is_none());
    assert_eq!(system[1]["text"], "[notice]");
    assert!(system[1].get("cache_control").is_none());
}

#[test]
fn test_anthropic_ephemeral_notice_placement_empty_system() {
    use crate::types::{ChatMessage, ChatRequest};
    use std::sync::Arc;

    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: Some("[notice-only]".to_string()),
    };

    let body = super::request::build_request_body("claude-test", 1024, &request);
    let system = body["system"].as_array().expect("notice-only should be a single-element array");
    assert_eq!(system.len(), 1);
    assert_eq!(system[0]["text"], "[notice-only]");
    assert!(system[0].get("cache_control").is_none());
}

#[test]
fn test_anthropic_ephemeral_notice_placement_empty_system_with_caching() {
    use crate::types::{ChatMessage, ChatRequest};
    use std::sync::Arc;

    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: true,  // <-- the previously-broken case
        thinking: None,
        context_management: None,
        ephemeral_system_notice: Some("[budget_notice]\n80% used\n[/budget_notice]".to_string()),
    };

    let body = super::request::build_request_body("claude-test", 1024, &request);
    let system = body["system"].as_array().expect("system should be array when notice present");
    assert_eq!(system.len(), 1, "empty system + notice => single-element array");
    let only_block = &system[0];
    assert!(only_block["text"].as_str().unwrap().contains("budget_notice"));
    assert!(
        only_block.get("cache_control").is_none(),
        "notice-only system must NOT carry cache_control even when caching=true"
    );
}
