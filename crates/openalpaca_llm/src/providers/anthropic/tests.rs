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
