use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use openalpaca_llm::{ChatMessage, ChatRequest, FinishReason, LlmProvider, ToolDefinition};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    pub max_tool_runtime: Duration,
    pub max_cost: f64,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_rounds: 15,
            max_tools_per_round: 5,
            max_tool_runtime: Duration::from_secs(60),
            max_cost: 1.00,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub final_content: String,
    pub rounds_used: usize,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub tool_calls_made: usize,
    pub finish_reason: LoopFinishReason,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopFinishReason {
    Complete,
    MaxRounds,
    CostExceeded,
    Error(String),
}

/// Run the agentic loop.
///
/// When `sandbox` is `Some`, tool calls are routed through the SandboxManager
/// with capability checks, input sanitization, and timeout enforcement.
/// When `sandbox` is `None`, falls back to stub behavior (backward compat).
pub async fn run_agentic_loop(
    provider: &dyn LlmProvider,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
) -> LoopResult {
    let mut messages = initial_messages;
    let mut rounds = 0usize;
    let mut total_input = 0u32;
    let mut total_output = 0u32;
    let mut tool_calls_made = 0usize;

    loop {
        if rounds >= config.max_rounds {
            return LoopResult {
                final_content: String::new(),
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::MaxRounds,
            };
        }

        let estimated_cost = estimate_cost(total_input, total_output);
        if estimated_cost > config.max_cost {
            return LoopResult {
                final_content: String::new(),
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::CostExceeded,
            };
        }

        let request = ChatRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            model: None,
            temperature: None,
            max_tokens: None,
        };

        match provider.chat(request).await {
            Ok(response) => {
                total_input += response.usage.input_tokens;
                total_output += response.usage.output_tokens;
                rounds += 1;

                if response.finish_reason == FinishReason::ToolUse
                    && !response.tool_calls.is_empty()
                {
                    // Record assistant message with tool calls
                    messages.push(ChatMessage::assistant_with_tools(&response));

                    // Enforce max_tools_per_round
                    let calls_this_round = response.tool_calls.len().min(config.max_tools_per_round);

                    for tc in response.tool_calls.iter().take(calls_this_round) {
                        tool_calls_made += 1;

                        let result_text = if let (Some(sbx), Some(policy)) =
                            (sandbox, sandbox_policy)
                        {
                            // Route through sandbox
                            match sbx.execute_tool(agent_id, tc, policy).await {
                                Ok(output) => output,
                                Err(err) => format!("Error: {}", err),
                            }
                        } else {
                            // Fallback: stub
                            format!("Error: tool '{}' not yet implemented", tc.name)
                        };

                        messages.push(ChatMessage::tool_result(&tc.id, &result_text));
                    }

                    // If we truncated, add error for remaining tool calls
                    for tc in response.tool_calls.iter().skip(calls_this_round) {
                        messages.push(ChatMessage::tool_result(
                            &tc.id,
                            "Error: max tools per round exceeded",
                        ));
                    }

                    continue;
                }

                // No tool calls or stop -> done
                return LoopResult {
                    final_content: response.content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Complete,
                };
            }
            Err(e) => {
                return LoopResult {
                    final_content: String::new(),
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Error(e.to_string()),
                };
            }
        }
    }
}

fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    // Simplified pricing: $3/1M input, $15/1M output (Claude Sonnet range)
    (input_tokens as f64 * 3.0 / 1_000_000.0) + (output_tokens as f64 * 15.0 / 1_000_000.0)
}

#[cfg(test)]
mod tests {
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
                },
                finish_reason: FinishReason::Stop,
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
                },
                finish_reason: FinishReason::ToolUse,
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
                },
                finish_reason: FinishReason::ToolUse,
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

        let result = run_agentic_loop(&provider, messages, vec![], &config, None, "test", None).await;

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
            ..Default::default()
        };

        let result = run_agentic_loop(&provider, messages, vec![], &config, None, "test", None).await;

        assert_eq!(result.finish_reason, LoopFinishReason::MaxRounds);
        assert_eq!(result.rounds_used, 3);
    }

    #[tokio::test]
    async fn test_respects_cost_limit() {
        // Mock returns high token usage, should exceed cost limit
        let provider = MockProvider::new(vec![Ok(MockProvider::high_usage_response())]);
        let messages = vec![ChatMessage::user("expensive query")];
        let config = LoopConfig {
            max_cost: 0.50,
            ..Default::default()
        };

        let result = run_agentic_loop(&provider, messages, vec![], &config, None, "test", None).await;

        assert_eq!(result.finish_reason, LoopFinishReason::CostExceeded);
    }

    #[tokio::test]
    async fn test_handles_provider_error() {
        let provider = MockProvider::new(vec![Err(LlmError::Http(
            "connection refused".to_string(),
        ))]);
        let messages = vec![ChatMessage::user("hello")];
        let config = LoopConfig::default();

        let result = run_agentic_loop(&provider, messages, vec![], &config, None, "test", None).await;

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

        let result = run_agentic_loop(&provider, messages, vec![], &config, None, "test", None).await;

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

        let result = run_agentic_loop(&provider, messages, vec![], &config, None, "test", None).await;

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

        let sandbox = SandboxManager::new(
            std::sync::Arc::new(TestExecutor),
            EventBus::default(),
        );
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
            &provider, messages, vec![], &config,
            Some(&sandbox), "test_agent", Some(&policy),
        ).await;

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

        let sandbox = SandboxManager::new(
            std::sync::Arc::new(TestExecutor),
            EventBus::default(),
        );
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
            &provider, messages, vec![], &config,
            Some(&sandbox), "test_agent", Some(&policy),
        ).await;

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
            },
            finish_reason: FinishReason::ToolUse,
        };

        let provider = MockProvider::new(vec![
            Ok(multi_tool_response),
            Ok(MockProvider::simple_response("Done.")),
        ]);
        let messages = vec![ChatMessage::user("test")];
        let config = LoopConfig {
            max_tools_per_round: 2, // Only allow 2 per round
            ..Default::default()
        };

        let result = run_agentic_loop(&provider, messages, vec![], &config, None, "test", None).await;

        assert_eq!(result.finish_reason, LoopFinishReason::Complete);
        // Only 2 tool calls should have been counted (the 3rd was truncated)
        assert_eq!(result.tool_calls_made, 2);
    }
}
