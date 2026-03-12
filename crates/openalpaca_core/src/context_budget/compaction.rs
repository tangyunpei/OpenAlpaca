use crate::utils::social::is_social_phrase;
use async_trait::async_trait;
use openalpaca_llm::{ChatMessage, Role};

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

impl CompactionPipeline {
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
