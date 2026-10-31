use super::budget::{CompactionTier, ContextBudgetManager, RenderedSection};
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
    assert_eq!(config.min_recent_messages, 4);
}

/// Design 11, CQ-3: `compaction_model` is gone — compaction runs on
/// `[orchestrator.costs] summary_model` — and a hand-edited file that still
/// names it is read as if it did not, like any stale key.
#[test]
fn test_context_budget_config_from_toml() {
    let toml_str = r#"
        autocompact_buffer_ratio = 0.20
        compaction_target_ratio = 0.60
        compaction_model = "claude-haiku-4-5-20251001"
        min_recent_messages = 6
    "#;
    let config: ContextBudgetConfig = toml::from_str(toml_str).unwrap();
    assert!((config.autocompact_buffer_ratio - 0.20).abs() < f64::EPSILON);
    assert_eq!(config.min_recent_messages, 6);
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
    let config = ContextBudgetConfig {
        autocompact_buffer_ratio: 0.25,
        ..Default::default()
    };
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

#[test]
fn test_compaction_tier_thresholds() {
    let config = ContextBudgetConfig {
        autocompact_buffer_ratio: 0.165,
        compaction_target_ratio: 0.50,
        min_recent_messages: 4,
    };
    let budget = ContextBudgetManager::new(200_000, &config);

    // 50% utilization → None (below 60%)
    assert_eq!(budget.compaction_tier(100_000), CompactionTier::None);
    // 65% → TruncateToolResults (60-70%)
    assert_eq!(budget.compaction_tier(130_000), CompactionTier::TruncateToolResults);
    // 72.5% → DropMultimedia (70-75%)
    assert_eq!(budget.compaction_tier(145_000), CompactionTier::DropMultimedia);
    // 77.5% → DiscardSocial (75-80%)
    assert_eq!(budget.compaction_tier(155_000), CompactionTier::DiscardSocial);
    // 82.5% → HeuristicSummary (80-85%)
    assert_eq!(budget.compaction_tier(165_000), CompactionTier::HeuristicSummary);
    // 90% → LlmSummary (85%+)
    assert_eq!(budget.compaction_tier(180_000), CompactionTier::LlmSummary);
}

// ── Compaction tests ──────────────────────────────────────────────
use super::compaction::{CompactionPipeline, Summarizer};
use crate::orchestrator::working_state::{
    BLOCK_PREFIX, PREVIOUS_BLOCK_PREFIX, VERSION, WorkingState,
};
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

fn state(topic: &str) -> WorkingState {
    WorkingState {
        v: VERSION,
        topic: topic.to_string(),
        goal: String::new(),
        done: Vec::new(),
        asked: Vec::new(),
        decisions: Vec::new(),
        open: Vec::new(),
        next: String::new(),
        refs: Vec::new(),
    }
}

/// A writer that answers `topic`'s state and keeps what it was handed.
struct MockSummarizer {
    topic: String,
    seen: std::sync::Mutex<Vec<(Option<WorkingState>, Vec<String>)>>,
}

impl MockSummarizer {
    fn new(topic: &str) -> Self {
        Self {
            topic: topic.to_string(),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Summarizer for MockSummarizer {
    async fn summarize(
        &self,
        previous: Option<&WorkingState>,
        messages: &[ChatMessage],
    ) -> Result<WorkingState, String> {
        self.seen.lock().unwrap().push((
            previous.cloned(),
            messages.iter().map(|m| m.content.clone()).collect(),
        ));
        Ok(state(&self.topic))
    }
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
    let summarizer = MockSummarizer::new("Rust");
    let (result, written) =
        CompactionPipeline::summarize_older(messages.clone(), 2, None, &[], &summarizer, 4000)
            .await
            .unwrap()
            .expect("the older messages are summarised");
    // System + initial + state + last 2 messages
    assert_eq!(result.len(), 5);
    assert_eq!(written, state("Rust"));
    assert!(result[2].content.starts_with(BLOCK_PREFIX));
    assert!(
        result[2].content.contains("Topic: Rust"),
        "{}",
        result[2].content
    );
    assert_eq!(
        result.last().unwrap().content,
        "Lifetimes ensure references are valid..."
    );
}

#[tokio::test]
async fn test_summarize_preserves_recent_when_nothing_to_summarize() {
    let messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("init"),
        ChatMessage::user("recent"),
        ChatMessage::assistant("answer"),
    ];
    let summarizer = MockSummarizer::new("summary");
    let result =
        CompactionPipeline::summarize_older(messages.clone(), 4, None, &[], &summarizer, 4000)
            .await
            .unwrap();
    assert!(result.is_none(), "no change");
    assert!(summarizer.seen.lock().unwrap().is_empty(), "and no call");
}

/// Design 11 §3.3 (panel defect U-6): the previous state's block is not read
/// as an older message — the previous state is handed to the writer as
/// itself — and it is taken out of the list, wherever it stands: at
/// `messages[1]` (the main loop's shape) or inside the older slice (the
/// lead's, after its objective). One block is left, the new state's.
#[tokio::test]
async fn one_state_block_is_left_after_compaction_wherever_the_previous_stood() {
    let previous = state("Lisbon trip");
    let block = previous.render("2026-09-28 10:00:00", 4000);
    let rows = |n: usize| -> Vec<ChatMessage> {
        (1..=n)
            .map(|i| {
                if i % 2 == 1 {
                    ChatMessage::user(&format!("u{i}"))
                } else {
                    ChatMessage::assistant(&format!("a{i}"))
                }
            })
            .collect()
    };
    let main_loop: Vec<ChatMessage> = [ChatMessage::system("sys"), ChatMessage::user(&block)]
        .into_iter()
        .chain(rows(8))
        .collect();
    let lead: Vec<ChatMessage> = [
        ChatMessage::system("sys"),
        ChatMessage::user("objective"),
        ChatMessage::user(&block),
    ]
    .into_iter()
    .chain(rows(8))
    .collect();

    for (shape, messages, kept_head) in [("main loop", main_loop, 1), ("lead", lead, 2)] {
        let summarizer = MockSummarizer::new("Lisbon trip, booked");
        let result =
            CompactionPipeline::compact(messages, 4, Some(&previous), &[], &summarizer, 4000).await;

        let blocks: Vec<&ChatMessage> = result
            .compacted_messages
            .iter()
            .filter(|m| m.content.starts_with(BLOCK_PREFIX))
            .collect();
        assert_eq!(blocks.len(), 1, "{shape}: one block after compaction");
        assert!(blocks[0].content.contains("Lisbon trip, booked"), "{shape}");
        assert_eq!(
            result.compacted_messages[kept_head].content, blocks[0].content,
            "{shape}: the state stands where the older messages stood"
        );
        let seen = summarizer.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{shape}: one call");
        assert_eq!(
            seen[0].0.as_ref(),
            Some(&previous),
            "{shape}: the previous state"
        );
        assert!(
            !seen[0].1.iter().any(|m| m.starts_with(BLOCK_PREFIX)),
            "{shape}: the previous block is not read as an older message: {:?}",
            seen[0].1
        );
        assert_eq!(seen[0].1.first().map(String::as_str), Some("u1"), "{shape}");
        let (_, boundary) = result.working_state.expect("the state and its boundary");
        assert_eq!(
            boundary,
            result.messages_before - 4,
            "{shape}: the boundary in the list as handed"
        );
    }
}

/// Design 11 rev 1.8 (C1; 12 rev 2 §6.1): the previous conversation's block
/// is never read by the writer, wherever it stands — at `messages[1]` (the
/// main loop's shape), which the list keeps, or inside the older slice (a
/// message ahead of it), which goes with the rest of that slice.
#[tokio::test]
async fn the_writer_never_reads_the_previous_conversations_block() {
    let block = format!(
        "{PREVIOUS_BLOCK_PREFIX} trust=\"user_derived\">\nThe previous conversation on this \
         project ended…\nTopic: Tomas birthday plan\n</context_data>"
    );
    let rows = |n: usize| -> Vec<ChatMessage> {
        (1..=n)
            .map(|i| {
                if i % 2 == 1 {
                    ChatMessage::user(&format!("u{i}"))
                } else {
                    ChatMessage::assistant(&format!("a{i}"))
                }
            })
            .collect()
    };
    let main_loop: Vec<ChatMessage> = [ChatMessage::system("sys"), ChatMessage::user(&block)]
        .into_iter()
        .chain(rows(8))
        .collect();
    let in_older: Vec<ChatMessage> = [
        ChatMessage::system("sys"),
        ChatMessage::user("dynamic context"),
        ChatMessage::user(&block),
    ]
    .into_iter()
    .chain(rows(8))
    .collect();

    for (shape, messages, kept) in [("main loop", main_loop, true), ("older", in_older, false)] {
        let summarizer = MockSummarizer::new("This conversation");
        let (result, _) =
            CompactionPipeline::summarize_older(messages, 4, None, &[], &summarizer, 4000)
                .await
                .unwrap()
                .expect("the older messages are summarised");
        let seen = summarizer.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{shape}: one call");
        assert!(
            !seen[0]
                .1
                .iter()
                .any(|m| m.starts_with(PREVIOUS_BLOCK_PREFIX) || m.contains("Tomas")),
            "{shape}: the writer read the previous conversation: {:?}",
            seen[0].1
        );
        assert_eq!(seen[0].1.first().map(String::as_str), Some("u1"), "{shape}");
        assert_eq!(
            result.iter().any(|m| m.content == block),
            kept,
            "{shape}: the list keeps the block only where it keeps any message"
        );
    }
}

/// Design 11 rev 1.2: the rows behind the list — between the previous
/// state and the list, in neither — are read first, oldest first, and only
/// when the list itself has older messages to summarise: no call is made
/// for them alone.
#[tokio::test]
async fn the_rows_behind_the_list_are_read_first_and_never_alone() {
    let behind = [ChatMessage::user("b1"), ChatMessage::assistant("b2")];
    let messages: Vec<ChatMessage> = [ChatMessage::system("sys"), ChatMessage::user("init")]
        .into_iter()
        .chain((1..=6).map(|i| {
            if i % 2 == 1 {
                ChatMessage::user(&format!("u{i}"))
            } else {
                ChatMessage::assistant(&format!("a{i}"))
            }
        }))
        .collect();
    let summarizer = MockSummarizer::new("read");
    CompactionPipeline::summarize_older(messages, 2, None, &behind, &summarizer, 4000)
        .await
        .unwrap()
        .expect("summarised");
    assert_eq!(
        summarizer.seen.lock().unwrap()[0].1,
        vec!["b1", "b2", "u1", "a2", "u3", "a4"],
        "behind first, then messages[2..6]"
    );

    let short = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("init"),
        ChatMessage::user("recent"),
        ChatMessage::assistant("answer"),
    ];
    let summarizer = MockSummarizer::new("read");
    let result = CompactionPipeline::summarize_older(short, 4, None, &behind, &summarizer, 4000)
        .await
        .unwrap();
    assert!(result.is_none());
    assert!(
        summarizer.seen.lock().unwrap().is_empty(),
        "no call for the rows behind alone"
    );
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

    let summarizer = MockSummarizer::new("Rust discussion");

    let result = CompactionPipeline::compact(messages, 2, None, &[], &summarizer, 4000).await;
    assert!(result.compacted_messages.len() < 10);
    // The social pair went before the summary, so the boundary is counted in
    // the list as handed: 10 messages less the 2 kept.
    assert_eq!(result.working_state, Some((state("Rust discussion"), 8)));
    assert!(result.messages_discarded > 0);
    assert!(result.error.is_none());
}

#[tokio::test]
async fn test_compaction_fallback_on_summarizer_failure() {
    struct FailingSummarizer;

    #[async_trait]
    impl Summarizer for FailingSummarizer {
        async fn summarize(
            &self,
            _: Option<&WorkingState>,
            _: &[ChatMessage],
        ) -> Result<WorkingState, String> {
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

    let result =
        CompactionPipeline::compact(messages, 2, None, &[], &FailingSummarizer, 4000).await;
    assert!(result.compacted_messages.len() < 10);
    assert!(
        result.working_state.is_none(),
        "no state from a failed summary"
    );
    assert!(
        result.compacted_messages[2]
            .content
            .starts_with("[Context compressed:"),
        "the heuristic stands in"
    );
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
        messages,
        2,
        None,
        &[],
        &MockSummarizer::new("summary"),
        4000,
    )
    .await;
    let last = result.compacted_messages.last().unwrap();
    assert_eq!(last.content, "recent_a");
}
