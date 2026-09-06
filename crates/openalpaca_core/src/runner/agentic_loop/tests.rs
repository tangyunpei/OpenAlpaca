use super::*;
use async_trait::async_trait;
use openalpaca_llm::{CallRecord, ChatResponse, LlmError, LlmRouter, ProviderType, Usage};
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
        // This test does not exercise the allow axis.
        allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
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
        // This test does not exercise the allow axis.
        allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
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
        // This test does not exercise the allow axis.
        allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
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
    // The compaction record is narrated into a real log, so P-14's fields are
    // asserted on what the writer actually produced.
    let dir = tempfile::tempdir().unwrap();
    let service = crate::session_log::SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        crate::session_log::SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-compaction");

    let config = LoopConfig {
        max_rounds: 10,
        max_cost: 10.0,
        enable_caching: false,
        thinking: None,
        context_tail_keep: 2,
        session_log: Some(handle.clone()),
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

    // P-14: the compaction record names the boundary §5.4's replay rule is
    // defined in terms of, and how much this run has dropped so far. The two
    // fields that need a message→log-seq map are explicit nulls, not guesses.
    assert!(handle.flush().await);
    let records = crate::session_log::read_records(&dir.path().join("sess-compaction")).unwrap();
    let compaction = records
        .iter()
        .find(|r| r.kind == "compaction")
        .expect("the loop narrates its compaction");
    let previous = records
        .iter()
        .rfind(|r| r.seq < compaction.seq)
        .expect("a compaction is never the first record here");
    assert_eq!(
        compaction.data["preserved_from_seq"].as_u64(),
        Some(previous.seq),
        "the boundary is the last seq written before the compaction"
    );
    assert_eq!(
        compaction.data["cumulative_dropped_tokens"].as_u64(),
        Some(
            (compaction.data["pre_tokens"].as_u64().unwrap())
                .saturating_sub(compaction.data["post_tokens"].as_u64().unwrap())
        ),
        "the first compaction of a run has dropped exactly its own delta"
    );
    assert!(compaction.data["dropped_from_seq"].is_null());
    assert!(compaction.data["summary_msg_id"].is_null());
}

/// P-14 again, on the field the single-compaction test cannot distinguish from
/// "this compaction's delta": `cumulative_dropped_tokens` is what the **run**
/// has dropped so far, so a second compaction reports the sum of both, not its
/// own. Without this the accumulator could be a plain assignment and nothing
/// would notice.
#[tokio::test]
async fn test_two_compactions_accumulate_dropped_tokens() {
    use crate::context_budget::ContextBudgetManager;
    use crate::daemon_config::ContextBudgetConfig;

    // A tighter window than the single-compaction test's 800 so the trigger is
    // crossed twice inside one run: each fat round adds ~200 tokens, and the
    // tail the compactor keeps is immediately refilled.
    let budget_config = ContextBudgetConfig::default();
    let budget = ContextBudgetManager::new(600, &budget_config);

    let make_fat_tool_response = |id: &str| ChatResponse {
        content: "x".repeat(400),
        tool_calls: vec![openalpaca_llm::ToolCall {
            id: id.to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"query": "a]".repeat(200)}),
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
        Ok(make_fat_tool_response("tc_e")),
        Ok(make_fat_tool_response("tc_f")),
        Ok(make_fat_tool_response("tc_g")),
        Ok(make_fat_tool_response("tc_h")),
        Ok(MockProvider::simple_response("Done.")),
    ]);

    let messages = vec![
        ChatMessage::system("You are a helpful assistant."),
        ChatMessage::user("Search for information repeatedly."),
    ];
    let dir = tempfile::tempdir().unwrap();
    let service = crate::session_log::SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        crate::session_log::SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-two-compactions");

    let config = LoopConfig {
        max_rounds: 12,
        max_cost: 10.0,
        enable_caching: false,
        thinking: None,
        context_tail_keep: 2,
        session_log: Some(handle.clone()),
        ..Default::default()
    };

    let _ = run_agentic_loop(
        &provider,
        messages,
        vec![],
        &config,
        None,
        "test_two_compactions",
        None,
        Some(&budget),
        None,
        None,
    )
    .await;

    assert!(handle.flush().await);
    let records =
        crate::session_log::read_records(&dir.path().join("sess-two-compactions")).unwrap();
    let compactions: Vec<_> = records.iter().filter(|r| r.kind == "compaction").collect();
    assert!(
        compactions.len() >= 2,
        "this run must compact at least twice, got {}",
        compactions.len()
    );

    let mut expected = 0u64;
    for (i, record) in compactions.iter().enumerate() {
        let delta = record.data["pre_tokens"]
            .as_u64()
            .unwrap()
            .saturating_sub(record.data["post_tokens"].as_u64().unwrap());
        expected += delta;
        assert_eq!(
            record.data["cumulative_dropped_tokens"].as_u64(),
            Some(expected),
            "compaction {i} reports the run's running total, not its own delta"
        );
    }
    assert!(
        expected
            > compactions[0].data["pre_tokens"]
                .as_u64()
                .unwrap()
                .saturating_sub(compactions[0].data["post_tokens"].as_u64().unwrap()),
        "the total must exceed the first compaction's own delta"
    );
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
    /// Records the `ephemeral_system_notice` field from every incoming
    /// `ChatRequest`. Exposed via `notices_seen()` for Task 5 (P0b) tests
    /// that need to assert what the backend actually received.
    notices: std::sync::Mutex<Vec<Option<String>>>,
    /// Records the system-message count of the messages sent with each
    /// `chat()` call. Used by Task 5 (P0b) to confirm the ephemeral
    /// notice is not persisted into the conversation (i.e., the request
    /// never carries more than the original single system prompt).
    system_count_per_call: std::sync::Mutex<Vec<usize>>,
    /// The content of every `Role::Tool` message the backend has been sent —
    /// what the *model* actually sees of a tool result, which is not what the
    /// session log records (§5.4: the log keeps the untruncated payload).
    tool_results_seen: std::sync::Mutex<Vec<String>>,
}

impl MockRouterProvider {
    fn new(responses: Vec<Result<ChatResponse, LlmError>>) -> Self {
        Self {
            responses,
            call_count: AtomicUsize::new(0),
            notices: std::sync::Mutex::new(Vec::new()),
            system_count_per_call: std::sync::Mutex::new(Vec::new()),
            tool_results_seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The tool-result texts the model was given, newest call last.
    #[allow(dead_code)]
    fn tool_results_seen(&self) -> Vec<String> {
        self.tool_results_seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Snapshot the notices observed across all `chat()` calls so far.
    #[allow(dead_code)]
    fn notices_seen(&self) -> Vec<Option<String>> {
        self.notices
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Snapshot the per-call count of system messages observed so far.
    #[allow(dead_code)]
    fn system_counts_seen(&self) -> Vec<usize> {
        self.system_count_per_call
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
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

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.notices
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(request.ephemeral_system_notice.clone());
        let system_count = request
            .messages
            .iter()
            .filter(|m| matches!(m.role, openalpaca_llm::Role::System))
            .count();
        self.system_count_per_call
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(system_count);
        {
            let mut seen = self
                .tool_results_seen
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            for message in request.messages.iter() {
                if matches!(message.role, openalpaca_llm::Role::Tool) {
                    seen.push(message.content.clone());
                }
            }
        }
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
async fn test_agentic_loop_routed_publishes_model_access_denied_event() {
    use crate::agent::subagent::AgentConstraints;
    use crate::bus::EventBus;
    use crate::events::SystemEvent;

    // The router "falls back" to a model the agent is not allowed to use:
    // the response reports a model on the agent's deny list.
    let provider = Arc::new(MockRouterProvider::new(vec![Ok(ChatResponse {
        content: "Should be rejected".to_string(),
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

    let mut constraints = AgentConstraints {
        denied_models: vec!["claude-sonnet-4-20250514".to_string()],
        ..Default::default()
    };
    constraints.normalize();

    let bus = EventBus::new(16);
    let mut event_rx = bus.subscribe();

    let config = LoopConfig {
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        thinking: None,
        agent_constraints: Some(constraints),
        event_bus: Some(bus),
        ..Default::default()
    };

    let result = run_agentic_loop_routed(
        &router,
        vec![ChatMessage::user("hello")],
        vec![],
        &config,
        None,          // sandbox
        "denied-agent", // agent_id
        None,          // sandbox_policy
        None,          // task_id
        None,          // context_budget
        None,          // cancel_token
        None,          // tool_context
        None,          // cost_accumulator
    )
    .await;

    // The loop aborts with an access error...
    match result.finish_reason {
        LoopFinishReason::Error(ref msg) => {
            assert!(
                msg.contains("Model access denied"),
                "unexpected error message: {msg}"
            );
        }
        other => panic!("expected Error finish reason, got {other:?}"),
    }

    // ...and publishes SystemEvent::ModelAccessDenied on the bus.
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
        .await
        .expect("timed out waiting for ModelAccessDenied event")
        .expect("event bus closed");
    match event {
        SystemEvent::ModelAccessDenied {
            agent_id,
            model_id,
            reason,
            ..
        } => {
            assert_eq!(agent_id, "denied-agent");
            assert_eq!(model_id, "claude-sonnet-4-20250514");
            assert!(!reason.is_empty());
        }
        other => panic!("expected ModelAccessDenied, got {other:?}"),
    }
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

// ─── A5: main-loop cost lockout (bug-main-loop-cost-lockout.md, option 1) ──
//
// The main loop reuses a literal `"orchestrator"` agent id across every chat
// turn and passes no `cost_accumulator` (a fresh zeroed `LoopCostAccumulator`
// each turn). `LlmBackend::agent_cost()` for the Router backend returns the
// cost tracker's *cumulative* total for that agent id, not this turn's
// spend. Before the fix, `LoopState::new()` seeded `last_cost` at `0.0`, so
// round 0's delta equaled the agent's entire lifetime spend — once that
// crossed `max_cost` every fresh turn exited `CostExceeded` before its first
// LLM call. These tests exercise the real routed loop (`LlmRouter` +
// `CostTracker`, not just the local `LoopCostAccumulator`) to reproduce and
// then guard against that mechanism.

#[tokio::test]
async fn main_loop_turn_with_prior_agent_spend_does_not_exit_cost_exceeded() {
    // Seed the cost tracker's "orchestrator" bucket with prior cumulative
    // spend well above the default $1 max_cost — as the main loop's fixed
    // agent id would accumulate across earlier turns today — then run one
    // fresh turn exactly as the main loop does (agent_id = "orchestrator",
    // cost_accumulator = None). The turn must still make its LLM call
    // instead of exiting CostExceeded at round 0.
    let provider = Arc::new(MockRouterProvider::new(vec![Ok(ChatResponse {
        content: "Hello there.".to_string(),
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
        Arc::clone(&provider) as Arc<dyn LlmProvider>,
        ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    // Prior cumulative spend under the shared "orchestrator" bucket — well
    // over the default $1.00 max_cost.
    router
        .cost_tracker
        .record(&CallRecord {
            agent_id: "orchestrator".to_string(),
            task_id: None,
            model: "claude-sonnet-4-20250514".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 5.00,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        })
        .await;

    let messages = vec![ChatMessage::user("hello")];
    let config = LoopConfig {
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        thinking: None,
        ..Default::default() // max_cost defaults to $1.00
    };

    let result = run_agentic_loop_routed(
        &router,
        messages,
        vec![],
        &config,
        None,           // sandbox
        "orchestrator", // agent_id — matches the main loop's fixed id
        None,           // sandbox_policy
        None,           // task_id
        None,           // context_budget
        None,           // cancel_token
        None,           // tool_context
        None,           // cost_accumulator — matches the main loop's fresh accumulator each turn
    )
    .await;

    assert_ne!(
        result.finish_reason,
        LoopFinishReason::CostExceeded,
        "prior cumulative agent spend must not lock out a fresh turn"
    );
    assert!(
        provider.call_count.load(Ordering::SeqCst) >= 1,
        "expected at least one LLM call, got {}",
        provider.call_count.load(Ordering::SeqCst)
    );
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.final_content, "Hello there.");
}

#[tokio::test]
async fn per_turn_cap_still_trips_within_one_turn() {
    // Regression guard: baselining `last_cost` from prior cumulative spend
    // must not defeat in-turn enforcement — a turn whose OWN rounds spend
    // past max_cost still exits CostExceeded, same as before the fix.
    let expensive_tool_use = ChatResponse {
        content: "Using tool.".to_string(),
        tool_calls: vec![openalpaca_llm::ToolCall {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({}),
        }],
        model: "claude-sonnet-4-20250514".to_string(),
        usage: Usage {
            input_tokens: 500_000,
            output_tokens: 200_000,
            ..Default::default()
        },
        finish_reason: FinishReason::ToolUse,
        thinking: None,
        parts: None,
    };
    let provider = Arc::new(MockRouterProvider::new(vec![Ok(expensive_tool_use)]));

    let router = LlmRouter::single_provider(
        Arc::clone(&provider) as Arc<dyn LlmProvider>,
        ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    let messages = vec![ChatMessage::user("expensive query")];
    let config = LoopConfig {
        max_cost: 1.00,
        max_rounds: 5,
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
        None,           // sandbox
        "orchestrator", // agent_id
        None,           // sandbox_policy
        None,           // task_id
        None,           // context_budget
        None,           // cancel_token
        None,           // tool_context
        None,           // cost_accumulator — same as production main-loop calls
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::CostExceeded);
    assert_eq!(
        result.rounds_used, 1,
        "only round 0's LLM call should complete before the cap trips on round 1's cost check"
    );
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
        ..Default::default()
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

// ---------------------------------------------------------------------------
// P3 — ten loop-step trace spans
// ---------------------------------------------------------------------------

/// Minimal single-iteration invocation of `run_agentic_loop_routed` with a
/// stub `LlmRouter` that returns a text-only response on the first call (so
/// the loop exits after one iteration through the `Complete` finish branch).
///
/// Used by `test_ten_loop_step_spans_emitted` to verify that each inner-loop
/// step emits its `tracing::info_span!` with the expected name.
async fn run_minimal_loop_for_span_test() -> LoopResult {
    // Reuse the MockRouterProvider pattern from earlier router tests.
    let provider = Arc::new(MockRouterProvider::new(vec![Ok(ChatResponse {
        content: "Done.".to_string(),
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

    let messages = vec![ChatMessage::user("span test")];
    let config = LoopConfig {
        model: Some("claude-sonnet-4-20250514".to_string()),
        // enable_caching = true so the Anthropic cache_markers span fires.
        enable_caching: true,
        thinking: None,
        ..Default::default()
    };

    run_agentic_loop_routed(
        &router,
        messages,
        vec![],
        &config,
        None, // sandbox
        "test-agent",
        None, // sandbox_policy
        None, // task_id
        None, // context_budget
        None, // cancel_token
        None, // tool_context
        None, // cost_accumulator
    )
    .await
}

#[tokio::test]
#[tracing_test::traced_test]
async fn test_ten_loop_step_spans_emitted() {
    let result = run_minimal_loop_for_span_test().await;
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);

    // NOTES on omissions:
    //
    // * `loop.step.pressure_layer` — emitted conditionally on an ephemeral
    //   context-pressure notice that is not wired up until Task 5 (P0b). Per
    //   Tricky Bit 2 of the Task 3 spec, we emit it only when `notice.is_some()`
    //   so it will not fire in this commit. Task 5 wires the notice in.
    //
    // * `loop.step.cache_markers` — lives inside
    //   `openalpaca_llm::providers::anthropic::request::build_request_body`
    //   (consolidated there as part of this commit). The MockRouterProvider
    //   used here bypasses the Anthropic request builder, so the span never
    //   fires in this integration path. Its existence and field values are
    //   verified indirectly by the Anthropic provider tests that exercise
    //   `enable_caching=true` (see `providers::anthropic::tests`), which
    //   assert the resulting JSON carries cache_control on all three
    //   breakpoints.
    //
    // We therefore assert the 8 core-loop spans that the routed loop emits
    // directly; Task 5 will add `pressure_layer`.
    for name in [
        "loop.step.cancellation_check",
        "loop.step.max_rounds_check",
        "loop.step.cost_check",
        "loop.step.compaction",
        "loop.step.build_request",
        "loop.step.llm_call",
        "loop.step.response_parse",
        "loop.step.persist_or_tools",
    ] {
        assert!(
            logs_contain(name),
            "expected span name {} to appear in trace output",
            name
        );
    }
}

// ─── Task 5 (P0b): ephemeral budget-pressure notice tests ────────────

/// Shared fixture: build a router over a `MockRouterProvider` that always
/// returns a cheap one-shot response, then drive `run_agentic_loop_routed`
/// with a pre-seeded `LoopCostAccumulator` so cost-pressure can be
/// simulated deterministically without depending on provider pricing.
///
/// Returns `(result, provider)` so callers can both inspect the final
/// `LoopResult` (messages, finish reason) and pull `notices_seen()` off
/// the mock to verify what the backend actually received.
async fn run_loop_with_seeded_cost(
    experimental_ephemeral_pressure: bool,
    simulated_cost: f64,
    max_cost: f64,
) -> (LoopResult, Arc<MockRouterProvider>) {
    let provider = Arc::new(MockRouterProvider::new(vec![Ok(ChatResponse {
        content: "Done.".to_string(),
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
        Arc::clone(&provider) as Arc<dyn LlmProvider>,
        ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    // Pre-seed the accumulator so iteration 1 sees the simulated cost at
    // the top of the loop (before the LLM call happens).
    let acc = LoopCostAccumulator::new();
    acc.add_usd(simulated_cost);

    let messages = vec![
        ChatMessage::system("You are a helpful assistant."),
        ChatMessage::user("hello"),
    ];

    let config = LoopConfig {
        max_rounds: 5,
        max_cost,
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        thinking: None,
        experimental_ephemeral_pressure,
        ..Default::default()
    };

    let result = run_agentic_loop_routed(
        &router,
        messages,
        vec![],
        &config,
        None,
        "test-agent",
        None,
        None,
        None,
        None,
        None,
        Some(acc),
    )
    .await;

    (result, provider)
}

#[tokio::test]
async fn test_ephemeral_notice_absent_when_flag_off() {
    // Even though cost is at 90% of the budget, the flag is off — so the
    // backend must see `ephemeral_system_notice = None`.
    let (_result, provider) = run_loop_with_seeded_cost(
        /* experimental_ephemeral_pressure = */ false,
        /* simulated_cost = */ 0.009,
        /* max_cost = */ 0.01,
    )
    .await;

    let notices = provider.notices_seen();
    assert!(
        !notices.is_empty(),
        "expected at least one chat() call, got none"
    );
    assert!(
        notices.iter().all(|n| n.is_none()),
        "flag off but backend saw a notice: {:?}",
        notices
    );
}

#[tokio::test]
async fn test_ephemeral_notice_fires_over_cost_threshold() {
    // Flag on, cost at 90% — the backend must see a notice containing the
    // `[budget_notice]` envelope and the 90% cost figure.
    let (_result, provider) = run_loop_with_seeded_cost(
        /* experimental_ephemeral_pressure = */ true,
        /* simulated_cost = */ 0.009,
        /* max_cost = */ 0.01,
    )
    .await;

    let notices = provider.notices_seen();
    let first = notices
        .into_iter()
        .next()
        .expect("expected at least one chat() call");
    let notice = first.expect("flag on with cost at 90% — expected Some(notice)");
    assert!(
        notice.contains("[budget_notice]"),
        "notice missing [budget_notice] envelope: {}",
        notice
    );
    assert!(
        notice.contains("90%") || notice.contains("90"),
        "notice missing 90% cost figure: {}",
        notice
    );
}

#[tokio::test]
async fn test_ephemeral_notice_not_persisted_in_messages() {
    // Flag on, cost at 90%. The notice must ride on the ephemeral
    // per-request field and never be appended to the persistent message
    // list. We assert this by inspecting the `role_counts` of the
    // messages the backend actually received: there must be exactly one
    // System message (the original system prompt), NOT two (the original
    // + a pushed notice, which is what the old broken append used to
    // do).
    let (result, provider) = run_loop_with_seeded_cost(
        /* experimental_ephemeral_pressure = */ true,
        /* simulated_cost = */ 0.009,
        /* max_cost = */ 0.01,
    )
    .await;

    assert_eq!(
        result.finish_reason,
        LoopFinishReason::Complete,
        "unexpected finish reason: {:?}",
        result.finish_reason
    );

    let system_counts = provider.system_counts_seen();
    assert_eq!(
        system_counts.len(),
        1,
        "expected exactly one chat() call, got {}",
        system_counts.len()
    );
    assert_eq!(
        system_counts[0], 1,
        "expected exactly one system message in the persistent history \
         (not 2 — the old broken append would have pushed a second one), \
         got {}",
        system_counts[0]
    );
}

// ─── Routing V2: steering drain tests ────────────────────────────────

use crate::runner::steering::{SteeringInbox, SteeringMsg};
use crate::security::policy::{Principal, Scope};

fn steering_msg(text: &str) -> SteeringMsg {
    SteeringMsg {
        text: text.to_string(),
        request_id: uuid::Uuid::new_v4(),
        principal: Principal::System,
        scope: Scope::Global,
        workspace_path: None,
        received_at: chrono::Utc::now(),
    }
}

/// Mock provider that records the full message list of every request and
/// runs a per-call hook (used to push steering messages "mid-call",
/// simulating an interjection arriving while the LLM call is in flight).
struct SteeringHookProvider {
    responses: Vec<Result<ChatResponse, LlmError>>,
    call_count: AtomicUsize,
    requests: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
    on_call: Box<dyn Fn(usize) + Send + Sync>,
}

impl SteeringHookProvider {
    fn new(
        responses: Vec<Result<ChatResponse, LlmError>>,
        on_call: Box<dyn Fn(usize) + Send + Sync>,
    ) -> Self {
        Self {
            responses,
            call_count: AtomicUsize::new(0),
            requests: std::sync::Mutex::new(Vec::new()),
            on_call,
        }
    }

    fn requests_seen(&self) -> Vec<Vec<ChatMessage>> {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

#[async_trait]
impl LlmProvider for SteeringHookProvider {
    fn name(&self) -> &str {
        "steering-hook"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(request.messages.to_vec());
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        (self.on_call)(idx);
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

fn contains_interjection(messages: &[ChatMessage], text: &str) -> bool {
    messages.iter().any(|m| {
        matches!(m.role, openalpaca_llm::Role::User)
            && m.content.starts_with("<user_interjection ts=\"")
            && m.content.contains(text)
    })
}

#[tokio::test]
async fn test_steering_interjection_appears_in_next_request() {
    // A message pushed during round 1's LLM call must appear as a user
    // message in round 2's request, wrapped in <user_interjection>.
    let inbox = Arc::new(SteeringInbox::default());
    let hook_inbox = Arc::clone(&inbox);
    let provider = SteeringHookProvider::new(
        vec![
            Ok(MockProvider::tool_use_response()),
            Ok(MockProvider::simple_response("Done.")),
        ],
        Box::new(move |idx| {
            if idx == 0 {
                hook_inbox.push(steering_msg("change course")).unwrap();
            }
        }),
    );
    let config = LoopConfig {
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.rounds_used, 2);
    let requests = provider.requests_seen();
    assert_eq!(requests.len(), 2);
    assert!(
        !contains_interjection(&requests[0], "change course"),
        "interjection must not appear before it was pushed"
    );
    assert!(
        contains_interjection(&requests[1], "change course"),
        "interjection missing from the round-2 request: {:?}",
        requests[1]
    );
    assert!(inbox.is_empty());
}

#[tokio::test]
async fn test_steering_completion_guard_continues_loop() {
    // A message arriving during the final (no-tool-calls) round must not be
    // dropped: the guard keeps the assistant answer, injects the
    // interjection, and runs another round.
    let inbox = Arc::new(SteeringInbox::default());
    let hook_inbox = Arc::clone(&inbox);
    let provider = SteeringHookProvider::new(
        vec![
            Ok(MockProvider::simple_response("First answer")),
            Ok(MockProvider::simple_response("Final answer")),
        ],
        Box::new(move |idx| {
            if idx == 0 {
                hook_inbox.push(steering_msg("one more thing")).unwrap();
            }
        }),
    );
    let config = LoopConfig {
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.rounds_used, 2);
    assert_eq!(result.final_content, "Final answer");
    let requests = provider.requests_seen();
    assert_eq!(requests.len(), 2);
    // Round 2 carries the would-be-final assistant answer plus the interjection.
    assert!(requests[1].iter().any(|m| {
        matches!(m.role, openalpaca_llm::Role::Assistant) && m.content == "First answer"
    }));
    assert!(contains_interjection(&requests[1], "one more thing"));
    assert!(inbox.is_empty());
}

#[tokio::test]
async fn test_steering_max_rounds_exit_reappends_undelivered() {
    // max_rounds=1: round 1 completes, the guard drains the interjection and
    // continues; round 2 completes and the guard drains a second interjection,
    // but the bonus cap (2× max_rounds = 2) exits the loop before another LLM
    // call — the drained-but-unsent message must be re-appended to the inbox.
    let inbox = Arc::new(SteeringInbox::default());
    let hook_inbox = Arc::clone(&inbox);
    let provider = SteeringHookProvider::new(
        vec![Ok(MockProvider::simple_response("Answer"))],
        Box::new(move |idx| {
            hook_inbox
                .push(steering_msg(&format!("msg-{}", idx)))
                .unwrap();
        }),
    );
    let config = LoopConfig {
        max_rounds: 1,
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::MaxRounds);
    assert_eq!(result.rounds_used, 2); // 2× cap of max_rounds=1
    // msg-1 was drained by the completion guard but never sent — it must be
    // back in the inbox for follow-up conversion.
    let leftover = inbox.drain_all();
    assert_eq!(leftover.len(), 1);
    assert_eq!(leftover[0].text, "msg-1");
}

#[tokio::test]
async fn test_steering_bonus_extends_but_caps_at_double() {
    // Every LLM call pushes an interjection, so every round grants a +5
    // bonus — yet the loop must stop at exactly 2× max_rounds.
    let inbox = Arc::new(SteeringInbox::default());
    let hook_inbox = Arc::clone(&inbox);
    let provider = SteeringHookProvider::new(
        vec![Ok(MockProvider::tool_use_response())],
        Box::new(move |idx| {
            // Ignore Full errors — the queue may back up near the cap.
            let _ = hook_inbox.push(steering_msg(&format!("steer-{}", idx)));
        }),
    );
    let config = LoopConfig {
        max_rounds: 2,
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::MaxRounds);
    assert!(
        result.rounds_used > 2,
        "bonus should extend past max_rounds, got {}",
        result.rounds_used
    );
    assert_eq!(result.rounds_used, 4, "bonus must cap at 2× max_rounds");
}

#[tokio::test]
async fn test_steering_closed_inbox_drains_nothing() {
    // A closed inbox must be inert: the completion guard skips it even when
    // it holds leftovers (re-appended via push_front_all, which bypasses the
    // closed flag), and no interjection is injected into any request.
    let inbox = Arc::new(SteeringInbox::default());
    let drained = inbox.close_and_drain();
    assert!(drained.is_empty());
    let hook_inbox = Arc::clone(&inbox);
    let provider = SteeringHookProvider::new(
        vec![Ok(MockProvider::simple_response("Done."))],
        Box::new(move |idx| {
            if idx == 0 {
                // Regular push is rejected on a closed inbox…
                assert_eq!(
                    hook_inbox.push(steering_msg("late")),
                    Err(crate::runner::steering::SteeringPushError::Closed)
                );
                // …but a budget-exit re-append lands even after close.
                hook_inbox.push_front_all(vec![steering_msg("leftover")]);
            }
        }),
    );
    let config = LoopConfig {
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert_eq!(result.rounds_used, 1, "completion guard must not fire on a closed inbox");
    let requests = provider.requests_seen();
    assert!(
        requests
            .iter()
            .all(|msgs| !contains_interjection(msgs, "leftover")),
        "closed inbox contents must never be injected"
    );
    // The leftover stays queued for the cleanup path.
    assert_eq!(inbox.drain_all().len(), 1);
}

#[tokio::test]
async fn test_steering_error_exit_reappends_undelivered() {
    // Widened re-append (Routing V2 chunk 3): an interjection drained into a
    // request whose LLM call FAILS was never seen by the model — the Error
    // exit must return it to the inbox for follow-up conversion.
    let inbox = Arc::new(SteeringInbox::default());
    let hook_inbox = Arc::clone(&inbox);
    let provider = SteeringHookProvider::new(
        vec![
            Ok(MockProvider::tool_use_response()),
            Err(LlmError::NotConfigured),
        ],
        Box::new(move |idx| {
            if idx == 0 {
                hook_inbox.push(steering_msg("urgent change")).unwrap();
            }
        }),
    );
    let config = LoopConfig {
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;

    assert!(matches!(result.finish_reason, LoopFinishReason::Error(_)));
    // Round 2's request did carry the interjection, but the call failed.
    let requests = provider.requests_seen();
    assert!(contains_interjection(&requests[1], "urgent change"));
    // The undelivered message is back in the inbox.
    let leftover = inbox.drain_all();
    assert_eq!(leftover.len(), 1);
    assert_eq!(leftover[0].text, "urgent change");
}

#[tokio::test]
async fn test_steering_cancel_during_llm_call_reappends_undelivered() {
    // Widened re-append (Routing V2 chunk 3): cancellation racing the LLM
    // call that carried a drained interjection must return the message to
    // the inbox instead of dropping it.
    use tokio_util::sync::CancellationToken;

    struct CancelSecondCallProvider {
        inbox: Arc<SteeringInbox>,
        token: CancellationToken,
        call_count: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for CancelSecondCallProvider {
        fn name(&self) -> &str {
            "cancel-second"
        }
        fn supports_tools(&self) -> bool {
            true
        }
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx == 0 {
                self.inbox.push(steering_msg("mid-flight fix")).unwrap();
                Ok(MockProvider::tool_use_response())
            } else {
                // Cancel while "in flight", then stall so the select! races
                // the cancellation branch to victory.
                self.token.cancel();
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(MockProvider::simple_response("never delivered"))
            }
        }
    }

    let inbox = Arc::new(SteeringInbox::default());
    let token = CancellationToken::new();
    let provider = CancelSecondCallProvider {
        inbox: Arc::clone(&inbox),
        token: token.clone(),
        call_count: AtomicUsize::new(0),
    };
    let config = LoopConfig {
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        ..Default::default()
    };

    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        Some(token),
        None,
    )
    .await;

    assert_eq!(result.finish_reason, LoopFinishReason::Cancelled);
    // The interrupted call never delivered the drained message — it must be
    // back in the inbox for follow-up conversion.
    let leftover = inbox.drain_all();
    assert_eq!(leftover.len(), 1);
    assert_eq!(leftover[0].text, "mid-flight fix");
}

// ── Session event log (§5.5) ────────────────────────────────────────

/// The loop's four emit points, on one run: a `round` per LLM response
/// carrying the `tool_use` blocks verbatim, a `tool_call` before dispatch and
/// a `tool_result` after it, and a `workflow_done` at the exit. Every record
/// is stamped with the run's `task_id` and the loop's `span_id` (P-20), and
/// the seq is gap-free across all of them.
#[tokio::test]
async fn the_loop_narrates_rounds_tools_and_its_exit_into_the_session_log() {
    use crate::bus::EventBus;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use crate::tools::ToolRegistry;

    struct EchoTool;
    #[async_trait]
    impl BuiltInTool for EchoTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("the search result".to_string())
        }
    }

    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "search".to_string(),
                description: "Search".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "research_agent::a1".to_string(),
        allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
    };

    let provider = Arc::new(MockRouterProvider::new(vec![
        Ok(ChatResponse {
            content: "Searching.".to_string(),
            tool_calls: vec![openalpaca_llm::ToolCall {
                id: "tc_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "kettle"}),
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
            content: "All done.".to_string(),
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

    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&db_dir.path().join("t.db")).unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        Some(db.clone()),
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-loop");

    let config = LoopConfig {
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        session_log: Some(handle.clone()),
        span_id: Some("lead::task-9".to_string()),
        ..Default::default()
    };

    let result = run_agentic_loop_routed(
        &router,
        vec![ChatMessage::user("find the kettle")],
        vec![],
        &config,
        Some(&sandbox),
        "research_agent::a1",
        Some(&policy),
        Some("task-9"),
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert!(handle.flush().await);

    let records = read_records(&dir.path().join("sess-loop")).unwrap();
    let kinds: Vec<&str> = records.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["round", "tool_call", "tool_result", "round", "workflow_done"],
        "the loop narrates in dispatch order: {kinds:?}"
    );
    for (i, record) in records.iter().enumerate() {
        assert_eq!(record.seq, (i + 1) as u64, "gap-free");
        assert_eq!(record.task_id.as_deref(), Some("task-9"));
        assert_eq!(record.span_id.as_deref(), Some("lead::task-9"));
        assert_eq!(record.agent.as_deref(), Some("research_agent::a1"));
    }

    // The `tool_use` blocks are verbatim — id, name and the full input JSON —
    // which is what makes replay-resume reconstructible (§5.4).
    let round = &records[0];
    assert_eq!(round.data["round"], 1);
    assert_eq!(round.data["model"], "claude-sonnet-4-20250514");
    assert_eq!(round.data["input_tokens"], 20);
    assert_eq!(round.data["text"], "Searching.");
    assert_eq!(round.data["tool_use"][0]["id"], "tc_1");
    assert_eq!(round.data["tool_use"][0]["name"], "search");
    assert_eq!(round.data["tool_use"][0]["input"]["query"], "kettle");

    assert_eq!(records[1].data["tool_use_id"], "tc_1");
    assert_eq!(records[1].data["input"]["query"], "kettle");
    // A builtin belongs to no extension — `ext` is null, never invented.
    assert!(records[1].data["ext"].is_null());
    assert_eq!(records[2].data["tool_use_id"], "tc_1");
    assert_eq!(records[2].data["ok"], true);
    assert_eq!(records[2].data["result"], "the search result");

    let done = records.last().unwrap();
    assert_eq!(done.data["finish_reason"], "complete");
    assert_eq!(done.data["rounds_used"], 2);
    assert_eq!(done.data["tool_calls_made"], 1);

    // The index row is the writer's, and points back at both records.
    let (session_id, task, log_seq, result_ref): (String, String, i64, String) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT session_id, task_id, log_seq, result_ref FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?)
        })
        .unwrap();
    assert_eq!(session_id, "sess-loop");
    assert_eq!(task, "task-9");
    assert_eq!(log_seq, 2, "log_seq points at the tool_call record");
    assert_eq!(result_ref, "log:3");
}

/// §5.4's "Spill, don't truncate", end to end through the loop: a result over
/// `tool_result_inline_bytes` is written once to the session's `results/`, the
/// record keeps the reference and a preview, and the **model** is handed the
/// stub naming the reference — so the tail of a long result is reachable
/// through `read_result` instead of destroyed.
#[tokio::test]
async fn a_result_over_the_inline_threshold_spills_and_the_model_gets_the_stub() {
    use crate::bus::EventBus;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use crate::tools::ToolRegistry;

    /// 40 KB: over `MAX_TOOL_RESULT_SIZE` (32 KB), under the 64 KB envelope
    /// cap — the band where the two bounds disagree.
    const RESULT_BYTES: usize = 40 * 1024;

    struct BigOutputTool;
    #[async_trait]
    impl BuiltInTool for BigOutputTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("q".repeat(RESULT_BYTES))
        }
    }

    let registry = ToolRegistry::default();
    registry
        .register(RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: "dump".to_string(),
                description: "Dump".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(Arc::new(BigOutputTool)),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
    let sandbox = SandboxManager::with_defaults(Arc::new(registry), EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "research_agent::a1".to_string(),
        allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
    };

    let provider = Arc::new(MockRouterProvider::new(vec![
        Ok(ChatResponse {
            content: "Dumping.".to_string(),
            tool_calls: vec![openalpaca_llm::ToolCall {
                id: "tc_big".to_string(),
                name: "dump".to_string(),
                arguments: serde_json::json!({}),
            }],
            model: "claude-sonnet-4-20250514".to_string(),
            usage: Usage::default(),
            finish_reason: FinishReason::ToolUse,
            thinking: None,
            parts: None,
        }),
        Ok(ChatResponse {
            content: "Done.".to_string(),
            tool_calls: vec![],
            model: "claude-sonnet-4-20250514".to_string(),
            usage: Usage::default(),
            finish_reason: FinishReason::Stop,
            thinking: None,
            parts: None,
        }),
    ]));
    let router = LlmRouter::single_provider(
        provider.clone(),
        ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    let dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&db_dir.path().join("t.db")).unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        Some(db.clone()),
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-big-result");

    let config = LoopConfig {
        model: Some("claude-sonnet-4-20250514".to_string()),
        enable_caching: false,
        session_log: Some(handle.clone()),
        ..Default::default()
    };

    let result = run_agentic_loop_routed(
        &router,
        vec![ChatMessage::user("dump it")],
        vec![],
        &config,
        Some(&sandbox),
        "research_agent::a1",
        Some(&policy),
        Some("task-big"),
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert!(handle.flush().await);

    let session_dir = dir.path().join("sess-big-result");
    let records = read_records(&session_dir).unwrap();
    let logged = records
        .iter()
        .find(|r| r.kind == "tool_result")
        .expect("the result is narrated");
    let rel = logged.data["result"]["spill"]["rel"].as_str().unwrap();
    assert_eq!(logged.data["result_ref"], format!("file:{rel}"));
    assert_eq!(logged.data["result"]["spill"]["bytes"], RESULT_BYTES);

    // One copy of the payload, in the file the record names.
    let spilled = std::fs::read_to_string(session_dir.join(rel)).unwrap();
    assert_eq!(spilled.len(), RESULT_BYTES, "the whole result is on disk");
    assert!(
        serde_json::to_string(&logged.data).unwrap().len()
            <= crate::session_log::ENVELOPE_DATA_CAP_BYTES,
        "a spilled record never trips the envelope cap"
    );

    // What the model was handed for the same call: §5.4's stub, verbatim.
    let seen = provider.tool_results_seen();
    assert_eq!(seen.len(), 1, "one tool result reached the backend: {seen:?}");
    assert_eq!(
        seen[0],
        crate::session_log::spill_stub(
            RESULT_BYTES,
            &"q".repeat(crate::session_log::PREVIEW_CHARS),
            rel,
        ),
        "the model sees the stub naming the reference, not a head-only cut"
    );
    assert!(seen[0].contains("use read_result to page"));

    // The index row points at the same file and previews the same bytes.
    let (preview, result_ref): (String, String) = db
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT result_preview, result_ref FROM tool_execution_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(result_ref, format!("file:{rel}"));
    assert_eq!(preview.chars().count(), crate::session_log::PREVIEW_CHARS);
}

/// §5.4: "`Err` results stay inline but switch to head+tail (compiler/test
/// errors sit at the tail)." A failing `cargo test` whose assertion is in the
/// last hundred bytes must still reach the model.
#[test]
fn an_error_result_keeps_its_head_and_its_tail() {
    let limit = 1024;
    let text = format!(
        "{}{}{}",
        "HEAD-MARKER",
        "m".repeat(8 * 1024),
        "TAIL-ASSERTION-FAILED"
    );
    let cut = head_tail_tool_result(text.clone(), limit);

    assert!(cut.len() < text.len());
    assert!(cut.starts_with("HEAD-MARKER"), "the head survives: {}", &cut[..40]);
    assert!(
        cut.ends_with("TAIL-ASSERTION-FAILED"),
        "the tail survives — that is the point"
    );
    assert!(cut.contains("the tail follows"), "the elision is named");

    // Under the bound nothing is touched.
    assert_eq!(head_tail_tool_result("small".to_string(), limit), "small");
}

/// A loop with no session log writes nothing and behaves identically — the
/// `None` default every non-session caller relies on.
#[tokio::test]
async fn a_loop_without_a_session_log_is_unchanged() {
    let provider = MockProvider::new(vec![Ok(MockProvider::simple_response("Hi."))]);
    let config = LoopConfig::default();
    assert!(config.session_log.is_none());
    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("hello")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
}

/// A drain is one line naming the request ids it delivered — the other half
/// of `push_steering`'s `steering` record, and what tells a reader an
/// interjection actually reached the model.
#[tokio::test]
async fn a_steering_drain_is_narrated_with_its_request_ids() {
    use crate::runner::steering::{SteeringInbox, SteeringMsg};
    use crate::security::policy::{Principal, Scope};
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    let inbox = Arc::new(SteeringInbox::default());
    let request_id = uuid::Uuid::new_v4();
    inbox
        .push(SteeringMsg {
            text: "focus on the tests".to_string(),
            request_id,
            principal: Principal::System,
            scope: Scope::Global,
            workspace_path: None,
            received_at: chrono::Utc::now(),
        })
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-steer");
    let config = LoopConfig {
        enable_caching: false,
        steering: Some(Arc::clone(&inbox)),
        session_log: Some(handle.clone()),
        ..Default::default()
    };

    let provider = MockProvider::new(vec![Ok(MockProvider::simple_response("Understood."))]);
    let result = run_agentic_loop(
        &provider,
        vec![ChatMessage::user("start")],
        vec![],
        &config,
        None,
        "test",
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(result.finish_reason, LoopFinishReason::Complete);
    assert!(handle.flush().await);

    let records = read_records(&dir.path().join("sess-steer")).unwrap();
    let drained = records
        .iter()
        .find(|r| r.kind == "steering_drained")
        .expect("the drain is narrated");
    assert_eq!(drained.data["at"], "round_boundary");
    assert_eq!(drained.data["count"], 1);
    assert_eq!(drained.data["request_ids"][0], request_id.to_string());
    // A main-loop turn carries no task_id — that absence is the signal.
    assert!(drained.task_id.is_none());
}
