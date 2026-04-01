use crate::context_budget::{CompactionTier, ContextBudgetManager};
use crate::context_budget::compaction::{MemoryExtractor, Summarizer};
use openalpaca_llm::{ChatMessage, ContentPart, Role};

/// Report of compaction actions taken.
#[derive(Debug, Default)]
pub struct CompactionReport {
    pub tiers_applied: Vec<CompactionTier>,
    pub initial_tokens: usize,
    pub final_tokens: usize,
    /// Number of memories extracted during LlmSummary phase.
    pub memories_extracted: usize,
    /// Number of messages discarded during compaction.
    pub messages_discarded: usize,
    /// Message count before compaction started.
    pub messages_before: usize,
    /// Message count after compaction completed.
    pub messages_after: usize,
    /// Error from LlmSummary phase (None = success or phase not reached).
    pub compaction_error: Option<String>,
}

impl CompactionReport {
    pub fn record_tier(&mut self, tier: CompactionTier) {
        self.tiers_applied.push(tier);
    }
}

pub struct GraduatedCompactor<'a> {
    budget: &'a ContextBudgetManager,
    extractor: &'a dyn MemoryExtractor,
    summarizer: &'a dyn Summarizer,
}

impl<'a> GraduatedCompactor<'a> {
    pub fn new(
        budget: &'a ContextBudgetManager,
        extractor: &'a dyn MemoryExtractor,
        summarizer: &'a dyn Summarizer,
    ) -> Self {
        Self { budget, extractor, summarizer }
    }

    pub async fn compact(
        &self,
        messages: &mut Vec<ChatMessage>,
        tail_keep: usize,
    ) -> CompactionReport {
        let mut report = CompactionReport {
            initial_tokens: crate::runner::estimate_messages_tokens(messages) as usize,
            messages_before: messages.len(),
            ..Default::default()
        };

        let mut current_tier = self.budget.compaction_tier(report.initial_tokens);
        if current_tier == CompactionTier::None {
            report.final_tokens = report.initial_tokens;
            return report;
        }

        loop {
            match current_tier {
                CompactionTier::None => break,
                CompactionTier::TruncateToolResults => truncate_tool_results(messages),
                CompactionTier::DropMultimedia => drop_multimedia(messages),
                CompactionTier::DiscardSocial => {
                    let min_recent = self.budget.min_recent_messages();
                    let cleaned =
                        crate::context_budget::compaction::CompactionPipeline::discard_social(
                            messages, min_recent,
                        );
                    *messages = cleaned;
                }
                CompactionTier::HeuristicSummary => {
                    crate::runner::compress_context(messages, tail_keep, Some(self.budget));
                }
                CompactionTier::LlmSummary => {
                    // Use existing CompactionPipeline for LLM-based summarization
                    let min_recent = self.budget.min_recent_messages();
                    let owned = std::mem::take(messages);
                    let result = crate::context_budget::compaction::CompactionPipeline::compact(
                        owned,
                        min_recent,
                        self.extractor,
                        self.summarizer,
                    )
                    .await;
                    report.memories_extracted = result.extracted_memories.len();
                    report.messages_discarded += result.messages_discarded;
                    report.compaction_error = result.error.clone();
                    *messages = result.compacted_messages;
                }
            }
            report.record_tier(current_tier);

            let tokens_now =
                crate::runner::estimate_messages_tokens(messages) as usize;
            if !self.budget.should_compact(tokens_now) {
                break; // Compaction succeeded — tokens are below trigger
            }
            match current_tier.next() {
                Some(next) => current_tier = next,
                None => {
                    tracing::warn!(
                        "Compaction exhausted all tiers, still at {tokens_now} tokens"
                    );
                    break;
                }
            }
        }

        report.final_tokens =
            crate::runner::estimate_messages_tokens(messages) as usize;
        report.messages_after = messages.len();
        report
    }
}

/// Tier 1: Truncate tool result messages to 50% of their size.
pub fn truncate_tool_results(messages: &mut [ChatMessage]) {
    for msg in messages.iter_mut() {
        if msg.role == Role::Tool && msg.content.len() > 200 {
            let target = msg.content.len() / 2;
            let end = msg.content.floor_char_boundary(target);
            msg.content.truncate(end);
            msg.content.push_str("\n[...truncated]");
        }
    }
}

/// Tier 2: Replace image/audio/document parts with text placeholders.
pub fn drop_multimedia(messages: &mut [ChatMessage]) {
    for msg in messages.iter_mut() {
        if let Some(ref mut parts) = msg.parts {
            for part in parts.iter_mut() {
                match part {
                    ContentPart::Image { .. } => {
                        *part = ContentPart::Text {
                            text: "[image removed to save context]".into(),
                        };
                    }
                    ContentPart::Audio { .. } => {
                        *part = ContentPart::Text {
                            text: "[audio removed to save context]".into(),
                        };
                    }
                    ContentPart::Document { filename, .. } => {
                        let name = filename.clone();
                        *part = ContentPart::Text {
                            text: format!("[document '{name}' removed to save context]"),
                        };
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_llm::ImageSource;

    #[tokio::test]
    async fn test_compaction_report_carries_result_data() {
        use crate::context_budget::compaction::{ExtractedMemory, MemoryExtractor, Summarizer};
        use crate::context_budget::ContextBudgetManager;
        use crate::daemon_config::ContextBudgetConfig;
        use openalpaca_llm::ChatMessage;

        struct TestExtractor;
        #[async_trait::async_trait]
        impl MemoryExtractor for TestExtractor {
            async fn extract(&self, _: &[ChatMessage]) -> Result<Vec<ExtractedMemory>, String> {
                Ok(vec![ExtractedMemory {
                    kind: "fact".into(),
                    content: "test memory".into(),
                }])
            }
        }

        struct TestSummarizer;
        #[async_trait::async_trait]
        impl Summarizer for TestSummarizer {
            async fn summarize(&self, _: &[ChatMessage]) -> Result<String, String> {
                Ok("[Summary]".into())
            }
        }

        let cfg = ContextBudgetConfig {
            autocompact_buffer_ratio: 0.15,
            ..ContextBudgetConfig::default()
        };
        let budget = ContextBudgetManager::new(100, &cfg);

        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("initial"),
        ];
        for i in 0..20 {
            messages.push(ChatMessage::user(&format!("msg {i}")));
            messages.push(ChatMessage::assistant(&format!("resp {i}")));
        }
        messages.push(ChatMessage::user("recent"));
        messages.push(ChatMessage::assistant("recent resp"));

        let compactor = GraduatedCompactor::new(&budget, &TestExtractor, &TestSummarizer);
        let report = compactor.compact(&mut messages, 2).await;

        assert!(report.memories_extracted > 0, "should carry extracted memory count");
        assert!(report.messages_before > 0, "should carry messages_before");
    }

    #[test]
    fn test_truncate_tool_results() {
        let mut messages = vec![
            ChatMessage::system("system"),
            ChatMessage::user("hello"),
            ChatMessage {
                role: Role::Tool,
                content: "x".repeat(1000),
                parts: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        truncate_tool_results(&mut messages);
        assert!(messages[2].content.len() <= 520); // 500 + "[...truncated]"
    }

    #[test]
    fn test_drop_multimedia() {
        let mut messages = vec![
            ChatMessage::system("system"),
            ChatMessage {
                role: Role::User,
                content: String::new(),
                parts: Some(vec![
                    ContentPart::Image {
                        source: ImageSource::Url {
                            url: "https://example.com/img.png".into(),
                        },
                        detail: None,
                    },
                    ContentPart::Text { text: "describe this".into() },
                ]),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        drop_multimedia(&mut messages);
        let parts = messages[1].parts.as_ref().unwrap();
        assert_eq!(parts.len(), 2);
        assert!(
            matches!(&parts[0], ContentPart::Text { text } if text.contains("[image removed"))
        );
    }
}
