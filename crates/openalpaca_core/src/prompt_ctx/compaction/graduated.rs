use crate::context_budget::{CompactionTier, ContextBudgetManager};
use crate::context_budget::compaction::Summarizer;
use crate::orchestrator::working_state::WorkingState;
use openalpaca_llm::{ChatMessage, ContentPart, Role};

/// Report of compaction actions taken.
#[derive(Debug, Default)]
pub struct CompactionReport {
    pub tiers_applied: Vec<CompactionTier>,
    pub initial_tokens: usize,
    pub final_tokens: usize,
    /// The working state the LlmSummary phase wrote, and its boundary:
    /// `messages[2..boundary]` of the list as the phase found it lie behind
    /// it (design 11 §3.3). `None` when the phase was not reached or fell
    /// back to the heuristic.
    pub working_state: Option<(WorkingState, usize)>,
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
    summarizer: &'a dyn Summarizer,
    /// The rendered working state's limit (`summary_max_chars`).
    max_chars: usize,
}

impl<'a> GraduatedCompactor<'a> {
    pub fn new(
        budget: &'a ContextBudgetManager,
        summarizer: &'a dyn Summarizer,
        max_chars: usize,
    ) -> Self {
        Self {
            budget,
            summarizer,
            max_chars,
        }
    }

    /// `previous` is the working state the LlmSummary phase folds the older
    /// messages into, and `behind` the rows between it and the list that
    /// neither holds, which that phase reads first.
    pub async fn compact(
        &self,
        messages: &mut Vec<ChatMessage>,
        tail_keep: usize,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        previous: Option<&WorkingState>,
        behind: &[ChatMessage],
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
            let msgs_before_tier = messages.len();

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
                    let result = if let Some(ref token) = cancel_token {
                        tokio::select! {
                            res = crate::context_budget::compaction::CompactionPipeline::compact(
                                owned.clone(), min_recent, previous, behind, self.summarizer, self.max_chars,
                            ) => res,
                            _ = token.cancelled() => {
                                *messages = owned;
                                tracing::info!("Compaction cancelled");
                                break;
                            }
                        }
                    } else {
                        crate::context_budget::compaction::CompactionPipeline::compact(
                            owned,
                            min_recent,
                            previous,
                            behind,
                            self.summarizer,
                            self.max_chars,
                        )
                        .await
                    };
                    report.working_state = result.working_state;
                    report.compaction_error = result.error.clone();
                    *messages = result.compacted_messages;
                }
            }
            report.record_tier(current_tier);

            let msgs_after_tier = messages.len();
            report.messages_discarded += msgs_before_tier.saturating_sub(msgs_after_tier);

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

    /// A writer that answers one fixed state, whatever it is handed.
    struct StateSummarizer;
    #[async_trait::async_trait]
    impl Summarizer for StateSummarizer {
        async fn summarize(
            &self,
            _: Option<&WorkingState>,
            _: &[ChatMessage],
        ) -> Result<WorkingState, String> {
            Ok(WorkingState {
                v: crate::orchestrator::working_state::VERSION,
                topic: "Summary".into(),
                goal: String::new(),
                done: Vec::new(),
                decisions: Vec::new(),
                open: Vec::new(),
                next: String::new(),
                refs: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn test_compaction_report_carries_result_data() {
        use crate::context_budget::ContextBudgetManager;
        use crate::daemon_config::ContextBudgetConfig;
        use openalpaca_llm::ChatMessage;

        let cfg = ContextBudgetConfig {
            autocompact_buffer_ratio: 0.15,
            ..ContextBudgetConfig::default()
        };
        // Use a small context window so the test messages (short strings) still
        // exceed the 85% utilization threshold required to reach the LlmSummary tier.
        // Previously the estimate_messages_tokens floor of 100 made this work at
        // window=100; without the floor the real token count is much lower.
        let budget = ContextBudgetManager::new(50, &cfg);

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

        let compactor = GraduatedCompactor::new(&budget, &StateSummarizer, 4000);
        let report = compactor.compact(&mut messages, 2, None, None, &[]).await;

        let (state, boundary) = report
            .working_state
            .expect("should carry the working state");
        assert_eq!(state.topic, "Summary");
        assert!(boundary > 2, "and the boundary behind it: {boundary}");
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

    #[tokio::test]
    async fn test_multi_tier_escalation() {
        use crate::context_budget::ContextBudgetManager;
        use crate::daemon_config::ContextBudgetConfig;
        use openalpaca_llm::ChatMessage;

        // Use a large buffer ratio (0.50) so should_compact triggers at 50%
        // utilization while compaction_tier starts at a lower tier (~65%).
        // This creates a gap where early tiers (TruncateToolResults, DropMultimedia)
        // are no-ops for text-only messages, forcing escalation through multiple tiers.
        let cfg = ContextBudgetConfig {
            autocompact_buffer_ratio: 0.50,
            ..ContextBudgetConfig::default()
        };
        let budget = ContextBudgetManager::new(1000, &cfg);

        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("initial query"),
        ];
        // Text-only assistant/user pairs (~28 tokens each).
        // TruncateToolResults and DropMultimedia are no-ops for these,
        // forcing escalation to DiscardSocial / HeuristicSummary.
        for i in 0..24 {
            messages.push(ChatMessage::assistant(&format!("Using tool {i}")));
            messages.push(ChatMessage::user(&"x".repeat(100)));
        }
        // Social messages (gives DiscardSocial something to remove)
        messages.push(ChatMessage::user("thanks!"));
        messages.push(ChatMessage::assistant("You're welcome!"));

        let compactor = GraduatedCompactor::new(&budget, &StateSummarizer, 4000);
        let report = compactor.compact(&mut messages, 2, None, None, &[]).await;

        assert!(
            report.tiers_applied.len() > 1,
            "Expected multi-tier escalation, got {:?}",
            report.tiers_applied
        );
        assert!(
            report.messages_before > report.messages_after,
            "Expected fewer messages after compaction"
        );
    }

    #[tokio::test]
    async fn test_messages_discarded_counts_all_tiers() {
        use crate::context_budget::ContextBudgetManager;
        use crate::daemon_config::ContextBudgetConfig;
        use openalpaca_llm::ChatMessage;

        // Same setup as test_multi_tier_escalation: large buffer ratio
        // forces early should_compact trigger, text-only messages force
        // escalation through no-op tiers until DiscardSocial/HeuristicSummary.
        let cfg = ContextBudgetConfig {
            autocompact_buffer_ratio: 0.50,
            ..ContextBudgetConfig::default()
        };
        let budget = ContextBudgetManager::new(1000, &cfg);

        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("initial query"),
        ];
        for i in 0..24 {
            messages.push(ChatMessage::assistant(&format!("Using tool {i}")));
            messages.push(ChatMessage::user(&"x".repeat(100)));
        }
        // Social messages so DiscardSocial tier can remove them
        messages.push(ChatMessage::user("thanks!"));
        messages.push(ChatMessage::assistant("You're welcome!"));

        let compactor = GraduatedCompactor::new(&budget, &StateSummarizer, 4000);
        let report = compactor.compact(&mut messages, 2, None, None, &[]).await;

        // messages_discarded should accumulate across all tiers that remove messages
        // (previously it was only set for LlmSummary)
        assert!(
            report.messages_discarded > 0,
            "Expected messages_discarded > 0 across tiers, got {}",
            report.messages_discarded
        );
    }

    #[test]
    fn test_truncate_and_drop_tiers_leave_interjections_untouched() {
        // Routing V2: steering interjections must survive the truncate/drop
        // tiers untouched (they are user text messages — tier 1 only
        // truncates tool results and tier 2 only replaces media parts).
        let interjection = format!(
            "{} ts=\"2026-08-30T00:00:00Z\">{}</user_interjection>",
            crate::runner::steering::USER_INTERJECTION_PREFIX,
            "y".repeat(1000)
        );
        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user(&interjection),
        ];
        truncate_tool_results(&mut messages);
        drop_multimedia(&mut messages);
        assert_eq!(messages[1].content, interjection);
        assert!(messages[1].parts.is_none());
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
