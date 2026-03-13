pub mod compaction;
pub mod manager;
pub mod section;
pub mod sources;

pub use section::{
    ContextBundle, ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority,
    TrustLevel,
};
pub use manager::ContextManager;
pub use sources::{ContextRequest, ContextSource, ExecutionPath};
