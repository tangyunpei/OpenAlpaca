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

/// OUTPUT-BUDGET (item 8): a request that names no `max_tokens` asks for
/// 16,000 output tokens, the owner's figure ("budget can be 16000 or
/// higher"), where it asked for 4096.
#[test]
fn a_request_naming_no_budget_asks_for_sixteen_thousand_tokens() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("Hello")]),
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
    assert_eq!(provider.build_request_body(&request)["max_tokens"], 16_000);
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
        thinking: Some(ThinkingConfig::Adaptive {
            display: ThinkingDisplay::ModelDefault,
        }),
        context_management: None,
        ephemeral_system_notice: None,
    };
    let body = provider.build_request_body(&request);
    assert_eq!(body["thinking"], serde_json::json!({"type": "adaptive"}));
    // Temperature must be removed for adaptive thinking too
    assert!(body.get("temperature").is_none());

    // A row whose model omits its thinking by default is asked for the
    // summary (ANTHROPIC-THINKING).
    let summarized = ChatRequest {
        thinking: Some(ThinkingConfig::Adaptive {
            display: ThinkingDisplay::Summarized,
        }),
        ..request
    };
    assert_eq!(
        provider.build_request_body(&summarized)["thinking"],
        serde_json::json!({"type": "adaptive", "display": "summarized"})
    );
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
    use crate::context_management::{ContextEdit, ContextManagement, ThinkingKept};

    let request = ChatRequest {
        messages: Arc::new(vec![ChatMessage::user("hello")]),
        tools: Arc::new(vec![]),
        model: Some("claude-sonnet-4-6".to_string()),
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: Some(ContextManagement {
            edits: vec![
                ContextEdit::ClearThinking {
                    keep: ThinkingKept::All,
                },
                ContextEdit::ClearToolUses {
                    trigger_tokens: 100_000,
                    keep_tool_uses: 5,
                },
            ],
        }),
        ephemeral_system_notice: None,
    };

    let body = request::build_request_body("claude-sonnet-4-6", 4096, &request);
    assert!(body.get("context_management").is_some());
    let edits = body["context_management"]["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0]["type"], "clear_thinking_20251015");
    assert_eq!(edits[0]["keep"], "all");
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

    let body = request::build_request_body("claude-sonnet-4-6", 4096, &request);
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

// ── ANTHROPIC-THINKING: the budget notice is the last message ─────────────
//
// Anthropic's pages, read 2026-11-14: "Put every update in a new message at
// the end" and keep `system` the same on every request (.../build-with-claude/
// preserved-thinking/prefix-checks); an updated value goes "in its own text
// block of the user message you're about to send, after any tool results"
// (.../preserved-thinking/change-tools-or-instructions); "Consecutive `user`
// or `assistant` turns in your request will be combined into a single turn"
// (.../api/messages).

/// A request in a tool loop: a system prompt, a tool call and its result.
fn notice_request(enable_caching: bool, notice: Option<&str>) -> ChatRequest {
    ChatRequest {
        messages: Arc::new(vec![
            ChatMessage::system("plain system"),
            ChatMessage::user("hello"),
            ChatMessage {
                tool_calls: Some(vec![crate::types::ToolCall {
                    id: "tc_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "x"}),
                }]),
                ..ChatMessage::assistant("")
            },
            ChatMessage::tool_result("tc_1", "found"),
        ]),
        tools: Arc::new(vec![]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: notice.map(str::to_string),
    }
}

/// The notice goes as the request's last message, a user text block after
/// the tool result, and `system` is what it is without one.
#[test]
fn the_budget_notice_is_the_last_message_and_not_in_system() {
    let with = request::build_request_body(
        "claude-test",
        1024,
        &notice_request(false, Some("[notice]")),
    );
    let without = request::build_request_body("claude-test", 1024, &notice_request(false, None));

    assert_eq!(with["system"], "plain system", "{with}");
    assert_eq!(with["system"], without["system"]);
    let messages = with["messages"].as_array().unwrap();
    let sent_without = without["messages"].as_array().unwrap();
    assert_eq!(messages.len(), sent_without.len() + 1);
    assert_eq!(&messages[..sent_without.len()], &sent_without[..]);
    assert_eq!(
        messages.last().unwrap(),
        &serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "[notice]"}],
        })
    );
    assert!(!with["system"].to_string().contains("[notice]"));
}

/// With caching on, the cached prefix is the request's without the notice:
/// the system block and the tool result carry `cache_control`, the notice
/// after them none.
#[test]
fn the_budget_notice_follows_the_cached_tail_and_carries_no_cache_control() {
    let with = request::build_request_body(
        "claude-test",
        1024,
        &notice_request(true, Some("[budget_notice]\n80% used\n[/budget_notice]")),
    );
    let without = request::build_request_body("claude-test", 1024, &notice_request(true, None));

    assert_eq!(with["system"], without["system"]);
    assert_eq!(
        with["system"],
        serde_json::json!([{
            "type": "text",
            "text": "plain system",
            "cache_control": {"type": "ephemeral"},
        }])
    );
    let messages = with["messages"].as_array().unwrap();
    let sent_without = without["messages"].as_array().unwrap();
    assert_eq!(&messages[..sent_without.len()], &sent_without[..]);
    assert_eq!(
        messages[messages.len() - 2]["content"][0]["cache_control"]["type"],
        "ephemeral",
        "the tool result keeps the tail breakpoint"
    );
    let notice = messages.last().unwrap();
    assert_eq!(notice["role"], "user");
    assert!(
        !notice.to_string().contains("cache_control"),
        "the notice carries no cache_control: {notice}"
    );
}

/// With no system prompt, a notice adds none: it is the last message.
#[test]
fn a_notice_with_no_system_prompt_sends_no_system() {
    for caching in [false, true] {
        let request = ChatRequest {
            messages: Arc::new(vec![ChatMessage::user("hello")]),
            ..notice_request(caching, Some("[notice-only]"))
        };
        let body = request::build_request_body("claude-test", 1024, &request);
        assert!(body.get("system").is_none(), "caching {caching}: {body}");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1],
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "[notice-only]"}],
            })
        );
    }
}

/// A request without the notice is sent byte for byte as before
/// (characterization: the parent's body, captured at `df260848`).
#[test]
fn a_request_without_the_notice_is_sent_byte_for_byte_as_before() {
    let request = ChatRequest {
        temperature: Some(0.5),
        tools: Arc::new(vec![crate::types::ToolDefinition {
            name: "search".to_string(),
            description: "Search.".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            ..Default::default()
        }]),
        ..notice_request(true, None)
    };
    let body = request::build_request_body("claude-test", 1024, &request);
    assert_eq!(serde_json::to_string(&body).unwrap(), GOLDEN_NO_NOTICE);
}

const GOLDEN_NO_NOTICE: &str = r#"{"max_tokens":1024,"messages":[{"content":"hello","role":"user"},{"content":[{"id":"tc_1","input":{"q":"x"},"name":"search","type":"tool_use"}],"role":"assistant"},{"content":[{"cache_control":{"type":"ephemeral"},"content":"found","tool_use_id":"tc_1","type":"tool_result"}],"role":"user"}],"model":"claude-test","system":[{"cache_control":{"type":"ephemeral"},"text":"plain system","type":"text"}],"temperature":0.5,"tools":[{"cache_control":{"type":"ephemeral"},"description":"Search.","input_schema":{"type":"object"},"name":"search"}]}"#;

/// The loop's `context_management` is sent under the context-editing page's
/// beta header, `context-management-2025-06-27` (.../build-with-claude/
/// context-editing, read 2026-11-14); a request without it sends none.
#[test]
fn the_context_management_beta_header_is_the_pages() {
    use crate::context_management::{ContextEdit, ContextManagement};

    let mut request = notice_request(false, None);
    assert_eq!(build_beta_header(&request), "");
    request.context_management = Some(ContextManagement {
        edits: vec![ContextEdit::ClearToolUses {
            trigger_tokens: 100_000,
            keep_tool_uses: 5,
        }],
    });
    assert_eq!(build_beta_header(&request), "context-management-2025-06-27");
}

/// The conversation a round with two tool calls leaves: one call answered,
/// one failed. The failed result is built from its wire form so the shape
/// under test is what a message carries, not how a constructor spells it.
fn one_answered_one_failed() -> Vec<ChatMessage> {
    let failed: ChatMessage = serde_json::from_value(serde_json::json!({
        "role": "tool",
        "content": "[tool_error] boom",
        "tool_call_id": "tc_bad",
        "is_error": true,
    }))
    .unwrap();
    vec![
        ChatMessage::user("go"),
        ChatMessage {
            tool_calls: Some(vec![
                crate::types::ToolCall {
                    id: "tc_ok".to_string(),
                    name: "file_read".to_string(),
                    arguments: serde_json::json!({"path": "a.txt"}),
                },
                crate::types::ToolCall {
                    id: "tc_bad".to_string(),
                    name: "shell_execute".to_string(),
                    arguments: serde_json::json!({"command": "make"}),
                },
            ]),
            ..ChatMessage::assistant("")
        },
        ChatMessage::tool_result("tc_ok", "fine"),
        failed,
    ]
}

/// N-TOOL-4 (Z1-13): a failed tool's `tool_result` carries Anthropic's
/// `is_error`, and a successful one is sent exactly as before — no such key.
#[test]
fn a_failed_tool_result_carries_is_error_and_a_successful_one_is_unchanged() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let request = ChatRequest {
        messages: Arc::new(one_answered_one_failed()),
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
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages[2],
        serde_json::json!({
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "tc_ok", "content": "fine"}],
        }),
        "a successful result is sent as it always was"
    );
    assert_eq!(
        messages[3],
        serde_json::json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": "tc_bad",
                "content": "[tool_error] boom",
                "is_error": true,
            }],
        }),
        "a failed result says so in Anthropic's own field"
    );
}

// ── CACHE-MULT: Anthropic's cache reads at the row's rate ───────────────

/// Claude Opus 5.5's row (the pricing page, read 2026-11-14): $4 input,
/// "5m cache writes" $5, "Cache hits and refreshes" $0.20 ("0.05x the base
/// input price"), $20 output. 1,000 uncached, 7,000 written and 4,000 read
/// input tokens and 1,000 output: $0.004 + $0.035 + $0.0008 + $0.02.
const OPUS_5_5_COST: f64 = 0.004 + 0.035 + 0.0008 + 0.02;

fn priced(model: &str, usage: &Usage) -> f64 {
    crate::routing::CostTracker::new(crate::routing::ModelRegistry::with_defaults())
        .calculate_cost_with_cache(
            model,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_creation_input_tokens,
            usage.cache_read_input_tokens,
        )
}

/// CACHE-MULT: a whole answer's cache reads and writes are priced at the
/// row's rates. Red on the parent, which read at 0.1x ($0.40 a million).
#[test]
fn a_whole_answers_cache_reads_and_writes_are_priced_by_the_row() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let response = provider
        .parse_response(serde_json::json!({
            "id": "msg_cache_priced",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": "Hi."}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 1000,
                "cache_creation_input_tokens": 7000,
                "cache_read_input_tokens": 4000
            }
        }))
        .unwrap();
    let cost = priced("claude-opus-5-5", &response.usage);
    assert!((cost - OPUS_5_5_COST).abs() < 1e-12, "cost={cost}");
}

/// The same on the stream: `message_start`'s usage carries the cache
/// counts, `message_delta`'s the output. Red on the parent.
#[tokio::test]
async fn a_streamed_answers_cache_reads_and_writes_are_priced_by_the_row() {
    let raw = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1000,\"cache_creation_input_tokens\":7000,\"cache_read_input_tokens\":4000}}}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi.\"}}\n",
        "\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1000}}\n",
        "\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n",
        "\n",
    );
    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let response = crate::streaming::collect_stream(
        Box::pin(parse_anthropic_sse(byte_stream)),
        "claude-opus-5-5".to_string(),
    )
    .await
    .expect("the stream collects");
    assert_eq!(response.usage.input_tokens, 12_000);
    assert_eq!(response.usage.cache_creation_input_tokens, 7_000);
    assert_eq!(response.usage.cache_read_input_tokens, 4_000);
    let cost = priced("claude-opus-5-5", &response.usage);
    assert!((cost - OPUS_5_5_COST).abs() < 1e-12, "cost={cost}");
}

/// MODELS-LATEST (the design's §4.9.5): the arm's model for a request that
/// names none is a compiled row, `claude-sonnet-5-5`. Red on the parent's
/// `claude-sonnet-4-6`, a retired row.
#[test]
fn the_arms_default_model_is_the_latest_sonnet() {
    assert_eq!(DEFAULT_MODEL, "claude-sonnet-5-5");
    assert!(
        crate::routing::model_registry::ModelRegistry::with_defaults()
            .get_model_info(DEFAULT_MODEL)
            .is_some()
    );
}
