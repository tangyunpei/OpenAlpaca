use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use openalpaca_llm::{
    ChatMessage, ChatRequest, FinishReason, LlmProvider, LlmRouter, RequestContext,
    RouterRequest, ToolDefinition,
};
use std::time::Duration;

/// Maximum tool result size before truncation (32 KB).
const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024;

/// Truncate tool result text if it exceeds the byte limit to prevent blowing
/// up the LLM context window. Uses byte-aware truncation at char boundaries.
fn truncate_tool_result(text: String) -> String {
    if text.len() <= MAX_TOOL_RESULT_SIZE {
        return text;
    }
    // Find the nearest char boundary at or before MAX_TOOL_RESULT_SIZE bytes
    let mut end = MAX_TOOL_RESULT_SIZE;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[... truncated: showing first {} of {} bytes]",
        &text[..end],
        end,
        text.len()
    )
}

#[derive(Debug, Clone)]
pub struct LoopConfig {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    pub max_tool_runtime: Duration,
    pub max_cost: f64,
    /// Override model for this loop (used by LlmRouter).
    pub model: Option<String>,
    /// Fallback models (informational — fallback is handled by router's fallback chain).
    pub fallback_models: Vec<String>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_rounds: 15,
            max_tools_per_round: 5,
            max_tool_runtime: Duration::from_secs(60),
            max_cost: 1.00,
            model: None,
            fallback_models: Vec::new(),
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
    /// Actual model from API response (may differ from requested due to fallback).
    pub model_used: Option<String>,
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
    let mut last_assistant_content = String::new();
    let mut last_model: Option<String> = None;

    tracing::info!(
        agent_id = agent_id,
        tools_count = tools.len(),
        max_rounds = config.max_rounds,
        max_cost = config.max_cost,
        "Agentic loop started (direct provider)"
    );

    loop {
        if rounds >= config.max_rounds {
            tracing::info!(agent_id = agent_id, rounds = rounds, "Agentic loop exiting: max rounds reached");
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::MaxRounds,
                model_used: last_model.clone(),
            };
        }

        let estimated_cost = estimate_cost(total_input, total_output);
        if estimated_cost > config.max_cost {
            tracing::info!(agent_id = agent_id, rounds = rounds, "Agentic loop exiting: cost limit exceeded");
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::CostExceeded,
                model_used: last_model.clone(),
            };
        }

        let request = ChatRequest {
            messages: messages.clone(),
            tools: tools.clone(),
            model: None,
            temperature: None,
            max_tokens: None,
        };

        tracing::debug!(agent_id = agent_id, round = rounds + 1, messages_count = messages.len(), "LLM call starting");

        match provider.chat(request).await {
            Ok(response) => {
                total_input += response.usage.input_tokens;
                total_output += response.usage.output_tokens;
                rounds += 1;
                last_model = Some(response.model.clone());

                tracing::debug!(
                    agent_id = agent_id,
                    round = rounds,
                    model = %response.model,
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    finish_reason = ?response.finish_reason,
                    "LLM call completed"
                );

                // Capture last content before any branching
                if !response.content.is_empty() {
                    last_assistant_content = response.content.clone();
                }

                if response.finish_reason == FinishReason::ToolUse
                    && !response.tool_calls.is_empty()
                {
                    // Record assistant message with tool calls
                    messages.push(ChatMessage::assistant_with_tools(&response));

                    // Enforce max_tools_per_round
                    let calls_this_round = response.tool_calls.len().min(config.max_tools_per_round);

                    for tc in response.tool_calls.iter().take(calls_this_round) {
                        // Enforce max_tool_calls from sandbox policy
                        if let Some(policy) = sandbox_policy {
                            if let Some(max_calls) = policy.max_tool_calls {
                                if tool_calls_made >= max_calls as usize {
                                    messages.push(ChatMessage::tool_result(
                                        &tc.id,
                                        "[tool_error] max_tool_calls limit reached — no more tool calls allowed",
                                    ));
                                    continue;
                                }
                            }
                        }

                        tool_calls_made += 1;

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            tool_call_number = tool_calls_made,
                            "Executing tool"
                        );

                        // Tool error convention: errors are prefixed with [tool_error]
                        // so the LLM sees a consistent format regardless of the tool.
                        let result_text = if let (Some(sbx), Some(policy)) =
                            (sandbox, sandbox_policy)
                        {
                            // Route through sandbox
                            match sbx.execute_tool(agent_id, tc, policy).await {
                                Ok(output) => truncate_tool_result(output),
                                Err(err) => truncate_tool_result(format!("[tool_error] {}", err)),
                            }
                        } else {
                            tracing::warn!(
                                agent_id = agent_id,
                                tool = tc.name,
                                "Sandbox not configured — returning stub for tool call (misconfiguration?)"
                            );
                            format!("[tool_error] tool '{}' not available — sandbox not configured", tc.name)
                        };

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            success = !result_text.starts_with("[tool_error]"),
                            result_len = result_text.len(),
                            "Tool execution completed"
                        );

                        messages.push(ChatMessage::tool_result(&tc.id, &result_text));
                    }

                    // If we truncated, add error for remaining tool calls
                    for tc in response.tool_calls.iter().skip(calls_this_round) {
                        messages.push(ChatMessage::tool_result(
                            &tc.id,
                            "[tool_error] max tools per round exceeded",
                        ));
                    }

                    continue;
                }

                // No tool calls or stop -> done
                tracing::info!(
                    agent_id = agent_id,
                    rounds = rounds,
                    total_input_tokens = total_input,
                    total_output_tokens = total_output,
                    tool_calls = tool_calls_made,
                    content_len = response.content.len(),
                    "Agentic loop completed successfully"
                );
                return LoopResult {
                    final_content: response.content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Complete,
                    model_used: last_model.clone(),
                };
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = agent_id,
                    rounds = rounds,
                    error = %e,
                    "Agentic loop exiting: LLM error"
                );
                return LoopResult {
                    final_content: last_assistant_content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Error(e.to_string()),
                    model_used: last_model.clone(),
                };
            }
        }
    }
}

/// Run the agentic loop using the LlmRouter (multi-provider, multi-key).
///
/// Same bounded-loop logic as `run_agentic_loop`, but uses `RouterRequest`
/// and `router.complete()` with key rotation, fallback chains, and cost tracking.
#[allow(clippy::too_many_arguments)]
pub async fn run_agentic_loop_routed(
    router: &LlmRouter,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    task_id: Option<&str>,
) -> LoopResult {
    let mut messages = initial_messages;
    let mut rounds = 0usize;
    let mut total_input = 0u32;
    let mut total_output = 0u32;
    let mut tool_calls_made = 0usize;
    let mut last_assistant_content = String::new();
    let mut last_model: Option<String> = None;

    let context = RequestContext {
        agent_id: Some(agent_id.to_string()),
        task_id: task_id.map(|s| s.to_string()),
    };

    tracing::info!(
        agent_id = agent_id,
        tools_count = tools.len(),
        max_rounds = config.max_rounds,
        max_cost = config.max_cost,
        "Agentic loop started"
    );

    loop {
        if rounds >= config.max_rounds {
            tracing::info!(agent_id = agent_id, rounds = rounds, "Agentic loop exiting: max rounds reached");
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::MaxRounds,
                model_used: last_model.clone(),
            };
        }

        // Check cost via router's cost tracker (agent-level)
        if let Some(usage) = router.cost_tracker.get_agent_usage(agent_id).await
            && usage.total_cost_usd > config.max_cost
        {
            tracing::info!(agent_id = agent_id, rounds = rounds, "Agentic loop exiting: cost limit exceeded");
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::CostExceeded,
                model_used: last_model.clone(),
            };
        }

        // Also check simple estimate as a fallback
        let estimated_cost = estimate_cost(total_input, total_output);
        if estimated_cost > config.max_cost {
            tracing::info!(agent_id = agent_id, rounds = rounds, "Agentic loop exiting: cost limit exceeded (estimate)");
            return LoopResult {
                final_content: last_assistant_content,
                rounds_used: rounds,
                total_input_tokens: total_input,
                total_output_tokens: total_output,
                tool_calls_made,
                finish_reason: LoopFinishReason::CostExceeded,
                model_used: last_model.clone(),
            };
        }

        let request = RouterRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            temperature: None,
            max_tokens: None,
            context: context.clone(),
        };

        tracing::debug!(agent_id = agent_id, round = rounds + 1, messages_count = messages.len(), "LLM call starting");

        match router.complete(request).await {
            Ok(response) => {
                total_input += response.usage.input_tokens;
                total_output += response.usage.output_tokens;
                rounds += 1;
                last_model = Some(response.model.clone());

                tracing::debug!(
                    agent_id = agent_id,
                    round = rounds,
                    model = %response.model,
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    finish_reason = ?response.finish_reason,
                    "LLM call completed"
                );

                // Capture last content before any branching
                if !response.content.is_empty() {
                    last_assistant_content = response.content.clone();
                }

                if response.finish_reason == FinishReason::ToolUse
                    && !response.tool_calls.is_empty()
                {
                    messages.push(ChatMessage::assistant_with_tools(&response));

                    let calls_this_round =
                        response.tool_calls.len().min(config.max_tools_per_round);

                    for tc in response.tool_calls.iter().take(calls_this_round) {
                        // Enforce max_tool_calls from sandbox policy
                        if let Some(policy) = sandbox_policy {
                            if let Some(max_calls) = policy.max_tool_calls {
                                if tool_calls_made >= max_calls as usize {
                                    messages.push(ChatMessage::tool_result(
                                        &tc.id,
                                        "[tool_error] max_tool_calls limit reached — no more tool calls allowed",
                                    ));
                                    continue;
                                }
                            }
                        }

                        tool_calls_made += 1;

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            tool_call_number = tool_calls_made,
                            "Executing tool"
                        );

                        let result_text = if let (Some(sbx), Some(policy)) =
                            (sandbox, sandbox_policy)
                        {
                            match sbx.execute_tool(agent_id, tc, policy).await {
                                Ok(output) => truncate_tool_result(output),
                                Err(err) => truncate_tool_result(format!("[tool_error] {}", err)),
                            }
                        } else {
                            tracing::warn!(
                                agent_id = agent_id,
                                tool = tc.name,
                                "Sandbox not configured — returning stub for tool call (misconfiguration?)"
                            );
                            format!("[tool_error] tool '{}' not available — sandbox not configured", tc.name)
                        };

                        tracing::debug!(
                            agent_id = agent_id,
                            round = rounds,
                            tool = %tc.name,
                            success = !result_text.starts_with("[tool_error]"),
                            result_len = result_text.len(),
                            "Tool execution completed"
                        );

                        messages.push(ChatMessage::tool_result(&tc.id, &result_text));
                    }

                    for tc in response.tool_calls.iter().skip(calls_this_round) {
                        messages.push(ChatMessage::tool_result(
                            &tc.id,
                            "[tool_error] max tools per round exceeded",
                        ));
                    }

                    continue;
                }

                tracing::info!(
                    agent_id = agent_id,
                    rounds = rounds,
                    total_input_tokens = total_input,
                    total_output_tokens = total_output,
                    tool_calls = tool_calls_made,
                    content_len = response.content.len(),
                    "Agentic loop completed successfully"
                );
                return LoopResult {
                    final_content: response.content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Complete,
                    model_used: last_model.clone(),
                };
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = agent_id,
                    rounds = rounds,
                    error = %e,
                    "Agentic loop exiting: LLM error"
                );
                return LoopResult {
                    final_content: last_assistant_content,
                    rounds_used: rounds,
                    total_input_tokens: total_input,
                    total_output_tokens: total_output,
                    tool_calls_made,
                    finish_reason: LoopFinishReason::Error(e.to_string()),
                    model_used: last_model.clone(),
                };
            }
        }
    }
}

/// Fallback cost rates for the non-routed (test) path.
/// Claude Sonnet pricing as conservative upper bound.
const FALLBACK_INPUT_RATE: f64 = 3.0; // $ per 1M tokens
const FALLBACK_OUTPUT_RATE: f64 = 15.0; // $ per 1M tokens

fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    (input_tokens as f64 * FALLBACK_INPUT_RATE / 1_000_000.0)
        + (output_tokens as f64 * FALLBACK_OUTPUT_RATE / 1_000_000.0)
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
                    ..Default::default()
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
                    ..Default::default()
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
                    ..Default::default()
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
        assert_eq!(result.final_content, "Using tool.");
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

        let sandbox = SandboxManager::with_defaults(
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

        let sandbox = SandboxManager::with_defaults(
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
                ..Default::default()
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
