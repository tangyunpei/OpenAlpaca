use crate::daemon_config::ContextBudgetConfig;

/// A rendered prompt section with its estimated token count.
#[derive(Debug, Clone)]
pub struct RenderedSection {
    pub content: String,
    pub token_estimate: usize,
}

impl RenderedSection {
    /// Create a new section, estimating tokens as `bytes / 4`.
    pub fn new(content: String) -> Self {
        let token_estimate = content.len() / 4;
        Self {
            content,
            token_estimate,
        }
    }

    /// Create with an explicit token estimate (e.g., from API response).
    pub fn with_token_estimate(content: String, token_estimate: usize) -> Self {
        Self {
            content,
            token_estimate,
        }
    }

    /// Empty section (zero tokens).
    pub fn empty() -> Self {
        Self {
            content: String::new(),
            token_estimate: 0,
        }
    }
}

/// Manages token budget accounting for a single context window.
///
/// One instance per request (orchestrator) or per sub-agent loop.
#[derive(Debug)]
pub struct ContextBudgetManager {
    model_context_window: usize,
    autocompact_buffer: usize,
    compaction_target_ratio: f64,
    min_recent_messages: usize,
    sections: Vec<(&'static str, usize)>,
}

impl ContextBudgetManager {
    pub fn new(model_context_window: usize, config: &ContextBudgetConfig) -> Self {
        let autocompact_buffer =
            (model_context_window as f64 * config.autocompact_buffer_ratio) as usize;
        Self {
            model_context_window,
            autocompact_buffer,
            compaction_target_ratio: config.compaction_target_ratio,
            min_recent_messages: config.min_recent_messages,
            sections: Vec::new(),
        }
    }

    pub fn register_section(&mut self, name: &'static str, tokens: usize) {
        self.sections.push((name, tokens));
    }

    pub fn model_context_window(&self) -> usize {
        self.model_context_window
    }

    pub fn autocompact_buffer(&self) -> usize {
        self.autocompact_buffer
    }

    pub fn fixed_zone_tokens(&self) -> usize {
        self.sections.iter().map(|(_, t)| t).sum()
    }

    pub fn free_zone_capacity(&self) -> usize {
        self.model_context_window
            .saturating_sub(self.autocompact_buffer)
            .saturating_sub(self.fixed_zone_tokens())
    }

    /// Total input tokens at which compaction fires (window - buffer).
    pub fn compaction_trigger(&self) -> usize {
        self.model_context_window
            .saturating_sub(self.autocompact_buffer)
    }

    pub fn should_compact(&self, message_tokens: usize) -> bool {
        self.fixed_zone_tokens() + message_tokens >= self.compaction_trigger()
    }

    pub fn compaction_target_tokens(&self) -> usize {
        (self.free_zone_capacity() as f64 * self.compaction_target_ratio) as usize
    }

    pub fn min_recent_messages(&self) -> usize {
        self.min_recent_messages
    }

    pub fn is_fixed_zone_oversized(&self) -> bool {
        self.fixed_zone_tokens() > self.model_context_window / 2
    }

    pub fn section_breakdown(&self) -> Vec<(&'static str, usize)> {
        self.sections.clone()
    }

    pub fn compaction_tier(&self, message_tokens: usize) -> CompactionTier {
        let total = self.fixed_zone_tokens() + message_tokens;
        let utilization = total as f64 / self.model_context_window as f64;
        match utilization {
            u if u < 0.60 => CompactionTier::None,
            u if u < 0.70 => CompactionTier::TruncateToolResults,
            u if u < 0.75 => CompactionTier::DropMultimedia,
            u if u < 0.80 => CompactionTier::DiscardSocial,
            u if u < 0.85 => CompactionTier::HeuristicSummary,
            _ => CompactionTier::LlmSummary,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompactionTier {
    None,
    TruncateToolResults,
    DropMultimedia,
    DiscardSocial,
    HeuristicSummary,
    LlmSummary,
}

impl CompactionTier {
    pub fn next(self) -> Option<CompactionTier> {
        match self {
            Self::None => Some(Self::TruncateToolResults),
            Self::TruncateToolResults => Some(Self::DropMultimedia),
            Self::DropMultimedia => Some(Self::DiscardSocial),
            Self::DiscardSocial => Some(Self::HeuristicSummary),
            Self::HeuristicSummary => Some(Self::LlmSummary),
            Self::LlmSummary => None,
        }
    }
}
