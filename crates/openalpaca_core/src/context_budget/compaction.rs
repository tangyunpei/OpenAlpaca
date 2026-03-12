use crate::utils::social::is_social_phrase;
use async_trait::async_trait;
use openalpaca_llm::{ChatMessage, Role};
use tracing;

/// A memory entry extracted during compaction Phase 1.
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub kind: String,
    pub content: String,
}

/// Trait for LLM-based memory extraction (mockable in tests).
#[async_trait]
pub trait MemoryExtractor: Send + Sync {
    async fn extract(&self, messages: &[ChatMessage]) -> Result<Vec<ExtractedMemory>, String>;
}

/// Trait for LLM-based message summarization (mockable in tests).
#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, messages: &[ChatMessage]) -> Result<String, String>;
}

/// 3-phase compaction pipeline for context window management.
pub struct CompactionPipeline;

/// Result of a full compaction pipeline run.
#[derive(Debug)]
pub struct CompactionResult {
    pub compacted_messages: Vec<ChatMessage>,
    pub extracted_memories: Vec<ExtractedMemory>,
    pub messages_discarded: usize,
    pub messages_before: usize,
    pub messages_after: usize,
    pub error: Option<String>,
}

impl CompactionPipeline {
    /// Run the full 3-phase compaction pipeline.
    ///
    /// Phase 1: Extract memories (LLM). On error: skip, log.
    /// Phase 2: Discard social messages (heuristic). Always succeeds.
    /// Phase 3: Summarize older messages (LLM). On error: fall back to
    ///          existing `compress_context()` heuristic.
    pub async fn compact(
        messages: Vec<ChatMessage>,
        min_recent: usize,
        extractor: &dyn MemoryExtractor,
        summarizer: &dyn Summarizer,
    ) -> CompactionResult {
        let messages_before = messages.len();

        // Phase 1: Memory extraction (best-effort)
        let boundary = messages.len().saturating_sub(min_recent).max(2);
        let older = if boundary > 2 { &messages[2..boundary] } else { &[] as &[ChatMessage] };

        let extracted_memories = match extractor.extract(older).await {
            Ok(memories) => {
                tracing::info!(count = memories.len(), "Compaction Phase 1: extracted memories");
                memories
            }
            Err(e) => {
                tracing::warn!("Compaction Phase 1 (extraction) failed, skipping: {e}");
                vec![]
            }
        };

        // Phase 2: Discard social messages (always succeeds)
        let after_discard = Self::discard_social(&messages, min_recent);
        let messages_discarded = messages_before - after_discard.len();
        tracing::info!(discarded = messages_discarded, "Compaction Phase 2: social discard");

        // Phase 3: Summarize older messages (with fallback)
        match Self::summarize_older(after_discard.clone(), min_recent, summarizer).await {
            Ok(compacted) => {
                let messages_after = compacted.len();
                CompactionResult {
                    compacted_messages: compacted,
                    extracted_memories,
                    messages_discarded,
                    messages_before,
                    messages_after,
                    error: None,
                }
            }
            Err(e) => {
                tracing::warn!("Compaction Phase 3 (summarize) failed, heuristic fallback: {e}");
                let mut fallback = after_discard;
                // compress_context expects tail_keep in "rounds" (×3 internally)
                // Convert min_recent (message count) to rounds via ceiling division
                let tail_keep = ((min_recent + 2) / 3).max(1);
                crate::runner::compress_context(&mut fallback, tail_keep, None);
                let messages_after = fallback.len();
                CompactionResult {
                    compacted_messages: fallback,
                    extracted_memories,
                    messages_discarded,
                    messages_before,
                    messages_after,
                    error: Some(format!("Phase 3 failed: {e}")),
                }
            }
        }
    }

    /// Phase 2: Discard social/low-value message pairs.
    ///
    /// Preserves: message 0 (system), message 1 (initial query),
    /// last `min_recent` messages. Removes social user messages
    /// and their immediately following assistant responses.
    pub fn discard_social(messages: &[ChatMessage], min_recent: usize) -> Vec<ChatMessage> {
        if messages.len() <= 2 + min_recent {
            return messages.to_vec();
        }

        let preserve_from = messages.len().saturating_sub(min_recent);
        let mut result = Vec::with_capacity(messages.len());

        // Always keep system + initial query
        if !messages.is_empty() {
            result.push(messages[0].clone());
        }
        if messages.len() > 1 {
            result.push(messages[1].clone());
        }

        let mut skip_next_assistant = false;
        for (i, msg) in messages.iter().enumerate().skip(2) {
            if i >= preserve_from {
                result.push(msg.clone());
                continue;
            }

            if skip_next_assistant {
                skip_next_assistant = false;
                if msg.role == Role::Assistant {
                    continue;
                }
            }

            if msg.role == Role::User && is_social_phrase(&msg.content) {
                skip_next_assistant = true;
                continue;
            }

            result.push(msg.clone());
        }

        result
    }

    /// Phase 3: Summarize older messages using an LLM.
    ///
    /// Replaces messages[2..boundary] with a single summary message.
    /// boundary = messages.len() - min_recent.
    pub async fn summarize_older(
        messages: Vec<ChatMessage>,
        min_recent: usize,
        summarizer: &dyn Summarizer,
    ) -> Result<Vec<ChatMessage>, String> {
        if messages.len() <= 2 + min_recent {
            return Ok(messages);
        }

        let boundary = messages.len().saturating_sub(min_recent);
        if boundary <= 2 {
            return Ok(messages);
        }

        let older = &messages[2..boundary];
        let summary_text = summarizer.summarize(older).await?;

        let mut result = Vec::with_capacity(2 + 1 + min_recent);
        result.push(messages[0].clone()); // system
        result.push(messages[1].clone()); // initial query
        result.push(ChatMessage::user(&summary_text)); // summary
        result.extend_from_slice(&messages[boundary..]); // recent

        Ok(result)
    }
}
