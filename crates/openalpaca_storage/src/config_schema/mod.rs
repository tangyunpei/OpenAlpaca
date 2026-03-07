//! Config key schema registry with validation, normalization, and category helpers.
//!
//! Provides a centralized definition of all known configuration keys,
//! their types, defaults, and validation rules. Used by the CLI for
//! validated writes (`set_checked`) and schema-driven TUI.

mod registry;
mod types;
pub mod validation;

#[cfg(test)]
mod tests;

pub use registry::CONFIG_KEYS;
pub use types::{ConfigBackend, ConfigKeyDef, ConfigKind};
pub use validation::*;
