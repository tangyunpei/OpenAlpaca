use super::*;
use async_trait::async_trait;
use openalpaca_llm::{ChatResponse, LlmError, Usage};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Mock provider for testing the agentic loop.
struct MockProvider {
    responses: Vec<Result<ChatResponse, LlmError>>,
    call_count: AtomicUsize,
}

impl MockProvider {
    fn new(responses: Vec<Result<ChatResponse, LlmError>>) -> Self {
        Self {
            responses,
            call_count: AtomicUsize::new(0),
        }
    }

    fn simple_response(content: &str) -> ChatResponse {
        ChatResponse {
            content: content.to_string(),
            tool_calls: vec![],
            model: "mock-model".to_string(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
            finish_reason: FinishReason::Stop,
            thinking: None,
        }
    }

    fn tool_use_response() -> ChatResponse {
        ChatResponse {
            content: "Using tool.".to_string(),
            tool_calls: vec![openalpaca_llm::ToolCall {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "test"}),
            }],
            model: "mock-model".to_string(),
            usage: Usage {
                input_tokens: 20,
                output_tokens: 15,
                ..Default::default()
            },
            finish_reason: FinishReason::ToolUse,
            thinking: None,
        }
    }

    fn high_usage_response() -> ChatResponse {
        ChatResponse {
            content: String::new(),
            tool_calls: vec![openalpaca_llm::ToolCall {
                id: "tc_x".to_string(),
                name: "expensive".to_string(),
                arguments: serde_json::json!({}),
            }],
            model: "mock-model".to_string(),
            usage: Usage {
                input_tokens: 100_000,
                output_tokens: 50_000,
                ..Default::default()
            },
            finish_reason: FinishReason::ToolUse,
            thinking: None,
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        if idx < self.responses.len() {
            self.responses[idx].clone()
        } else {
            // Repeat last response for overflow
            self.responses
                .last()
                .cloned()
                .unwrap_or(Err(LlmError::NotConfigured))
        }
    }
}

#[tokio::test]
async fn test_completes_simple_query() {
    let provider = MockProvider::new(vec![Ok(MockProvider::simple_response(
        r#"{"answer": "Hello!"}"#,
    ))]);
    let messages = vec![ChatMessage::user("hello")];
    let config = LoopConfig::default();

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.rounds_used, 1);
    assert_eq!(result.final_content, r#"{"answer": "Hello!"}"#);
    assert_eq!(result.tool_calls_made, 0);
}

#[tokio::test]
async fn test_respects_max_rounds() {
    // Mock always returns tool_use, so loop should hit max_rounds
    let provider = MockProvider::new(vec![Ok(MockProvider::tool_use_response())]);
    let messages = vec![ChatMessage::user("search forever")];
    let config = LoopConfig {
        max_rounds: 3,
        enable_caching: false,
        thinking: None,
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::MaxRounds);
    assert_eq!(result.rounds_used, 3);
    assert_eq!(result.final_content, "Using tool.");
}

#[tokio::test]
async fn test_respects_cost_limit() {
    // Mock returns high token usage, should exceed cost limit
    let provider = MockProvider::new(vec![Ok(MockProvider::high_usage_response())]);
    let messages = vec![ChatMessage::user("expensive query")];
    let config = LoopConfig {
        max_cost: 0.50,
        enable_caching: false,
        thinking: None,
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::CostExceeded);
}

#[tokio::test]
async fn test_handles_provider_error() {
    let provider = MockProvider::new(vec![Err(LlmError::Http("connection refused".to_string()))]);
    let messages = vec![ChatMessage::user("hello")];
    let config = LoopConfig::default();

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
    )
    .await;

    match result.finish_reason {
        LoopFinishReason::Error(msg) => assert!(msg.contains("connection refused")),
        other => panic!("Expected Error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_tool_stub_returns_error() {
    // First call returns tool_use, second returns stop
    let provider = MockProvider::new(vec![
        Ok(MockProvider::tool_use_response()),
        Ok(MockProvider::simple_response("Done.")),
    ]);
    let messages = vec![ChatMessage::user("search something")];
    let config = LoopConfig::default();

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.rounds_used, 2);
    assert_eq!(result.tool_calls_made, 1);
    assert_eq!(result.final_content, "Done.");
}

#[tokio::test]
async fn test_tracks_usage() {
    let provider = MockProvider::new(vec![
        Ok(MockProvider::tool_use_response()),
        Ok(MockProvider::simple_response("Final.")),
    ]);
    let messages = vec![ChatMessage::user("test")];
    let config = LoopConfig::default();

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
    )
    .await;

    assert_eq!(result.rounds_used, 2);
    assert_eq!(result.total_input_tokens, 30); // 20 + 10
    assert_eq!(result.total_output_tokens, 20); // 15 + 5
    assert_eq!(result.tool_calls_made, 1);
}

#[tokio::test]
async fn test_sandbox_execution() {
    use crate::bus::EventBus;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy, ToolExecutor};

    struct TestExecutor;

    #[async_trait]
    impl ToolExecutor for TestExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            Ok("sandbox result".to_string())
        }
        fn registered_tools(&self) -> Vec<String> {
            vec!["search".to_string()]
        }
    }

    let sandbox =
        SandboxManager::with_defaults(std::sync::Arc::new(TestExecutor), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "test_agent".to_string(),
        allowed_capabilities: vec![],
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
    };

    let provider = MockProvider::new(vec![
        Ok(MockProvider::tool_use_response()),
        Ok(MockProvider::simple_response("Done with sandbox.")),
    ]);
    let messages = vec![ChatMessage::user("test")];
    let config = LoopConfig::default();

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        Some(&sandbox),
        "test_agent",
        Some(&policy),
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.tool_calls_made, 1);
    assert_eq!(result.final_content, "Done with sandbox.");
}

#[tokio::test]
async fn test_sandbox_denied_tool() {
    use crate::bus::EventBus;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy, ToolExecutor};

    struct TestExecutor;

    #[async_trait]
    impl ToolExecutor for TestExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            Ok("should not reach".to_string())
        }
        fn registered_tools(&self) -> Vec<String> {
            vec!["search".to_string()]
        }
    }

    let sandbox =
        SandboxManager::with_defaults(std::sync::Arc::new(TestExecutor), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "test_agent".to_string(),
        allowed_capabilities: vec![],
        denied_capabilities: vec!["search".to_string()], // deny the tool
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
    };

    let provider = MockProvider::new(vec![
        Ok(MockProvider::tool_use_response()),
        Ok(MockProvider::simple_response("Fallback.")),
    ]);
    let messages = vec![ChatMessage::user("test")];
    let config = LoopConfig::default();

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        Some(&sandbox),
        "test_agent",
        Some(&policy),
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.tool_calls_made, 1);
    // The LLM gets back an error and responds with "Fallback."
    assert_eq!(result.final_content, "Fallback.");
}

#[tokio::test]
async fn test_max_tools_per_round_enforced() {
    // Create a response with 3 tool calls
    let multi_tool_response = ChatResponse {
        content: "Using tools.".to_string(),
        tool_calls: vec![
            openalpaca_llm::ToolCall {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({}),
            },
            openalpaca_llm::ToolCall {
                id: "tc_2".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({}),
            },
            openalpaca_llm::ToolCall {
                id: "tc_3".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({}),
            },
        ],
        model: "mock-model".to_string(),
        usage: Usage {
            input_tokens: 20,
            output_tokens: 15,
            ..Default::default()
        },
        finish_reason: FinishReason::ToolUse,
        thinking: None,
    };

    let provider = MockProvider::new(vec![
        Ok(multi_tool_response),
        Ok(MockProvider::simple_response("Done.")),
    ]);
    let messages = vec![ChatMessage::user("test")];
    let config = LoopConfig {
        max_tools_per_round: 2, // Only allow 2 per round
        enable_caching: false,
        thinking: None,
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    // Only 2 tool calls should have been counted (the 3rd was truncated)
    assert_eq!(result.tool_calls_made, 2);
}

#[tokio::test]
async fn test_cancellation_before_first_round() {
    let provider = MockProvider::new(vec![Ok(MockProvider::simple_response("Never reached"))]);
    let messages = vec![ChatMessage::user("hello")];
    let config = LoopConfig::default();

    let token = CancellationToken::new();
    token.cancel(); // pre-cancel before loop starts

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        Some(token),
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Cancelled);
    assert_eq!(result.rounds_used, 0);
    assert_eq!(result.total_input_tokens, 0);
    assert_eq!(result.total_output_tokens, 0);
}

#[tokio::test]
async fn test_cancellation_during_tool_execution() {
    use crate::bus::EventBus;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy, ToolExecutor};

    /// Executor that cancels the token when a tool runs, simulating
    /// cancellation arriving mid-parallel-execution.
    struct CancellingExecutor {
        token: CancellationToken,
    }

    #[async_trait]
    impl ToolExecutor for CancellingExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            self.token.cancel();
            // Yield so tokio::select! can observe the cancellation
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok("should be dropped".to_string())
        }
        fn registered_tools(&self) -> Vec<String> {
            vec!["search".to_string()]
        }
    }

    let token = CancellationToken::new();
    let executor = CancellingExecutor {
        token: token.clone(),
    };
    let sandbox =
        SandboxManager::with_defaults(std::sync::Arc::new(executor), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "test_agent".to_string(),
        allowed_capabilities: vec![],
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
    };

    let provider = MockProvider::new(vec![
        Ok(MockProvider::tool_use_response()),
        Ok(MockProvider::simple_response("Never reached")),
    ]);
    let messages = vec![ChatMessage::user("test")];
    let config = LoopConfig::default();

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        Some(&sandbox),
        "test_agent",
        Some(&policy),
        Some(token),
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Cancelled);
    assert_eq!(result.rounds_used, 1); // LLM call completed, then cancelled during tools
}
