use crate::orchestrator::working_state::{BLOCK_PREFIX, PREVIOUS_BLOCK_PREFIX, WorkingState};
use crate::utils::social::is_social_phrase;
use async_trait::async_trait;
use openalpaca_llm::{ChatMessage, Role};
use tracing;

/// The working-state writer at compaction (mockable in tests): the state
/// after `messages`, folded into `previous` — one model call (design 11
/// §3.3).
#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(
        &self,
        previous: Option<&WorkingState>,
        messages: &[ChatMessage],
    ) -> Result<WorkingState, String>;
}

/// The messages at the head of a list every compaction tier keeps by
/// default: the system prompt and the initial query (`messages[..2]`). A
/// loop handed a longer head — a spawned subagent's, its objective last
/// (SA-2b) — passes its own count (`LoopConfig::kept_head`).
pub const DEFAULT_KEPT_HEAD: usize = 2;

/// 2-phase compaction pipeline for context window management.
pub struct CompactionPipeline;

/// Result of a full compaction pipeline run.
#[derive(Debug)]
pub struct CompactionResult {
    pub compacted_messages: Vec<ChatMessage>,
    /// The state the summary wrote and its boundary: `messages[head..boundary]`
    /// of the list the pipeline was handed lie behind it. `None` when nothing
    /// was summarised or the summary fell back to the heuristic.
    pub working_state: Option<(WorkingState, usize)>,
    #[allow(dead_code)]
    pub messages_discarded: usize,
    #[allow(dead_code)]
    pub messages_before: usize,
    #[allow(dead_code)]
    pub messages_after: usize,
    pub error: Option<String>,
}

impl CompactionPipeline {
    /// Run the compaction pipeline (design 11 §3.3, CQ-1: the extraction
    /// phase is gone).
    ///
    /// Phase 2: Discard social messages (heuristic). Always succeeds.
    /// Phase 3: Write the working state over the older messages (LLM) — the
    ///          rows `behind` the list first — and put it in their place,
    ///          rendered within `max_chars`. On error (an unusable answer
    ///          too): fall back to the existing `compress_context()`
    ///          heuristic.
    ///
    /// Every phase keeps `messages[..head]` (`DEFAULT_KEPT_HEAD` but for a
    /// spawned subagent's loop).
    pub async fn compact(
        messages: Vec<ChatMessage>,
        head: usize,
        min_recent: usize,
        previous: Option<&WorkingState>,
        behind: &[ChatMessage],
        summarizer: &dyn Summarizer,
        max_chars: usize,
    ) -> CompactionResult {
        let messages_before = messages.len();

        // Phase 2: Discard social messages (always succeeds)
        let after_discard = Self::discard_social(&messages, head, min_recent);
        let messages_discarded = messages_before - after_discard.len();
        tracing::info!(
            discarded = messages_discarded,
            "Compaction Phase 2: social discard"
        );

        // Phase 3: Summarize older messages (with fallback)
        match Self::summarize_older(
            after_discard.clone(),
            head,
            min_recent,
            previous,
            behind,
            summarizer,
            max_chars,
        )
        .await
        {
            Ok(summarised) => {
                let (compacted, working_state) = match summarised {
                    // Phase 2 keeps the last `min_recent` messages, so the
                    // boundary in the list as handed is its length less them.
                    Some((compacted, state)) => {
                        (compacted, Some((state, messages_before - min_recent)))
                    }
                    None => (after_discard, None),
                };
                let messages_after = compacted.len();
                CompactionResult {
                    compacted_messages: compacted,
                    working_state,
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
                let tail_keep = min_recent.div_ceil(3).max(1);
                crate::runner::compress_context(&mut fallback, head, tail_keep, None);
                let messages_after = fallback.len();
                CompactionResult {
                    compacted_messages: fallback,
                    working_state: None,
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
    /// Preserves: `messages[..head]` (the system prompt and the initial
    /// query by default), last `min_recent` messages. Removes social user
    /// messages and their immediately following assistant responses.
    pub fn discard_social(
        messages: &[ChatMessage],
        head: usize,
        min_recent: usize,
    ) -> Vec<ChatMessage> {
        if messages.len() <= head + min_recent {
            return messages.to_vec();
        }

        let preserve_from = messages.len().saturating_sub(min_recent);
        let mut result = Vec::with_capacity(messages.len());

        // Always keep the head (system + initial query by default)
        result.extend(messages[..head].iter().cloned());

        let mut skip_next_assistant = false;
        for (i, msg) in messages.iter().enumerate().skip(head) {
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

    /// Phase 3: Write the working state over the older messages.
    ///
    /// `messages[head..boundary]` (boundary = `messages.len() - min_recent`) go
    /// to the writer with `previous`, after the rows `behind` the list —
    /// rows between `previous` and the list that neither holds, oldest first
    /// (the main loop's §3.2 cursor; empty elsewhere) — and the state it
    /// returns takes their place, rendered within `max_chars`. A message that
    /// is the previous state's block is not read — `previous` is the state —
    /// and is taken out of the list wherever it stands, so the list holds one
    /// block after (design 11 §3.3). Nor is the previous conversation's block
    /// read (design 11 rev 1.8, C1): it is not this conversation's work; the
    /// list keeps it where it keeps any message — at `messages[..head]` or
    /// among the recent ones — and in the older slice it goes with the rest.
    /// `Ok(None)` when the list has nothing to summarise, whatever lies
    /// behind it.
    pub async fn summarize_older(
        messages: Vec<ChatMessage>,
        head: usize,
        min_recent: usize,
        previous: Option<&WorkingState>,
        behind: &[ChatMessage],
        summarizer: &dyn Summarizer,
        max_chars: usize,
    ) -> Result<Option<(Vec<ChatMessage>, WorkingState)>, String> {
        if messages.len() <= head + min_recent {
            return Ok(None);
        }

        let boundary = messages.len().saturating_sub(min_recent);
        if boundary <= head {
            return Ok(None);
        }

        let is_block = |m: &&ChatMessage| m.content.starts_with(BLOCK_PREFIX);
        let is_previous = |m: &&ChatMessage| m.content.starts_with(PREVIOUS_BLOCK_PREFIX);
        let older: Vec<ChatMessage> = messages[head..boundary]
            .iter()
            .filter(|m| !is_block(m) && !is_previous(m))
            .cloned()
            .collect();
        if older.is_empty() {
            return Ok(None);
        }
        let read: Vec<ChatMessage> = behind.iter().cloned().chain(older).collect();
        let state = summarizer.summarize(previous, &read).await?;
        let updated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        let mut result = Vec::with_capacity(head + 1 + min_recent);
        result.extend(messages[..head].iter().filter(|m| !is_block(m)).cloned()); // the head
        result.push(ChatMessage::user(&state.render(&updated_at, max_chars))); // the state
        result.extend(
            messages[boundary..]
                .iter()
                .filter(|m| !is_block(m))
                .cloned(),
        ); // recent

        Ok(Some((result, state)))
    }
}
