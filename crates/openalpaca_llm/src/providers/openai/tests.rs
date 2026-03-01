use super::*;
use reqwest::header::{HeaderMap, HeaderValue};

#[test]
fn test_request_format() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let request = ChatRequest {
        messages: vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("Hello"),
        ],
        tools: vec![],
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
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
        messages: vec![ChatMessage::user_with_parts(vec![
            ContentPart::Text {
                text: "".to_string(),
            },
            ContentPart::Image {
                source: ImageSource::Url {
                    url: "https://example.com/test.jpg".to_string(),
                },
                detail: None,
            },
        ])],
        tools: vec![],
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
    assert_eq!(blocks[0]["type"], "image_url");
}

#[test]
fn test_request_serialization_empty_parts_get_placeholder() {
    let provider = OpenAiProvider::new("test-key".to_string(), None, None, None);
    let request = ChatRequest {
        messages: vec![ChatMessage::user_with_parts(vec![ContentPart::Text {
            text: "  \n\t".to_string(),
        }])],
        tools: vec![],
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
