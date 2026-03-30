/// Ordering: Optional(0) < Low(1) < Normal(2) < High(3) < Critical(4).
/// Use `b.priority.cmp(&a.priority)` for highest-first sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionPriority {
    Optional,
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContextKind {
    Memory,
    ConversationSummary,
    SkillReference,
    WorkspaceArtifact,
    UserProfile,
}

impl ContextKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::ConversationSummary => "conversation_summary",
            Self::SkillReference => "skill_reference",
            Self::WorkspaceArtifact => "workspace_artifact",
            Self::UserProfile => "user_profile",
        }
    }
}

#[derive(Debug, Clone)]
pub enum InjectionMode {
    SystemPrompt,
    SystemMessage,
    UserMessage { tag: String, trust: TrustLevel },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustLevel {
    Trusted,
    Untrusted,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ContextKey {
    Memory(i64),
    ConversationSummary,
    SkillSource(String, String),
    WorkspaceArtifact(String),
    UserProfile,
    Package(String),
}

#[derive(Debug, Clone)]
pub struct ContextSection {
    pub source: &'static str,
    pub kind: ContextKind,
    pub content: String,
    pub token_estimate: usize,
    pub priority: SectionPriority,
    pub relevance: f32,
    pub key: ContextKey,
    pub injection: InjectionMode,
}

#[derive(Debug, Clone)]
pub struct ContextBundle {
    pub sections: Vec<ContextSection>,
    pub total_tokens: usize,
    pub available_budget: usize,
}

impl ContextBundle {
    pub fn empty() -> Self {
        Self {
            sections: Vec::new(),
            total_tokens: 0,
            available_budget: 0,
        }
    }
}
