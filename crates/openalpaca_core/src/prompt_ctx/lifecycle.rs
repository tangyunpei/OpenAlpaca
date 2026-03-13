// crates/openalpaca_core/src/prompt_ctx/lifecycle.rs
use crate::prompt_ctx::ContextKey;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct ContextLifecycle {
    seen: HashMap<ContextKey, SeenEntry>,
}

struct SeenEntry {
    message_index: usize,
    injected_at: Instant,
    token_cost: usize,
}

impl Default for ContextLifecycle {
    fn default() -> Self {
        Self { seen: HashMap::new() }
    }
}

impl ContextLifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn should_inject(&self, key: &ContextKey, staleness_threshold: Duration) -> bool {
        match self.seen.get(key) {
            None => true,
            Some(entry) => entry.injected_at.elapsed() > staleness_threshold,
        }
    }

    pub fn mark_injected(&mut self, key: ContextKey, message_index: usize, tokens: usize) {
        self.seen.insert(key, SeenEntry {
            message_index,
            injected_at: Instant::now(),
            token_cost: tokens,
        });
    }

    pub fn tokens_before(&self, index: usize) -> usize {
        self.seen.values()
            .filter(|e| e.message_index < index)
            .map(|e| e.token_cost)
            .sum()
    }
}
