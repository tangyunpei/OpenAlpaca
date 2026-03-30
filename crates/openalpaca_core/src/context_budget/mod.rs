mod budget;
pub(crate) mod compaction;

#[cfg(test)]
mod tests;

pub use budget::{CompactionTier, ContextBudgetManager, RenderedSection};
