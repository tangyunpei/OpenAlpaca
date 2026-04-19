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
pub use crate::prompt_ctx::{
    AgentSummary, ContextBundle, ContextPackage, ExecutionPath, SectionPriority,
};

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

// === Local stand-ins for types that do not yet exist in the codebase ===
//
// The spec refers to `PlanState`, `WorkspaceSnapshot`, and `ConnectorSummary`.
// None of these exist as a concrete struct in `openalpaca_core` as of Phase 1.
// We introduce slim wrapper structs here so the `ComposeRequest` variants
// compile; later phases (the loader layer in particular) can either replace
// these with richer types or keep the wrapper as the stable contract.

/// Slim wrapper for the planner's current-plan snapshot.
///
/// Phase 1 placeholder: holds an opaque JSON payload. The replanner migration
/// (Phase 4) will either swap this for a richer struct or keep it as a
/// transport envelope around whatever the existing replanner prompt uses.
#[derive(Debug, Clone, Default)]
pub struct PlanState {
    pub payload: Arc<str>,
}

/// Slim wrapper for the replanner's workspace snapshot.
///
/// Phase 1 placeholder: holds an opaque JSON payload. Replaced or kept as
/// an envelope when the replanner migration lands (Phase 4).
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSnapshot {
    pub payload: Arc<str>,
}

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
    PlannerHierarchical,
    SocialMinimal,
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
    Planner {
        idle_agents: Arc<Vec<AgentSummary>>,
        user_message: String,
        active_tasks_block: Option<Arc<str>>,
        overrides: ComposeOverrides,
    },
    Replanner {
        current_plan: Arc<PlanState>,
        workspace_snapshot: Arc<WorkspaceSnapshot>,
        overrides: ComposeOverrides,
    },
    Social {
        lane_key: String,
        query: String,
        overrides: ComposeOverrides,
    },
    PipelineStep {
        agent: Arc<AgentConfig>,
        step_index: usize,
        step_description: Arc<str>,
        scope_block: Arc<str>,
        output_block: Arc<str>,
        context_package: Arc<ContextPackage>,
        memory_block: Option<Arc<str>>,
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
            Self::Planner { .. } => f.debug_struct("Planner").finish_non_exhaustive(),
            Self::Replanner { .. } => f.debug_struct("Replanner").finish_non_exhaustive(),
            Self::Social { lane_key, .. } => f
                .debug_struct("Social")
                .field("lane_key", lane_key)
                .finish_non_exhaustive(),
            Self::PipelineStep { step_index, .. } => f
                .debug_struct("PipelineStep")
                .field("step_index", step_index)
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
            Self::Skill { .. } => (P::Default, S::Default, D::Default, H::Default),
            Self::Planner { .. } => (P::Minimal, S::PlannerHierarchical, D::Skip, H::Skip),
            Self::Replanner { .. } => (P::Minimal, S::PlannerHierarchical, D::Skip, H::Skip),
            Self::Social { .. } => (P::Minimal, S::SocialMinimal, D::Skip, H::Default),
            Self::PipelineStep { memory_block, .. } => (
                P::Minimal,
                S::Default,
                D::Skip,
                match memory_block {
                    Some(mb) => H::FirstStepOnly {
                        memory_block: mb.clone(),
                    },
                    None => H::Skip,
                },
            ),
            Self::DagNode { .. } => (P::Minimal, S::Default, D::Skip, H::Skip),
            Self::LeadAgent { .. } => (P::Minimal, S::Default, D::Skip, H::Skip),
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
    /// Planner-mode inputs (backward-compat addition for
    /// `StaticPromptMode::PlannerHierarchical`; ignored by other modes).
    ///
    /// The real `orchestrator::task_planner::prompt::build_hierarchical_prompt`
    /// takes `(&[SubAgent], bool)` — it does not consume the persona output or
    /// raw_blocks. These two optional fields plumb those inputs through so
    /// Layer 2 can call it unchanged.
    #[doc(hidden)]
    pub planner_agents: Option<Arc<Vec<AgentConfig>>>,
    /// Planner v2 protocol flag (see `PlannerLimits.plan_protocol_v2_enabled`).
    /// Defaults to false when the planner_agents field is None.
    #[doc(hidden)]
    pub planner_protocol_v2: bool,
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
