use super::budget::{ContextBudgetManager, RenderedSection};
use crate::daemon_config::ContextBudgetConfig;

#[test]
fn test_rendered_section_creation() {
    let section = RenderedSection::new("Hello world".to_string());
    assert_eq!(section.content, "Hello world");
    assert_eq!(section.token_estimate, 2);
}

#[test]
fn test_rendered_section_empty() {
    let section = RenderedSection::new(String::new());
    assert_eq!(section.token_estimate, 0);
}

#[test]
fn test_rendered_section_with_explicit_tokens() {
    let section = RenderedSection::with_token_estimate("content".to_string(), 500);
    assert_eq!(section.token_estimate, 500);
    assert_eq!(section.content, "content");
}

#[test]
fn test_context_budget_config_defaults() {
    let config = ContextBudgetConfig::default();
    assert!((config.autocompact_buffer_ratio - 0.165).abs() < f64::EPSILON);
    assert!((config.compaction_target_ratio - 0.50).abs() < f64::EPSILON);
    assert_eq!(config.compaction_model, None);
    assert_eq!(config.max_extractions_per_compaction, 10);
    assert_eq!(config.min_recent_messages, 4);
}

#[test]
fn test_context_budget_config_from_toml() {
    let toml_str = r#"
        autocompact_buffer_ratio = 0.20
        compaction_target_ratio = 0.60
        compaction_model = "claude-haiku-4-5-20251001"
        max_extractions_per_compaction = 5
        min_recent_messages = 6
    "#;
    let config: ContextBudgetConfig = toml::from_str(toml_str).unwrap();
    assert!((config.autocompact_buffer_ratio - 0.20).abs() < f64::EPSILON);
    assert_eq!(
        config.compaction_model.as_deref(),
        Some("claude-haiku-4-5-20251001")
    );
    assert_eq!(config.max_extractions_per_compaction, 5);
}

#[test]
fn test_budget_computation_basic() {
    let config = ContextBudgetConfig::default();
    let mgr = ContextBudgetManager::new(200_000, &config);
    assert_eq!(mgr.model_context_window(), 200_000);
    assert_eq!(mgr.autocompact_buffer(), 33_000);
    assert_eq!(mgr.fixed_zone_tokens(), 0);
    assert_eq!(mgr.free_zone_capacity(), 167_000);
}

#[test]
fn test_budget_with_fixed_sections() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    mgr.register_section("tools", 3_000);
    mgr.register_section("memory", 1_000);
    assert_eq!(mgr.fixed_zone_tokens(), 9_000);
    assert_eq!(mgr.free_zone_capacity(), 158_000);
}

#[test]
fn test_budget_various_models() {
    let config = ContextBudgetConfig::default();
    assert_eq!(
        ContextBudgetManager::new(8_192, &config).autocompact_buffer(),
        1_351
    );
    assert_eq!(
        ContextBudgetManager::new(128_000, &config).autocompact_buffer(),
        21_120
    );
    assert_eq!(
        ContextBudgetManager::new(200_000, &config).autocompact_buffer(),
        33_000
    );
}

#[test]
fn test_compaction_trigger_threshold() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    assert!(!mgr.should_compact(161_999));
    assert!(mgr.should_compact(162_000));
    assert!(mgr.should_compact(170_000));
}

#[test]
fn test_compaction_not_triggered_below_threshold() {
    let config = ContextBudgetConfig::default();
    let mgr = ContextBudgetManager::new(200_000, &config);
    assert!(!mgr.should_compact(0));
    assert!(!mgr.should_compact(100_000));
    assert!(!mgr.should_compact(166_999));
}

#[test]
fn test_autocompact_buffer_ratio_config() {
    let mut config = ContextBudgetConfig::default();
    config.autocompact_buffer_ratio = 0.25;
    let mgr = ContextBudgetManager::new(100_000, &config);
    assert_eq!(mgr.autocompact_buffer(), 25_000);
    assert_eq!(mgr.free_zone_capacity(), 75_000);
}

#[test]
fn test_fixed_zone_overflow_warning() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(10_000, &config);
    mgr.register_section("huge_prompt", 6_000);
    assert!(mgr.is_fixed_zone_oversized());
}

#[test]
fn test_section_breakdown() {
    let config = ContextBudgetConfig::default();
    let mut mgr = ContextBudgetManager::new(200_000, &config);
    mgr.register_section("system_prompt", 5_000);
    mgr.register_section("tools", 3_000);
    let breakdown = mgr.section_breakdown();
    assert_eq!(breakdown.len(), 2);
    assert_eq!(breakdown[0], ("system_prompt", 5_000));
    assert_eq!(breakdown[1], ("tools", 3_000));
}

// ── Compaction tests ──────────────────────────────────────────────
use super::compaction::{CompactionPipeline, ExtractedMemory, MemoryExtractor, Summarizer};
use async_trait::async_trait;
use openalpaca_llm::ChatMessage;

#[test]
fn test_discard_removes_social() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("thanks"),
        ChatMessage::assistant("You're welcome!"),
        ChatMessage::user("ok"),
        ChatMessage::assistant("Anything else?"),
        ChatMessage::user("What's the weather?"),
        ChatMessage::assistant("It's sunny."),
    ];
    let result = CompactionPipeline::discard_social(&messages, 2);
    assert!(result.len() < messages.len());
    assert_eq!(result[0].role, openalpaca_llm::Role::System);
    assert!(result.last().unwrap().content.contains("sunny"));
}

#[test]
fn test_discard_preserves_substantive() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("Write a sort function"),
        ChatMessage::assistant("Here's the implementation..."),
    ];
    let result = CompactionPipeline::discard_social(&messages, 2);
    assert_eq!(result.len(), messages.len());
}

struct MockExtractor(Vec<ExtractedMemory>);

#[async_trait]
impl MemoryExtractor for MockExtractor {
    async fn extract(&self, _messages: &[ChatMessage]) -> Result<Vec<ExtractedMemory>, String> {
        Ok(self.0.clone())
    }
}

struct MockSummarizer(String);

#[async_trait]
impl Summarizer for MockSummarizer {
    async fn summarize(&self, _messages: &[ChatMessage]) -> Result<String, String> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn test_extraction_returns_memories() {
    let messages = vec![
        ChatMessage::user("I prefer TypeScript over JavaScript"),
        ChatMessage::assistant("Noted, I'll use TypeScript."),
    ];
    let extractor = MockExtractor(vec![ExtractedMemory {
        kind: "user_preference".to_string(),
        content: "User prefers TypeScript".to_string(),
    }]);
    let extracted = extractor.extract(&messages).await.unwrap();
    assert_eq!(extracted.len(), 1);
    assert_eq!(extracted[0].kind, "user_preference");
}

#[tokio::test]
async fn test_summarize_replaces_older_messages() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("Tell me about Rust"),
        ChatMessage::assistant("Rust is a systems language..."),
        ChatMessage::user("What about ownership?"),
        ChatMessage::assistant("Ownership is Rust's core..."),
        ChatMessage::user("How do lifetimes work?"),
        ChatMessage::assistant("Lifetimes ensure references are valid..."),
    ];
    let summarizer = MockSummarizer("[Summary: discussed Rust]".to_string());
    let result = CompactionPipeline::summarize_older(messages.clone(), 2, &summarizer)
        .await
        .unwrap();
    // System + initial + summary + last 2 messages
    assert!(result.len() <= 5);
    assert!(result.iter().any(|m| m.content.contains("[Summary:")));
    assert_eq!(result.last().unwrap().content, "Lifetimes ensure references are valid...");
}

#[tokio::test]
async fn test_summarize_preserves_recent_when_nothing_to_summarize() {
    let messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("init"),
        ChatMessage::user("recent"),
        ChatMessage::assistant("answer"),
    ];
    let summarizer = MockSummarizer("summary".to_string());
    let result = CompactionPipeline::summarize_older(messages.clone(), 4, &summarizer)
        .await
        .unwrap();
    assert_eq!(result.len(), messages.len()); // no change
}

#[tokio::test]
async fn test_compaction_full_pipeline() {
    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("thanks"),
        ChatMessage::assistant("You're welcome!"),
        ChatMessage::user("Tell me about Rust"),
        ChatMessage::assistant("Rust is a systems language..."),
        ChatMessage::user("What about ownership?"),
        ChatMessage::assistant("Ownership is the core..."),
        ChatMessage::user("How do lifetimes work?"),
        ChatMessage::assistant("Lifetimes ensure..."),
    ];

    let extractor = MockExtractor(vec![ExtractedMemory {
        kind: "fact".to_string(),
        content: "Discussed Rust ownership".to_string(),
    }]);
    let summarizer = MockSummarizer("[Summary: Rust discussion]".to_string());

    let result = CompactionPipeline::compact(messages, 2, &extractor, &summarizer).await;
    assert!(result.compacted_messages.len() < 10);
    assert_eq!(result.extracted_memories.len(), 1);
    assert!(result.messages_discarded > 0);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn test_compaction_fallback_on_summarizer_failure() {
    struct FailingSummarizer;

    #[async_trait]
    impl Summarizer for FailingSummarizer {
        async fn summarize(&self, _: &[ChatMessage]) -> Result<String, String> {
            Err("model unavailable".to_string())
        }
    }

    let messages = vec![
        ChatMessage::system("system prompt"),
        ChatMessage::user("initial query"),
        ChatMessage::user("message 1"),
        ChatMessage::assistant("response 1"),
        ChatMessage::user("message 2"),
        ChatMessage::assistant("response 2"),
        ChatMessage::user("message 3"),
        ChatMessage::assistant("response 3"),
        ChatMessage::user("recent"),
        ChatMessage::assistant("recent response"),
    ];

    let result = CompactionPipeline::compact(messages, 2, &MockExtractor(vec![]), &FailingSummarizer).await;
    assert!(result.compacted_messages.len() < 10);
    assert!(result.error.is_some());
    assert!(result.error.unwrap().contains("model unavailable"));
}

#[tokio::test]
async fn test_compaction_preserves_recent() {
    let messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("init"),
        ChatMessage::user("old1"),
        ChatMessage::assistant("old_resp1"),
        ChatMessage::user("recent_q"),
        ChatMessage::assistant("recent_a"),
    ];
    let result = CompactionPipeline::compact(
        messages, 2, &MockExtractor(vec![]), &MockSummarizer("summary".to_string()),
    ).await;
    let last = result.compacted_messages.last().unwrap();
    assert_eq!(last.content, "recent_a");
}

// ── ContextPackage tests ──────────────────────────────────────────
use super::package::ContextPackageBuilder;

#[test]
fn test_context_package_always_has_task() {
    let pkg = ContextPackageBuilder::new("Analyze the logs".to_string()).build();
    assert_eq!(pkg.task_description, "Analyze the logs");
}

#[test]
fn test_context_package_includes_optional_sections() {
    let pkg = ContextPackageBuilder::new("Fix the bug".to_string())
        .conversation_summary("User reported a crash on login".to_string())
        .user_context("Prefers verbose logging".to_string())
        .workspace_artifact("Agent A found NPE in auth.rs line 42".to_string())
        .build();
    assert!(pkg.conversation_summary.is_some());
    assert!(pkg.user_context.is_some());
    assert_eq!(pkg.workspace_artifacts.len(), 1);
}

#[test]
fn test_context_package_denied_sections() {
    let denied = vec!["conversation_summary".to_string(), "user_context".to_string()];
    let pkg = ContextPackageBuilder::new("Task".to_string())
        .conversation_summary("summary".to_string())
        .user_context("prefs".to_string())
        .workspace_artifact("artifact".to_string())
        .denied_sections(&denied)
        .build();
    assert!(pkg.conversation_summary.is_none());
    assert!(pkg.user_context.is_none());
    assert_eq!(pkg.workspace_artifacts.len(), 1);
}

#[test]
fn test_context_package_denied_sections_case_insensitive() {
    let denied = vec!["Conversation_Summary".to_string()];
    let pkg = ContextPackageBuilder::new("Task".to_string())
        .conversation_summary("summary".to_string())
        .denied_sections(&denied)
        .build();
    assert!(pkg.conversation_summary.is_none());
}

#[test]
fn test_context_package_minimum_exposure() {
    let pkg = ContextPackageBuilder::new("Pure compute".to_string()).build();
    assert!(pkg.conversation_summary.is_none());
    assert!(pkg.relevant_memories.is_empty());
    assert!(pkg.user_context.is_none());
    assert!(pkg.workspace_artifacts.is_empty());
}

#[test]
fn test_context_package_format_for_prompt() {
    let pkg = ContextPackageBuilder::new("Fix the bug".to_string())
        .conversation_summary("User saw a crash".to_string())
        .workspace_artifact("Previous analysis output".to_string())
        .build();
    let prompt = pkg.format_for_prompt();
    assert!(prompt.contains("Fix the bug"));
    assert!(prompt.contains("User saw a crash"));
    assert!(prompt.contains("Previous analysis output"));
}

#[test]
fn test_context_package_sections_included() {
    let pkg = ContextPackageBuilder::new("Task".to_string())
        .conversation_summary("summary".to_string())
        .user_context("prefs".to_string())
        .build();
    let sections = pkg.sections_included();
    assert!(sections.contains(&"task_description"));
    assert!(sections.contains(&"conversation_summary"));
    assert!(sections.contains(&"user_context"));
    assert!(!sections.contains(&"workspace_artifacts"));
    assert!(!sections.contains(&"relevant_memories"));
}
