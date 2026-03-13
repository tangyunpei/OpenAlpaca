pub mod compaction;
pub mod lifecycle;
pub mod manager;
pub mod package;
pub mod section;
pub mod sources;

pub use section::{
    ContextBundle, ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority,
    TrustLevel,
};
pub use manager::ContextManager;
pub use package::{AgentSummary, ContextPackage, HandoffContext, PackageSection, PackageSectionKind};
pub use sources::{ContextRequest, ContextSource, ExecutionPath};
