mod budget;
pub(crate) mod compaction;
pub(crate) mod package;

#[cfg(test)]
mod tests;

pub use budget::{ContextBudgetManager, RenderedSection};
pub use package::{ContextPackage, ContextPackageBuilder, KNOWN_SECTIONS};
