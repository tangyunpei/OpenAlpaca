//! Public types for the layered compose engine (spec sections Components 1-3).
//!
//! Phase 1 scaffolding: all types compile; stub layers produce minimal-but-valid
//! outputs; Phase 2 and Phase 3 fill in the layer logic.

use std::sync::Arc;

use openalpaca_llm::{ChatMessage, ContentPart, ToolDefinition};

// Existing persona/identity/user types live in `middleware`.
pub use crate::middleware::identity::IdentityDocument;
pub use crate::middleware::prompt::{AgentPersona, SystemPersona};
pub use crate::middleware::user::UserDocument;

// Existing context types live in `prompt_ctx`.
pub use crate::prompt_ctx::{ContextBundle, ExecutionPath, SectionPriority};

// Existing agent types — `AgentConfig` as a struct does not exist in this
// codebase (the runtime uses `SubAgent`; the TOML file mirror is
// `AgentConfigFile`). Re-export `SubAgent` here under the name `AgentConfig`
// so the spec-level type name is preserved for call sites; a later phase may
// promote this to a distinct loader-owned struct if needed.
pub use crate::agent::subagent::SubAgent as AgentConfig;

// === Fingerprint type aliases (spec section Component 1) ===

pub type PersonaFingerprint = [u8; 32];
pub type StaticPromptFingerprint = [u8; 32];
pub type DynamicContextFingerprint = [u8; 32];
pub type HistoryFingerprint = [u8; 32];
pub type CompositeFingerprint = [u8; 32];

/// Slim wrapper carrying one connector's status for prompt injection.
///
/// The daemon today flattens connector status into `(String, String)` tuples;
/// this struct is a stable shape the compose engine can fingerprint.
#[derive(Debug, Clone)]
pub struct ConnectorSummary {
    pub id: String,
    pub status: String,
    pub sendable: bool,
}

// === Shared building blocks ===

/// One named, prioritized block of text headed for the system message.
#[derive(Debug, Clone)]
pub struct SystemBlock {
    pub name: &'static str,
    pub content: Arc<str>,
    pub priority: SectionPriority,
}

// === Layer mode enums (spec section Component 1) ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaMode {
    Default,
    Minimal,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticPromptMode {
    Default,
    SocialMinimal,
    /// Phase 5 Commit 1: Skill Invocation's pre-migration section order.
    /// Matches `orchestrator/skill/invocation.rs:114-230` PromptBuilder chain:
    ///   persona -> agent_persona -> bootstrap -> skills_catalog -> skill_body
    ///   -> raw_blocks -> message_source -> connector_guidance -> tools ->
    ///   send_context. Differs from `Default` in that `message_source` precedes
    ///   `tools`/`connector_guidance`, and `raw_blocks` land between
    ///   `skill_body` and `message_source` so `skill_context` sits at its
    ///   pre-migration position.
    SkillInvocationDefault,
    /// Phase 6 Commit 1: Minimal "subagent"-shape emission used by
    /// DagNode / LeadAgent. Emits only `raw_blocks` in registration order —
    /// the caller pre-renders tools via `format_tool_guidance(...)` and
    /// injects it (plus connector guidance, task-description text, etc.) as
    /// raw_blocks at the correct positions relative to the other subagent
    /// blocks. The pre-migration subagent builders never used persona /
    /// agent_persona / identity / bootstrap / skills_catalog / send_context /
    /// message_source, so this mode short-circuits all of those. See
    /// `build_subagent_minimal`.
    SubagentMinimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DynamicContextMode {
    Default,
    Skip,
}

#[derive(Debug, Clone)]
pub enum HistoryMode {
    Default,
    Skip,
    FirstStepOnly { memory_block: Arc<str> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryWrapMode {
    UntrustedWrap,
    Plain,
}

// === ComposeRequest variants (spec section Component 2) ===

pub enum ComposeRequest {
    SimpleQuery {
        lane_key: String,
        agent_persona: Arc<AgentPersona>,
        query: String,
        current_parts: Option<Vec<ContentPart>>,
        message_source: Arc<str>,
        overrides: ComposeOverrides,
    },
    Skill {
        lane_key: String,
        agent_persona: Arc<AgentPersona>,
        skill_id: String,
        skill_block: Arc<str>,
        injected_context: Arc<str>,
        query: String,
        message_source: Arc<str>,
        overrides: ComposeOverrides,
    },
    Social {
        lane_key: String,
        query: String,
        overrides: ComposeOverrides,
    },
    DagNode {
        agent: Arc<AgentConfig>,
        assignment: Arc<str>,
        workspace_context: Arc<str>,
        tools: Arc<Vec<ToolDefinition>>,
        overrides: ComposeOverrides,
    },
    LeadAgent {
        base_persona: Arc<str>,
        agents_catalog: Arc<str>,
        objective: Arc<str>,
        overrides: ComposeOverrides,
    },
}

impl std::fmt::Debug for ComposeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SimpleQuery { lane_key, .. } => f
                .debug_struct("SimpleQuery")
                .field("lane_key", lane_key)
                .finish_non_exhaustive(),
            Self::Skill {
                lane_key, skill_id, ..
            } => f
                .debug_struct("Skill")
                .field("lane_key", lane_key)
                .field("skill_id", skill_id)
                .finish_non_exhaustive(),
            Self::Social { lane_key, .. } => f
                .debug_struct("Social")
                .field("lane_key", lane_key)
                .finish_non_exhaustive(),
            Self::DagNode { .. } => f.debug_struct("DagNode").finish_non_exhaustive(),
            Self::LeadAgent { .. } => f.debug_struct("LeadAgent").finish_non_exhaustive(),
        }
    }
}

impl ComposeRequest {
    /// Per-variant default layer modes, per spec Component 2 default dispatch table.
    pub fn default_modes(
        &self,
    ) -> (
        PersonaMode,
        StaticPromptMode,
        DynamicContextMode,
        HistoryMode,
    ) {
        use DynamicContextMode as D;
        use HistoryMode as H;
        use PersonaMode as P;
        use StaticPromptMode as S;
        match self {
            Self::SimpleQuery { .. } => (P::Default, S::Default, D::Default, H::Default),
            Self::Skill { .. } => (
                P::Default,
                S::SkillInvocationDefault,
                D::Default,
                H::Default,
            ),
            Self::Social { .. } => (P::Minimal, S::SocialMinimal, D::Skip, H::Default),
            // Phase 6 Commit 1 spec errata: the default-dispatch table lists
            // (Minimal, Default, Skip, FirstStepOnly|Skip). Pre-migration
            // emits NO SystemPersona content at all, so Skip matches
            // byte-identically. StaticPromptMode::SubagentMinimal carries the
            // raw_blocks-only emission order that DagNode/LeadAgent use.
            // DynamicContextMode::Default routes caller-supplied bundle
            // sections through Layer 3 as user messages; HistoryMode::Default
            // lets the caller attach memory / current_user_turn entries.
            Self::DagNode { .. } => (P::Skip, S::SubagentMinimal, D::Default, H::Default),
            Self::LeadAgent { .. } => (P::Skip, S::SubagentMinimal, D::Default, H::Default),
        }
    }

    pub fn lane_key(&self) -> Option<&str> {
        match self {
            Self::SimpleQuery { lane_key, .. }
            | Self::Skill { lane_key, .. }
            | Self::Social { lane_key, .. } => Some(lane_key),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ComposeOverrides {
    pub persona_mode: Option<PersonaMode>,
    pub static_prompt_mode: Option<StaticPromptMode>,
    pub dynamic_context_mode: Option<DynamicContextMode>,
    pub history_mode: Option<HistoryMode>,
    pub tools_override: Option<Arc<Vec<ToolDefinition>>>,
    pub context_window_override: Option<u32>,
    /// Reserved; not consumed by the engine in Phase 1.
    pub custom_layers: Vec<CustomLayer>,
}

/// Reserved placeholder for the future `custom_layers` runtime feature
/// (spec section Future Phases, FP-3). Ignored by the Phase 1 engine.
#[derive(Debug, Clone)]
pub struct CustomLayer {
    pub name: &'static str,
    pub payload: Arc<str>,
}

// === Per-layer input / output types (spec section Component 1) ===

#[derive(Debug, Clone)]
pub struct PersonaInput {
    pub system_persona: Arc<SystemPersona>,
    pub user_document: Arc<Option<UserDocument>>,
    pub identity_document: Arc<Option<IdentityDocument>>,
    pub persona_version: u64,
    pub mode: PersonaMode,
    /// Per-caller truncation budget for the identity block, sourced from
    /// `daemon.orchestrator.prompt_budgets.identity_budget`. `None` defers
    /// to the helper's internal default (300 chars).
    pub identity_budget: Option<usize>,
    /// Per-caller truncation budget for the user document block, sourced from
    /// `daemon.orchestrator.prompt_budgets.user_profile_budget`. `None` defers
    /// to the helper's internal default.
    pub user_budget: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct PersonaOutput {
    pub blocks: Vec<SystemBlock>,
    pub fingerprint: PersonaFingerprint,
}

#[derive(Debug, Clone)]
pub struct StaticPromptInput {
    pub persona_output: Arc<PersonaOutput>,
    pub agent_persona: Option<Arc<AgentPersona>>,
    pub agent_config_fingerprint: [u8; 32],
    pub skill_block: Option<Arc<str>>,
    pub skills_catalog: Option<Arc<str>>,
    pub bootstrap: Option<Arc<str>>,
    pub tools: Arc<Vec<ToolDefinition>>,
    pub connector_status: Arc<Vec<ConnectorSummary>>,
    pub send_tool_context: Option<Arc<str>>,
    pub message_source: Option<Arc<str>>,
    pub raw_blocks: Vec<SystemBlock>,
    pub mode: StaticPromptMode,
    pub model_window: u32,
}

#[derive(Debug, Clone)]
pub struct StaticPromptOutput {
    pub system_message: Arc<str>,
    pub section_registry: Vec<&'static str>,
    pub fingerprint: StaticPromptFingerprint,
}

#[derive(Debug, Clone)]
pub struct DynamicContextInput {
    pub query: Arc<str>,
    pub path: ExecutionPath,
    pub reserved_tokens: usize,
    pub memory_retrieval_hash: [u8; 32],
    pub context_bundle: Arc<ContextBundle>,
    pub mode: DynamicContextMode,
}

#[derive(Debug, Clone)]
pub struct DynamicContextOutput {
    pub context_messages: Vec<ChatMessage>,
    pub additional_system_blocks: Vec<SystemBlock>,
    pub fingerprint: DynamicContextFingerprint,
}

#[derive(Debug, Clone)]
pub struct HistoryInput {
    pub summary: Option<Arc<str>>,
    pub summary_wrap_mode: SummaryWrapMode,
    pub recent_messages: Arc<Vec<ChatMessage>>,
    pub current_user_turn: Option<ChatMessage>,
    pub lane_tip_fingerprint: [u8; 32],
    pub mode: HistoryMode,
}

#[derive(Debug, Clone)]
pub struct HistoryOutput {
    pub messages: Vec<ChatMessage>,
    pub fingerprint: HistoryFingerprint,
}

// === Output type (spec section Component 3) ===

#[derive(Debug, Clone)]
pub struct ComposedRequest {
    pub messages: Arc<Vec<ChatMessage>>,
    pub tools: Arc<Vec<ToolDefinition>>,
    pub fingerprints: ComposedFingerprints,
    pub token_budget: TokenBudget,
    pub section_registry: Vec<&'static str>,
    pub layer_trace: LayerTrace,
}

#[derive(Debug, Clone, Copy)]
pub struct ComposedFingerprints {
    pub persona: PersonaFingerprint,
    pub static_prompt: StaticPromptFingerprint,
    pub dynamic_context: DynamicContextFingerprint,
    pub history: HistoryFingerprint,
    pub composite: CompositeFingerprint,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TokenBudget {
    pub persona_tokens: u32,
    pub static_prompt_tokens: u32,
    pub dynamic_context_tokens: u32,
    pub history_tokens: u32,
    pub current_turn_tokens: u32,
    pub total: u32,
    pub model_window: u32,
}

#[derive(Debug, Clone)]
pub struct LayerTrace {
    pub persona_mode: PersonaMode,
    pub static_prompt_mode: StaticPromptMode,
    pub dynamic_context_mode: DynamicContextMode,
    pub history_mode: HistoryMode,
    pub memo_hits: LayerMemoHits,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LayerMemoHits {
    pub persona: bool,
    pub static_prompt: bool,
    pub dynamic_context: bool,
    pub history: bool,
}
