use super::*;
use async_trait::async_trait;
use openalpaca_llm::{ChatResponse, LlmError, LlmRouter, ProviderType, Usage};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::orchestrator::skill::constraints::*;
use crate::runner::LoopCostAccumulator;
use crate::tools::registry::ToolContext;

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
            parts: None,
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
            parts: None,
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
            parts: None,
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
        None, // context_budget
        None,
        None, // tool_context
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
        None, // context_budget
        None,
        None, // tool_context
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
        None, // context_budget
        None,
        None, // tool_context
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
        None, // context_budget
        None,
        None, // tool_context
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
        None, // context_budget
        None,
        None, // tool_context
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
        None, // context_budget
        None,
        None, // tool_context
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
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use crate::tools::ToolRegistry;

    struct TestTool;

    #[async_trait]
    impl BuiltInTool for TestTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("sandbox result".to_string())
        }
    }

    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(std::sync::Arc::new(TestTool)),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();
    let sandbox =
        SandboxManager::with_defaults(std::sync::Arc::new(registry), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "test_agent".to_string(),
        allowed_capabilities: vec![],
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
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
        None, // context_budget
        None,
        None, // tool_context
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.tool_calls_made, 1);
    assert_eq!(result.final_content, "Done with sandbox.");
}

#[tokio::test]
async fn test_sandbox_denied_tool() {
    use crate::bus::EventBus;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use crate::tools::ToolRegistry;

    struct TestTool;

    #[async_trait]
    impl BuiltInTool for TestTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("should not reach".to_string())
        }
    }

    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(std::sync::Arc::new(TestTool)),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();
    let sandbox =
        SandboxManager::with_defaults(std::sync::Arc::new(registry), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "test_agent".to_string(),
        allowed_capabilities: vec![],
        denied_capabilities: vec!["search".to_string()], // deny the tool
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
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
        None, // context_budget
        None,
        None, // tool_context
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
        parts: None,
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
        None, // context_budget
        None,
        None, // tool_context
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
        None, // context_budget
        Some(token),
        None, // tool_context
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
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use crate::tools::ToolRegistry;

    /// Tool that cancels the token when executed, simulating
    /// cancellation arriving mid-parallel-execution.
    struct CancellingTool {
        token: CancellationToken,
    }

    #[async_trait]
    impl BuiltInTool for CancellingTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            self.token.cancel();
            // Yield so tokio::select! can observe the cancellation
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok("should be dropped".to_string())
        }
    }

    let token = CancellationToken::new();
    let cancelling_tool = CancellingTool {
        token: token.clone(),
    };
    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(std::sync::Arc::new(cancelling_tool)),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();
    let sandbox =
        SandboxManager::with_defaults(std::sync::Arc::new(registry), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "test_agent".to_string(),
        allowed_capabilities: vec![],
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
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
        None, // context_budget
        Some(token),
        None, // tool_context
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Cancelled);
    assert_eq!(result.rounds_used, 1); // LLM call completed, then cancelled during tools
}

// ─── context_threshold tests ──────────────────────────────────────────

#[test]
fn test_context_threshold_custom() {
    use openalpaca_llm::{ModelInfo, ModelRegistry, ProviderType};
    use std::collections::HashMap;

    let mut models = HashMap::new();
    models.insert(
        "test-model".to_string(),
        ModelInfo {
            provider: ProviderType::Anthropic,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 100_000,
            discovered: false,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
        },
    );
    let registry = ModelRegistry::new(models);

    // Custom threshold of 0.8 → expect 100_000 * 0.8 = 80_000
    let config = LoopConfig {
        context_threshold: 0.8,
        ..Default::default()
    };
    let config = config.with_context_window(&registry, Some("test-model"));
    assert_eq!(config.max_context_tokens, 80_000);

    // Default threshold of 0.6 → expect 100_000 * 0.6 = 60_000
    let config = LoopConfig::default().with_context_window(&registry, Some("test-model"));
    assert_eq!(config.max_context_tokens, 60_000);
}

// ─── max_tokens recovery tests ────────────────────────────────────────

#[tokio::test]
async fn test_max_tokens_triggers_continuation() {
    // Round 1: MaxTokens → should inject continuation and retry
    // Round 2: Complete → done
    let truncated = ChatResponse {
        content: "Partial output...".to_string(),
        tool_calls: vec![],
        model: "mock-model".to_string(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 10,
            ..Default::default()
        },
        finish_reason: FinishReason::MaxTokens,
        thinking: None,
        parts: None,
    };
    let provider = MockProvider::new(vec![
        Ok(truncated),
        Ok(MockProvider::simple_response("Completed.")),
    ]);
    let config = LoopConfig {
        enable_caching: false,
        ..Default::default()
    };
    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("test")],
        vec![],
        &config,
        None,
        "test",
        None,
        None, // context_budget
        None,
        None, // tool_context
    )
    .await;
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.rounds_used, 2); // 1 retry + 1 completion
}

#[tokio::test]
async fn test_max_tokens_retries_exhausted_returns_truncated() {
    // All 3 rounds return MaxTokens → exhausted after 2 retries
    let make_truncated = || ChatResponse {
        content: "Partial...".to_string(),
        tool_calls: vec![],
        model: "mock-model".to_string(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 10,
            ..Default::default()
        },
        finish_reason: FinishReason::MaxTokens,
        thinking: None,
        parts: None,
    };
    let provider = MockProvider::new(vec![
        Ok(make_truncated()),
        Ok(make_truncated()),
        Ok(make_truncated()),
    ]);
    let config = LoopConfig {
        enable_caching: false,
        ..Default::default()
    };
    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("test")],
        vec![],
        &config,
        None,
        "test",
        None,
        None, // context_budget
        None,
        None, // tool_context
    )
    .await;
    assert_eq!(result.finish_reason, LoopFinishReason::Truncated);
    assert_eq!(result.rounds_used, 3);
}

// ─── tool error hint tests ────────────────────────────────────────────

#[test]
fn test_tool_error_hint_file_read() {
    let result = format_tool_error_with_hint("file_read", "not found: /foo/bar.txt");
    assert!(result.starts_with("[tool_error] not found:"));
    assert!(result.contains("Hint: verify the path exists"));
}

#[test]
fn test_tool_error_hint_no_match_returns_plain() {
    let result = format_tool_error_with_hint("unknown_tool", "something broke");
    assert_eq!(result, "[tool_error] something broke");
    assert!(!result.contains("Hint:"));
}

// ─── cost warning tests ───────────────────────────────────────────────

#[tokio::test]
async fn test_cost_warning_at_80_percent() {
    // Round 1: tool_use response with enough tokens to hit ~90% of budget.
    // At fallback rates (input=$3/M, output=$15/M):
    //   100_000 input → $0.30, 40_000 output → $0.60, total = $0.90 = 90%
    let round1 = ChatResponse {
        content: "Using tool.".to_string(),
        tool_calls: vec![openalpaca_llm::ToolCall {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "test"}),
        }],
        model: "mock-model".to_string(),
        usage: Usage {
            input_tokens: 100_000,
            output_tokens: 40_000,
            ..Default::default()
        },
        finish_reason: FinishReason::ToolUse,
        thinking: None,
        parts: None,
    };
    // Round 2: simple text response to complete
    let provider = MockProvider::new(vec![
        Ok(round1),
        Ok(MockProvider::simple_response("Done.")),
    ]);
    let messages = vec![ChatMessage::user("test")];
    let config = LoopConfig {
        max_cost: 1.00,
        enable_caching: false,
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider, messages, vec![], &config,
        None, "test", None, None, None, None,
    )
    .await;

    // Loop should complete (warning emitted at 90% but not stopped)
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.rounds_used, 2);
}

// ─── truncate_tool_result tests ───────────────────────────────────────

#[test]
fn test_truncate_at_sentence_boundary() {
    // Build a string > 32KB where a sentence ends in the last 25%
    let limit = MAX_TOOL_RESULT_SIZE; // 32768
    // Fill most of the string, then place a sentence boundary in the last 25%
    let prefix_len = limit - 200; // well within the last 25%
    let mut text = "x".repeat(prefix_len);
    text.push_str(". "); // sentence boundary at prefix_len
    text.push_str(&"y".repeat(500)); // push over the limit

    let result = truncate_tool_result(text.clone());
    // Should cut right after the ". " punctuation (at prefix_len + 1)
    assert!(result.contains("[... truncated:"));
    let truncated_content = result.split("\n\n[... truncated:").next().unwrap();
    assert!(truncated_content.ends_with('.'));
    assert_eq!(truncated_content.len(), prefix_len + 1); // includes the '.'
}

#[test]
fn test_truncate_falls_back_to_line_boundary() {
    // String > 32KB with no sentence-ending punctuation, but with newlines
    let limit = MAX_TOOL_RESULT_SIZE;
    let line_pos = limit - 100; // newline in the last 25%
    let mut text = "x".repeat(line_pos);
    text.push('\n');
    text.push_str(&"x".repeat(500)); // push over the limit

    let result = truncate_tool_result(text);
    assert!(result.contains("[... truncated:"));
    let truncated_content = result.split("\n\n[... truncated:").next().unwrap();
    // Should cut at the newline
    assert_eq!(truncated_content.len(), line_pos);
}

#[test]
fn test_truncate_falls_back_to_word_boundary() {
    // String > 32KB with no newlines and no sentence punctuation, but with spaces
    let limit = MAX_TOOL_RESULT_SIZE;
    let space_pos = limit - 50; // space in the last 25%
    let mut text = "x".repeat(space_pos);
    text.push(' ');
    text.push_str(&"x".repeat(500)); // push over the limit

    let result = truncate_tool_result(text);
    assert!(result.contains("[... truncated:"));
    let truncated_content = result.split("\n\n[... truncated:").next().unwrap();
    // Should cut at the space
    assert_eq!(truncated_content.len(), space_pos);
}

// ─── context compaction inside agentic loop ─────────────────────────

#[tokio::test]
async fn test_compaction_triggers_during_agentic_loop() {
    use crate::context_budget::ContextBudgetManager;
    use crate::daemon_config::ContextBudgetConfig;

    // Create a budget with a very small context window so that a few rounds
    // of tool-use responses push the token count past the compaction trigger.
    //
    // Window: 800 tokens
    //   autocompact_buffer = 800 * 0.165 = 132
    //   compaction_trigger = 800 - 132 = 668
    //
    // Each tool-use round adds ~300 tokens of content (padded assistant message
    // + stub tool-error result). After 3 rounds the accumulated messages
    // (~900+ tokens estimated) should exceed the trigger and fire compaction.
    let budget_config = ContextBudgetConfig::default();
    let budget = ContextBudgetManager::new(800, &budget_config);

    // Build a tool-use response with a large content payload to inflate tokens.
    let make_fat_tool_response = |id: &str| ChatResponse {
        content: "x".repeat(400), // ~100 tokens
        tool_calls: vec![openalpaca_llm::ToolCall {
            id: id.to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"query": "a]".repeat(200)}), // ~100 tokens
        }],
        model: "mock-model".to_string(),
        usage: Usage {
            input_tokens: 50,
            output_tokens: 50,
            ..Default::default()
        },
        finish_reason: FinishReason::ToolUse,
        thinking: None,
        parts: None,
    };

    let provider = MockProvider::new(vec![
        Ok(make_fat_tool_response("tc_a")),
        Ok(make_fat_tool_response("tc_b")),
        Ok(make_fat_tool_response("tc_c")),
        Ok(make_fat_tool_response("tc_d")),
        Ok(MockProvider::simple_response("Final compacted answer.")),
    ]);

    let messages = vec![
        ChatMessage::system("You are a helpful assistant."),
        ChatMessage::user("Search for information repeatedly."),
    ];
    let config = LoopConfig {
        max_rounds: 10,
        max_cost: 10.0,
        enable_caching: false,
        thinking: None,
        context_tail_keep: 2,
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test_compaction",
        None,
        Some(&budget), // context_budget
        None,
        None,
    )
    .await;

    // The loop must complete successfully with the expected final answer.
    // If compaction didn't work, the context would overflow or the loop
    // would bail out early — neither of which produces this content.
    assert_eq!(
        result.finish_reason,
        LoopFinishReason::Complete,
        "Loop should complete successfully, got: {:?}",
        result.finish_reason
    );
    assert_eq!(result.final_content, "Final compacted answer.");
    // At least 4 tool-use rounds + 1 final = 5 rounds
    assert!(
        result.rounds_used >= 5,
        "Expected at least 5 rounds (4 tool + 1 final), got {}",
        result.rounds_used
    );
    assert_eq!(result.tool_calls_made, 4);
}

#[test]
fn test_truncate_min_cut_guard_skips_distant_sentence() {
    // Sentence boundary exists but is below the 75% threshold — should fall through
    let limit = MAX_TOOL_RESULT_SIZE;
    let min_cut = limit * 3 / 4; // 24576
    // Place sentence boundary well below min_cut
    let sentence_pos = min_cut - 1000;
    let mut text = "x".repeat(sentence_pos);
    text.push_str(". "); // sentence boundary below threshold
    // Fill the rest with 'y' (no sentence/line/word boundaries after this)
    // But add a newline near the end so line fallback kicks in
    let newline_pos = limit - 100;
    text.push_str(&"y".repeat(newline_pos - sentence_pos - 2));
    text.push('\n');
    text.push_str(&"y".repeat(500)); // push over the limit

    let result = truncate_tool_result(text);
    assert!(result.contains("[... truncated:"));
    let truncated_content = result.split("\n\n[... truncated:").next().unwrap();
    // Should NOT cut at the distant sentence boundary — should use line boundary instead
    assert_eq!(truncated_content.len(), newline_pos);
}

// ─── Router backend path tests ───────────────────────────────────────

/// Mock provider for testing the router-based agentic loop path.
/// Implements LlmProvider and returns canned responses via `chat_with_key`.
struct MockRouterProvider {
    responses: Vec<Result<ChatResponse, LlmError>>,
    call_count: AtomicUsize,
}

impl MockRouterProvider {
    fn new(responses: Vec<Result<ChatResponse, LlmError>>) -> Self {
        Self {
            responses,
            call_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for MockRouterProvider {
    fn name(&self) -> &str {
        "mock-router"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        if idx < self.responses.len() {
            self.responses[idx].clone()
        } else {
            self.responses
                .last()
                .cloned()
                .unwrap_or(Err(LlmError::NotConfigured))
        }
    }
}

#[tokio::test]
async fn test_agentic_loop_routed_completes() {
    // Build a real LlmRouter with a mock provider using single_provider().
    // This exercises the production code path (run_agentic_loop_routed) that
    // goes through LlmRouter::complete → execute_with_retry → chat_with_key.
    let provider = Arc::new(MockRouterProvider::new(vec![Ok(ChatResponse {
        content: "Router response".to_string(),
        tool_calls: vec![],
        model: "claude-sonnet-4-20250514".to_string(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        finish_reason: FinishReason::Stop,
        thinking: None,
        parts: None,
    })]));

    let router = LlmRouter::single_provider(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    let messages = vec![ChatMessage::user("hello via router")];
    let config = LoopConfig {
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        thinking: None,
        ..Default::default()
    };

    let result = run_agentic_loop_routed(
        &router,
        messages,
        vec![],
        &config,
        None,    // sandbox
        "test",  // agent_id
        None,    // sandbox_policy
        None,    // task_id
        None,    // context_budget
        None,    // cancel_token
        None,    // tool_context
        None,    // cost_accumulator
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.final_content, "Router response");
    assert_eq!(result.rounds_used, 1);
    assert_eq!(result.total_input_tokens, 10);
    assert_eq!(result.total_output_tokens, 5);
    assert_eq!(
        result.model_used,
        Some("claude-sonnet-4-20250514".to_string())
    );
}

#[tokio::test]
async fn test_agentic_loop_routed_with_tool_calls() {
    // Test that router path handles tool calls properly (two rounds:
    // first returns tool_use, second returns stop).
    let provider = Arc::new(MockRouterProvider::new(vec![
        Ok(ChatResponse {
            content: "Using tool.".to_string(),
            tool_calls: vec![openalpaca_llm::ToolCall {
                id: "tc_r1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "test"}),
            }],
            model: "claude-sonnet-4-20250514".to_string(),
            usage: Usage {
                input_tokens: 20,
                output_tokens: 15,
                ..Default::default()
            },
            finish_reason: FinishReason::ToolUse,
            thinking: None,
            parts: None,
        }),
        Ok(ChatResponse {
            content: "Router done.".to_string(),
            tool_calls: vec![],
            model: "claude-sonnet-4-20250514".to_string(),
            usage: Usage {
                input_tokens: 30,
                output_tokens: 10,
                ..Default::default()
            },
            finish_reason: FinishReason::Stop,
            thinking: None,
            parts: None,
        }),
    ]));

    let router = LlmRouter::single_provider(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    let messages = vec![ChatMessage::user("search via router")];
    let config = LoopConfig {
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        thinking: None,
        ..Default::default()
    };

    let result = run_agentic_loop_routed(
        &router,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        Some("task-123"), // task_id set for cost tracking
        None,
        None,
        None,
        None, // cost_accumulator
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.final_content, "Router done.");
    assert_eq!(result.rounds_used, 2);
    assert_eq!(result.tool_calls_made, 1);
    // Tokens are cumulative: 20+30 input, 15+10 output
    assert_eq!(result.total_input_tokens, 50);
    assert_eq!(result.total_output_tokens, 25);
}

#[tokio::test]
async fn test_agentic_loop_routed_respects_max_rounds() {
    // Router path should also respect max_rounds — mock always returns tool_use
    let provider = Arc::new(MockRouterProvider::new(vec![Ok(ChatResponse {
        content: "Using tool.".to_string(),
        tool_calls: vec![openalpaca_llm::ToolCall {
            id: "tc_r1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({}),
        }],
        model: "claude-sonnet-4-20250514".to_string(),
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        finish_reason: FinishReason::ToolUse,
        thinking: None,
        parts: None,
    })]));

    let router = LlmRouter::single_provider(
        provider,
        ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    let messages = vec![ChatMessage::user("loop forever")];
    let config = LoopConfig {
        max_rounds: 3,
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        thinking: None,
        ..Default::default()
    };

    let result = run_agentic_loop_routed(
        &router,
        messages,
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
        None,
        None, // cost_accumulator
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::MaxRounds);
    assert_eq!(result.rounds_used, 3);
}

// ---------------------------------------------------------------------------
// Integration tests: nested skill coherence (cost, context, constraints)
// ---------------------------------------------------------------------------

#[test]
fn test_nested_cost_accumulator_aggregation() {
    let acc = LoopCostAccumulator::new();
    let child_acc = acc.clone();

    acc.add_usd(0.30);
    assert!((acc.total_usd() - 0.30).abs() < 0.000_01);

    child_acc.add_usd(0.20);

    // Both see $0.50 aggregate
    assert!((acc.total_usd() - 0.50).abs() < 0.000_01);
    assert!((child_acc.total_usd() - 0.50).abs() < 0.000_01);
}

#[test]
fn test_nested_budget_remaining_calculation() {
    let parent_max_cost = 1.0;
    let acc = LoopCostAccumulator::new();

    acc.add_usd(0.80);
    let remaining = (parent_max_cost - acc.total_usd()).max(0.0);
    assert!((remaining - 0.20).abs() < 0.000_01);

    acc.add_usd(0.25);
    let remaining = (parent_max_cost - acc.total_usd()).max(0.0);
    assert_eq!(remaining, 0.0); // clamped to 0
}

#[test]
fn test_nested_skill_context_inheritance() {
    let parent_ctx = ToolContext {
        agent_id: Some("agent-1".into()),
        task_id: Some("task-1".into()),
        owner_id: Some("user-1".into()),
        workspace_id: Some("ws-1".into()),
        skill_stack: vec![],
        effective_constraints: None,
    };

    let child_ctx = parent_ctx.with_skill_pushed("skill-A");
    assert_eq!(child_ctx.agent_id, Some("agent-1".into()));
    assert_eq!(child_ctx.task_id, Some("task-1".into()));
    assert_eq!(child_ctx.skill_stack, vec!["skill-A".to_string()]);

    let grandchild_ctx = child_ctx.with_skill_pushed("skill-B");
    assert_eq!(grandchild_ctx.skill_stack, vec!["skill-A".to_string(), "skill-B".to_string()]);
    assert_eq!(grandchild_ctx.owner_id, Some("user-1".into()));
}

#[test]
fn test_three_level_nesting_integration() {
    // Cost: shared across 3 levels
    let acc = LoopCostAccumulator::new();

    // Level 1: root
    let ctx_l1 = ToolContext::default().with_skill_pushed("A");
    acc.add_usd(0.10);

    // Level 2: A -> B
    let ctx_l2 = ctx_l1.with_skill_pushed("B");
    let acc_l2 = acc.clone();
    acc_l2.add_usd(0.20);

    // Level 3: A -> B -> C
    let ctx_l3 = ctx_l2.with_skill_pushed("C");
    let acc_l3 = acc.clone();
    acc_l3.add_usd(0.30);

    // Verify skill_stack
    assert_eq!(ctx_l3.skill_stack, vec!["A", "B", "C"]);

    // Verify cost aggregation: 0.10 + 0.20 + 0.30 = 0.60
    assert!((acc.total_usd() - 0.60).abs() < 0.000_01);
    assert!((acc_l3.total_usd() - 0.60).abs() < 0.000_01);

    // Constraints compose across chain
    let c1 = EffectiveToolSet {
        denied: ["tool_x".into()].into_iter().collect(),
        source_chain: vec!["A".into()],
        ..Default::default()
    };
    let c2 = compose_constraints(&c1, None, &["tool_y".into()], &[], "B").unwrap();
    let c3 = compose_constraints(&c2, None, &[], &[], "C").unwrap();

    assert!(c3.denied.contains("tool_x"));
    assert!(c3.denied.contains("tool_y"));
    assert_eq!(c3.source_chain, vec!["A", "B", "C"]);
}
